//! Turns step reports into log lines, printing only what changed.
//!
//! Cricket cannot hear, so this log is how a human checks its decisions:
//! every state change with its reason, every command, and what counted as
//! activity.

use std::time::Duration;

use crate::activity::ActivityCause;
use crate::audio::PlaybackState;
use crate::engine::Command;
use crate::runner::StepReport;

/// A fade sets the volume every tick. Logging each one would bury the rest.
const VOLUME_LOG_INTERVAL: Duration = Duration::from_millis(250);

/// Remembers the previous step so only changes produce lines.
#[derive(Debug, Default)]
pub struct EventLog {
    started: bool,
    /// Activity without its peak, which changes every tick.
    activity: Option<String>,
    playback: Option<PlaybackState>,
    has_volume: bool,
    errors: Vec<String>,
    volume_logged: Duration,
}

impl EventLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the previous step, so the next one is described in full.
    /// Use when the music source changes.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Lines describing what changed in this step. `now` is any monotonic
    /// time; it only spaces out volume lines during a fade.
    pub fn lines(&mut self, now: Duration, report: &StepReport) -> Vec<String> {
        let mut lines = Vec::new();

        if report.errors != self.errors {
            lines.extend(report.errors.iter().map(|error| format!("ERROR {error}")));
            if report.errors.is_empty() {
                lines.push("errors cleared".to_string());
            }
            self.errors = report.errors.clone();
        }
        // A step that could not read the machine carries no readings.
        if report.snapshot.is_none() {
            return lines;
        }

        let first = !self.started;
        self.started = true;

        if report.playback != self.playback || first {
            lines.push(match report.playback {
                Some(state) => format!("source playback: {state:?}"),
                None => "source playback: no media session (is the app open?)".to_string(),
            });
            self.playback = report.playback;
        }
        if report.volume.is_some() != self.has_volume || first {
            lines.push(match report.volume {
                Some(volume) => format!("source volume: {volume:.3}"),
                None => "source volume: no audio session yet, so it will pause without a fade"
                    .to_string(),
            });
            self.has_volume = report.volume.is_some();
        }

        let activity = report.activity.as_ref().map(activity_key);
        if activity != self.activity || first {
            lines.push(match &report.activity {
                Some(cause) => format!("activity: {cause}"),
                None => "activity: none".to_string(),
            });
            self.activity = activity;
        }

        if let Some(transition) = report.output.transition {
            lines.push(format!(
                "STATE {:?} -> {:?} ({})",
                transition.from, transition.to, transition.reason
            ));
        }

        let eventful = report.output.transition.is_some()
            || report
                .output
                .commands
                .iter()
                .any(|command| !matches!(command, Command::SetVolume(_)));
        for command in &report.output.commands {
            match command {
                Command::SetVolume(level) => {
                    if eventful || now.saturating_sub(self.volume_logged) >= VOLUME_LOG_INTERVAL {
                        lines.push(format!("  volume -> {level:.3}"));
                        self.volume_logged = now;
                    }
                }
                Command::Pause => lines.push("COMMAND pause".to_string()),
                Command::Resume => lines.push("COMMAND resume".to_string()),
            }
        }
        lines
    }
}

fn activity_key(cause: &ActivityCause) -> String {
    match cause {
        ActivityCause::Sound { app, .. } => format!("sound:{app}"),
        ActivityCause::Microphone { app } => format!("mic:{app}"),
        ActivityCause::Tab { title } => format!("tab:{title}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ActivitySnapshot, AppId};
    use crate::engine::{State, TickOutput, Transition};

    fn report() -> StepReport {
        StepReport {
            snapshot: Some(ActivitySnapshot::default()),
            playback: Some(PlaybackState::Playing),
            volume: Some(0.5),
            ..StepReport::default()
        }
    }

    fn sound(peak: f32) -> Option<ActivityCause> {
        Some(ActivityCause::Sound {
            app: AppId::new("chrome.exe"),
            peak,
        })
    }

    #[test]
    fn the_first_step_is_described_in_full() {
        let mut log = EventLog::new();

        let lines = log.lines(Duration::ZERO, &report());

        assert_eq!(
            lines,
            vec![
                "source playback: Playing",
                "source volume: 0.500",
                "activity: none"
            ]
        );
    }

    #[test]
    fn an_unchanged_step_logs_nothing() {
        let mut log = EventLog::new();
        log.lines(Duration::ZERO, &report());

        assert!(log.lines(Duration::from_millis(50), &report()).is_empty());
    }

    #[test]
    fn a_changing_peak_alone_is_not_a_change() {
        let mut log = EventLog::new();
        log.lines(Duration::ZERO, &report());

        let mut busy = report();
        busy.activity = sound(0.3);
        assert_eq!(
            log.lines(Duration::from_millis(50), &busy),
            vec!["activity: sound from chrome.exe (peak 0.300)"]
        );

        busy.activity = sound(0.6);
        assert!(log.lines(Duration::from_millis(100), &busy).is_empty());
    }

    #[test]
    fn transitions_and_commands_are_logged() {
        let mut log = EventLog::new();
        log.lines(Duration::ZERO, &report());

        let mut step = report();
        step.output = TickOutput {
            commands: vec![Command::SetVolume(0.0), Command::Pause],
            transition: Some(Transition {
                from: State::FadingOut,
                to: State::PausedByCricket,
                reason: "fade out finished",
            }),
        };

        assert_eq!(
            log.lines(Duration::from_millis(50), &step),
            vec![
                "STATE FadingOut -> PausedByCricket (fade out finished)",
                "  volume -> 0.000",
                "COMMAND pause"
            ]
        );
    }

    #[test]
    fn volume_lines_are_spaced_out_during_a_fade() {
        let mut log = EventLog::new();
        log.lines(Duration::ZERO, &report());

        let mut logged = 0;
        for tick in 1..=20u64 {
            let mut step = report();
            step.output.commands = vec![Command::SetVolume(0.4)];
            logged += log.lines(Duration::from_millis(tick * 50), &step).len();
        }

        // One second of fading at 50 ms per tick, one line per 250 ms.
        assert_eq!(logged, 4);
    }

    #[test]
    fn errors_are_logged_once_and_when_they_clear() {
        let mut log = EventLog::new();
        let failed = StepReport {
            errors: vec!["read audio sessions: OS error: gone".to_string()],
            ..StepReport::default()
        };

        assert_eq!(
            log.lines(Duration::ZERO, &failed),
            vec!["ERROR read audio sessions: OS error: gone"]
        );
        assert!(log.lines(Duration::from_millis(50), &failed).is_empty());
        assert_eq!(
            log.lines(Duration::from_millis(100), &report())[0],
            "errors cleared"
        );
    }
}
