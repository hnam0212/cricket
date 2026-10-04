// The toolbar popup: enter the pairing token, see whether Cricket is connected.
(() => {
  const DEFAULT_PORT = 47835;

  // All popup text in one place, so it can be localized later.
  const strings = {
    unpaired: "Not paired. Paste the token from the Cricket app.",
    connecting: "Connecting…",
    connected: "Connected to Cricket.",
    rejected: (error: string | null) =>
      `Cricket refused the connection: ${error ?? "unknown reason"}.`,
    offline: "Cricket is not running, or the port is wrong. Retrying…",
    saved: "Saved.",
    badPort: "Port must be a number between 1 and 65535.",
    siteDefault: (host: string) => `Cricket works on ${host}.`,
    siteAllowed: (host: string) => `You allowed Cricket on ${host}.`,
    siteNeeded: (host: string) => `Cricket cannot control music on ${host} yet.`,
    siteUnsupported: "Cricket cannot run on this kind of page.",
    allow: (host: string) => `Allow Cricket on ${host}`,
    allowedNote: "Allowed. If the music does not respond, reload the tab.",
    notGranted: "Permission was not granted.",
    remove: "Remove",
  };

  interface Link {
    state: "unpaired" | "connecting" | "connected" | "rejected" | "offline";
    error: string | null;
  }

  const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  const tokenInput = byId<HTMLInputElement>("token");
  const portInput = byId<HTMLInputElement>("port");
  const status = byId<HTMLParagraphElement>("status");
  const note = byId<HTMLParagraphElement>("note");
  const form = byId<HTMLFormElement>("form");
  const site = byId<HTMLElement>("site");
  const siteText = byId<HTMLParagraphElement>("site-text");
  const allowButton = byId<HTMLButtonElement>("site-allow");
  const allowedDetails = byId<HTMLDetailsElement>("allowed");
  const siteList = byId<HTMLUListElement>("sites");

  // --- Sites ------------------------------------------------------------

  /** "www.example.com" and "example.com" are one site. */
  const siteOf = (address: string): string | null => {
    try {
      const url = new URL(address);
      if (url.protocol !== "http:" && url.protocol !== "https:") return null;
      return url.hostname.replace(/^www\./, "");
    } catch {
      return null;
    }
  };

  const originsOf = (host: string) => [`*://${host}/*`, `*://*.${host}/*`];

  const renderSite = async () => {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    const host = tab?.url ? siteOf(tab.url) : null;
    site.hidden = false;

    const answer = (await chrome.runtime
      .sendMessage({ type: "cricket-site-status", url: tab?.url ?? "" })
      .catch(() => undefined)) as { covered: boolean; allowed: string[] } | undefined;
    const allowed = answer?.allowed ?? [];

    allowButton.hidden = true;
    if (!host) {
      siteText.textContent = strings.siteUnsupported;
    } else if (answer?.covered) {
      siteText.textContent = allowed.includes(host)
        ? strings.siteAllowed(host)
        : strings.siteDefault(host);
    } else {
      siteText.textContent = strings.siteNeeded(host);
      allowButton.hidden = false;
      allowButton.textContent = strings.allow(host);
      allowButton.onclick = () => {
        // Must be the first thing in the click: Chrome only shows the
        // permission prompt for a user gesture.
        void chrome.permissions.request({ origins: originsOf(host) }).then(async (granted) => {
          if (!granted) {
            note.textContent = strings.notGranted;
            return;
          }
          await chrome.runtime.sendMessage({ type: "cricket-allow-site", host, tabId: tab?.id });
          note.textContent = strings.allowedNote;
          await renderSite();
        });
      };
    }

    siteList.replaceChildren();
    allowedDetails.hidden = allowed.length === 0;
    for (const entry of allowed) {
      const item = document.createElement("li");
      const name = document.createElement("span");
      name.textContent = entry;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = strings.remove;
      remove.onclick = () => {
        void chrome.runtime
          .sendMessage({ type: "cricket-forget-site", host: entry })
          .then(renderSite);
      };
      item.append(name, remove);
      siteList.append(item);
    }
  };
  void renderSite();

  const showLink = (link: Link | undefined) => {
    const state = link?.state ?? "unpaired";
    status.dataset.state = state;
    status.textContent =
      state === "rejected" ? strings.rejected(link?.error ?? null) : strings[state];
  };

  void chrome.storage.local.get(["token", "port"]).then((stored) => {
    tokenInput.value = typeof stored.token === "string" ? stored.token : "";
    portInput.value = String(typeof stored.port === "number" ? stored.port : DEFAULT_PORT);
  });
  void chrome.storage.session.get("link").then((stored) => showLink(stored.link as Link));
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area === "session" && changes.link) showLink(changes.link.newValue as Link);
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const port = Number(portInput.value);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      note.textContent = strings.badPort;
      return;
    }
    void chrome.storage.local
      .set({ token: tokenInput.value.trim(), port })
      .then(() => chrome.runtime.sendMessage({ type: "cricket-settings-changed" }))
      .then(() => {
        note.textContent = strings.saved;
      })
      .catch(() => undefined);
  });
})();
