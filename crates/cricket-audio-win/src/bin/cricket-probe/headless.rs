//! `cricket-probe run`: the real engine against the real backends, with no
//! UI. Everything it decides is logged with a timestamp.

use std::thread::sleep;
use std::time::{Duration, Instant};

use cricket_audio_win::{shutdown, SmtcController, WinAudioBackend};
use cricket_core::activity::ActivityCause;
use cricket_core::audio::{AppId, PlaybackState};
use cricket_core::clock::SystemClock;
use cricket_core::engine::{Command, State};
use cricket_core::runner::{Runner, StepReport};
use cricket_core::settings::Settings;

use crate::probe::{text, CliResult, Log, POLL_INTERVAL};

/// A fade sets the volume every tick. Logging each one would bury the rest.
const VOLUME_LOG_INTERVAL: Duration = Duration::from_millis(250);

pub fn run(log: &Log, app: &AppId, settings: Settings) -> CliResult {
    let audio = text(WinAudioBackend::new())?;
    let media = text(SmtcController::new())?;
    text(shutdown::install())?;

    log.line(&format!("music source: {app}"));
    log.line(&format!(
        "settings: trigger delay {} ms, resume cooldown {} ms, fade out {} ms, fade in {} ms, \
         threshold {}, microphone counts: {}, system sounds ignored: {}",
        settings.trigger_delay_ms,
        settings.resume_cooldown_ms,
        settings.fade_out_ms,
        settings.fade_in_ms,
        settings.sound_threshold,
        settings.mic_counts_as_activity,
        settings.ignore_system_sounds,
    ));
    log.line("running; stop with Ctrl+C");

    let mut runner = Runner::new(audio, media, SystemClock::new(), app.clone(), settings);
    let mut changes = ChangeLog::new();
    let mut stopping = false;
    loop {
        if shutdown::requested() && !stopping {
            log.line("stopping: handing the source back");
            // Disabling makes the engine restore the volume on its next
            // step if a fade was under way.
            runner.set_enabled(false);
            stopping = true;
        }

        let report = runner.step();
        changes.log(log, &report);

        if stopping && runner.state() == State::Idle {
            log.line("stopped");
            return Ok(());
        }
        sleep(POLL_INTERVAL);
    }
}

/// Remembers the previous step so only changes are printed.
struct ChangeLog {
    first: bool,
    /// Activity without its peak, which changes every tick.
    activity: Option<String>,
    playback: Option<PlaybackState>,
    has_volume: bool,
    errors: Vec<String>,
    volume_logged: Instant,
}

impl ChangeLog {
    fn new() -> Self {
        Self {
            first: true,
            activity: None,
            playback: None,
            has_volume: false,
            errors: Vec::new(),
            volume_logged: Instant::now(),
        }
    }

    fn log(&mut self, log: &Log, report: &StepReport) {
        if report.errors != self.errors {
            for error in &report.errors {
                log.line(&format!("ERROR {error}"));
            }
            if report.errors.is_empty() {
                log.line("errors cleared");
            }
            self.errors = report.errors.clone();
        }
        // A step that could not read the machine carries no readings.
        if report
            .errors
            .iter()
            .any(|e| e.starts_with("read audio sessions"))
        {
            return;
        }

        if report.playback != self.playback || self.first {
            match report.playback {
                Some(state) => log.line(&format!("source playback: {state:?}")),
                None => log.line("source playback: no media session (is the app open?)"),
            }
            self.playback = report.playback;
        }
        if report.volume.is_some() != self.has_volume || self.first {
            match report.volume {
                Some(volume) => log.line(&format!("source volume: {volume:.3}")),
                None => {
                    log.line("source volume: no audio session yet, so it will pause without a fade")
                }
            }
            self.has_volume = report.volume.is_some();
        }

        let activity = report.activity.as_ref().map(activity_key);
        if activity != self.activity || self.first {
            match &report.activity {
                Some(cause) => log.line(&format!("activity: {cause}")),
                None => log.line("activity: none"),
            }
            self.activity = activity;
        }
        self.first = false;

        if let Some(transition) = report.output.transition {
            log.line(&format!(
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
                    if eventful || self.volume_logged.elapsed() >= VOLUME_LOG_INTERVAL {
                        log.line(&format!("  volume -> {level:.3}"));
                        self.volume_logged = Instant::now();
                    }
                }
                Command::Pause => log.line("COMMAND pause"),
                Command::Resume => log.line("COMMAND resume"),
            }
        }
    }
}

fn activity_key(cause: &ActivityCause) -> String {
    match cause {
        ActivityCause::Sound { app, .. } => format!("sound:{app}"),
        ActivityCause::Microphone { app } => format!("mic:{app}"),
    }
}
