//! What can be a music source: a desktop app or one browser tab. Tabs are
//! reached through a [`TabBridge`] (the browser extension).

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::audio::{AppId, BackendError, BackendResult, PlaybackState};

/// A browser tab id. Only valid while that browser session lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(pub i64);

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One open browser tab, as reported by the extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: TabId,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    /// The browser considers the tab to be making sound.
    #[serde(default)]
    pub audible: bool,
    /// The extension can reach this page's player. `false` for sites the
    /// user has not allowed yet; such a tab still counts as activity when
    /// it is audible, but cannot be the music source. Extensions that do
    /// not say are assumed to manage.
    #[serde(default = "controllable_by_default")]
    pub controllable: bool,
}

fn controllable_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SourceRepr", tag = "kind", rename_all = "snake_case")]
pub enum Source {
    App {
        app: AppId,
    },
    Tab {
        id: TabId,
        /// The title when the tab was picked. Only for display while the
        /// live tab list is not available; it goes stale as tracks change.
        title: String,
        /// Which browser session the tab id belongs to. Tab ids are only
        /// unique within one run of the browser: after a restart the same
        /// number can be a different tab. `None` for sources saved before
        /// sessions existed, and until the app fills it in on picking.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
}

impl Source {
    pub fn app(name: &str) -> Self {
        Self::App {
            app: AppId::new(name),
        }
    }

    /// Whether both refer to the same app or the same tab. A tab's title is
    /// not part of its identity; its browser session is.
    pub fn same_target(&self, other: &Source) -> bool {
        match (self, other) {
            (Self::App { app: a }, Self::App { app: b }) => a == b,
            (
                Self::Tab {
                    id: a, session: sa, ..
                },
                Self::Tab {
                    id: b, session: sb, ..
                },
            ) => a == b && sa == sb,
            _ => false,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::App { app } => write!(f, "{app}"),
            Self::Tab { id, title, .. } => write!(f, "tab {id} ({title})"),
        }
    }
}

/// Accepts the current tagged form and the bare app name that configuration
/// version 1 saved.
#[derive(Deserialize)]
#[serde(untagged)]
enum SourceRepr {
    Tagged(TaggedSource),
    LegacyApp(String),
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TaggedSource {
    App {
        app: AppId,
    },
    Tab {
        id: TabId,
        #[serde(default)]
        title: String,
        #[serde(default)]
        session: Option<String>,
    },
}

impl From<SourceRepr> for Source {
    fn from(repr: SourceRepr) -> Self {
        match repr {
            SourceRepr::Tagged(TaggedSource::App { app }) => Self::App { app },
            SourceRepr::Tagged(TaggedSource::Tab { id, title, session }) => {
                Self::Tab { id, title, session }
            }
            SourceRepr::LegacyApp(name) => Self::app(&name),
        }
    }
}

/// The link to the browser extension: lists tabs and controls one of them.
pub trait TabBridge {
    /// An extension is connected and paired.
    fn connected(&mut self) -> bool;

    /// The browser's executable. When a tab is the music source, the desktop
    /// side ignores this whole process for sound, because it contains the
    /// music itself; the extension reports the other tabs instead.
    fn browser(&mut self) -> AppId;

    fn tabs(&mut self) -> Vec<TabInfo>;

    /// Tells the extension which tab is the music source, so it reports that
    /// tab's playback state.
    ///
    /// `session` is the browser session the tab id was picked in. If the
    /// extension is in a different session now (the browser was restarted),
    /// the id means nothing and must not be selected.
    fn select(&mut self, tab: Option<TabId>, session: Option<&str>);

    /// The selected tab was picked in another browser session than the one
    /// that is connected, so it cannot be told apart from some other tab.
    fn selection_stale(&mut self) -> bool {
        false
    }

    fn playback_state(&mut self, tab: TabId) -> BackendResult<PlaybackState>;
    fn volume(&mut self, tab: TabId) -> BackendResult<f32>;
    fn set_volume(&mut self, tab: TabId, volume: f32) -> BackendResult<()>;
    fn pause(&mut self, tab: TabId) -> BackendResult<()>;
    fn play(&mut self, tab: TabId) -> BackendResult<()>;

