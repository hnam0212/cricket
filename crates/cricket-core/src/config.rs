//! The app's saved configuration: versioned JSON (architecture rule 7).
//!
//! Only the format lives here. Reading and writing the file is up to the
//! app, which knows where the OS keeps app config.

use serde::{Deserialize, Serialize};

use crate::settings::Settings;
use crate::source::Source;

pub const CONFIG_VERSION: u32 = 2;

/// Missing fields take their defaults and unknown fields are ignored, so a
/// file written by an older or newer Cricket still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    /// The music source. `None` until the user picks one.
    pub source: Option<Source>,
    /// The main on/off switch.
    pub enabled: bool,
    pub settings: Settings,
    /// Closing the window keeps Cricket running in the tray.
    pub minimize_to_tray: bool,
    /// The window shows only the status and the on/off switch.
    pub mini_mode: bool,
    pub show_diagnostics: bool,
    /// Secret the browser extension must present. Empty until the app
    /// generates one on first start.
    pub bridge_token: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            source: None,
            enabled: true,
            settings: Settings::default(),
            minimize_to_tray: true,
            mini_mode: false,
            show_diagnostics: false,
            bridge_token: String::new(),
        }
    }
}

impl AppConfig {
    /// Parses a saved configuration. Values out of range are pulled back
    /// into range. Fails only if the text is not a usable JSON object, in
    /// which case the caller should fall back to the defaults.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let mut config: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        config.version = CONFIG_VERSION;
        config.settings = config.settings.sanitized();
        Ok(config)
    }

    pub fn to_json(&self) -> String {
        // Serializing plain data to a string cannot fail.
        serde_json::to_string_pretty(self).expect("config serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let config = AppConfig {
            source: Some(Source::app("spotify.exe")),
            enabled: false,
            mini_mode: true,
            settings: Settings {
                fade_out_ms: 3000,
                ..Settings::default()
            },
            ..AppConfig::default()
        };

        assert_eq!(AppConfig::from_json(&config.to_json()), Ok(config));
    }

    #[test]
    fn empty_object_gives_the_defaults() {
        assert_eq!(AppConfig::from_json("{}"), Ok(AppConfig::default()));
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let config =
            AppConfig::from_json(r#"{"enabled": false, "settings": {"fade_in_ms": 400}}"#).unwrap();

        assert!(!config.enabled);
        assert_eq!(config.settings.fade_in_ms, 400);
        assert_eq!(config.settings.fade_out_ms, Settings::default().fade_out_ms);
        assert!(config.minimize_to_tray);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let config = AppConfig::from_json(
            r#"{"version": 7, "future_thing": [1, 2], "settings": {"also_new": true}}"#,
        )
        .unwrap();

        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn the_source_is_normalized() {
        let config = AppConfig::from_json(r#"{"source": " Spotify.EXE "}"#).unwrap();
        assert_eq!(config.source, Some(Source::app("spotify.exe")));
    }

    #[test]
    fn out_of_range_values_are_pulled_back() {
        let config = AppConfig::from_json(
            r#"{"settings": {"sound_threshold": 7.5, "fade_out_ms": 99999999}}"#,
        )
        .unwrap();

        assert_eq!(config.settings.sound_threshold, 1.0);
        assert_eq!(config.settings.fade_out_ms, 60_000);
    }

    #[test]
    fn broken_json_is_an_error() {
        assert!(AppConfig::from_json("not json").is_err());
        assert!(AppConfig::from_json("[1, 2]").is_err());
    }
}
