// Runs inside every page, in the extension's isolated world. It only passes
// messages along: the page-world script (hook.ts) cannot talk to the
// extension, and the extension cannot see the page's players, so the two
// meet here through DOM events on the shared document.
//
// Must stay a plain script (no imports or exports).
(() => {
  const TO_PAGE = "cricket:to-page";
  const FROM_PAGE = "cricket:from-page";

  chrome.runtime.onMessage.addListener((message: { target?: string; payload?: unknown }) => {
    if (message?.target === "cricket-page") {
      document.dispatchEvent(
        new CustomEvent(TO_PAGE, { detail: JSON.stringify(message.payload) }),
      );
    }
  });

  document.addEventListener(FROM_PAGE, (event) => {
    try {
      const state: unknown = JSON.parse((event as CustomEvent<string>).detail);
      // Rejects if the service worker is asleep or the extension was
      // reloaded under this page; the next report tries again.
      chrome.runtime
        .sendMessage({ type: "cricket-state", state })
        .catch(() => undefined);
    } catch {
      // Malformed detail, or the extension context is gone. Nothing to do.
    }
  });
})();
