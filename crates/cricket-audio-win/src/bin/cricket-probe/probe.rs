use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::{Duration, Instant};

use cricket_audio_win::{SmtcController, WinAudioBackend};
use cricket_core::audio::{
    ActivitySnapshot, AppActivity, AppId, AudioBackend, BackendResult, MediaController,
};
use cricket_core::fade::fade_level;
use cricket_core::settings::Settings;

use crate::headless;

const USAGE: &str = "\
cricket-probe: inspect and control Windows audio sessions

USAGE:
  cricket-probe list
      One snapshot: audio sessions per app, microphone users, media sessions.

  cricket-probe watch [--interval-ms 500] [--threshold 0.02]
      Live view. Polls every 50 ms and prints the loudest peak per app for
      each interval. Session and microphone changes are logged as they
      happen. Stop with Ctrl+C.

  cricket-probe status <app>
      Volume and playback state of one app.

  cricket-probe pause <app> [--fade-ms 2500]
  cricket-probe fade <app> [--fade-ms 2500]
      Fade the app's volume down to silence, then pause it through the
      system media controls (SMTC). The volume is put back once the app is
      paused, so it is not left muted. `fade` and `pause` are the same.
      Use --fade-ms 0 to pause at once.

  cricket-probe resume <app> [--fade-ms 1500]
      Resume from silence and fade the volume back up.
      Use --fade-ms 0 to resume at once.

  cricket-probe volume <app> [level]
      Print the app's session volume, or set it (0.0 to 1.0).

  cricket-probe cycle <app> [--fade-out-ms 2500] [--hold-ms 3000] [--fade-in-ms 1500]
      Pause, wait, resume: the two commands above with a hold in between.

  cricket-probe run <app> [--trigger-ms 500] [--cooldown-ms 3000]
                          [--fade-out-ms 2500] [--fade-in-ms 1500]
                          [--threshold 0.02] [--mic on|off] [--system-sounds on|off]
      Cricket without the UI: keeps <app> playing while the machine is
      quiet, fades it out and pauses it when anything else makes sound or
      the microphone is in use, and brings it back after the cooldown.
      Logs every decision. --system-sounds on makes Windows notification
      sounds count as activity. Stop with Ctrl+C.

<app> is an executable name as shown by `list`, for example spotify.exe.
";

/// Matches the engine's planned polling rate (CLAUDE.md, architecture rule 6).
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);
const FADE_STEP: Duration = Duration::from_millis(20);
const FADE_LOG_INTERVAL: Duration = Duration::from_millis(250);
/// Defaults from SPEC.md section 5.3.
const DEFAULT_FADE_OUT_MS: f64 = 2500.0;
const DEFAULT_FADE_IN_MS: f64 = 1500.0;
/// Below this a session volume counts as "left muted by accident".
const QUIET_VOLUME: f32 = 0.05;
const BAR_WIDTH: usize = 20;

