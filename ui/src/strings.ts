// All user-facing text goes through this file so it can be localized later.
import type { Activity, Status } from "./api";

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
    "Pick the app whose music Cricket should pause. An app shows up here once it has played sound.",
  sourceNone: "None",
  sourceEmpty: "No apps with audio yet. Start your music app and play something.",
  sourceNotRunning: "not playing sound right now",
  sourceSelected: "selected",
  makingSound: "making sound",
  silent: "silent",

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
  diagnosticsEngine: "Engine",
  diagnosticsLog: "Event log",
  colApp: "App",
  colPeak: "Peak",
  colState: "State",
  colSessions: "Sessions",
  active: "active",
  inactive: "inactive",
  systemSounds: "system sounds",
  microphone: "Microphone",
  micNotInUse: "not in use",
  micInUseBy: (apps: string[]) => `in use by ${apps.join(", ")}`,
  engineState: "State",
  engineReason: "Reason",
  enginePlayback: "Source playback",
  engineVolume: "Source volume",
  engineActivity: "Activity",
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

export function activityText(activity: Activity | null): string {
  if (!activity) return strings.none;
  return activity.kind === "sound"
    ? `sound from ${activity.app}`
    : `microphone in use by ${activity.app}`;
}

/** The headline and the detail line of the status area. */
export function statusText(status: Status): { headline: string; detail: string } {
  if (status.fatal) {
    return { headline: "Audio is unavailable", detail: status.fatal };
  }
  if (!status.source) {
    return { headline: "No music source", detail: "Pick the app that plays your music." };
  }
  if (!status.enabled) {
    return { headline: "Cricket is off", detail: "Your music is left alone." };
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
      return status.playback === null
        ? {
            headline: "Music source not available",
            detail: `${status.source} is not open, or does not offer media controls.`,
          }
        : {
            headline: "Music is not playing",
            detail: "Cricket did not pause it, so it will not start it. Press play to continue.",
          };
    case "Idle":
      return { headline: strings.loading, detail: "" };
  }
}
