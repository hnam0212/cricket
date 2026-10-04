//! The ducking engine: decides when to fade out, pause, resume and fade in.
//!
//! A pure state machine (SPEC.md 5.2). Time comes from an injected
//! [`Clock`], input is one [`Observation`] per tick, output is a list of
//! [`Command`]s for the caller to carry out. No I/O happens here.

use std::time::Duration;

use crate::audio::PlaybackState;
use crate::clock::Clock;
use crate::fade::fade_level;
use crate::settings::Settings;

/// How long the source gets to report the effect of a pause or resume
/// command before the engine concludes the command did nothing.
const COMMAND_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Disabled, or not started yet.
    Idle,
    Playing,
    FadingOut,
    PausedByCricket,
    FadingIn,
    /// The source is not playing and Cricket did not pause it, so Cricket
    /// will not start it.
    PausedByUser,
}

/// What the caller must do to the music source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Pause,
    Resume,
    SetVolume(f32),
}

/// What the engine sees at one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Something other than the music source is making sound, or the
    /// microphone is in use.
    pub other_activity: bool,
    /// Playback state of the source. `None` if it has no media session.
    pub playback: Option<PlaybackState>,
    /// Volume of the source. `None` if it has no audio session, in which
    /// case the engine pauses and resumes without fading.
    pub volume: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from: State,
    pub to: State,
    /// For logs and the diagnostics view.
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutput {
    pub commands: Vec<Command>,
    /// At most one state change happens per tick.
    pub transition: Option<Transition>,
}

pub struct Engine<C: Clock> {
    clock: C,
    settings: Settings,
    enabled: bool,
    state: State,
    /// Start of the current unbroken stretch of other activity.
    activity_since: Option<Duration>,
    /// Start of the current unbroken stretch of silence.
    quiet_since: Option<Duration>,
    /// The source's volume before Cricket touched it.
    original_volume: Option<f32>,
    /// The source's volume currently differs from `original_volume` because
    /// of a command we issued, so it must be put back before we let go.
    volume_dirty: bool,
    /// Where a fade stands: 0.0 is silent, 1.0 is the original volume.
    /// Tracking a position instead of a start time is what lets a fade
    /// reverse from wherever it is without a jump.
    fade_position: f32,
    fade_updated: Duration,
    /// When a pause or resume command was issued that the source has not
    /// reflected in its playback state yet.
    awaiting_since: Option<Duration>,
}

