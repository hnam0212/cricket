//! Platform-agnostic audio types and the two backend traits.
//!
//! Everything OS-specific lives behind [`AudioBackend`] and
//! [`MediaController`]. Nothing in this module touches an OS audio API.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Identifies an application by its executable name, lowercased
/// (for example `spotify.exe`).
///
/// One app can own several audio sessions and several processes (Chrome has
/// many), so the executable name is the unit Cricket reasons about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct AppId(String);

impl From<String> for AppId {
    fn from(name: String) -> Self {
        Self::new(&name)
    }
}

impl From<AppId> for String {
    fn from(app: AppId) -> Self {
        app.0
    }
}

impl AppId {
    pub fn new(name: &str) -> Self {
        Self(name.trim().to_lowercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One audio session as reported by the OS, before grouping.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub app: AppId,
    pub pid: u32,
    /// The OS considers the session active (a stream is running). An active
    /// session can still be silent; use `peak` for "is it making sound".
    pub active: bool,
    /// Peak level of the session, 0.0 to 1.0.
    pub peak: f32,
    /// The OS notification-sounds session.
    pub is_system_sounds: bool,
}

/// All sessions of one application, merged.
#[derive(Debug, Clone, PartialEq)]
pub struct AppActivity {
    pub app: AppId,
    /// Distinct process ids owning the sessions, ascending.
    pub pids: Vec<u32>,
    pub session_count: usize,
    /// At least one session is active.
    pub active: bool,
    /// Loudest session peak, 0.0 to 1.0.
    pub peak: f32,
    pub is_system_sounds: bool,
}

/// What the machine is doing at one instant.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ActivitySnapshot {
    /// Output activity per app, sorted by app id.
    pub apps: Vec<AppActivity>,
    /// Apps with an active capture (microphone) session, sorted.
    pub mic_users: Vec<AppId>,
}

impl ActivitySnapshot {
    pub fn mic_in_use(&self) -> bool {
        !self.mic_users.is_empty()
    }
}

/// Merges per-session readings into one entry per app, sorted by app id.
pub fn group_sessions(sessions: &[SessionInfo]) -> Vec<AppActivity> {
    let mut by_app: BTreeMap<&AppId, AppActivity> = BTreeMap::new();
    for session in sessions {
        let entry = by_app.entry(&session.app).or_insert_with(|| AppActivity {
            app: session.app.clone(),
            pids: Vec::new(),
            session_count: 0,
            active: false,
            peak: 0.0,
            is_system_sounds: false,
        });
        if !entry.pids.contains(&session.pid) {
            entry.pids.push(session.pid);
        }
        entry.session_count += 1;
        entry.active |= session.active;
        entry.peak = entry.peak.max(session.peak);
        entry.is_system_sounds |= session.is_system_sounds;
    }
    let mut apps: Vec<AppActivity> = by_app.into_values().collect();
    for app in &mut apps {
        app.pids.sort_unstable();
    }
    apps
}

/// Playback state of a media source as far as the OS can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
    /// The source exists but reports something else (opening, changing, ...).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The app has no audio session or no media session right now.
    AppNotFound(AppId),
    /// The OS accepted the call but refused the request (for example the
    /// media session rejected a pause).
    Rejected(String),
    /// The source cannot be reached right now (tab closed, browser
    /// extension not connected).
    Unavailable(String),
    /// An OS API call failed.
    Os(String),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AppNotFound(app) => write!(f, "app not found: {app}"),
            Self::Rejected(what) => write!(f, "request rejected: {what}"),
            Self::Unavailable(what) => write!(f, "unavailable: {what}"),
            Self::Os(message) => write!(f, "OS error: {message}"),
        }
    }
}

impl std::error::Error for BackendError {}

pub type BackendResult<T> = Result<T, BackendError>;

/// Reads audio activity and controls per-app volume.
///
/// Implementations may hold thread-bound OS handles, so a backend is created
/// and used on one thread (the audio polling thread).
pub trait AudioBackend {
    /// Current output sessions grouped by app, plus microphone users.
    /// The backend leaves its own process out.
    fn snapshot(&mut self) -> BackendResult<ActivitySnapshot>;

    /// Volume of the app's sessions, 0.0 to 1.0. If the sessions differ, the
    /// loudest one is returned.
    fn volume(&mut self, app: &AppId) -> BackendResult<f32>;

    /// Sets the volume of every session of the app, 0.0 to 1.0.
    fn set_volume(&mut self, app: &AppId, volume: f32) -> BackendResult<()>;
}

/// Pauses and resumes a media source.
pub trait MediaController {
    fn playback_state(&mut self, app: &AppId) -> BackendResult<PlaybackState>;
    fn pause(&mut self, app: &AppId) -> BackendResult<()>;
    fn play(&mut self, app: &AppId) -> BackendResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(app: &str, pid: u32, active: bool, peak: f32) -> SessionInfo {
        SessionInfo {
            app: AppId::new(app),
            pid,
            active,
            peak,
            is_system_sounds: false,
        }
    }

    #[test]
    fn app_id_is_case_insensitive() {
        assert_eq!(AppId::new("Spotify.exe"), AppId::new(" spotify.EXE "));
        assert_eq!(AppId::new("Spotify.exe").as_str(), "spotify.exe");
    }

    #[test]
    fn groups_sessions_of_one_app() {
        let apps = group_sessions(&[
            session("chrome.exe", 30, false, 0.0),
            session("Chrome.exe", 10, true, 0.4),
            session("chrome.exe", 10, true, 0.1),
        ]);

        assert_eq!(apps.len(), 1);
        let chrome = &apps[0];
        assert_eq!(chrome.app, AppId::new("chrome.exe"));
        assert_eq!(chrome.pids, vec![10, 30]);
        assert_eq!(chrome.session_count, 3);
        assert!(chrome.active);
        assert_eq!(chrome.peak, 0.4);
    }

    #[test]
    fn keeps_apps_apart_and_sorted() {
        let apps = group_sessions(&[
            session("spotify.exe", 2, true, 0.5),
            session("chrome.exe", 1, false, 0.0),
        ]);

        let names: Vec<&str> = apps.iter().map(|a| a.app.as_str()).collect();
        assert_eq!(names, vec!["chrome.exe", "spotify.exe"]);
        assert!(!apps[0].active);
        assert!(apps[1].active);
    }

    #[test]
    fn system_sounds_flag_survives_grouping() {
        let mut system = session("system sounds", 0, true, 0.2);
        system.is_system_sounds = true;

        let apps = group_sessions(&[system, session("spotify.exe", 2, true, 0.5)]);

        assert!(!apps[0].is_system_sounds);
        assert!(apps[1].is_system_sounds);
    }

    #[test]
    fn no_sessions_means_no_apps_and_no_mic() {
        assert!(group_sessions(&[]).is_empty());
        assert!(!ActivitySnapshot::default().mic_in_use());
    }
}
