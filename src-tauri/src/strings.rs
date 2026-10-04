//! User-facing text shown by the Rust side (tray icon and menu). Kept in one
//! place so it can be localized later. Window text lives in `ui/src/strings.ts`.

use cricket_core::engine::State;

pub const APP_NAME: &str = "Cricket";
pub const TRAY_SHOW: &str = "Show Cricket";
pub const TRAY_ENABLED: &str = "Enabled";
pub const TRAY_QUIT: &str = "Quit";

pub fn tray_tooltip(state: State, has_source: bool) -> String {
    let status = match state {
        State::Idle if has_source => "off",
        State::Idle => "no music source",
        State::Playing => "music playing",
        State::FadingOut => "fading out",
        State::PausedByCricket => "music paused by Cricket",
        State::FadingIn => "resuming",
        State::PausedByUser => "music not playing",
    };
    format!("{APP_NAME}: {status}")
}
