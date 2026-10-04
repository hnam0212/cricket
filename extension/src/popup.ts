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
