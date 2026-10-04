// The extension's service worker: holds the WebSocket to the Cricket app,
// reports the tab list, and passes commands on to the music tab.
//
// Protocol: see crates/cricket-bridge/src/protocol.rs. Keep the two in step.
//
// Plain script, no imports or exports, so one tsconfig serves this file and
// the content scripts.
(() => {
  const PROTOCOL_VERSION = 2;
  const DEFAULT_PORT = 47835;
  const TICK_MS = 250;
  /** Re-arm the music tab this often, so it survives page reloads. */
  const ARM_EVERY_TICKS = 8;
  /** Chrome suspends a service worker after 30 s without activity; a
   *  WebSocket message counts as activity. */
  const PING_MS = 20_000;
  const FRAME_STATE_MAX_AGE_MS = 2_000;
  const RETRY_MIN_MS = 1_000;
  const RETRY_MAX_MS = 30_000;
  const TABS_DEBOUNCE_MS = 100;
  const ALARM = "cricket-reconnect";

  type ServerMessage =
    | { type: "challenge"; version: number; nonce: string }
    | { type: "hello_ack"; ok: boolean; error?: string; proof?: string }
    | { type: "select_source"; tabId: number | null }
    | {
        type: "command";
        cmd: "pause" | "resume" | "set_volume";
        tabId: number;
        volume?: number;
      };

  interface FrameState {
    hasMedia: boolean;
    playing: boolean;
    volume: number | null;
    at: number;
  }

  /** Shown in the popup. */
  type LinkState = "unpaired" | "connecting" | "connected" | "rejected" | "offline";

  let socket: WebSocket | null = null;
  let paired = false;
  /** The app refused the token. Retrying with the same token is pointless. */
  let rejected = false;
  let retryMs = RETRY_MIN_MS;
  let retryTimer: number | undefined;
  let sourceTabId: number | null = null;
  const frames = new Map<number, FrameState>();
  let ticks = 0;
  let lastPing = 0;
  let tabsTimer: number | undefined;
  /** Messages are handled one at a time, in order: checking the app's proof
   *  is asynchronous and what follows it must wait. */
  let inbox: Promise<void> = Promise.resolve();
  /** State of the handshake on the current connection. */
  let handshake: { key: CryptoKey; serverNonce: string; clientNonce: string } | null = null;

  const setLink = (state: LinkState, error?: string) =>
    void chrome.storage.session.set({ link: { state, error: error ?? null } });

  const send = (message: object) => {
    if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  };

  // --- Pairing ----------------------------------------------------------
  //
  // The token never crosses the connection. The app sends a nonce; this side
  // answers with an HMAC of both nonces under the token, and the app answers
  // with a second HMAC, so each proves it knows the token. Keep in step with
  // crates/cricket-bridge/src/auth.rs.

  const encoder = new TextEncoder();

  const toHex = (bytes: ArrayBuffer | Uint8Array): string =>
    [...new Uint8Array(bytes)].map((byte) => byte.toString(16).padStart(2, "0")).join("");

  const fromHex = (hex: string): Uint8Array<ArrayBuffer> | null => {
    if (hex.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(hex)) return null;
    const bytes = new Uint8Array(hex.length / 2);
    for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    return bytes;
  };

  const randomNonce = (): string => toHex(crypto.getRandomValues(new Uint8Array(16)));

  const importKey = (token: string) =>
    crypto.subtle.importKey(
      "raw",
      encoder.encode(token),
      { name: "HMAC", hash: "SHA-256" },
      false,
      ["sign", "verify"],
    );

  const proofMessage = (role: string, serverNonce: string, clientNonce: string) =>
    encoder.encode(`${role}:${serverNonce}:${clientNonce}`);

  /** Identifies this browser run. Tab ids are only unique within one, so the
   *  app uses it to tell a saved tab from a different tab that got the same
   *  number after a restart. Session storage lives exactly as long as the
   *  browser run and survives the service worker being stopped. */
  const sessionId = async (): Promise<string> => {
    const stored = await chrome.storage.session.get("sessionId");
    if (typeof stored.sessionId === "string") return stored.sessionId;
    const id = crypto.randomUUID();
    await chrome.storage.session.set({ sessionId: id });
    return id;
  };

  const browserExe = (): string => {
    const agent = navigator.userAgent;
    if (agent.includes("Edg/")) return "msedge.exe";
    if (agent.includes("OPR/")) return "opera.exe";
    if ("brave" in navigator) return "brave.exe";
    return "chrome.exe";
  };

  // --- Connection -------------------------------------------------------

  const connect = async () => {
    if (socket || rejected) return;
    const stored = await chrome.storage.local.get(["token", "port"]);
    const token = typeof stored.token === "string" ? stored.token : "";
    const port = typeof stored.port === "number" ? stored.port : DEFAULT_PORT;
    if (!token) {
      setLink("unpaired");
      return;
    }
    // Another call may have connected while storage was being read.
    if (socket) return;

    setLink("connecting");
    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
    socket = ws;

    handshake = null;
    inbox = Promise.resolve();
    // The app speaks first (a challenge); nothing is sent until then.
    ws.onmessage = (event) => {
      let message: ServerMessage;
      try {
        message = JSON.parse(String(event.data)) as ServerMessage;
      } catch {
        return; // Not JSON.
      }
      inbox = inbox
        .then(() => (socket === ws ? handle(message, ws, token) : undefined))
        // A message type this version does not know, or a failed check.
        .catch(() => undefined);
    };
    ws.onclose = () => {
      if (socket !== ws) return;
      socket = null;
      paired = false;
      frames.clear();
      if (!rejected) {
        setLink("offline");
        scheduleRetry();
      }
    };
    // onclose always follows; nothing extra to do here.
    ws.onerror = () => undefined;
  };

  const scheduleRetry = () => {
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => void connect(), retryMs);
    retryMs = Math.min(retryMs * 2, RETRY_MAX_MS);
  };

  /** Drops the connection and starts over, after the settings changed. */
  const reconnect = () => {
    rejected = false;
    retryMs = RETRY_MIN_MS;
    clearTimeout(retryTimer);
    const old = socket;
    socket = null;
    paired = false;
    old?.close();
    void connect();
  };

  const reject = (ws: WebSocket, error: string) => {
    rejected = true;
    setLink("rejected", error);
    ws.close();
  };

  const handle = async (message: ServerMessage, ws: WebSocket, token: string) => {
    switch (message.type) {
      case "challenge": {
        if (message.version !== PROTOCOL_VERSION) {
          reject(
            ws,
            `Cricket speaks protocol ${message.version}, this extension ${PROTOCOL_VERSION}. Update both.`,
          );
          break;
        }
        const key = await importKey(token);
        const clientNonce = randomNonce();
        handshake = { key, serverNonce: message.nonce, clientNonce };
        const proof = await crypto.subtle.sign(
          "HMAC",
          key,
          proofMessage("cricket-v2-client", message.nonce, clientNonce),
        );
        ws.send(
          JSON.stringify({
            type: "hello",
            version: PROTOCOL_VERSION,
            clientNonce,
            proof: toHex(proof),
            session: await sessionId(),
            browser: browserExe(),
          }),
        );
        break;
      }
      case "hello_ack": {
        if (!message.ok) {
          reject(ws, message.error ?? "The app refused the connection.");
          break;
        }
        // The app must prove it knows the token too: otherwise whatever
        // holds the port could feed this extension commands.
        const expected = handshake && message.proof ? fromHex(message.proof) : null;
        const genuine =
          handshake !== null &&
          expected !== null &&
          (await crypto.subtle.verify(
            "HMAC",
            handshake.key,
            expected,
            proofMessage("cricket-v2-server", handshake.serverNonce, handshake.clientNonce),
          ));
        if (!genuine) {
          reject(ws, "The app on this port could not prove it is Cricket. Is something else using it?");
          break;
        }
        paired = true;
        retryMs = RETRY_MIN_MS;
        setLink("connected");
        void sendTabs();
        break;
      }
      case "select_source":
        selectSource(message.tabId);
        break;
      case "command":
        toPage(message.tabId, { cmd: message.cmd, volume: message.volume });
        break;
    }
  };

  // --- Tabs -------------------------------------------------------------

  const sendTabs = async () => {
    if (!paired) return;
    const [tabs, patterns] = await Promise.all([chrome.tabs.query({}), coveredPatterns()]);
    send({
      type: "tabs",
      tabs: tabs
        .filter((tab) => tab.id !== undefined)
        .map((tab) => ({
          id: tab.id,
          // Only what Cricket needs: a title to pick by, the sound flag and
          // whether this extension can control the page. Addresses stay in
          // the browser.
          title: tab.title ?? "",
          audible: tab.audible ?? false,
          controllable: isCovered(patterns, tab.url ?? ""),
        })),
    });
  };

  const scheduleTabs = () => {
    clearTimeout(tabsTimer);
    tabsTimer = setTimeout(() => void sendTabs(), TABS_DEBOUNCE_MS);
  };

  chrome.tabs.onCreated.addListener(scheduleTabs);
  chrome.tabs.onRemoved.addListener((tabId) => {
    if (tabId === sourceTabId) frames.clear();
    scheduleTabs();
  });
  chrome.tabs.onUpdated.addListener((tabId, change) => {
    if (
      change.audible !== undefined ||
      change.title !== undefined ||
      change.status !== undefined
    ) {
      scheduleTabs();
    }
    // A reload or navigation gives the tab a fresh page that does not know
    // it is the music source yet.
    if (tabId === sourceTabId && change.status === "complete") arm(tabId, true);
  });

  // --- Which sites the extension may control ----------------------------
  //
  // The page scripts are not injected everywhere. They run on a short list of
  // music sites (the manifest's content_scripts) and on sites the user
  // allowed from the popup (optional host permissions plus a registered
  // content script). Any other tab still counts as activity through
  // Chrome's own "audible" flag; it just cannot be the music source.

  const defaultPatterns = (): string[] =>
    chrome.runtime.getManifest().content_scripts?.[0]?.matches ?? [];

  const grantedPatterns = async (): Promise<string[]> =>
    (await chrome.permissions.getAll()).origins ?? [];

  const coveredPatterns = async (): Promise<string[]> => [
    ...defaultPatterns(),
    ...(await grantedPatterns()),
  ];

  /** Whether a Chrome match pattern (`*://*.example.com/*`) covers a URL. */
  const patternMatches = (pattern: string, url: URL): boolean => {
    const parts = /^(\*|https?):\/\/([^/]+)\/(.*)$/.exec(pattern);
    if (!parts) return false;
    const [, scheme, host, path] = parts;
    if (scheme === "*" ? !/^https?:$/.test(url.protocol) : `${scheme}:` !== url.protocol) {
      return false;
    }
    const hostOk =
      host === "*" ||
      (host.startsWith("*.")
        ? url.hostname === host.slice(2) || url.hostname.endsWith(host.slice(1))
        : url.hostname === host);
    if (!hostOk) return false;
    const escaped = path.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
    return new RegExp(`^/${escaped}$`).test(url.pathname + url.search);
  };

  const isCovered = (patterns: string[], address: string): boolean => {
    let url: URL;
    try {
      url = new URL(address);
    } catch {
      return false;
    }
    return patterns.some((pattern) => patternMatches(pattern, url));
  };

  /** The two patterns that make up "this site": the host and its subdomains. */
  const sitePatterns = (host: string) => [`*://${host}/*`, `*://*.${host}/*`];

  const registeredIds = (host: string) => [`cricket-hook-${host}`, `cricket-relay-${host}`];

  const registerSite = async (host: string, tabId: number | undefined) => {
    const matches = sitePatterns(host);
    const [hookId, relayId] = registeredIds(host);
    // Registering the same id twice fails; start clean.
    await chrome.scripting.unregisterContentScripts({ ids: [hookId, relayId] }).catch(() => undefined);
    await chrome.scripting.registerContentScripts([
      {
        id: hookId,
        matches,
        js: ["dist/hook.js"],
        runAt: "document_start",
        allFrames: true,
        world: "MAIN",
        persistAcrossSessions: true,
      },
      {
        id: relayId,
        matches,
        js: ["dist/relay.js"],
        runAt: "document_start",
        allFrames: true,
        persistAcrossSessions: true,
      },
    ]);
    // The page that is open now was loaded without them: add them in place,
    // so it works without a reload (except for players the page created
    // before this moment and never attached to the document).
    if (tabId !== undefined) {
      const target = { tabId, allFrames: true };
      await chrome.scripting
        .executeScript({ target, files: ["dist/hook.js"], world: "MAIN" })
        .catch(() => undefined);
      await chrome.scripting
        .executeScript({ target, files: ["dist/relay.js"] })
        .catch(() => undefined);
    }
    scheduleTabs();
  };

  const forgetSite = async (host: string) => {
    await chrome.scripting
      .unregisterContentScripts({ ids: registeredIds(host) })
      .catch(() => undefined);
    await chrome.permissions.remove({ origins: sitePatterns(host) }).catch(() => undefined);
    scheduleTabs();
  };

  /** Hosts the user allowed, from the granted patterns. */
  const allowedHosts = async (): Promise<string[]> =>
    (await grantedPatterns())
      .map((pattern) => /^\*:\/\/([^*/]+)\/\*$/.exec(pattern)?.[1])
      .filter((host): host is string => host !== undefined);

  chrome.permissions.onRemoved.addListener(scheduleTabs);
  chrome.permissions.onAdded.addListener(scheduleTabs);

  // --- The music tab ----------------------------------------------------

  const toPage = (tabId: number, payload: object) =>
    // Rejects if the tab is gone or has no content script (browser pages,
    // or a tab opened before the extension was installed).
    void chrome.tabs
      .sendMessage(tabId, { target: "cricket-page", payload })
      .catch(() => undefined);

  const arm = (tabId: number, on: boolean) => toPage(tabId, { arm: on });

  const selectSource = (tabId: number | null) => {
    if (sourceTabId !== null && sourceTabId !== tabId) arm(sourceTabId, false);
    sourceTabId = tabId;
    frames.clear();
    if (tabId !== null) arm(tabId, true);
  };

  const reportSource = async () => {
    const tabId = sourceTabId;
    if (tabId === null || !paired) return;

    const now = Date.now();
    for (const [frameId, state] of frames) {
      if (now - state.at > FRAME_STATE_MAX_AGE_MS) frames.delete(frameId);
    }
    const media = [...frames.values()].filter((state) => state.hasMedia);

    if (media.length > 0) {
      const volumes = media.map((state) => state.volume).filter((v) => v !== null);
      send({
        type: "source_state",
        tabId,
        playing: media.some((state) => state.playing),
        volume: volumes.length > 0 ? Math.max(...volumes) : undefined,
      });
      return;
    }

    // The page has no player this extension can reach (not reloaded since
    // install, or media it cannot see). Fall back to Chrome's own "tab is
    // making sound" flag, without a volume: Cricket then knows not to fade.
    try {
      const tab = await chrome.tabs.get(tabId);
      send({ type: "source_state", tabId, playing: tab.audible ?? false });
    } catch {
      // The tab was closed. Saying nothing lets the app notice it is gone.
    }
  };

  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (message?.type === "cricket-state" && sender.tab?.id === sourceTabId) {
      const state = message.state as Omit<FrameState, "at">;
      frames.set(sender.frameId ?? 0, {
        hasMedia: state.hasMedia === true,
        playing: state.playing === true,
        volume: typeof state.volume === "number" ? state.volume : null,
        at: Date.now(),
      });
    }
    if (message?.type === "cricket-settings-changed") {
      reconnect();
      sendResponse(true);
    }
    if (message?.type === "cricket-site-status" && typeof message.url === "string") {
      void Promise.all([coveredPatterns(), allowedHosts()]).then(([patterns, allowed]) =>
        sendResponse({ covered: isCovered(patterns, message.url), allowed }),
      );
      return true; // the answer is asynchronous
    }
    if (message?.type === "cricket-allow-site" && typeof message.host === "string") {
      void registerSite(message.host, message.tabId).then(
        () => sendResponse(true),
        () => sendResponse(false),
      );
      return true;
    }
    if (message?.type === "cricket-forget-site" && typeof message.host === "string") {
      void forgetSite(message.host).then(() => sendResponse(true));
      return true;
    }
    return false;
  });

  // --- Timers -----------------------------------------------------------

  setInterval(() => {
    if (!paired) return;
    ticks += 1;
    if (sourceTabId !== null && ticks % ARM_EVERY_TICKS === 0) arm(sourceTabId, true);
    void reportSource();
    if (Date.now() - lastPing >= PING_MS) {
      lastPing = Date.now();
      send({ type: "ping" });
    }
  }, TICK_MS);

  // Timers stop when Chrome suspends the worker. The alarm wakes it up so
  // it can reconnect when the Cricket app is started later.
  void chrome.alarms.create(ALARM, { periodInMinutes: 0.5 });
  chrome.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === ALARM) void connect();
  });
  chrome.runtime.onStartup.addListener(() => void connect());
  chrome.runtime.onInstalled.addListener(() => void connect());

  void connect();
})();
