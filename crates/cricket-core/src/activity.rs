//! Decides whether "other sound" is present in a snapshot (SPEC.md 5.1).

use std::fmt;

use crate::audio::{ActivitySnapshot, AppId};
use crate::settings::Settings;

/// Why the machine counts as busy. Kept for logs and the diagnostics view,
/// so the user can see why the music paused.
#[derive(Debug, Clone, PartialEq)]
pub enum ActivityCause {
    Sound { app: AppId, peak: f32 },
    Microphone { app: AppId },
}

impl fmt::Display for ActivityCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sound { app, peak } => write!(f, "sound from {app} (peak {peak:.3})"),
            Self::Microphone { app } => write!(f, "microphone in use by {app}"),
        }
    }
}

/// The activity that should keep the music paused, if any: the loudest app
/// above the threshold, otherwise a microphone user. The music source itself
/// never counts.
pub fn detect_activity(
    snapshot: &ActivitySnapshot,
    source: &AppId,
    settings: &Settings,
) -> Option<ActivityCause> {
    let loudest = snapshot
        .apps
        .iter()
        .filter(|app| &app.app != source)
        .filter(|app| !(settings.ignore_system_sounds && app.is_system_sounds))
        .filter(|app| app.peak >= settings.sound_threshold)
        .max_by(|a, b| a.peak.total_cmp(&b.peak));
    if let Some(app) = loudest {
        return Some(ActivityCause::Sound {
            app: app.app.clone(),
            peak: app.peak,
        });
    }

    if settings.mic_counts_as_activity {
        if let Some(app) = snapshot.mic_users.iter().find(|app| *app != source) {
            return Some(ActivityCause::Microphone { app: app.clone() });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AppActivity;

    fn app(name: &str, peak: f32) -> AppActivity {
        AppActivity {
            app: AppId::new(name),
            pids: vec![1],
            session_count: 1,
            active: true,
            peak,
            is_system_sounds: false,
        }
    }

    fn snapshot(apps: Vec<AppActivity>, mic_users: &[&str]) -> ActivitySnapshot {
        ActivitySnapshot {
            apps,
            mic_users: mic_users.iter().map(|name| AppId::new(name)).collect(),
        }
    }

    fn source() -> AppId {
        AppId::new("spotify.exe")
    }

    #[test]
    fn the_music_source_is_not_activity() {
        let snapshot = snapshot(vec![app("spotify.exe", 0.9)], &[]);
        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            None
        );
    }

    #[test]
    fn another_app_above_the_threshold_is_activity() {
        let snapshot = snapshot(vec![app("spotify.exe", 0.9), app("chrome.exe", 0.3)], &[]);
        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            Some(ActivityCause::Sound {
                app: AppId::new("chrome.exe"),
                peak: 0.3
            })
        );
    }

    #[test]
    fn sound_below_the_threshold_is_silence() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.019)], &[]);
        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            None
        );
    }

    #[test]
    fn the_loudest_app_is_reported() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.1), app("game.exe", 0.6)], &[]);
        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            Some(ActivityCause::Sound {
                app: AppId::new("game.exe"),
                peak: 0.6
            })
        );
    }

    #[test]
    fn system_sounds_are_ignored_unless_enabled() {
        let mut system = app("system sounds", 0.5);
        system.is_system_sounds = true;
        let snapshot = snapshot(vec![system], &[]);

        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            None
        );

        let settings = Settings {
            ignore_system_sounds: false,
            ..Settings::default()
        };
        assert!(detect_activity(&snapshot, &source(), &settings).is_some());
    }

    #[test]
    fn microphone_use_is_activity_unless_disabled() {
        let snapshot = snapshot(vec![], &["zoom.exe"]);

        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            Some(ActivityCause::Microphone {
                app: AppId::new("zoom.exe")
            })
        );

        let settings = Settings {
            mic_counts_as_activity: false,
            ..Settings::default()
        };
        assert_eq!(detect_activity(&snapshot, &source(), &settings), None);
    }

    #[test]
    fn the_source_using_the_microphone_is_not_activity() {
        let snapshot = snapshot(vec![], &["spotify.exe"]);
        assert_eq!(
            detect_activity(&snapshot, &source(), &Settings::default()),
            None
        );
    }
}
