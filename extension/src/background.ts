// The extension's service worker: holds the WebSocket to the Cricket app,
// reports the tab list, and passes commands on to the music tab.
//
// Protocol: see crates/cricket-bridge/src/protocol.rs. Keep the two in step.
//
// Plain script, no imports or exports, so one tsconfig serves this file and
// the content scripts.
(() => {
  const PROTOCOL_VERSION = 1;
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
    | { type: "hello_ack"; ok: boolean; error?: string }
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

  const setLink = (state: LinkState, error?: string) =>
    void chrome.storage.session.set({ link: { state, error: error ?? null } });

  const send = (message: object) => {
    if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  };

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

    ws.onopen = async () => {
      const session = await sessionId();
      // The socket may have closed while storage was being read.
      if (ws.readyState !== WebSocket.OPEN) return;
      ws.send(
        JSON.stringify({
          type: "hello",
          version: PROTOCOL_VERSION,
          token,
          session,
          browser: browserExe(),
        }),
      );
    };
    ws.onmessage = (event) => {
      try {
        handle(JSON.parse(String(event.data)) as ServerMessage);
      } catch {
        // Not JSON, or a message type this version does not know.
      }
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

  const handle = (message: ServerMessage) => {
    switch (message.type) {
      case "hello_ack":
        if (message.ok) {
          paired = true;
          retryMs = RETRY_MIN_MS;
          setLink("connected");
          void sendTabs();
        } else {
          rejected = true;
          setLink("rejected", message.error);
        }
        break;
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
    const tabs = await chrome.tabs.query({});
    send({
      type: "tabs",
      tabs: tabs
        .filter((tab) => tab.id !== undefined)
        .map((tab) => ({
          id: tab.id,
          title: tab.title ?? "",
          url: tab.url ?? "",
          audible: tab.audible ?? false,
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
      change.url !== undefined ||
      change.status !== undefined
    ) {
      scheduleTabs();
    }
    // A reload or navigation gives the tab a fresh page that does not know
    // it is the music source yet.
    if (tabId === sourceTabId && change.status === "complete") arm(tabId, true);
  });

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
