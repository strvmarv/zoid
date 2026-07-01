//! Ⓡ2 motion — the pure math layer (easing, interpolation, animation progress,
//! caret blink). No clocks, no rendering: the `zoid` bin owns the `Instant`
//! clock and feeds elapsed milliseconds in. `reduced_motion` short-circuits
//! every animated value to its resting/final state (spec §13).

/// Frames per second for the bin's motion tick. The tick only runs while an
/// animation is active (the select-arm guard), so this is a ceiling, not a
/// steady draw rate.
pub const MOTION_FPS: u64 = 30;

/// Linear interpolation with `t` clamped to `[0, 1]`.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Ease-out cubic: `1 - (1 - t)^3`, clamped. Fast start, gentle stop.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A running animation as elapsed/duration milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anim {
    pub elapsed_ms: u32,
    pub duration_ms: u32,
}

impl Anim {
    /// Linear progress in `[0, 1]`. Returns `1.0` immediately when
    /// `reduced_motion` is set or the duration is zero (no division).
    pub fn progress(self, reduced_motion: bool) -> f32 {
        if reduced_motion || self.duration_ms == 0 {
            return 1.0;
        }
        (self.elapsed_ms as f32 / self.duration_ms as f32).clamp(0.0, 1.0)
    }
}

/// Whether the streaming caret is visible this instant. Steady-on under
/// reduced-motion (or a zero period); otherwise on for the first half of each
/// `period_ms` window.
pub fn caret_on(elapsed_ms: u64, period_ms: u64, reduced_motion: bool) -> bool {
    if reduced_motion || period_ms == 0 {
        return true;
    }
    (elapsed_ms % period_ms) < period_ms / 2
}

/// Number of lines to show at eased progress `t` of a fold/unfold reveal.
pub fn reveal_count(total: usize, t: f32) -> usize {
    let eased = ease_out_cubic(t);
    ((total as f32) * eased).round() as usize
}

/// Pure reveal gate: `Some(cap)` while a zoom animation is mid-flight, `None`
/// when no cap should apply (reduced-motion, zero duration, or finished — i.e.
/// show the final frame). Keeps the animate-or-not decision out of the bin's
/// impure draw closure so it is unit-testable (spec §13 determinism).
pub fn zoom_reveal(total: usize, elapsed_ms: u64, anim_ms: u64, reduced_motion: bool) -> Option<usize> {
    if reduced_motion || anim_ms == 0 || elapsed_ms >= anim_ms {
        return None;
    }
    Some(reveal_count(total, elapsed_ms as f32 / anim_ms as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_clamps_and_interpolates() {
        assert_eq!(lerp(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 10.0, 1.0), 10.0);
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
        assert_eq!(lerp(0.0, 10.0, 2.0), 10.0); // clamped above
        assert_eq!(lerp(0.0, 10.0, -1.0), 0.0); // clamped below
    }

    #[test]
    fn ease_out_cubic_endpoints_and_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        // ease-out is front-loaded: past the midpoint by t=0.5
        assert!(ease_out_cubic(0.5) > 0.5);
        // monotonic non-decreasing
        let mut prev = 0.0;
        for i in 0..=10 {
            let v = ease_out_cubic(i as f32 / 10.0);
            assert!(v >= prev, "not monotonic at {i}");
            prev = v;
        }
    }

    #[test]
    fn anim_progress_and_reduced_motion() {
        let a = Anim { elapsed_ms: 50, duration_ms: 100 };
        assert_eq!(a.progress(false), 0.5);
        // reduced-motion correctness: jumps to the end immediately
        assert_eq!(a.progress(true), 1.0);
        // zero duration never divides by zero
        assert_eq!(Anim { elapsed_ms: 0, duration_ms: 0 }.progress(false), 1.0);
        // past the end clamps
        assert_eq!(Anim { elapsed_ms: 999, duration_ms: 100 }.progress(false), 1.0);
    }

    #[test]
    fn reveal_count_eases_from_zero_to_total() {
        assert_eq!(reveal_count(10, 0.0), 0);
        assert_eq!(reveal_count(10, 1.0), 10);
        assert_eq!(reveal_count(10, -0.5), 0); // clamped
        assert_eq!(reveal_count(10, 2.0), 10); // clamped
        // monotonic non-decreasing
        let mut prev = 0;
        for i in 0..=10 {
            let c = reveal_count(10, i as f32 / 10.0);
            assert!(c >= prev);
            prev = c;
        }
        assert_eq!(reveal_count(0, 0.5), 0); // empty stays empty
    }

    #[test]
    fn zoom_reveal_gates_on_motion_and_completion() {
        // mid-animation → a capped count; the reduced-motion and completion cases
        // resolve to None (no cap = final frame). This is the determinism the inline
        // draw-closure logic couldn't be tested for.
        assert_eq!(zoom_reveal(10, 0, 160, false), Some(0));
        assert_eq!(zoom_reveal(10, 80, 160, false), Some(reveal_count(10, 0.5)));
        assert_eq!(zoom_reveal(10, 160, 160, false), None); // animation complete
        assert_eq!(zoom_reveal(10, 200, 160, false), None); // past the end
        assert_eq!(zoom_reveal(10, 80, 160, true), None); // reduced-motion → instant final frame
        assert_eq!(zoom_reveal(10, 80, 0, false), None); // zero duration never divides
    }

    #[test]
    fn caret_blinks_unless_reduced_motion() {
        // first half of the period: on; second half: off
        assert!(caret_on(0, 1000, false));
        assert!(caret_on(499, 1000, false));
        assert!(!caret_on(500, 1000, false));
        assert!(!caret_on(999, 1000, false));
        assert!(caret_on(1000, 1000, false)); // wraps
        // reduced-motion: steady on
        assert!(caret_on(500, 1000, true));
        // degenerate period: steady on
        assert!(caret_on(123, 0, false));
    }
}