type Next = Option<(State, &'static str)>;

impl<C: Clock> Engine<C> {
    pub fn new(clock: C, settings: Settings) -> Self {
        Self {
            clock,
            settings,
            enabled: true,
            state: State::Idle,
            activity_since: None,
            quiet_since: None,
            original_volume: None,
            volume_dirty: false,
            fade_position: 1.0,
            fade_updated: Duration::ZERO,
            awaiting_since: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Takes effect on the next tick.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    pub fn tick(&mut self, observation: &Observation) -> TickOutput {
        let now = self.clock.now();
        self.track_streaks(now, observation.other_activity);
        let playing = observation.playback == Some(PlaybackState::Playing);

        let mut commands = Vec::new();
        let next = if !self.enabled {
            self.tick_disabled(&mut commands)
        } else {
            match self.state {
                State::Idle => Some(if playing {
                    (State::Playing, "enabled while the source is playing")
                } else {
                    (
                        State::PausedByUser,
                        "enabled while the source is not playing",
                    )
                }),
                State::Playing => self.tick_playing(now, observation, playing, &mut commands),
                State::FadingOut => self.tick_fading_out(now, playing, &mut commands),
                State::PausedByCricket => self.tick_paused_by_cricket(now, playing, &mut commands),
                State::FadingIn => self.tick_fading_in(now, playing, &mut commands),
                State::PausedByUser => {
                    playing.then_some((State::Playing, "source started playing"))
                }
            }
        };

        let transition = next.map(|(to, reason)| {
            let from = self.state;
            self.state = to;
            Transition { from, to, reason }
        });
        TickOutput {
            commands,
            transition,
        }
    }

    fn track_streaks(&mut self, now: Duration, other_activity: bool) {
        if other_activity {
            self.quiet_since = None;
            self.activity_since.get_or_insert(now);
        } else {
            self.activity_since = None;
            self.quiet_since.get_or_insert(now);
        }
    }

    fn activity_lasted(&self, now: Duration, at_least: Duration) -> bool {
        self.activity_since
            .is_some_and(|since| now.saturating_sub(since) >= at_least)
    }

    fn quiet_lasted(&self, now: Duration, at_least: Duration) -> bool {
        self.quiet_since
            .is_some_and(|since| now.saturating_sub(since) >= at_least)
    }

    fn command_timed_out(&self, now: Duration, sent: Duration) -> bool {
        now.saturating_sub(sent) >= COMMAND_TIMEOUT
    }

    fn tick_disabled(&mut self, commands: &mut Vec<Command>) -> Next {
        if self.state == State::Idle {
            return None;
        }
        // Music that Cricket paused stays paused: switching Cricket off is
        // not a request to start playback. The volume is put back, though.
        self.restore_volume(commands);
        self.awaiting_since = None;
        Some((State::Idle, "disabled"))
    }

    fn tick_playing(
        &mut self,
        now: Duration,
        observation: &Observation,
        playing: bool,
        commands: &mut Vec<Command>,
    ) -> Next {
        if !playing {
            return Some((
                State::PausedByUser,
                "source stopped without a Cricket command",
            ));
        }
        if !self.activity_lasted(now, self.settings.trigger_delay()) {
            return None;
        }

        self.original_volume = observation.volume;
        if observation.volume.is_none() || self.settings.fade_out().is_zero() {
            commands.push(Command::Pause);
            self.awaiting_since = Some(now);
            return Some((
                State::PausedByCricket,
                "other sound lasted the trigger delay; paused without a fade",
            ));
        }
        self.fade_position = 1.0;
        self.fade_updated = now;
        Some((State::FadingOut, "other sound lasted the trigger delay"))
    }

    fn tick_fading_out(
        &mut self,
        now: Duration,
        playing: bool,
        commands: &mut Vec<Command>,
    ) -> Next {
        if !playing {
            self.restore_volume(commands);
            return Some((State::PausedByUser, "source stopped during the fade out"));
        }
        // The same debounce as for triggering: a short gap in the other
        // sound (between words, say) must not make the volume wobble.
        if self.quiet_lasted(now, self.settings.trigger_delay()) {
            self.fade_updated = now;
            return Some((State::FadingIn, "other sound stopped during the fade out"));
        }

        self.advance_fade(now, 0.0, self.settings.fade_out(), commands);
        if self.fade_position <= 0.0 {
            commands.push(Command::Pause);
            self.awaiting_since = Some(now);
            return Some((State::PausedByCricket, "fade out finished"));
        }
        None
    }

    fn tick_paused_by_cricket(
        &mut self,
        now: Duration,
        playing: bool,
        commands: &mut Vec<Command>,
    ) -> Next {
        if let Some(sent) = self.awaiting_since {
            if !playing {
                // The pause took effect. Put the volume back right away:
                // the OS remembers per-app volume, so a source left at zero
                // would stay muted if Cricket exited or the user pressed
                // play themselves.
                self.awaiting_since = None;
                self.restore_volume(commands);
            } else if self.command_timed_out(now, sent) {
                self.awaiting_since = None;
                self.restore_volume(commands);
                // Start the trigger delay over, so a source that ignores
                // pause is retried at a steady pace instead of every tick.
                self.activity_since = self.activity_since.map(|_| now);
                return Some((State::Playing, "pause had no effect"));
            }
            return None;
        }

        if playing {
            return Some((State::Playing, "source was resumed by the user"));
        }
        if !self.quiet_lasted(now, self.settings.resume_cooldown()) {
            return None;
        }

        if self.original_volume.is_some() && !self.settings.fade_in().is_zero() {
            // Drop to zero only at the moment of resuming.
            commands.push(Command::SetVolume(0.0));
            self.volume_dirty = true;
            self.fade_position = 0.0;
        } else {
            self.fade_position = 1.0;
        }
        commands.push(Command::Resume);
        self.awaiting_since = Some(now);
        self.fade_updated = now;
        Some((State::FadingIn, "quiet for the resume cooldown"))
    }

    fn tick_fading_in(
        &mut self,
        now: Duration,
        playing: bool,
        commands: &mut Vec<Command>,
    ) -> Next {
        if let Some(sent) = self.awaiting_since {
            if playing {
                self.awaiting_since = None;
            } else if self.command_timed_out(now, sent) {
                self.awaiting_since = None;
                self.restore_volume(commands);
                return Some((State::PausedByUser, "resume had no effect"));
            }
        } else if !playing {
            self.restore_volume(commands);
            return Some((State::PausedByUser, "source stopped during the fade in"));
        } else if self.activity_lasted(now, self.settings.trigger_delay()) {
            self.fade_updated = now;
            return Some((State::FadingOut, "other sound returned during the fade in"));
        }

        self.advance_fade(now, 1.0, self.settings.fade_in(), commands);
        if self.fade_position >= 1.0 && self.awaiting_since.is_none() {
            self.volume_dirty = false;
            return Some((State::Playing, "fade in finished"));
        }
        None
    }

    /// Moves the fade toward `target` (0.0 or 1.0) by the time that passed
    /// since the last update and emits the matching volume.
    fn advance_fade(
        &mut self,
        now: Duration,
        target: f32,
        full_duration: Duration,
        commands: &mut Vec<Command>,
    ) {
        let elapsed = now.saturating_sub(self.fade_updated);
        self.fade_updated = now;
        if self.fade_position == target {
            // Nothing to fade (no volume to control, or a zero duration).
            return;
        }
        let step = if full_duration.is_zero() {
            1.0
        } else {
            elapsed.as_secs_f32() / full_duration.as_secs_f32()
        };
        self.fade_position = if target > self.fade_position {
            (self.fade_position + step).min(target)
        } else {
            (self.fade_position - step).max(target)
        };

        if let Some(original) = self.original_volume {
            commands.push(Command::SetVolume(fade_level(
                0.0,
                original,
                self.fade_position,
            )));
            self.volume_dirty = self.fade_position < 1.0;
        }
    }

    fn restore_volume(&mut self, commands: &mut Vec<Command>) {
        if self.volume_dirty {
            if let Some(original) = self.original_volume {
                commands.push(Command::SetVolume(original));
            }
            self.volume_dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;

    const TICK: Duration = Duration::from_millis(50);
    const VOLUME: f32 = 0.8;

    /// The engine plus a pretend music source that obeys its commands.
    struct Sim {
        clock: ManualClock,
        engine: Engine<ManualClock>,
        playing: bool,
        volume: Option<f32>,
        activity: bool,
        obeys_pause: bool,
        obeys_resume: bool,
        /// Ticks before a pause or resume shows in the playback state.
        lag_ticks: u32,
        pending: Option<(u32, bool)>,
        commands: Vec<Command>,
        transitions: Vec<Transition>,
    }

    impl Sim {
        /// A source that is playing, with the engine already in `Playing`.
        fn playing() -> Self {
            Self::playing_with(Settings::default())
        }

        fn playing_with(settings: Settings) -> Self {
            let clock = ManualClock::default();
            let mut sim = Self {
                engine: Engine::new(clock.clone(), settings),
                clock,
                playing: true,
                volume: Some(VOLUME),
                activity: false,
                obeys_pause: true,
                obeys_resume: true,
                lag_ticks: 0,
                pending: None,
                commands: Vec::new(),
                transitions: Vec::new(),
            };
            sim.tick();
            assert_eq!(sim.state(), State::Playing);
            sim.transitions.clear();
            sim
        }

        fn state(&self) -> State {
            self.engine.state()
        }

        fn tick(&mut self) {
            if let Some((ticks_left, playing)) = self.pending {
                if ticks_left == 0 {
                    self.playing = playing;
                    self.pending = None;
                } else {
                    self.pending = Some((ticks_left - 1, playing));
                }
            }

            let output = self.engine.tick(&Observation {
                other_activity: self.activity,
                playback: Some(if self.playing {
                    PlaybackState::Playing
                } else {
                    PlaybackState::Paused
                }),
                volume: self.volume,
            });

            for command in &output.commands {
                match *command {
                    Command::Pause if self.obeys_pause => self.set_playing_after_lag(false),
                    Command::Resume if self.obeys_resume => self.set_playing_after_lag(true),
                    Command::SetVolume(level) => self.volume = Some(level),
                    _ => {}
                }
            }
            self.commands.extend(output.commands);
            self.transitions.extend(output.transition);
            self.clock.advance(TICK);
        }

        fn set_playing_after_lag(&mut self, playing: bool) {
            if self.lag_ticks == 0 {
                self.playing = playing;
            } else {
                self.pending = Some((self.lag_ticks - 1, playing));
            }
        }

        fn run(&mut self, duration: Duration) {
            for _ in 0..(duration.as_millis() / TICK.as_millis()) {
                self.tick();
            }
        }

        fn run_ms(&mut self, ms: u64) {
            self.run(Duration::from_millis(ms));
        }

        /// Ticks until the engine reaches `state`, and returns how long that
        /// took. Panics if it does not get there within a minute.
        fn run_until(&mut self, state: State) -> Duration {
            let start = self.clock.now();
            while self.state() != state {
                assert!(
                    self.clock.now() - start < Duration::from_secs(60),
                    "never reached {state:?}, stuck in {:?}",
                    self.state()
                );
                self.tick();
            }
            self.clock.now() - start
        }

        fn count(&self, wanted: Command) -> usize {
            self.commands.iter().filter(|c| **c == wanted).count()
        }

        fn volumes(&self) -> Vec<f32> {
            self.commands
                .iter()
                .filter_map(|command| match command {
                    Command::SetVolume(level) => Some(*level),
                    _ => None,
                })
                .collect()
        }

        fn path(&self) -> Vec<State> {
            self.transitions.iter().map(|t| t.to).collect()
        }

        /// Drives a full pause: activity until Cricket has paused the source.
        fn pause_by_activity(&mut self) {
            self.activity = true;
            self.run_until(State::PausedByCricket);
            // Let a lagging source catch up, then one tick for the restore.
            for _ in 0..20 {
                if !self.playing {
                    break;
                }
                self.tick();
            }
            self.tick();
            assert!(!self.playing);
        }
    }

    fn assert_no_jumps(volumes: &[f32]) {
        for pair in volumes.windows(2) {
            assert!(
                (pair[0] - pair[1]).abs() < 0.06,
                "volume jumped from {} to {}",
                pair[0],
                pair[1]
            );
        }
    }

    // Scenario 3 (and 1 and 2, which differ only in where the activity
    // comes from): other sound pauses the music, silence brings it back.
    #[test]
    fn other_sound_pauses_and_silence_resumes() {
        let mut sim = Sim::playing();

        sim.activity = true;
        sim.run_until(State::PausedByCricket);
        sim.run_ms(100);
        assert!(!sim.playing);
        assert_eq!(sim.count(Command::Pause), 1);

        sim.activity = false;
        sim.run_until(State::Playing);
        assert!(sim.playing);
        assert_eq!(sim.count(Command::Resume), 1);
        assert_eq!(sim.volume, Some(VOLUME));
        assert_eq!(
            sim.path(),
            vec![
                State::FadingOut,
                State::PausedByCricket,
                State::FadingIn,
                State::Playing
            ]
        );
    }

    #[test]
    fn fade_out_starts_only_after_the_trigger_delay() {
        let mut sim = Sim::playing();
        sim.activity = true;

        sim.run_ms(500);
        assert_eq!(sim.state(), State::Playing);
        sim.tick();
        assert_eq!(sim.state(), State::FadingOut);
    }

    #[test]
    fn fade_out_ramps_down_to_zero_then_pauses() {
        let mut sim = Sim::playing();
        sim.activity = true;
        sim.run_until(State::FadingOut);

        let fade = sim.run_until(State::PausedByCricket);

        assert_eq!(fade, Duration::from_millis(2500) + TICK);
        let volumes = sim.volumes();
        assert!(volumes.windows(2).all(|pair| pair[1] < pair[0]));
        assert_no_jumps(&volumes);
        assert_eq!(volumes.last(), Some(&0.0));
        assert_eq!(sim.commands.last(), Some(&Command::Pause));
    }

    #[test]
    fn volume_is_restored_as_soon_as_the_pause_takes_effect() {
        let mut sim = Sim::playing();
        sim.lag_ticks = 4;
        sim.activity = true;
        sim.run_until(State::PausedByCricket);

        // Still reported as playing: the volume must stay down, or the
        // music would blip back in before it stops.
        sim.run_ms(150);
        assert!(sim.playing);
        assert_eq!(sim.volume, Some(0.0));

        sim.run_ms(200);
        assert!(!sim.playing);
        assert_eq!(sim.volume, Some(VOLUME));
        assert_eq!(sim.state(), State::PausedByCricket);
    }

    #[test]
    fn resume_starts_from_zero_and_fades_up_to_the_original_volume() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.commands.clear();

        sim.activity = false;
        sim.run_until(State::FadingIn);
        assert_eq!(sim.commands, vec![Command::SetVolume(0.0), Command::Resume]);

        let fade = sim.run_until(State::Playing);
        assert_eq!(fade, Duration::from_millis(1500));
        let volumes = sim.volumes();
        assert!(volumes.windows(2).all(|pair| pair[1] >= pair[0]));
        assert_no_jumps(&volumes);
        assert_eq!(volumes.last(), Some(&VOLUME));
    }

    #[test]
    fn resume_waits_for_the_full_cooldown_of_silence() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();

        sim.activity = false;
        sim.run_ms(3000);
        assert_eq!(sim.state(), State::PausedByCricket);
        sim.tick();
        assert_eq!(sim.state(), State::FadingIn);
    }

    #[test]
    fn sound_during_the_cooldown_restarts_it() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();

        sim.activity = false;
        sim.run_ms(2500);
        sim.activity = true;
        sim.run_ms(100);
        sim.activity = false;
        sim.run_ms(2900);

        assert_eq!(sim.state(), State::PausedByCricket);
        assert_eq!(sim.count(Command::Resume), 0);
        sim.run_ms(200);
        assert_eq!(sim.state(), State::FadingIn);
    }

    // Scenario 4.
    #[test]
    fn a_sound_shorter_than_the_trigger_delay_does_not_pause() {
        let mut sim = Sim::playing();

        sim.activity = true;
        sim.run_ms(400);
        sim.activity = false;
        sim.run_ms(5000);

        assert_eq!(sim.state(), State::Playing);
        assert!(sim.commands.is_empty());
    }

    #[test]
    fn short_sounds_do_not_add_up() {
        let mut sim = Sim::playing();

        for _ in 0..10 {
            sim.activity = true;
            sim.run_ms(400);
            sim.activity = false;
            sim.run_ms(100);
        }

        assert_eq!(sim.state(), State::Playing);
        assert!(sim.commands.is_empty());
    }

    // Scenario 5.
    #[test]
    fn music_paused_by_the_user_is_never_resumed() {
        let mut sim = Sim::playing();

        sim.playing = false;
        sim.tick();
        assert_eq!(sim.state(), State::PausedByUser);

        sim.activity = true;
        sim.run_ms(5000);
        sim.activity = false;
        sim.run_ms(60_000);

        assert_eq!(sim.state(), State::PausedByUser);
        assert!(sim.commands.is_empty());
    }

    #[test]
    fn paused_by_user_returns_to_playing_when_the_source_plays_again() {
        let mut sim = Sim::playing();
        sim.playing = false;
        sim.tick();

        sim.playing = true;
        sim.tick();

        assert_eq!(sim.state(), State::Playing);
        assert!(sim.commands.is_empty());
    }

    #[test]
    fn user_pause_during_the_fade_out_keeps_it_paused_and_restores_volume() {
        let mut sim = Sim::playing();
        sim.activity = true;
        sim.run_until(State::FadingOut);
        sim.run_ms(1000);
        assert!(sim.volume.unwrap() < VOLUME);

        sim.playing = false;
        sim.tick();
        assert_eq!(sim.state(), State::PausedByUser);
        assert_eq!(sim.volume, Some(VOLUME));

        sim.activity = false;
        sim.run_ms(60_000);
        assert_eq!(sim.count(Command::Pause), 0);
        assert_eq!(sim.count(Command::Resume), 0);
    }

    #[test]
    fn user_pause_during_the_fade_in_keeps_it_paused_and_restores_volume() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.activity = false;
        sim.run_until(State::FadingIn);
        sim.run_ms(500);
        assert!(sim.volume.unwrap() < VOLUME);

        sim.playing = false;
        sim.tick();
        assert_eq!(sim.state(), State::PausedByUser);
        assert_eq!(sim.volume, Some(VOLUME));

        sim.run_ms(60_000);
        assert_eq!(sim.count(Command::Resume), 1);
    }

    #[test]
    fn user_resuming_music_that_cricket_paused_goes_back_to_playing() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.activity = false;
        sim.run_ms(1000);

        sim.playing = true;
        sim.tick();

        assert_eq!(sim.state(), State::Playing);
        assert_eq!(sim.count(Command::Resume), 0);
        assert_eq!(sim.volume, Some(VOLUME));
    }

    #[test]
    fn fade_out_reverses_smoothly_when_the_sound_stops() {
        let mut sim = Sim::playing();
        sim.activity = true;
        sim.run_until(State::FadingOut);
        sim.run_ms(1000);
        let lowest = sim.volume.unwrap();
        assert!(lowest > 0.0 && lowest < VOLUME);

        sim.activity = false;
        sim.run_until(State::Playing);

        assert_eq!(
            sim.path(),
            vec![State::FadingOut, State::FadingIn, State::Playing]
        );
        assert_eq!(sim.count(Command::Pause), 0);
        assert_eq!(sim.count(Command::Resume), 0);
        assert_no_jumps(&sim.volumes());
        assert_eq!(sim.volume, Some(VOLUME));
    }

    #[test]
    fn a_short_gap_does_not_reverse_the_fade_out() {
        let mut sim = Sim::playing();
        sim.activity = true;
        sim.run_until(State::FadingOut);

        sim.run_ms(500);
        sim.activity = false;
        sim.run_ms(300);
        sim.activity = true;
        sim.run_until(State::PausedByCricket);

        assert_eq!(sim.path(), vec![State::FadingOut, State::PausedByCricket]);
    }

    #[test]
    fn fade_in_reverses_smoothly_when_the_sound_returns() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.activity = false;
        sim.run_until(State::FadingIn);
        sim.run_ms(700);
        sim.commands.clear();
        sim.transitions.clear();

        sim.activity = true;
        sim.run_until(State::PausedByCricket);
        sim.run_ms(100);

        assert_eq!(sim.path(), vec![State::FadingOut, State::PausedByCricket]);
        assert!(!sim.playing);
        // Up for the rest of the trigger delay, then down, without a jump.
        let volumes = sim.volumes();
        let down = &volumes[..volumes.len() - 1];
        assert_no_jumps(down);
        assert_eq!(down.last(), Some(&0.0));
        // The last volume command is the restore once the pause took effect.
        assert_eq!(volumes.last(), Some(&VOLUME));
    }

    #[test]
    fn a_short_sound_during_the_fade_in_does_not_reverse_it() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.activity = false;
        sim.run_until(State::FadingIn);
        sim.transitions.clear();

        sim.run_ms(300);
        sim.activity = true;
        sim.run_ms(300);
        sim.activity = false;
        sim.run_until(State::Playing);

        assert_eq!(sim.path(), vec![State::Playing]);
    }

    #[test]
    fn slow_resume_is_not_mistaken_for_a_user_pause() {
        let mut sim = Sim::playing();
        sim.lag_ticks = 6;
        sim.pause_by_activity();
        sim.run_ms(500);

        sim.activity = false;
        sim.run_until(State::Playing);

        assert!(sim.playing);
        assert_eq!(sim.count(Command::Resume), 1);
        assert!(!sim.path().contains(&State::PausedByUser));
    }

    #[test]
    fn slow_pause_is_not_mistaken_for_a_user_resume() {
        let mut sim = Sim::playing();
        sim.lag_ticks = 6;
        sim.activity = true;

        sim.run_until(State::PausedByCricket);
        sim.run_ms(2000);

        assert_eq!(sim.state(), State::PausedByCricket);
        assert!(!sim.playing);
        assert_eq!(sim.count(Command::Pause), 1);
    }

    #[test]
    fn a_pause_that_has_no_effect_restores_the_volume_and_retries() {
        let mut sim = Sim::playing();
        sim.obeys_pause = false;
        sim.activity = true;

        sim.run_until(State::PausedByCricket);
        sim.run_until(State::Playing);

        assert_eq!(
            sim.transitions.last().unwrap().reason,
            "pause had no effect"
        );
        assert_eq!(sim.volume, Some(VOLUME));

        // Not straight away: the trigger delay runs again first.
        sim.run_ms(450);
        assert_eq!(sim.state(), State::Playing);
        sim.run_until(State::FadingOut);
    }

    #[test]
    fn a_resume_that_has_no_effect_gives_up_and_restores_the_volume() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.obeys_resume = false;

        sim.activity = false;
        sim.run_until(State::PausedByUser);

        assert_eq!(
            sim.transitions.last().unwrap().reason,
            "resume had no effect"
        );
        assert_eq!(sim.volume, Some(VOLUME));
        sim.run_ms(60_000);
        assert_eq!(sim.count(Command::Resume), 1);
    }

    #[test]
    fn a_source_without_a_volume_is_paused_and_resumed_without_fading() {
        let mut sim = Sim::playing();
        sim.volume = None;

        sim.activity = true;
        sim.run_ms(500);
        sim.tick();
        assert_eq!(sim.state(), State::PausedByCricket);
        assert_eq!(sim.commands, vec![Command::Pause]);

        sim.activity = false;
        sim.run_until(State::Playing);
        assert_eq!(sim.commands, vec![Command::Pause, Command::Resume]);
    }

    #[test]
    fn zero_fade_durations_pause_and_resume_at_once() {
        let mut sim = Sim::playing_with(Settings {
            fade_out_ms: 0,
            fade_in_ms: 0,
            ..Settings::default()
        });

        sim.activity = true;
        sim.run_ms(500);
        sim.tick();
        assert_eq!(sim.state(), State::PausedByCricket);

        sim.activity = false;
        sim.run_until(State::Playing);
        assert_eq!(sim.commands, vec![Command::Pause, Command::Resume]);
        assert_eq!(sim.volume, Some(VOLUME));
    }

    #[test]
    fn enabling_while_the_source_is_paused_does_not_start_it() {
        let clock = ManualClock::default();
        let mut engine = Engine::new(clock, Settings::default());

        let output = engine.tick(&Observation {
            other_activity: false,
            playback: Some(PlaybackState::Paused),
            volume: Some(VOLUME),
        });

        assert_eq!(engine.state(), State::PausedByUser);
        assert!(output.commands.is_empty());
    }

    #[test]
    fn a_source_without_a_media_session_counts_as_not_playing() {
        let clock = ManualClock::default();
        let mut engine = Engine::new(clock, Settings::default());

        engine.tick(&Observation {
            other_activity: true,
            playback: None,
            volume: None,
        });

        assert_eq!(engine.state(), State::PausedByUser);
    }

    #[test]
    fn disabling_mid_fade_restores_the_volume_and_goes_idle() {
        let mut sim = Sim::playing();
        sim.activity = true;
        sim.run_until(State::FadingOut);
        sim.run_ms(1000);

        sim.engine.set_enabled(false);
        sim.tick();

        assert_eq!(sim.state(), State::Idle);
        assert_eq!(sim.volume, Some(VOLUME));
        assert!(sim.playing);
        sim.run_ms(10_000);
        assert_eq!(sim.count(Command::Pause), 0);
    }

    #[test]
    fn disabling_while_paused_by_cricket_does_not_resume() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();

        sim.engine.set_enabled(false);
        sim.activity = false;
        sim.run_ms(10_000);

        assert_eq!(sim.state(), State::Idle);
        assert!(!sim.playing);
        assert_eq!(sim.count(Command::Resume), 0);
        assert_eq!(sim.volume, Some(VOLUME));
    }

    #[test]
    fn enabling_again_picks_up_from_the_source_state() {
        let mut sim = Sim::playing();
        sim.engine.set_enabled(false);
        sim.tick();
        assert_eq!(sim.state(), State::Idle);

        sim.engine.set_enabled(true);
        sim.tick();
        assert_eq!(sim.state(), State::Playing);
    }

    #[test]
    fn every_transition_carries_a_reason() {
        let mut sim = Sim::playing();
        sim.pause_by_activity();
        sim.activity = false;
        sim.run_until(State::Playing);

        assert!(sim.transitions.iter().all(|t| !t.reason.is_empty()));
        assert!(sim.transitions.iter().all(|t| t.from != t.to));
    }
}
