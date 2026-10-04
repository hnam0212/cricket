// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";

export type EngineState =
  | "Idle"
  | "Playing"
  | "FadingOut"
  | "PausedByCricket"
  | "FadingIn"
  | "PausedByUser";

export type PlaybackState = "Playing" | "Paused" | "Stopped" | "Unknown";

export type Activity =
  | { kind: "sound"; app: string; peak: number }
  | { kind: "microphone"; app: string }
  | { kind: "tab"; title: string };

/** The music source: a desktop app or one browser tab. */
export type Source =
  | { kind: "app"; app: string }
  | {
      kind: "tab";
      id: number;
      title: string;
      /** Which browser run the id belongs to. Filled in by the app when picking. */
      session?: string;
    };

export interface TabInfo {
  id: number;
  title: string;
  audible: boolean;
  /** The extension can reach this tab's player (the site is allowed). */
  controllable: boolean;
}

export interface BridgeStatus {
  connected: boolean;
  port: number;
  error: string | null;
  /** Executable of the browser whose sound is ignored for a tab source. */
  browser: string | null;
  browser_source: "chosen" | "detected" | "reported" | "assumed" | null;
}

export interface AppView {
  app: string;
  peak: number;
  making_sound: boolean;
  active: boolean;
  sessions: number;
  is_system_sounds: boolean;
}

export interface LogEvent {
  at: number;
  text: string;
}

export interface Status {
  state: EngineState;
  reason: string;
  source: Source | null;
  enabled: boolean;
  playback: PlaybackState | null;
  /** False while a browser tab has no live link; Cricket holds its state. */
  source_available: boolean;
  /** The tab was picked before a browser restart; it must be picked again. */
  source_stale: boolean;
  volume: number | null;
  activity: Activity | null;
  apps: AppView[];
  mic_users: string[];
  tabs: TabInfo[];
  bridge: BridgeStatus;
  errors: string[];
  events: LogEvent[];
  fatal: string | null;
}

export interface Settings {
  trigger_delay_ms: number;
  resume_cooldown_ms: number;
  fade_out_ms: number;
  fade_in_ms: number;
  sound_threshold: number;
  mic_counts_as_activity: boolean;
  ignore_system_sounds: boolean;
}

export interface AppConfig {
  version: number;
  source: Source | null;
  enabled: boolean;
  settings: Settings;
  minimize_to_tray: boolean;
  mini_mode: boolean;
  show_diagnostics: boolean;
  bridge_token: string;
  browser_override: string | null;
}

export const api = {
  coreInfo: () => invoke<string>("core_info"),
  getStatus: () => invoke<Status>("get_status"),
  getConfig: () => invoke<AppConfig>("get_config"),
  setSource: (source: Source | null) => invoke<AppConfig>("set_source", { source }),
  setEnabled: (enabled: boolean) => invoke<AppConfig>("set_enabled", { enabled }),
  setSettings: (settings: Settings) => invoke<AppConfig>("set_settings", { settings }),
  setMinimizeToTray: (value: boolean) => invoke<AppConfig>("set_minimize_to_tray", { value }),
  setShowDiagnostics: (value: boolean) => invoke<AppConfig>("set_show_diagnostics", { value }),
  setMiniMode: (value: boolean) => invoke<AppConfig>("set_mini_mode", { value }),
  regenerateToken: () => invoke<AppConfig>("regenerate_token"),
  setBrowserOverride: (value: string | null) =>
    invoke<AppConfig>("set_browser_override", { value }),
};
