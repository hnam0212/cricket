//! The volume curve used for fades.

/// Volume at a point of a fade from `from` to `to`, all 0.0 to 1.0.
/// `progress` runs from 0.0 (start) to 1.0 (end) and is clamped.
///
/// Session volume is an amplitude factor, but loudness is heard roughly
/// logarithmically. A straight line in amplitude stays loud for most of a
/// fade-out and then drops off a cliff at the end, which sounds like an
/// abrupt stop. Moving along a square curve instead takes the level down
/// early and lets it tail off, which the ear hears as an even fade.
pub fn fade_level(from: f32, to: f32, progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    // Exact at both ends: squaring a square root is off by a rounding
    // error, and a fade must land precisely on the user's volume.
    if progress <= 0.0 {
        return from.clamp(0.0, 1.0);
    }
    if progress >= 1.0 {
        return to.clamp(0.0, 1.0);
    }
    let from = from.clamp(0.0, 1.0).sqrt();
    let to = to.clamp(0.0, 1.0).sqrt();
    let position = from + (to - from) * progress;
    position * position
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn starts_and_ends_exactly_on_the_given_levels() {
        assert!(close(fade_level(0.8, 0.0, 0.0), 0.8));
        assert!(close(fade_level(0.8, 0.0, 1.0), 0.0));
        assert!(close(fade_level(0.0, 0.6, 0.0), 0.0));
        assert!(close(fade_level(0.0, 0.6, 1.0), 0.6));
    }

    #[test]
    fn fade_out_is_already_quiet_at_the_midpoint() {
        // A straight line would still be at 0.5 here.
        assert!(close(fade_level(1.0, 0.0, 0.5), 0.25));
    }

    #[test]
    fn fade_in_mirrors_fade_out() {
        for step in 0..=10 {
            let progress = step as f32 / 10.0;
            assert!(close(
                fade_level(0.0, 0.7, progress),
                fade_level(0.7, 0.0, 1.0 - progress)
            ));
        }
    }

    #[test]
    fn moves_in_one_direction_without_jumps() {
        let mut previous = fade_level(1.0, 0.0, 0.0);
        for step in 1..=100 {
            let level = fade_level(1.0, 0.0, step as f32 / 100.0);
            assert!(level <= previous);
            assert!(previous - level < 0.03);
            previous = level;
        }
    }

    #[test]
    fn progress_outside_the_range_is_clamped() {
        assert!(close(fade_level(0.5, 0.0, -1.0), 0.5));
        assert!(close(fade_level(0.5, 0.0, 2.0), 0.0));
    }
}
