//! Glue between the engine and the backends: one [`Runner::step`] reads the
//! machine, ticks the engine and carries out its commands.
//!
//! The engine itself stays free of I/O; all of it happens here, through the
//! backend traits. The caller owns the loop and the sleeping.

use std::time::Duration;

use crate::activity::{detect_activity, detect_tab_activity, ActivityCause};
use crate::audio::{ActivitySnapshot, AudioBackend, BackendError, MediaController, PlaybackState};
use crate::clock::Clock;
use crate::engine::{Command, Engine, Observation, State, TickOutput};
use crate::settings::Settings;
use crate::source::{Source, TabBridge, TabInfo};

/// How long shutdown waits for the last commands to reach the browser.
const SHUTDOWN_FLUSH: Duration = Duration::from_millis(300);

/// Everything that happened in one step, for logs and diagnostics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepReport {
    /// What the machine was doing. `None` if it could not be read.
    pub snapshot: Option<ActivitySnapshot>,
    /// Open browser tabs, if an extension is connected.
    pub tabs: Vec<TabInfo>,
    pub bridge_connected: bool,
    /// Why the machine counts as busy, if it does.
    pub activity: Option<ActivityCause>,
    pub playback: Option<PlaybackState>,
    pub volume: Option<f32>,
    /// The music source could be seen this step. Always true for a desktop
    /// app; for a browser tab it means the extension is connected and the
    /// tab is reporting. While false the engine holds its state.
    pub source_available: bool,
    /// A tab source picked in an earlier browser session: the browser was
    /// restarted and the tab has to be picked again.
    pub source_stale: bool,
    pub output: TickOutput,
    /// Backend calls that failed during this step.
    pub errors: Vec<String>,
}

pub struct Runner<A, M, B, C: Clock + Clone> {
    audio: A,
    media: M,
    tabs: B,
    clock: C,
    engine: Engine<C>,
    source: Option<Source>,
    settings: Settings,
    enabled: bool,
}

impl<A: AudioBackend, M: MediaController, B: TabBridge, C: Clock + Clone> Runner<A, M, B, C> {
    /// Starts enabled and without a music source.
    pub fn new(audio: A, media: M, tabs: B, clock: C, settings: Settings) -> Self {
        Self {
            audio,
            media,
            tabs,
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

    pub fn source(&self) -> Option<&Source> {
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
    pub fn set_source(&mut self, source: Option<Source>) -> Vec<String> {
        let unchanged = match (&self.source, &source) {
            (Some(old), Some(new)) => old.same_target(new),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return Vec::new();
        }
        let errors = self.hand_back();
        match &source {
            Some(Source::Tab { id, session, .. }) => {
                self.tabs.select(Some(*id), session.as_deref())
            }
            _ => self.tabs.select(None, None),
        }
        self.source = source;
        // A fresh engine: timers and the remembered volume belong to the
        // old source.
        self.engine = Engine::new(self.clock.clone(), self.settings.clone());
        self.engine.set_enabled(self.enabled);
        errors
    }

    /// Call before exiting, so the source is not left with a lowered volume.
    pub fn shutdown(&mut self) -> Vec<String> {
        let errors = self.set_source(None);
        // The restore commands only reach a browser tab through a queue.
        self.tabs.flush(SHUTDOWN_FLUSH);
        errors
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
            source_available: true,
        });
        let mut errors = Vec::new();
        self.execute(&source, &output.commands, &mut errors);
        errors
    }

    pub fn step(&mut self) -> StepReport {
        let mut report = StepReport {
            bridge_connected: self.tabs.connected(),
            tabs: self.tabs.tabs(),
            // Only a tab source can be out of sight; see below.
            source_available: true,
            ..StepReport::default()
        };

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

        match &source {
            Source::App { app } => {
                report.activity = detect_activity(&snapshot, app, &self.settings);
                report.playback = optional(
                    self.media.playback_state(app),
                    "read playback state",
                    &mut report.errors,
                );
                report.volume = optional(self.audio.volume(app), "read volume", &mut report.errors);
                // A desktop app without a media session simply counts as not
                // playing; there is no link that can drop.
                report.source_available = true;
            }
            Source::Tab { id, .. } => {
                let browser = self.tabs.browser();
                report.activity =
                    detect_tab_activity(&snapshot, &browser, &report.tabs, *id, &self.settings);
                report.playback = optional(
                    self.tabs.playback_state(*id),
                    "read tab playback state",
                    &mut report.errors,
                );
                report.volume =
                    optional(self.tabs.volume(*id), "read tab volume", &mut report.errors);
                // No fresh report means the extension is gone, or the tab is
                // closed or silent about itself: we are blind, not paused.
                report.source_available = report.playback.is_some();
                report.source_stale = self.tabs.selection_stale();
            }
        }
        report.snapshot = Some(snapshot);

        report.output = self.engine.tick(&Observation {
            other_activity: report.activity.is_some(),
            playback: report.playback,
            volume: report.volume,
            source_available: report.source_available,
        });
        let commands = report.output.commands.clone();
        self.execute(&source, &commands, &mut report.errors);
        report
    }

    fn execute(&mut self, source: &Source, commands: &[Command], errors: &mut Vec<String>) {
        for command in commands {
            let result = match (source, *command) {
                (Source::App { app }, Command::Pause) => self.media.pause(app),
                (Source::App { app }, Command::Resume) => self.media.play(app),
                (Source::App { app }, Command::SetVolume(level)) => {
                    self.audio.set_volume(app, level)
                }
                (Source::Tab { id, .. }, Command::Pause) => self.tabs.pause(*id),
                (Source::Tab { id, .. }, Command::Resume) => self.tabs.play(*id),
                (Source::Tab { id, .. }, Command::SetVolume(level)) => {
                    self.tabs.set_volume(*id, level)
                }
            };
            // The engine notices a command that did not work through the
            // next observations; here it only needs to be reported.
            if let Err(error) = result {
                errors.push(format!("{command:?}: {error}"));
            }
        }
    }
}

/// A source that is not there right now (no session, tab closed, extension
/// not connected) is a normal answer, not an error.
fn optional<T>(
    result: Result<T, BackendError>,
    context: &str,
    errors: &mut Vec<String>,
) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(BackendError::AppNotFound(_) | BackendError::Unavailable(_)) => None,
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
    use crate::audio::{ActivitySnapshot, AppActivity, AppId, BackendResult};
    use crate::clock::ManualClock;
    use crate::source::{NoTabs, TabId};

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
        runner: Runner<Fake, Fake, NoTabs, ManualClock>,
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
            let mut runner = Runner::new(
                fake.clone(),
                fake,
                NoTabs,
                clock.clone(),
                Settings::default(),
            );
            runner.set_source(Some(Source::App { app: music() }));
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