pub type CliResult = Result<(), String>;

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> CliResult {
    let args = Args::parse(args)?;
    let Some(command) = args.positional.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    let log = Log::start();

    match command.as_str() {
        "list" => list(&log),
        "watch" => watch(
            &log,
            Duration::from_millis(args.number("interval-ms", 500.0)? as u64),
            args.number("threshold", 0.02)? as f32,
        ),
        "status" => status(&log, &args.app()?),
        // `fade` is the same action under the name used in conversation.
        "pause" | "fade" => {
            if args.positional.len() > 2 {
                return Err(format!(
                    "{command} takes only an app name; it always fades to silence and pauses. \
                     To set a volume level use: cricket-probe volume <app> <level>"
                ));
            }
            pause(
                &log,
                &args.app()?,
                Duration::from_millis(args.number("fade-ms", DEFAULT_FADE_OUT_MS)? as u64),
            )
        }
        "resume" => resume(
            &log,
            &args.app()?,
            Duration::from_millis(args.number("fade-ms", DEFAULT_FADE_IN_MS)? as u64),
        ),
        "volume" => match args.positional.get(2) {
            None => status(&log, &args.app()?),
            Some(level) => set_volume(&log, &args.app()?, parse_level(level)?),
        },
        "cycle" => cycle(
            &log,
            &args.app()?,
            Duration::from_millis(args.number("fade-out-ms", DEFAULT_FADE_OUT_MS)? as u64),
            Duration::from_millis(args.number("hold-ms", 3000.0)? as u64),
            Duration::from_millis(args.number("fade-in-ms", DEFAULT_FADE_IN_MS)? as u64),
        ),
        "run" => {
            let defaults = Settings::default();
            let settings = Settings {
                trigger_delay_ms: args.number("trigger-ms", defaults.trigger_delay_ms as f64)?
                    as u64,
                resume_cooldown_ms: args
                    .number("cooldown-ms", defaults.resume_cooldown_ms as f64)?
                    as u64,
                fade_out_ms: args.number("fade-out-ms", defaults.fade_out_ms as f64)? as u64,
                fade_in_ms: args.number("fade-in-ms", defaults.fade_in_ms as f64)? as u64,
                sound_threshold: args.number("threshold", defaults.sound_threshold as f64)? as f32,
                mic_counts_as_activity: args.switch("mic", defaults.mic_counts_as_activity)?,
                ignore_system_sounds: !args
                    .switch("system-sounds", !defaults.ignore_system_sounds)?,
            };
            headless::run(&log, &args.app()?, settings)
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

/// Positional arguments plus `--name value` options.
struct Args {
    positional: Vec<String>,
    options: HashMap<String, String>,
}

impl Args {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut positional = Vec::new();
        let mut options = HashMap::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.strip_prefix("--") {
                Some("help") => positional.insert(0, "help".to_string()),
                Some(name) => {
                    let value = iter
                        .next()
                        .ok_or_else(|| format!("option --{name} needs a value"))?;
                    options.insert(name.to_string(), value.clone());
                }
                None => positional.push(arg.clone()),
            }
        }
        Ok(Self {
            positional,
            options,
        })
    }

    fn app(&self) -> Result<AppId, String> {
        self.positional
            .get(1)
            .map(|name| AppId::new(name))
            .ok_or_else(|| "this command needs an app name, for example spotify.exe".to_string())
    }

    fn switch(&self, name: &str, default: bool) -> Result<bool, String> {
        match self.options.get(name).map(String::as_str) {
            None => Ok(default),
            Some("on") => Ok(true),
            Some("off") => Ok(false),
            Some(other) => Err(format!("--{name} must be on or off, got `{other}`")),
        }
    }

    fn number(&self, name: &str, default: f64) -> Result<f64, String> {
        match self.options.get(name) {
            None => Ok(default),
            Some(text) => text
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| format!("--{name} must be a non-negative number, got `{text}`")),
        }
    }
}

fn parse_level(text: &str) -> Result<f32, String> {
    text.parse::<f32>()
        .ok()
        .filter(|level| (0.0..=1.0).contains(level))
        .ok_or_else(|| format!("level must be between 0.0 and 1.0, got `{text}`"))
}

/// Prints lines prefixed with the time since the probe started, so pasted
/// logs show how long things took.
pub struct Log {
    started: Instant,
}

impl Log {
    fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    pub fn line(&self, message: &str) {
        println!("[{:>8.3}s] {message}", self.started.elapsed().as_secs_f64());
    }
}

pub fn text<T>(result: BackendResult<T>) -> Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn list(log: &Log) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    // Peak meters read zero on the very first call, so sample for a moment.
    let window = sample(&mut backend, Duration::from_millis(300))?;
    log.line("audio sessions (loudest peak over 300 ms):");
    print_apps(log, &window, 0.02);
    print_mic(log, &window.last);
    print_media_sessions(log)?;
    Ok(())
}

fn watch(log: &Log, interval: Duration, threshold: f32) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    log.line(&format!(
        "watching: poll every {} ms, report every {} ms, sound threshold {threshold}",
        POLL_INTERVAL.as_millis(),
        interval.as_millis()
    ));

    let mut known_apps: BTreeSet<AppId> = BTreeSet::new();
    let mut known_mic_users: Vec<AppId> = Vec::new();
    let mut first = true;
    loop {
        let window = sample_with(&mut backend, interval, |snapshot| {
            let apps: BTreeSet<AppId> = snapshot.apps.iter().map(|a| a.app.clone()).collect();
            if !first {
                for app in apps.difference(&known_apps) {
                    log.line(&format!("EVENT session appeared: {app}"));
                }
                for app in known_apps.difference(&apps) {
                    log.line(&format!("EVENT session gone: {app}"));
                }
                if snapshot.mic_users != known_mic_users {
                    log.line(&format!("EVENT microphone: {}", mic_text(snapshot)));
                }
            }
            known_apps = apps;
            known_mic_users = snapshot.mic_users.clone();
            first = false;
        })?;

        log.line("----");
        print_apps(log, &window, threshold);
        print_mic(log, &window.last);
    }
}

