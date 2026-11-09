//! The generic strike pose a stand-in body (a plain cylinder) uses for any melee-style attack, so
//! a fighter with no model still reads as striking. Game-independent: a core gives the action's
//! clock (current frame, first active frame, end of recovery) and the host pitches the rendered
//! body forward by the returned angle about its center and local right axis. It only affects what
//! is drawn, never hit volumes or movement.

/// How far the body tips forward at the first active frame.
pub const STRIKE_PITCH_DEGREES: f32 = 45.;

fn ease(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// Forward pitch in radians for an action `frame` frames in. The body tips from upright at the
/// action's start to the full pitch on `first_active`, then eases back to upright by `end`.
/// A held smash charge should pass the frame it is frozen on, so the body stays upright until release.
/// Degenerate clocks (`first_active <= 0`, `end <= first_active`) snap to the full pitch at the
/// first active frame and upright at `end`, never producing NaN.
pub fn pitch(frame: f32, first_active: f32, end: f32) -> f32 {
    let full = STRIKE_PITCH_DEGREES.to_radians();
    if !frame.is_finite() || frame < 0. {
        return 0.;
    }
    if frame <= first_active {
        let t = if first_active > 0. { frame / first_active } else { 1. };
        full * ease(t)
    } else if frame < end {
        let span = (end - first_active).max(f32::EPSILON);
        full * (1. - ease((frame - first_active) / span))
    } else {
        0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaches_forty_five_degrees_at_the_first_active_frame_and_is_upright_at_the_ends() {
        assert_eq!(pitch(0., 3., 18.), 0.);
        assert!((pitch(3., 3., 18.) - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert_eq!(pitch(18., 3., 18.), 0.);
        assert_eq!(pitch(40., 3., 18.), 0.);
    }

    #[test]
    fn rises_into_the_strike_and_eases_out_of_it() {
        let rise: Vec<f32> = (0..=3).map(|f| pitch(f as f32, 3., 18.)).collect();
        assert!(rise.windows(2).all(|w| w[1] > w[0]), "{rise:?}");
        let fall: Vec<f32> = (3..=18).map(|f| pitch(f as f32, 3., 18.)).collect();
        assert!(fall.windows(2).all(|w| w[1] <= w[0]), "{fall:?}");
    }

    #[test]
    fn bad_clocks_never_produce_nan_or_overshoot() {
        for (f, a, e) in [(0., 0., 0.), (5., 0., 10.), (2., 4., 4.), (f32::NAN, 3., 9.), (-1., 3., 9.), (7., 3., 2.)] {
            let p = pitch(f, a, e);
            assert!(p.is_finite() && (0. ..=std::f32::consts::FRAC_PI_4 + 1e-6).contains(&p), "{f} {a} {e} -> {p}");
        }
    }
}
