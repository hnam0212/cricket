//! Decides whether "other sound" is present in a snapshot (SPEC.md 5.1).

use std::fmt;

use crate::audio::{ActivitySnapshot, AppId};
use crate::settings::Settings;
use crate::source::{TabId, TabInfo};

/// Why the machine counts as busy. Kept for logs and the diagnostics view,
/// so the user can see why the music paused.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityCause {
    Sound {
        app: AppId,
        peak: f32,
    },
    Microphone {
        app: AppId,
    },
    /// Another browser tab is making sound.
    Tab {
        title: String,
    },
}

impl fmt::Display for ActivityCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sound { app, peak } => write!(f, "sound from {app} (peak {peak:.3})"),
            Self::Microphone { app } => write!(f, "microphone in use by {app}"),
            Self::Tab { title } => write!(f, "sound from the tab \"{title}\""),
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
    loudest_app(snapshot, source, settings)
        .or_else(|| microphone_user(snapshot, Some(source), settings))
}

/// The same question when the music source is a browser tab.
///
/// The desktop side cannot tell tabs apart: the whole browser is one app,
/// and it contains the music. So the browser's sound is ignored here and
/// the extension's list of audible tabs stands in for it. Microphone use by
/// the browser still counts, since a call in another tab is exactly when the
/// music should stop.
pub fn detect_tab_activity(
    snapshot: &ActivitySnapshot,
    browser: &AppId,
    tabs: &[TabInfo],
    source: TabId,
    settings: &Settings,
) -> Option<ActivityCause> {
    loudest_app(snapshot, browser, settings)
        .or_else(|| {
            tabs.iter()
                .find(|tab| tab.audible && tab.id != source)
                .map(|tab| ActivityCause::Tab {
                    title: tab.title.clone(),
                })
        })
        .or_else(|| microphone_user(snapshot, None, settings))
}

fn microphone_user(
    snapshot: &ActivitySnapshot,
    excluded: Option<&AppId>,
    settings: &Settings,
) -> Option<ActivityCause> {
    if !settings.mic_counts_as_activity {
        return None;
    }
    snapshot
        .mic_users
        .iter()
        .find(|app| Some(*app) != excluded)
        .map(|app| ActivityCause::Microphone { app: app.clone() })
}

fn loudest_app(
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
    loudest.map(|app| ActivityCause::Sound {
        app: app.app.clone(),
        peak: app.peak,
    })
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

    fn tab(id: i64, title: &str, audible: bool) -> TabInfo {
        TabInfo {
            id: TabId(id),
            title: title.to_string(),
            audible,
            controllable: true,
        }
    }

    fn chrome() -> AppId {
        AppId::new("chrome.exe")
    }

    #[test]
    fn with_a_tab_source_the_browser_process_is_not_activity() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.9)], &[]);
        let tabs = [tab(1, "Music", true)];

        assert_eq!(
            detect_tab_activity(&snapshot, &chrome(), &tabs, TabId(1), &Settings::default()),
            None
        );
    }

    #[test]
    fn another_audible_tab_is_activity() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.9)], &[]);
        let tabs = [tab(1, "Music", true), tab(2, "Cat video", true)];

        assert_eq!(
            detect_tab_activity(&snapshot, &chrome(), &tabs, TabId(1), &Settings::default()),
            Some(ActivityCause::Tab {
                title: "Cat video".to_string()
            })
        );
    }

    #[test]
    fn a_silent_other_tab_is_not_activity() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.9)], &[]);
        let tabs = [tab(1, "Music", true), tab(2, "Docs", false)];

        assert_eq!(
            detect_tab_activity(&snapshot, &chrome(), &tabs, TabId(1), &Settings::default()),
            None
        );
    }

    #[test]
    fn with_a_tab_source_a_desktop_app_is_still_activity() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.9), app("zoom.exe", 0.2)], &[]);
        let tabs = [tab(1, "Music", true)];

        assert_eq!(
            detect_tab_activity(&snapshot, &chrome(), &tabs, TabId(1), &Settings::default()),
            Some(ActivityCause::Sound {
                app: AppId::new("zoom.exe"),
                peak: 0.2
            })
        );
    }

    #[test]
    fn with_a_tab_source_the_browser_using_the_microphone_is_activity() {
        let snapshot = snapshot(vec![app("chrome.exe", 0.9)], &["chrome.exe"]);
        let tabs = [tab(1, "Music", true)];

        assert_eq!(
            detect_tab_activity(&snapshot, &chrome(), &tabs, TabId(1), &Settings::default()),
            Some(ActivityCause::Microphone { app: chrome() })
        );
    }
}
