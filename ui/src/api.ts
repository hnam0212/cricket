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
  | { kind: "microphone"; app: string };

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
  source: string | null;
  enabled: boolean;
  playback: PlaybackState | null;
  volume: number | null;
  activity: Activity | null;
  apps: AppView[];
  mic_users: string[];
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
  source: string | null;
  enabled: boolean;
  settings: Settings;
  minimize_to_tray: boolean;
  mini_mode: boolean;
  show_diagnostics: boolean;
}

export const api = {
  coreInfo: () => invoke<string>("core_info"),
  getStatus: () => invoke<Status>("get_status"),
  getConfig: () => invoke<AppConfig>("get_config"),
  setSource: (source: string | null) => invoke<AppConfig>("set_source", { source }),
  setEnabled: (enabled: boolean) => invoke<AppConfig>("set_enabled", { enabled }),
  setSettings: (settings: Settings) => invoke<AppConfig>("set_settings", { settings }),
  setMinimizeToTray: (value: boolean) => invoke<AppConfig>("set_minimize_to_tray", { value }),
  setShowDiagnostics: (value: boolean) => invoke<AppConfig>("set_show_diagnostics", { value }),
  setMiniMode: (value: boolean) => invoke<AppConfig>("set_mini_mode", { value }),
};
