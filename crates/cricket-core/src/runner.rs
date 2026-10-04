//! Glue between the engine and the backends: one [`Runner::step`] reads the
//! machine, ticks the engine and carries out its commands.
//!
//! The engine itself stays free of I/O; all of it happens here, through the
//! two backend traits. The caller owns the loop and the sleeping.

use crate::activity::{detect_activity, ActivityCause};
use crate::audio::{
    ActivitySnapshot, AppId, AudioBackend, BackendError, MediaController, PlaybackState,
};
use crate::clock::Clock;
use crate::engine::{Command, Engine, Observation, State, TickOutput};
use crate::settings::Settings;

/// Everything that happened in one step, for logs and diagnostics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepReport {
    /// What the machine was doing. `None` if it could not be read.
    pub snapshot: Option<ActivitySnapshot>,
    /// Why the machine counts as busy, if it does.
    pub activity: Option<ActivityCause>,
    pub playback: Option<PlaybackState>,
    pub volume: Option<f32>,
    pub output: TickOutput,
    /// Backend calls that failed during this step.
    pub errors: Vec<String>,
}

pub struct Runner<A, M, C: Clock + Clone> {
    audio: A,
    media: M,
    clock: C,
    engine: Engine<C>,
    source: Option<AppId>,
    settings: Settings,
    enabled: bool,
}

impl<A: AudioBackend, M: MediaController, C: Clock + Clone> Runner<A, M, C> {
    /// Starts enabled and without a music source.
    pub fn new(audio: A, media: M, clock: C, settings: Settings) -> Self {
        Self {
            audio,
            media,
            engine: Engine::new(clock.clone(), settings.clone()),
            clock,
            source: None,
            settings,
            enabled: true,
        }
    }

    /// `Idle` while there is no music source.
    pub fn state(&self) -> State {
        match self.source {
            Some(_) => self.engine.state(),
            None => State::Idle,
        }
    }

    pub fn source(&self) -> Option<&AppId> {
        self.source.as_ref()
    }

    /// Takes effect on the next step.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.engine.set_enabled(enabled);
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.engine.set_settings(settings.clone());
        self.settings = settings;
    }

    /// Switches to another music source, or to none. The old source is
    /// handed back first: if a fade was under way its volume is restored.
    /// Music Cricket had paused stays paused. Returns backend errors.
    pub fn set_source(&mut self, source: Option<AppId>) -> Vec<String> {
        if source == self.source {
            return Vec::new();
        }
        let errors = self.hand_back();
        self.source = source;
        // A fresh engine: timers and the remembered volume belong to the
        // old source.
        self.engine = Engine::new(self.clock.clone(), self.settings.clone());
        self.engine.set_enabled(self.enabled);
        errors
    }

    /// Call before exiting, so the source is not left with a lowered volume.
    pub fn shutdown(&mut self) -> Vec<String> {
        self.set_source(None)
    }

    fn hand_back(&mut self) -> Vec<String> {
        let Some(source) = self.source.clone() else {
            return Vec::new();
        };
        self.engine.set_enabled(false);
        // A disabled engine ignores the observation and only emits what is
        // needed to let go of the source.
        let output = self.engine.tick(&Observation {
            other_activity: false,
            playback: None,
            volume: None,
        });
        let mut errors = Vec::new();
        self.execute(&source, &output.commands, &mut errors);
        errors
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
        let Some(source) = self.source.clone() else {
            report.snapshot = Some(snapshot);
            return report;
        };

        report.activity = detect_activity(&snapshot, &source, &self.settings);
        report.snapshot = Some(snapshot);
        report.playback = optional(
            self.media.playback_state(&source),
            "read playback state",
            &mut report.errors,
        );
        report.volume = optional(
            self.audio.volume(&source),
            "read volume",
            &mut report.errors,
        );

        report.output = self.engine.tick(&Observation {
            other_activity: report.activity.is_some(),
            playback: report.playback,
            volume: report.volume,
        });
        let commands = report.output.commands.clone();
        self.execute(&source, &commands, &mut report.errors);
        report
    }

    fn execute(&mut self, source: &AppId, commands: &[Command], errors: &mut Vec<String>) {
        for command in commands {
            let result = match *command {
                Command::Pause => self.media.pause(source),
                Command::Resume => self.media.play(source),
                Command::SetVolume(level) => self.audio.set_volume(source, level),
            };
            // The engine notices a command that did not work through the
            // next observations; here it only needs to be reported.
            if let Err(error) = result {
                errors.push(format!("{command:?}: {error}"));
            }
        }
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
            let mut runner = Runner::new(fake.clone(), fake, clock.clone(), Settings::default());
            runner.set_source(Some(music()));
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

    #[test]
    fn without_a_source_it_idles_but_still_reads_the_machine() {
        let mut rig = Rig::new();
        rig.runner.set_source(None);
        rig.machine.borrow_mut().other_peak = 0.4;

        for _ in 0..100 {
            let report = rig.step();
            assert!(report.snapshot.is_some());
            assert!(report.output.commands.is_empty());
        }

        assert_eq!(rig.runner.state(), State::Idle);
        assert_eq!(rig.machine.borrow().pauses, 0);
    }

    #[test]
    fn changing_the_source_mid_fade_restores_the_old_volume() {
        let mut rig = Rig::new();
        rig.step();
        rig.machine.borrow_mut().other_peak = 0.4;
        rig.run_until(State::FadingOut);
        for _ in 0..20 {
            rig.step();
        }
        assert!(rig.machine.borrow().music_volume.unwrap() < 0.6);

        let errors = rig.runner.set_source(Some(AppId::new("vlc.exe")));

        assert!(errors.is_empty());
        assert_eq!(rig.machine.borrow().music_volume, Some(0.6));
        assert_eq!(rig.runner.source(), Some(&AppId::new("vlc.exe")));
    }

    #[test]
    fn shutdown_does_not_resume_music_cricket_paused() {
        let mut rig = Rig::new();
        rig.step();
        rig.machine.borrow_mut().other_peak = 0.4;
        rig.run_until(State::PausedByCricket);
        rig.step();

        rig.runner.shutdown();

        let machine = rig.machine.borrow();
        assert!(!machine.music_playing);
        assert_eq!(machine.music_volume, Some(0.6));
        assert_eq!(rig.runner.state(), State::Idle);
    }

    #[test]
    fn disabled_stays_disabled_across_a_source_change() {
        let mut rig = Rig::new();
        rig.runner.set_enabled(false);
        rig.runner.set_source(Some(AppId::new("vlc.exe")));
        rig.machine.borrow_mut().other_peak = 0.4;

        for _ in 0..100 {
            rig.step();
        }

        assert_eq!(rig.runner.state(), State::Idle);
        assert_eq!(rig.machine.borrow().pauses, 0);
    }
}
