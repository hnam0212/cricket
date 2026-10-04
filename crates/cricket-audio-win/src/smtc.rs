use cricket_core::audio::{AppId, BackendError, BackendResult, MediaController, PlaybackState};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as MediaSession,
    GlobalSystemMediaTransportControlsSessionManager as MediaSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};

use crate::com::{os_error, ComGuard};
use crate::matching::best_match;

/// One media session as SMTC reports it, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSessionInfo {
    /// Raw AppUserModelId of the session's app.
    pub app_user_model_id: String,
    pub state: PlaybackState,
}

/// SMTC implementation of [`MediaController`].
///
/// Only apps that register with the system media transport controls show up
/// here (Spotify, browsers, most players). Calls block briefly while Windows
/// talks to the app, so keep them off the UI thread.
pub struct SmtcController {
    manager: MediaSessionManager,
    // Last field, so it is dropped after the manager.
    _com: ComGuard,
}

impl SmtcController {
    pub fn new() -> BackendResult<Self> {
        let com = ComGuard::new()?;
        let manager = MediaSessionManager::RequestAsync()
            .and_then(|request| request.join())
            .map_err(|e| os_error("request media session manager", &e))?;
        Ok(Self { manager, _com: com })
    }

    /// Every media session SMTC currently knows about.
    pub fn sessions(&self) -> BackendResult<Vec<MediaSessionInfo>> {
        Ok(self
            .list()?
            .into_iter()
            .map(|(app_user_model_id, session)| MediaSessionInfo {
                app_user_model_id,
                state: playback_state(&session),
            })
            .collect())
    }

    fn list(&self) -> BackendResult<Vec<(String, MediaSession)>> {
        let sessions = self
            .manager
            .GetSessions()
            .map_err(|e| os_error("list media sessions", &e))?;
        let mut list = Vec::new();
        for session in sessions {
            // A session can close while we iterate; skip it.
            if let Ok(id) = session.SourceAppUserModelId() {
                list.push((id.to_string(), session));
            }
        }
        Ok(list)
    }

    fn find(&self, app: &AppId) -> BackendResult<MediaSession> {
        let mut sessions = self.list()?;
        let ids: Vec<&str> = sessions.iter().map(|(id, _)| id.as_str()).collect();
        let index = best_match(&ids, app).ok_or_else(|| BackendError::AppNotFound(app.clone()))?;
        Ok(sessions.swap_remove(index).1)
    }
}

impl MediaController for SmtcController {
    fn playback_state(&mut self, app: &AppId) -> BackendResult<PlaybackState> {
        Ok(playback_state(&self.find(app)?))
    }

    fn pause(&mut self, app: &AppId) -> BackendResult<()> {
        let accepted = self
            .find(app)?
            .TryPauseAsync()
            .and_then(|request| request.join())
            .map_err(|e| os_error("pause media session", &e))?;
        if accepted {
            Ok(())
        } else {
            Err(BackendError::Rejected(format!(
                "{app} did not accept pause"
            )))
        }
    }

    fn play(&mut self, app: &AppId) -> BackendResult<()> {
        let accepted = self
            .find(app)?
            .TryPlayAsync()
            .and_then(|request| request.join())
            .map_err(|e| os_error("resume media session", &e))?;
        if accepted {
            Ok(())
        } else {
            Err(BackendError::Rejected(format!("{app} did not accept play")))
        }
    }
}

fn playback_state(session: &MediaSession) -> PlaybackState {
    let status = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus());
    match status {
        Ok(PlaybackStatus::Playing) => PlaybackState::Playing,
        Ok(PlaybackStatus::Paused) => PlaybackState::Paused,
        Ok(PlaybackStatus::Stopped) => PlaybackState::Stopped,
        _ => PlaybackState::Unknown,
    }
}
