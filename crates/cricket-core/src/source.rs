//! What can be a music source: a desktop app or one browser tab. Tabs are
//! reached through a [`TabBridge`] (the browser extension).

use std::fmt;

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
    },
}

impl Source {
    pub fn app(name: &str) -> Self {
        Self::App {
            app: AppId::new(name),
        }
    }

    /// Whether both refer to the same app or the same tab. A tab's title is
    /// not part of its identity.
    pub fn same_target(&self, other: &Source) -> bool {
        match (self, other) {
            (Self::App { app: a }, Self::App { app: b }) => a == b,
            (Self::Tab { id: a, .. }, Self::Tab { id: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::App { app } => write!(f, "{app}"),
            Self::Tab { id, title } => write!(f, "tab {id} ({title})"),
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
    },
}

impl From<SourceRepr> for Source {
    fn from(repr: SourceRepr) -> Self {
        match repr {
            SourceRepr::Tagged(TaggedSource::App { app }) => Self::App { app },
            SourceRepr::Tagged(TaggedSource::Tab { id, title }) => Self::Tab { id, title },
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
    fn select(&mut self, tab: Option<TabId>);

    fn playback_state(&mut self, tab: TabId) -> BackendResult<PlaybackState>;
    fn volume(&mut self, tab: TabId) -> BackendResult<f32>;
    fn set_volume(&mut self, tab: TabId, volume: f32) -> BackendResult<()>;
    fn pause(&mut self, tab: TabId) -> BackendResult<()>;
    fn play(&mut self, tab: TabId) -> BackendResult<()>;
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

    fn select(&mut self, _tab: Option<TabId>) {}

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
                title: "Mix".to_string()
            })
            .unwrap(),
            r#"{"kind":"tab","id":7,"title":"Mix"}"#
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
        let before = Source::Tab {
            id: TabId(7),
            title: "Song A".to_string(),
        };
        let after = Source::Tab {
            id: TabId(7),
            title: "Song B".to_string(),
        };
        let other = Source::Tab {
            id: TabId(8),
            title: "Song A".to_string(),
        };

        assert!(before.same_target(&after));
        assert!(!before.same_target(&other));
        assert!(!before.same_target(&Source::app("chrome.exe")));
    }
}
