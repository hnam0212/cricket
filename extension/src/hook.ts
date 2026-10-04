// Runs inside every page, in the page's own JavaScript world, from the very
// start of the page load. It finds the page's media players and, when told
// to by the Cricket app, pauses, resumes and fades them.
//
// It has to live in the page's world: players such as Spotify Web create
// audio elements that are never added to the document, so the only way to
// find them is to watch calls to play(). And the cleanest way to pause is the
// page's own media-key handler, which is also only visible from here.
//
// This file must stay a plain script (no imports or exports): Chrome loads
// content scripts as classic scripts. Everything is inside one function so
// nothing leaks into the page's globals.
(() => {
  const marker = "__cricketHookInstalled";
  const globals = window as unknown as Record<string, unknown>;
  if (globals[marker]) return;
  globals[marker] = true;

  /** Messages from the extension arrive as this DOM event (see relay.ts). */
  const TO_PAGE = "cricket:to-page";
  const FROM_PAGE = "cricket:from-page";
  const REPORT_INTERVAL_MS = 250;
  /** The extension re-arms the source tab every few seconds. Without that
   *  for this long, the tab is no longer the source and stops reporting. */
  const ARM_TIMEOUT_MS = 10_000;
  /** Time the page's own handler gets before the direct fallback is used. */
  const HANDLER_GRACE_MS = 200;

  // --- Finding media elements -------------------------------------------

  // Weak references: tracking an element must not keep a page's discarded
  // players alive.
  const tracked: WeakRef<HTMLMediaElement>[] = [];
  const known = new WeakSet<HTMLMediaElement>();

  const track = (element: HTMLMediaElement) => {
    if (!known.has(element)) {
      known.add(element);
      tracked.push(new WeakRef(element));
    }
  };

  const originalPlay = HTMLMediaElement.prototype.play;
  HTMLMediaElement.prototype.play = function (this: HTMLMediaElement) {
    track(this);
    return originalPlay.call(this);
  };

  const elements = (): HTMLMediaElement[] => {
    document.querySelectorAll<HTMLMediaElement>("video, audio").forEach(track);
    const live: HTMLMediaElement[] = [];
    for (let i = tracked.length - 1; i >= 0; i--) {
      const element = tracked[i].deref();
      if (element) live.push(element);
      else tracked.splice(i, 1);
    }
    return live;
  };

  // --- The page's own media-key handlers --------------------------------

  // Sites register these for hardware media keys. Calling them pauses the
  // way the site intends, so its play button stays in sync.
  const handlers = new Map<string, MediaSessionActionHandler>();
  if ("mediaSession" in navigator) {
    const session = navigator.mediaSession;
    const originalSet = session.setActionHandler.bind(session);
    session.setActionHandler = (action, handler) => {
      if (handler) handlers.set(action, handler);
      else handlers.delete(action);
      originalSet(action, handler);
    };
  }

  const callHandler = (action: "play" | "pause"): boolean => {
    const handler = handlers.get(action);
    if (!handler) return false;
    try {
      handler({ action });
      return true;
    } catch {
      return false;
    }
  };

  // --- State and commands -----------------------------------------------

  // A muted element is not what the user is listening to (autoplaying
  // previews, background video), so it neither counts as playing nor is it
  // touched.
  const audible = (element: HTMLMediaElement) => !element.muted;
  const isPlaying = (element: HTMLMediaElement) =>
    !element.paused && !element.ended && audible(element);

  /** Elements Cricket paused, so resume restarts exactly those. */
  let pausedByCricket: HTMLMediaElement[] = [];

  const pause = () => {
    const playing = elements().filter(isPlaying);
    if (playing.length === 0) return;
    pausedByCricket = playing;
    const handled = callHandler("pause");
    window.setTimeout(
      () => playing.filter(isPlaying).forEach((element) => element.pause()),
      handled ? HANDLER_GRACE_MS : 0,
    );
  };

  const resume = () => {
    const targets = pausedByCricket.filter((element) => element.paused);
    pausedByCricket = [];
    const handled = callHandler("play");
    window.setTimeout(
      () =>
        targets
          .filter((element) => element.paused)
          // play() is refused if the browser decides the page may not
          // start audio; nothing useful can be done about that here.
          .forEach((element) => void element.play().catch(() => undefined)),
      handled ? HANDLER_GRACE_MS : 0,
    );
  };

  const setVolume = (volume: number) => {
    const level = Math.min(Math.max(volume, 0), 1);
    for (const element of elements()) {
      if (audible(element) && (isPlaying(element) || pausedByCricket.includes(element))) {
        element.volume = level;
      }
    }
  };

  const report = () => {
    const candidates = elements().filter(
      (element) => isPlaying(element) || pausedByCricket.includes(element),
    );
    const state = {
      hasMedia: candidates.length > 0,
      playing: candidates.some(isPlaying),
      volume: candidates.length > 0 ? Math.max(...candidates.map((e) => e.volume)) : null,
    };
    // A string, because objects do not reliably cross between the page's
    // world and the extension's.
    document.dispatchEvent(new CustomEvent(FROM_PAGE, { detail: JSON.stringify(state) }));
  };

  // --- Arming -----------------------------------------------------------

  let timer: number | undefined;
  let armedAt = 0;

  const disarm = () => {
    window.clearInterval(timer);
    timer = undefined;
  };

  const arm = () => {
    armedAt = Date.now();
    if (timer !== undefined) return;
    timer = window.setInterval(() => {
      if (Date.now() - armedAt > ARM_TIMEOUT_MS) disarm();
      else report();
    }, REPORT_INTERVAL_MS);
    report();
  };

  document.addEventListener(TO_PAGE, (event) => {
    let message: { arm?: boolean; cmd?: string; volume?: number };
    try {
      message = JSON.parse((event as CustomEvent<string>).detail);
    } catch {
      return;
    }
    if (message.arm === true) arm();
    if (message.arm === false) disarm();
    if (message.cmd === "pause") pause();
    if (message.cmd === "resume") resume();
    if (message.cmd === "set_volume" && typeof message.volume === "number") {
      setVolume(message.volume);
    }
  });
})();
