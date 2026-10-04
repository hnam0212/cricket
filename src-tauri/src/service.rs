//! The audio thread: polls the machine about every 50 ms, runs the engine
//! and publishes a status for the UI.
//!
//! The thread owns the audio backends (they hold COM objects and must stay
//! on one thread) and never waits on the UI: the UI sends messages through a
//! channel and reads the latest status from behind a short-lived lock.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cricket_audio_win::{SmtcController, WinAudioBackend};
use cricket_core::activity::ActivityCause;
use cricket_core::audio::{AppId, PlaybackState};
use cricket_core::clock::SystemClock;
use cricket_core::config::AppConfig;
use cricket_core::engine::State;
use cricket_core::eventlog::EventLog;
use cricket_core::runner::{Runner, StepReport};
use cricket_core::settings::Settings;
use serde::Serialize;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_EVENTS: usize = 200;
/// Per tick. Lets a level shown in the UI fall back over about half a
/// second instead of flickering with every 50 ms reading.
const PEAK_DECAY: f32 = 0.85;

/// What the UI shows. Replaced as a whole on every tick.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub state: State,
    /// Why the engine entered the current state.
    pub reason: String,
    pub source: Option<AppId>,
    pub enabled: bool,
    pub playback: Option<PlaybackState>,
    pub volume: Option<f32>,
    pub activity: Option<ActivityCause>,
    pub apps: Vec<AppView>,
    pub mic_users: Vec<AppId>,
    pub errors: Vec<String>,
    /// Most recent last.
    pub events: Vec<Event>,
    /// Set if the audio backend could not start; nothing works then.
    pub fatal: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppView {
    pub app: AppId,
    /// Smoothed for display.
    pub peak: f32,
    pub making_sound: bool,
    pub active: bool,
    pub sessions: usize,
    pub is_system_sounds: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// Seconds since Cricket started.
    pub at: f64,
    pub text: String,
}

enum Message {
    SetSource(Option<AppId>),
    SetEnabled(bool),
    SetSettings(Settings),
    Shutdown,
}