/// Readings collected over one reporting interval.
struct Window {
    /// Loudest peak seen per app during the interval.
    peaks: BTreeMap<AppId, f32>,
    last: ActivitySnapshot,
}

fn sample(backend: &mut WinAudioBackend, duration: Duration) -> Result<Window, String> {
    sample_with(backend, duration, |_| {})
}

fn sample_with(
    backend: &mut WinAudioBackend,
    duration: Duration,
    mut on_snapshot: impl FnMut(&ActivitySnapshot),
) -> Result<Window, String> {
    let deadline = Instant::now() + duration;
    let mut peaks: BTreeMap<AppId, f32> = BTreeMap::new();
    loop {
        let snapshot = text(backend.snapshot())?;
        on_snapshot(&snapshot);
        for app in &snapshot.apps {
            let peak = peaks.entry(app.app.clone()).or_insert(0.0);
            *peak = peak.max(app.peak);
        }
        if Instant::now() >= deadline {
            return Ok(Window {
                peaks,
                last: snapshot,
            });
        }
        sleep(POLL_INTERVAL);
    }
}

fn print_apps(log: &Log, window: &Window, threshold: f32) {
    if window.last.apps.is_empty() {
        log.line("  (no audio sessions)");
    }
    for app in &window.last.apps {
        let peak = window.peaks.get(&app.app).copied().unwrap_or(app.peak);
        log.line(&app_line(app, peak, threshold));
    }
}

fn app_line(app: &AppActivity, peak: f32, threshold: f32) -> String {
    let filled = ((peak.clamp(0.0, 1.0) * BAR_WIDTH as f32).round() as usize).min(BAR_WIDTH);
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(BAR_WIDTH - filled));
    let pids: Vec<String> = app.pids.iter().map(u32::to_string).collect();
    format!(
        "  {:<28} {:<8} peak {peak:.3} [{bar}] {:<5} sessions={} pids={}{}",
        app.app.as_str(),
        if app.active { "active" } else { "inactive" },
        if peak >= threshold { "SOUND" } else { "" },
        app.session_count,
        pids.join(","),
        if app.is_system_sounds {
            " (system sounds)"
        } else {
            ""
        },
    )
}

fn print_mic(log: &Log, snapshot: &ActivitySnapshot) {
    log.line(&format!("  microphone: {}", mic_text(snapshot)));
}

fn mic_text(snapshot: &ActivitySnapshot) -> String {
    if snapshot.mic_in_use() {
        let users: Vec<&str> = snapshot.mic_users.iter().map(AppId::as_str).collect();
        format!("IN USE by {}", users.join(", "))
    } else {
        "not in use".to_string()
    }
}

fn print_media_sessions(log: &Log) -> CliResult {
    let controller = text(SmtcController::new())?;
    let sessions = text(controller.sessions())?;
    log.line("media sessions (SMTC):");
    if sessions.is_empty() {
        log.line("  (none; start playback in a media app to see it here)");
    }
    for session in sessions {
        log.line(&format!(
            "  {:<50} {:?}",
            session.app_user_model_id, session.state
        ));
    }
    Ok(())
}

fn status(log: &Log, app: &AppId) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    let mut controller = text(SmtcController::new())?;
    match backend.volume(app) {
        Ok(volume) => log.line(&format!("{app} volume: {volume:.3}")),
        Err(error) => log.line(&format!("{app} volume: unavailable ({error})")),
    }
    match controller.playback_state(app) {
        Ok(state) => log.line(&format!("{app} playback: {state:?}")),
        Err(error) => log.line(&format!("{app} playback: unavailable ({error})")),
    }
    Ok(())
}

fn pause(log: &Log, app: &AppId, fade_out: Duration) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    let mut controller = text(SmtcController::new())?;
    let original = text(backend.volume(app))?;
    log.line(&format!("{app} volume: {original:.3}"));
    log_playback(log, &mut controller, app, "before pause");
    fade_out_and_pause(log, &mut backend, &mut controller, app, original, fade_out)
}

fn resume(log: &Log, app: &AppId, fade_in: Duration) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    let mut controller = text(SmtcController::new())?;
    let mut target = text(backend.volume(app))?;
    if target < QUIET_VOLUME {
        // Left over from an earlier run or the Windows mixer. Fading in to
        // it would produce music nobody can hear.
        log.line(&format!(
            "NOTE {app} volume was {target:.3}; fading in to 1.000 instead"
        ));
        target = 1.0;
    }
    log_playback(log, &mut controller, app, "before resume");
    resume_and_fade_in(log, &mut backend, &mut controller, app, target, fade_in)
}

