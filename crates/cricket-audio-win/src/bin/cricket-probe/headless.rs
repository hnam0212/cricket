//! `cricket-probe run`: the real engine against the real backends, with no
//! UI. Everything it decides is logged with a timestamp.

use std::thread::sleep;

use cricket_audio_win::{shutdown, SmtcController, WinAudioBackend};
use cricket_core::audio::AppId;
use cricket_core::clock::SystemClock;
use cricket_core::engine::State;
use cricket_core::eventlog::EventLog;
use cricket_core::runner::Runner;
use cricket_core::settings::Settings;

use crate::probe::{text, CliResult, Log, POLL_INTERVAL};

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

    let mut runner = Runner::new(audio, media, SystemClock::new(), settings);
    runner.set_source(Some(app.clone()));
    let mut events = EventLog::new();
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
        for line in events.lines(log.elapsed(), &report) {
            log.line(&line);
        }

        if stopping && runner.state() == State::Idle {
            log.line("stopped");
            return Ok(());
        }
        sleep(POLL_INTERVAL);
    }
}
