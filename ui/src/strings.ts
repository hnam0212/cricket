// All user-facing text goes through this file so it can be localized later.
import type { Activity, Source, Status, TabInfo } from "./api";

export const strings = {
  appName: "Cricket",
  loading: "Starting…",
  loadError: "Could not reach the Cricket backend:",

  enabledLabel: "Cricket is on",
  disabledLabel: "Cricket is off",
  miniModeOn: "Compact view",
  miniModeOff: "Full view",

  sourceHeading: "Music source",
  sourceHint:
    "Pick the app or browser tab whose music Cricket should pause. An app shows up here once it has played sound.",
  sourceNone: "None",
  sourceApps: "Apps",
  sourceTabs: "Browser tabs",
  sourceEmpty: "No apps with audio yet. Start your music app and play something.",
  sourceNotRunning: "not playing sound right now",
  tabNotAvailable: "tab not available",
  tabsNotConnected:
    "Connect the Cricket browser extension to pick a tab. See Browser extension below.",
  tabsEmpty: "The extension is connected but reported no tabs yet.",
  untitledTab: "(untitled tab)",
  makingSound: "making sound",
  silent: "silent",

  extensionHeading: "Browser extension",
  extensionConnected: "Connected",
  extensionNotConnected: "Not connected",
  extensionFailed: (port: number, error: string) =>
    `Cricket could not listen on port ${port}: ${error}`,
  extensionIntro:
    "With the Cricket extension, one browser tab can be the music source, and other tabs count as activity.",
  extensionSteps: [
    "In Chrome, open chrome://extensions and turn on Developer mode.",
    "Choose Load unpacked and select the extension folder of the Cricket project.",
    "Click the Cricket icon in the toolbar, paste the pairing token below and save.",
    "Reload the tab that plays your music, then pick it under Music source.",
  ],
  extensionToken: "Pairing token",
  extensionPort: "Port",
  copy: "Copy",
  copied: "Copied",

  settingsHeading: "Settings",
  triggerDelay: "Trigger delay",
  triggerDelayHelp: "How long other sound must last before the music pauses.",
  resumeCooldown: "Resume cooldown",
  resumeCooldownHelp: "How much continuous silence before the music resumes.",
  fadeOut: "Fade out",
  fadeOutHelp: "Fade to silence before pausing. 0 pauses at once.",
  fadeIn: "Fade in",
  fadeInHelp: "Fade back up after resuming. 0 resumes at full volume.",
  threshold: "Sound threshold",
  thresholdHelp: "Peak level (0 to 1) below which an app counts as silent.",
  micCounts: "Treat microphone use as activity",
  ignoreSystemSounds: "Ignore Windows notification sounds",
  minimizeToTray: "Keep running in the tray when the window is closed",
  resetSettings: "Reset to defaults",
  milliseconds: "ms",

  diagnosticsToggle: "Show diagnostics",
  diagnosticsSessions: "Audio sessions",
  diagnosticsTabs: "Browser tabs",
  diagnosticsEngine: "Engine",
  diagnosticsLog: "Event log",
  colApp: "App",
  colPeak: "Peak",
  colState: "State",
  colSessions: "Sessions",
  colTab: "Tab",
  colAudible: "Audible",
  yes: "yes",
  no: "no",
  active: "active",
  inactive: "inactive",
  systemSounds: "system sounds",
  microphone: "Microphone",
  micNotInUse: "not in use",
  micInUseBy: (apps: string[]) => `in use by ${apps.join(", ")}`,
  engineState: "State",
  engineReason: "Reason",
  engineSource: "Source",
  enginePlayback: "Source playback",
  engineVolume: "Source volume",
  engineActivity: "Activity",
  engineExtension: "Browser extension",
  none: "none",
  unavailable: "unavailable",
  noEvents: "Nothing logged yet.",
  errors: "Errors",
} as const;

export const defaultSettings = {
  trigger_delay_ms: 500,
  resume_cooldown_ms: 3000,
  fade_out_ms: 2500,
  fade_in_ms: 1500,
  sound_threshold: 0.02,
  mic_counts_as_activity: true,
  ignore_system_sounds: true,
};

export function tabTitle(tab: { title: string }): string {
  return tab.title.trim() || strings.untitledTab;
}

/** A short name for the source. A tab's live title wins over the saved one. */
export function sourceName(source: Source, tabs: TabInfo[]): string {
  if (source.kind === "app") return source.app;
  const live = tabs.find((tab) => tab.id === source.id);
  return tabTitle(live ?? source);
}

export function activityText(activity: Activity | null): string {
  if (!activity) return strings.none;
  switch (activity.kind) {
    case "sound":
      return `sound from ${activity.app}`;
    case "microphone":
      return `microphone in use by ${activity.app}`;
    case "tab":
      return `sound from the tab "${tabTitle(activity)}"`;
  }
}

/** The headline and the detail line of the status area. */
/** A browser tab source that cannot be seen right now. Cricket keeps its state meanwhile. */
function unavailableText(status: Status): { headline: string; detail: string } {
  const reason = status.bridge.connected
    ? "The tab is closed, or needs a reload so the extension can reach its player."
    : "The browser extension is not connected.";
  const held =
    status.state === "PausedByCricket" || status.state === "FadingIn"
      ? "Cricket will pick up where it left off once the tab is back."
      : "Cricket will carry on once the tab is back.";
  return { headline: "Waiting for the browser tab", detail: `${reason} ${held}` };
}

export function statusText(status: Status): { headline: string; detail: string } {
  if (status.fatal) {
    return { headline: "Audio is unavailable", detail: status.fatal };
  }
  if (!status.source) {
    return { headline: "No music source", detail: "Pick the app or tab that plays your music." };
  }
  if (!status.enabled) {
    return { headline: "Cricket is off", detail: "Your music is left alone." };
  }

  if (!status.source_available) {
    return unavailableText(status);
  }

  const cause = activityText(status.activity);
  switch (status.state) {
    case "Playing":
      return {
        headline: "Music is playing",
        detail: status.activity ? `Hearing ${cause}…` : "Everything else is quiet.",
      };
    case "FadingOut":
      return { headline: "Fading out", detail: `Because of ${cause}.` };
    case "PausedByCricket":
      return {
        headline: "Music paused by Cricket",
        detail: status.activity
          ? `Waiting for quiet: ${cause}.`
          : "Quiet now. Resuming after the cooldown.",
      };
    case "FadingIn":
      return { headline: "Resuming", detail: "Fading the music back in." };
    case "PausedByUser":
      if (status.playback !== null) {
        return {
          headline: "Music is not playing",
          detail: "Cricket did not pause it, so it will not start it. Press play to continue.",
        };
      }
      return {
        headline: "Music source not available",
        detail:
          status.source.kind === "app"
            ? `${status.source.app} is not open, or does not offer media controls.`
            : "The tab is not reporting.",
      };
    case "Idle":
      return { headline: strings.loading, detail: "" };
  }
}