        let errors = rig.runner.set_source(Some(Source::app("vlc.exe")));

        assert!(errors.is_empty());
        assert_eq!(rig.machine.borrow().music_volume, Some(0.6));
        assert_eq!(rig.runner.source(), Some(&Source::app("vlc.exe")));
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
        rig.runner.set_source(Some(Source::app("vlc.exe")));
        rig.machine.borrow_mut().other_peak = 0.4;

        for _ in 0..100 {
            rig.step();
        }

        assert_eq!(rig.runner.state(), State::Idle);
        assert_eq!(rig.machine.borrow().pauses, 0);
    }

    /// A pretend browser with one music tab and one other tab.
    #[derive(Default)]
    struct Browser {
        connected: bool,
        music_playing: bool,
        music_volume: f32,
        other_tab_audible: bool,
        selected: Option<TabId>,
        pauses: u32,
        resumes: u32,
    }

    #[derive(Clone)]
    struct FakeTabs(Rc<RefCell<Browser>>);

    const MUSIC_TAB: TabId = TabId(1);

    impl TabBridge for FakeTabs {
        fn connected(&mut self) -> bool {
            self.0.borrow().connected
        }

        fn browser(&mut self) -> AppId {
            AppId::new("chrome.exe")
        }

        fn tabs(&mut self) -> Vec<TabInfo> {
            let browser = self.0.borrow();
            if !browser.connected {
                return Vec::new();
            }
            vec![
                TabInfo {
                    id: MUSIC_TAB,
                    title: "Music".to_string(),
                    url: String::new(),
                    audible: browser.music_playing,
                },
                TabInfo {
                    id: TabId(2),
                    title: "Video".to_string(),
                    url: String::new(),
                    audible: browser.other_tab_audible,
                },
            ]
        }

        fn select(&mut self, tab: Option<TabId>, _session: Option<&str>) {
            self.0.borrow_mut().selected = tab;
        }

        fn playback_state(&mut self, _tab: TabId) -> BackendResult<PlaybackState> {
            let browser = self.0.borrow();
            if !browser.connected {
                return Err(BackendError::Unavailable("not connected".to_string()));
            }
            Ok(if browser.music_playing {
                PlaybackState::Playing
            } else {
                PlaybackState::Paused
            })
        }

        fn volume(&mut self, _tab: TabId) -> BackendResult<f32> {
            let browser = self.0.borrow();
            if !browser.connected {
                return Err(BackendError::Unavailable("not connected".to_string()));
            }
            Ok(browser.music_volume)
        }

        fn set_volume(&mut self, _tab: TabId, volume: f32) -> BackendResult<()> {
            self.0.borrow_mut().music_volume = volume;
            Ok(())
        }

        fn pause(&mut self, _tab: TabId) -> BackendResult<()> {
            let mut browser = self.0.borrow_mut();
            browser.music_playing = false;
            browser.pauses += 1;
            Ok(())
        }

        fn play(&mut self, _tab: TabId) -> BackendResult<()> {
            let mut browser = self.0.borrow_mut();
            browser.music_playing = true;
            browser.resumes += 1;
            Ok(())
        }
    }

    struct TabRig {
        machine: Rc<RefCell<Machine>>,
        browser: Rc<RefCell<Browser>>,
        clock: ManualClock,
        runner: Runner<Fake, Fake, FakeTabs, ManualClock>,
    }

    impl TabRig {
        /// Chrome is audible on the desktop (it is playing the music), and
        /// the music tab is the source.
        fn new() -> Self {
            let machine = Rc::new(RefCell::new(Machine {
                other_peak: 0.5,
                ..Machine::default()
            }));
            let browser = Rc::new(RefCell::new(Browser {
                connected: true,
                music_playing: true,
                music_volume: 1.0,
                ..Browser::default()
            }));
            let clock = ManualClock::default();
            let fake = Fake(Rc::clone(&machine));
            let mut runner = Runner::new(
                fake.clone(),
                fake,
                FakeTabs(Rc::clone(&browser)),
                clock.clone(),
                Settings::default(),
            );
            runner.set_source(Some(Source::Tab {
                id: MUSIC_TAB,
                title: "Music".to_string(),
                session: None,
            }));
            Self {
                machine,
                browser,
                clock,
                runner,
            }
        }

        fn step(&mut self) -> StepReport {
            let report = self.runner.step();
            self.clock.advance(TICK);
            report
        }

        fn run_until(&mut self, state: State) {
            for _ in 0..2000 {
                if self.runner.state() == state {
                    return;
                }
                self.step();
            }
            panic!(
                "never reached {state:?}, stuck in {:?}",
                self.runner.state()
            );
        }
    }

    #[test]
    fn picking_a_tab_tells_the_extension_which_one() {
        let mut rig = TabRig::new();
        assert_eq!(rig.browser.borrow().selected, Some(MUSIC_TAB));

        rig.runner.set_source(Some(Source::app("spotify.exe")));
        assert_eq!(rig.browser.borrow().selected, None);
    }

    // Scenario 2: music in tab A, a video in tab B.
    #[test]
    fn another_tab_pauses_the_music_tab_and_silence_resumes_it() {
        let mut rig = TabRig::new();
        for _ in 0..100 {
            rig.step();
        }
        // Chrome's own sound (the music) must not have triggered anything.
        assert_eq!(rig.runner.state(), State::Playing);

        rig.browser.borrow_mut().other_tab_audible = true;
        rig.run_until(State::PausedByCricket);
        rig.step();
        assert!(!rig.browser.borrow().music_playing);
        assert_eq!(rig.browser.borrow().music_volume, 1.0);

        rig.browser.borrow_mut().other_tab_audible = false;
        rig.run_until(State::Playing);
        let browser = rig.browser.borrow();
        assert!(browser.music_playing);
        assert_eq!(browser.music_volume, 1.0);
        assert_eq!((browser.pauses, browser.resumes), (1, 1));
        // The desktop media controls were never used for a tab source.
        assert_eq!(rig.machine.borrow().pauses, 0);
    }

    #[test]
    fn a_tab_source_reports_the_tab_list() {
        let mut rig = TabRig::new();

        let report = rig.step();

        assert!(report.bridge_connected);
        assert_eq!(report.tabs.len(), 2);
        assert_eq!(report.playback, Some(PlaybackState::Playing));
    }

    #[test]
    fn a_disconnected_extension_means_the_tab_is_not_available() {
        let mut rig = TabRig::new();
        rig.step();
        rig.browser.borrow_mut().connected = false;

        let report = rig.step();

        assert_eq!(report.playback, None);
        assert!(!report.source_available);
        assert!(report.errors.is_empty());
        // Blind, not paused by the user: the state is held.
        assert_eq!(rig.runner.state(), State::Playing);
        assert_eq!(rig.browser.borrow().pauses, 0);

        rig.browser.borrow_mut().connected = true;
        let report = rig.step();
        assert!(report.source_available);
        assert_eq!(rig.runner.state(), State::Playing);
    }
}