    /// Waits, for at most `timeout`, until queued commands have been handed
    /// to the extension. Called on shutdown so a last "restore the volume"
    /// is not lost. A bridge with nothing queued returns at once.
    fn flush(&mut self, _timeout: Duration) {}
}

/// A bridge with no browser behind it, for when only desktop apps can be a
/// source (the debug CLI).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTabs;

impl NoTabs {
    fn unavailable<T>() -> BackendResult<T> {
        Err(BackendError::Unavailable(
            "no browser extension".to_string(),
        ))
    }
}

impl TabBridge for NoTabs {
    fn connected(&mut self) -> bool {
        false
    }

    fn browser(&mut self) -> AppId {
        AppId::new("chrome.exe")
    }

    fn tabs(&mut self) -> Vec<TabInfo> {
        Vec::new()
    }

    fn select(&mut self, _tab: Option<TabId>, _session: Option<&str>) {}

    fn playback_state(&mut self, _tab: TabId) -> BackendResult<PlaybackState> {
        Self::unavailable()
    }

    fn volume(&mut self, _tab: TabId) -> BackendResult<f32> {
        Self::unavailable()
    }

    fn set_volume(&mut self, _tab: TabId, _volume: f32) -> BackendResult<()> {
        Self::unavailable()
    }

    fn pause(&mut self, _tab: TabId) -> BackendResult<()> {
        Self::unavailable()
    }

    fn play(&mut self, _tab: TabId) -> BackendResult<()> {
        Self::unavailable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_round_trip_through_json() {
        for source in [
            Source::app("spotify.exe"),
            Source::Tab {
                id: TabId(42),
                title: "YouTube Music".to_string(),
                session: None,
            },
            Source::Tab {
                id: TabId(42),
                title: "YouTube Music".to_string(),
                session: Some("abc".to_string()),
            },
        ] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(serde_json::from_str::<Source>(&json).unwrap(), source);
        }
    }

    #[test]
    fn json_shape_is_tagged_by_kind() {
        assert_eq!(
            serde_json::to_string(&Source::app("spotify.exe")).unwrap(),
            r#"{"kind":"app","app":"spotify.exe"}"#
        );
        assert_eq!(
            serde_json::to_string(&Source::Tab {
                id: TabId(7),
                title: "Mix".to_string(),
                session: None,
            })
            .unwrap(),
            r#"{"kind":"tab","id":7,"title":"Mix"}"#
        );
    }

    #[test]
    fn a_tab_saved_without_a_session_still_loads() {
        let source: Source =
            serde_json::from_str(r#"{"kind":"tab","id":3,"title":"Mix"}"#).unwrap();
        assert_eq!(
            source,
            Source::Tab {
                id: TabId(3),
                title: "Mix".to_string(),
                session: None
            }
        );
    }

    #[test]
    fn a_bare_app_name_from_an_old_config_still_loads() {
        assert_eq!(
            serde_json::from_str::<Source>(r#""Spotify.exe""#).unwrap(),
            Source::app("spotify.exe")
        );
    }

    #[test]
    fn a_tab_is_the_same_target_whatever_its_title() {
        let tab = |id: i64, title: &str, session: Option<&str>| Source::Tab {
            id: TabId(id),
            title: title.to_string(),
            session: session.map(str::to_string),
        };
        let before = tab(7, "Song A", Some("s1"));
        let after = tab(7, "Song B", Some("s1"));
        let other = tab(8, "Song A", Some("s1"));
        let other_session = tab(7, "Song A", Some("s2"));

        assert!(before.same_target(&after));
        assert!(!before.same_target(&other));
        // The same number after a browser restart is not the same tab.
        assert!(!before.same_target(&other_session));
        assert!(!before.same_target(&Source::app("chrome.exe")));
    }
}
