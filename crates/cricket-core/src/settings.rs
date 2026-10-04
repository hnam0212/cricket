//! User settings and their defaults (SPEC.md section 5.3).
//!
//! Persistence as versioned JSON arrives with the UI in Phase 3; durations
//! are already plain millisecond fields so they map straight onto it.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Longest delay or fade a saved configuration may ask for.
const MAX_DURATION_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// How long other sound must last before the music is paused.
    pub trigger_delay_ms: u64,
    /// Continuous silence required before the music resumes.
    pub resume_cooldown_ms: u64,
    /// Fade to zero before pausing. 0 pauses immediately.
    pub fade_out_ms: u64,
    /// Fade up after resuming. 0 resumes at full volume.
    pub fade_in_ms: u64,
    /// Peak level below which an app counts as silent.
    pub sound_threshold: f32,
    /// An active capture session counts as activity.
    pub mic_counts_as_activity: bool,
    /// OS notification sounds do not trigger a pause.
    pub ignore_system_sounds: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            trigger_delay_ms: 500,
            resume_cooldown_ms: 3000,
            fade_out_ms: 2500,
            fade_in_ms: 1500,
            sound_threshold: 0.02,
            mic_counts_as_activity: true,
            ignore_system_sounds: true,
        }
    }
}

impl Settings {
    /// The same settings with every value pulled into its valid range.
    pub fn sanitized(mut self) -> Self {
        self.trigger_delay_ms = self.trigger_delay_ms.min(MAX_DURATION_MS);
        self.resume_cooldown_ms = self.resume_cooldown_ms.min(MAX_DURATION_MS);
        self.fade_out_ms = self.fade_out_ms.min(MAX_DURATION_MS);
        self.fade_in_ms = self.fade_in_ms.min(MAX_DURATION_MS);
        self.sound_threshold = if self.sound_threshold.is_finite() {
            self.sound_threshold.clamp(0.0, 1.0)
        } else {
            Settings::default().sound_threshold
        };
        self
    }

    pub fn trigger_delay(&self) -> Duration {
        Duration::from_millis(self.trigger_delay_ms)
    }

    pub fn resume_cooldown(&self) -> Duration {
        Duration::from_millis(self.resume_cooldown_ms)
    }

    pub fn fade_out(&self) -> Duration {
        Duration::from_millis(self.fade_out_ms)
    }

    pub fn fade_in(&self) -> Duration {
        Duration::from_millis(self.fade_in_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let settings = Settings::default();
        assert_eq!(settings.trigger_delay(), Duration::from_millis(500));
        assert_eq!(settings.resume_cooldown(), Duration::from_millis(3000));
        assert_eq!(settings.fade_out(), Duration::from_millis(2500));
        assert_eq!(settings.fade_in(), Duration::from_millis(1500));
        assert_eq!(settings.sound_threshold, 0.02);
        assert!(settings.mic_counts_as_activity);
        assert!(settings.ignore_system_sounds);
    }
}