pub struct Service {
    sender: Sender<Message>,
    status: Arc<Mutex<Status>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Service {
    /// Starts the audio thread. `on_state` is called from that thread
    /// whenever the engine state changes.
    pub fn start(config: &AppConfig, on_state: impl Fn(State, bool) + Send + 'static) -> Self {
        let status = Arc::new(Mutex::new(Status {
            state: State::Idle,
            reason: String::new(),
            source: config.source.clone(),
            enabled: config.enabled,
            playback: None,
            volume: None,
            activity: None,
            apps: Vec::new(),
            mic_users: Vec::new(),
            errors: Vec::new(),
            events: Vec::new(),
            fatal: None,
        }));
        let (sender, receiver) = mpsc::channel();

        let worker = Worker {
            receiver,
            status: Arc::clone(&status),
            config: config.clone(),
            on_state: Box::new(on_state),
        };
        let thread = thread::Builder::new()
            .name("cricket-audio".to_string())
            .spawn(move || worker.run())
            .expect("spawn audio thread");

        Self {
            sender,
            status,
            thread: Mutex::new(Some(thread)),
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().expect("status lock").clone()
    }

    pub fn set_source(&self, source: Option<AppId>) {
        self.send(Message::SetSource(source));
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.send(Message::SetEnabled(enabled));
    }

    pub fn set_settings(&self, settings: Settings) {
        self.send(Message::SetSettings(settings));
    }

    /// Stops the audio thread and waits for it, so the music source gets its
    /// volume back before the process exits. Safe to call more than once.
    pub fn shutdown(&self) {
        self.send(Message::Shutdown);
        if let Some(thread) = self.thread.lock().expect("thread lock").take() {
            let _ = thread.join();
        }
    }

    fn send(&self, message: Message) {
        // Fails only if the audio thread is gone, which the status already
        // reports through `fatal`.
        let _ = self.sender.send(message);
    }
}

struct Worker {
    receiver: Receiver<Message>,
    status: Arc<Mutex<Status>>,
    config: AppConfig,
    on_state: Box<dyn Fn(State, bool) + Send>,
}

impl Worker {
    fn run(self) {
        let started = Instant::now();
        let backends = WinAudioBackend::new()
            .and_then(|audio| SmtcController::new().map(|media| (audio, media)));
        let (audio, media) = match backends {
            Ok(backends) => backends,
            Err(error) => {
                eprintln!("audio backend failed to start: {error}");
                self.status.lock().expect("status lock").fatal = Some(error.to_string());
                return;
            }
        };

        let mut runner = Runner::new(
            audio,
            media,
            SystemClock::new(),
            self.config.settings.clone(),
        );
        runner.set_enabled(self.config.enabled);
        runner.set_source(self.config.source.clone());

        let mut enabled = self.config.enabled;
        let mut sound_threshold = self.config.settings.sound_threshold;
        let mut event_log = EventLog::new();
        let mut events: VecDeque<Event> = VecDeque::new();
        let mut shown_peaks: HashMap<AppId, f32> = HashMap::new();
        let mut reason = String::new();
        let mut last_state = runner.state();
        (self.on_state)(last_state, runner.source().is_some());

        loop {
            let tick_started = Instant::now();
            let mut note = |text: String| {
                let at = started.elapsed().as_secs_f64();
                eprintln!("[{at:>9.3}s] {text}");
                events.push_back(Event { at, text });
                if events.len() > MAX_EVENTS {
                    events.pop_front();
                }
            };

            loop {
                match self.receiver.try_recv() {
                    Ok(Message::SetSource(source)) => {
                        note(match &source {
                            Some(app) => format!("music source set to {app}"),
                            None => "music source cleared".to_string(),
                        });
                        for error in runner.set_source(source) {
                            note(format!("ERROR {error}"));
                        }
                        event_log.reset();
                        reason.clear();
                    }
                    Ok(Message::SetEnabled(value)) => {
                        note(format!(
                            "Cricket turned {}",
                            if value { "on" } else { "off" }
                        ));
                        enabled = value;
                        runner.set_enabled(value);
                    }
                    Ok(Message::SetSettings(settings)) => {
                        note(format!("settings changed: {settings:?}"));
                        sound_threshold = settings.sound_threshold;
                        runner.set_settings(settings);
                    }
                    // The app is exiting, or the UI side is gone.
                    Ok(Message::Shutdown) | Err(TryRecvError::Disconnected) => {
                        for error in runner.shutdown() {
                            eprintln!("shutdown: {error}");
                        }
                        return;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }

            let report = runner.step();
            // Without a source there is no playback or volume to describe.
            if runner.source().is_some() {
                for line in event_log.lines(started.elapsed(), &report) {
                    note(line);
                }
            }
            if let Some(transition) = report.output.transition {
                reason = transition.reason.to_string();
            }

            let state = runner.state();
            if state != last_state {
                last_state = state;
                (self.on_state)(state, runner.source().is_some());
            }

            let apps = app_views(&report, &mut shown_peaks, sound_threshold);
            {
                let mut status = self.status.lock().expect("status lock");
                status.state = state;
                status.reason = reason.clone();
                status.source = runner.source().cloned();
                status.enabled = enabled;
                status.playback = report.playback;
                status.volume = report.volume;
                status.activity = report.activity;
                if let Some(snapshot) = report.snapshot {
                    status.apps = apps;
                    status.mic_users = snapshot.mic_users;
                }
                status.errors = report.errors;
                status.events = events.iter().cloned().collect();
            }

            thread::sleep(POLL_INTERVAL.saturating_sub(tick_started.elapsed()));
        }
    }
}

fn app_views(
    report: &StepReport,
    shown_peaks: &mut HashMap<AppId, f32>,
    sound_threshold: f32,
) -> Vec<AppView> {
    let Some(snapshot) = &report.snapshot else {
        return Vec::new();
    };
    // Forget apps whose sessions are gone.
    shown_peaks.retain(|app, _| snapshot.apps.iter().any(|a| &a.app == app));
    snapshot
        .apps
        .iter()
        .map(|app| {
            let shown = shown_peaks.entry(app.app.clone()).or_insert(0.0);
            *shown = app.peak.max(*shown * PEAK_DECAY);
            AppView {
                app: app.app.clone(),
                peak: *shown,
                making_sound: *shown >= sound_threshold,
                active: app.active,
                sessions: app.session_count,
                is_system_sounds: app.is_system_sounds,
            }
        })
        .collect()
}
