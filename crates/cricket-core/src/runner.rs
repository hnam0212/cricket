//! Glue between the engine and the backends: one [`Runner::step`] reads the
//! machine, ticks the engine and carries out its commands.
//!
//! The engine itself stays free of I/O; all of it happens here, through the
//! two backend traits. The caller owns the loop and the sleeping.

use crate::activity::{detect_activity, ActivityCause};
use crate::audio::{AppId, AudioBackend, BackendError, MediaController, PlaybackState};
use crate::clock::Clock;
use crate::engine::{Command, Engine, Observation, State, TickOutput};
use crate::settings::Settings;

/// Everything that happened in one step, for logs and diagnostics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepReport {
    /// Why the machine counts as busy, if it does.
    pub activity: Option<ActivityCause>,
    pub playback: Option<PlaybackState>,
    pub volume: Option<f32>,
    pub output: TickOutput,
    /// Backend calls that failed during this step.
    pub errors: Vec<String>,
}

pub struct Runner<A, M, C: Clock> {
    audio: A,
    media: M,
    engine: Engine<C>,
    source: AppId,
    settings: Settings,
}

impl<A: AudioBackend, M: MediaController, C: Clock> Runner<A, M, C> {
    pub fn new(audio: A, media: M, clock: C, source: AppId, settings: Settings) -> Self {
        Self {
            audio,
            media,
            engine: Engine::new(clock, settings.clone()),
            source,
            settings,
        }
    }

    pub fn state(&self) -> State {
        self.engine.state()
    }

    pub fn source(&self) -> &AppId {
        &self.source
    }

    /// Takes effect on the next step.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.engine.set_enabled(enabled);
    }

    pub fn step(&mut self) -> StepReport {
        let mut report = StepReport::default();

        let snapshot = match self.audio.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // Without a reading, do not tick: feeding the engine a made
                // up "silence" could resume music in the middle of a call.
                report.errors.push(format!("read audio sessions: {error}"));
                return report;
            }
        };
        report.activity = detect_activity(&snapshot, &self.source, &self.settings);
        report.playback = optional(
            self.media.playback_state(&self.source),
            "read playback state",
            &mut report.errors,
        );
        report.volume = optional(
            self.audio.volume(&self.source),
            "read volume",
            &mut report.errors,
        );

        report.output = self.engine.tick(&Observation {
            other_activity: report.activity.is_some(),
            playback: report.playback,
            volume: report.volume,
        });

        for command in &report.output.commands {
            let result = match *command {
                Command::Pause => self.media.pause(&self.source),
                Command::Resume => self.media.play(&self.source),
                Command::SetVolume(level) => self.audio.set_volume(&self.source, level),
            };
            // The engine notices a command that did not work through the
            // next observations; here it only needs to be reported.
            if let Err(error) = result {
                report.errors.push(format!("{command:?}: {error}"));
            }
        }
        report
    }
}