/// Fades to silence, pauses, then puts the volume back.
fn fade_out_and_pause(
    log: &Log,
    backend: &mut WinAudioBackend,
    controller: &mut SmtcController,
    app: &AppId,
    original: f32,
    fade_out: Duration,
) -> CliResult {
    let result = (|| {
        if !fade_out.is_zero() {
            fade(log, backend, app, original, 0.0, fade_out)?;
        }
        text(controller.pause(app))?;
        log.line("pause accepted");
        settle_and_log(log, controller, app, "after pause");
        Ok(())
    })();

    // Put the volume back as soon as the music is paused, and also when a
    // step failed. Windows remembers session volume per app, so a muted
    // source would stay muted if Cricket exited or the user resumed by hand.
    match backend.set_volume(app, original) {
        Ok(()) => log.line(&format!("volume restored to {original:.3} while paused")),
        Err(error) => log.line(&format!("could not restore volume: {error}")),
    }
    result
}

/// Resumes from silence and fades up to `target`.
fn resume_and_fade_in(
    log: &Log,
    backend: &mut WinAudioBackend,
    controller: &mut SmtcController,
    app: &AppId,
    target: f32,
    fade_in: Duration,
) -> CliResult {
    let result = (|| {
        if !fade_in.is_zero() {
            // Drop to zero only at the moment of resuming.
            text(backend.set_volume(app, 0.0))?;
        }
        text(controller.play(app))?;
        log.line("resume accepted");
        if !fade_in.is_zero() {
            fade(log, backend, app, 0.0, target, fade_in)?;
        }
        Ok(())
    })();

    if result.is_err() {
        match backend.set_volume(app, target) {
            Ok(()) => log.line(&format!("restored volume to {target:.3} after failure")),
            Err(error) => log.line(&format!("could not restore volume: {error}")),
        }
    }
    log_playback(log, controller, app, "after resume");
    result
}

fn log_playback(log: &Log, controller: &mut SmtcController, app: &AppId, label: &str) {
    match controller.playback_state(app) {
        Ok(state) => log.line(&format!("{app} playback {label}: {state:?}")),
        Err(error) => log.line(&format!("{app} playback {label}: unavailable ({error})")),
    }
}

/// Apps report their new state a moment after accepting a command.
fn settle_and_log(log: &Log, controller: &mut SmtcController, app: &AppId, label: &str) {
    sleep(Duration::from_millis(500));
    log_playback(log, controller, app, label);
}

fn set_volume(log: &Log, app: &AppId, level: f32) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    let before = text(backend.volume(app))?;
    text(backend.set_volume(app, level))?;
    let after = text(backend.volume(app))?;
    log.line(&format!("{app} volume: {before:.3} -> {after:.3}"));
    Ok(())
}

fn fade(
    log: &Log,
    backend: &mut WinAudioBackend,
    app: &AppId,
    from: f32,
    to: f32,
    duration: Duration,
) -> CliResult {
    log.line(&format!(
        "fade start: {from:.3} -> {to:.3} over {} ms",
        duration.as_millis()
    ));
    let steps = (duration.as_millis() / FADE_STEP.as_millis()).max(1) as u32;
    let mut last_logged = Instant::now();
    for step in 1..=steps {
        let level = fade_level(from, to, step as f32 / steps as f32);
        text(backend.set_volume(app, level))?;
        // A few readings along the way, so the log shows the ramp.
        if last_logged.elapsed() >= FADE_LOG_INTERVAL && step < steps {
            log.line(&format!("  fading: {level:.3}"));
            last_logged = Instant::now();
        }
        if step < steps {
            sleep(FADE_STEP);
        }
    }
    let reached = text(backend.volume(app))?;
    log.line(&format!("fade done: volume is {reached:.3}"));
    Ok(())
}

fn cycle(
    log: &Log,
    app: &AppId,
    fade_out: Duration,
    hold: Duration,
    fade_in: Duration,
) -> CliResult {
    let mut backend = text(WinAudioBackend::new())?;
    let mut controller = text(SmtcController::new())?;

    let original = text(backend.volume(app))?;
    log.line(&format!(
        "{app} original volume: {original:.3}. If this run is interrupted, restore it with: \
         cricket-probe volume {app} {original:.3}"
    ));
    log_playback(log, &mut controller, app, "at start");

    fade_out_and_pause(log, &mut backend, &mut controller, app, original, fade_out)?;

    log.line(&format!("holding for {} ms", hold.as_millis()));
    sleep(hold);

    resume_and_fade_in(log, &mut backend, &mut controller, app, original, fade_in)
}