/// A missing app is a normal answer (no session right now), not an error.
fn optional<T>(
    result: Result<T, BackendError>,
    context: &str,
    errors: &mut Vec<String>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(BackendError::AppNotFound(_)) => None,
        Err(error) => {
            errors.push(format!("{context}: {error}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use super::*;
    use crate::audio::{ActivitySnapshot, AppActivity, BackendResult};
    use crate::clock::ManualClock;

    const TICK: Duration = Duration::from_millis(50);

    /// A pretend machine shared by the fake audio backend and the fake
    /// media controller.
    #[derive(Default)]
    struct Machine {
        music_playing: bool,
        music_volume: Option<f32>,
        other_peak: f32,
        snapshot_fails: bool,
        pause_rejected: bool,
        pauses: u32,
        resumes: u32,
    }

    #[derive(Clone)]
    struct Fake(Rc<RefCell<Machine>>);

    fn music() -> AppId {
        AppId::new("spotify.exe")
    }

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

    impl AudioBackend for Fake {
        fn snapshot(&mut self) -> BackendResult<ActivitySnapshot> {
            let machine = self.0.borrow();
            if machine.snapshot_fails {
                return Err(BackendError::Os("device gone".to_string()));
            }
            Ok(ActivitySnapshot {
                apps: vec![
                    app("chrome.exe", machine.other_peak),
                    app("spotify.exe", if machine.music_playing { 0.5 } else { 0.0 }),
                ],
                mic_users: Vec::new(),
            })
        }

        fn volume(&mut self, app: &AppId) -> BackendResult<f32> {
            self.0
                .borrow()
                .music_volume
                .ok_or_else(|| BackendError::AppNotFound(app.clone()))
        }

        fn set_volume(&mut self, _app: &AppId, volume: f32) -> BackendResult<()> {
            self.0.borrow_mut().music_volume = Some(volume);
            Ok(())
        }
    }

    impl MediaController for Fake {
        fn playback_state(&mut self, _app: &AppId) -> BackendResult<PlaybackState> {
            Ok(if self.0.borrow().music_playing {
                PlaybackState::Playing
            } else {
                PlaybackState::Paused
            })
        }

        fn pause(&mut self, _app: &AppId) -> BackendResult<()> {
            let mut machine = self.0.borrow_mut();
            if machine.pause_rejected {
                return Err(BackendError::Rejected("pause".to_string()));
            }
            machine.music_playing = false;
            machine.pauses += 1;
            Ok(())
        }

        fn play(&mut self, _app: &AppId) -> BackendResult<()> {
            let mut machine = self.0.borrow_mut();
            machine.music_playing = true;
            machine.resumes += 1;
            Ok(())
        }
    }

    struct Rig {
        machine: Rc<RefCell<Machine>>,
        clock: ManualClock,
        runner: Runner<Fake, Fake, ManualClock>,
    }

    impl Rig {
        fn new() -> Self {
            let machine = Rc::new(RefCell::new(Machine {
                music_playing: true,
                music_volume: Some(0.6),
                ..Machine::default()
            }));
            let clock = ManualClock::default();
            let fake = Fake(Rc::clone(&machine));
            let runner = Runner::new(
                fake.clone(),
                fake,
                clock.clone(),
                music(),
                Settings::default(),
            );
            Self {
                machine,
                clock,
                runner,
            }
        }

        fn step(&mut self) -> StepReport {
            let report = self.runner.step();
            self.clock.advance(TICK);
            report
        }

        fn run_until(&mut self, state: State) -> Vec<StepReport> {
            let mut reports = Vec::new();
            for _ in 0..2000 {
                if self.runner.state() == state {
                    return reports;
                }
                reports.push(self.step());
            }
            panic!(
                "never reached {state:?}, stuck in {:?}",
                self.runner.state()
            );
        }
    }

    #[test]
    fn other_sound_pauses_the_source_and_silence_resumes_it() {
        let mut rig = Rig::new();
        rig.step();
        assert_eq!(rig.runner.state(), State::Playing);

        rig.machine.borrow_mut().other_peak = 0.4;
        rig.run_until(State::PausedByCricket);
        rig.step();
        assert!(!rig.machine.borrow().music_playing);
        assert_eq!(rig.machine.borrow().music_volume, Some(0.6));

        rig.machine.borrow_mut().other_peak = 0.0;
        rig.run_until(State::Playing);
        let machine = rig.machine.borrow();
        assert!(machine.music_playing);
        assert_eq!(machine.music_volume, Some(0.6));
        assert_eq!((machine.pauses, machine.resumes), (1, 1));
    }

    #[test]
    fn the_source_playing_is_not_other_activity() {
        let mut rig = Rig::new();

        for _ in 0..200 {
            let report = rig.step();
            assert_eq!(report.activity, None);
        }

        assert_eq!(rig.runner.state(), State::Playing);
        assert_eq!(rig.machine.borrow().pauses, 0);
    }

    #[test]
    fn the_report_names_the_cause_and_the_readings() {
        let mut rig = Rig::new();
        rig.machine.borrow_mut().other_peak = 0.4;

        let report = rig.step();

        assert_eq!(
            report.activity,
            Some(ActivityCause::Sound {
                app: AppId::new("chrome.exe"),
                peak: 0.4
            })
        );
        assert_eq!(report.playback, Some(PlaybackState::Playing));
        assert_eq!(report.volume, Some(0.6));
        assert!(report.errors.is_empty());
    }

    #[test]
    fn a_failed_snapshot_is_reported_and_the_engine_is_not_ticked() {
        let mut rig = Rig::new();
        rig.machine.borrow_mut().other_peak = 0.4;
        rig.run_until(State::PausedByCricket);
        rig.step();

        // The other sound is still going, but readings start failing. The
        // music must stay paused however long that lasts.
        rig.machine.borrow_mut().snapshot_fails = true;
        for _ in 0..200 {
            let report = rig.step();
            assert_eq!(report.errors.len(), 1);
            assert_eq!(report.output, TickOutput::default());
        }

        assert_eq!(rig.runner.state(), State::PausedByCricket);
        assert_eq!(rig.machine.borrow().resumes, 0);
    }

    #[test]
    fn a_missing_volume_is_not_an_error() {
        let mut rig = Rig::new();
        rig.machine.borrow_mut().music_volume = None;

        let report = rig.step();

        assert_eq!(report.volume, None);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn a_rejected_command_is_reported_and_the_volume_comes_back() {
        let mut rig = Rig::new();
        rig.step();
        rig.machine.borrow_mut().pause_rejected = true;
        rig.machine.borrow_mut().other_peak = 0.4;

        let reports = rig.run_until(State::PausedByCricket);
        assert!(reports
            .last()
            .unwrap()
            .errors
            .iter()
            .any(|error| error.contains("Pause")));

        // The engine gives the pause time to show, then gives up.
        rig.run_until(State::Playing);
        let machine = rig.machine.borrow();
        assert!(machine.music_playing);
        assert_eq!(machine.music_volume, Some(0.6));
    }
}
