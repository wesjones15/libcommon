//! Keyframe interpolation shared by the animation readers. Each game keeps its own key
//! formats, loop rules and timing; this is only the curve between two keys.

/// Cubic Hermite from `p0` to `p1` at normalized position `s` (0 at the first key, 1 at
/// the second) over a span of `span` time units. `m0` is the first key's outgoing tangent
/// and `m1` the second key's incoming tangent, in value per time unit; scale them first
/// when a format stores them in other units.
///
/// `s` is not clamped, so out-of-range values extrapolate along the cubic.
pub fn hermite_s(s: f32, span: f32, p0: f32, m0: f32, p1: f32, m1: f32) -> f32 {
    let (s2, s3) = (s * s, s * s * s);
    (2.0 * s3 - 3.0 * s2 + 1.0) * p0
        + (s3 - 2.0 * s2 + s) * span * m0
        + (-2.0 * s3 + 3.0 * s2) * p1
        + (s3 - s2) * span * m1
}

/// [`hermite_s`] for a time `t` between keys at `t0` and `t1`. A key pair with no span
/// (`t1 <= t0`) is a step: it returns `p0`, and callers that want the other key's value
/// check for that first.
pub fn hermite(t: f32, t0: f32, p0: f32, m0: f32, t1: f32, p1: f32, m1: f32) -> f32 {
    let span = t1 - t0;
    if span <= 0.0 {
        return p0;
    }
    hermite_s((t - t0) / span, span, p0, m0, p1, m1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// The expression every standard-order caller used before the move, kept as the
    /// reference: libsms `Track::value`, libsms JPA key frames, libssbm tracks.
    fn standard(t: f32, t0: f32, p0: f32, m0: f32, t1: f32, p1: f32, m1: f32) -> f32 {
        let dt = t1 - t0;
        let s = (t - t0) / dt;
        let (s2, s3) = (s * s, s * s * s);
        (2.0 * s3 - 3.0 * s2 + 1.0) * p0
            + (s3 - 2.0 * s2 + s) * dt * m0
            + (-2.0 * s3 + 3.0 * s2) * p1
            + (s3 - s2) * dt * m1
    }

    /// libpikmin `.anm` (shapeBase.cpp `extract`): tangents per second at 30 fps.
    fn pikmin_anim(f: f32, t0: f32, v0: f32, m0: f32, t1: f32, v1: f32, m1: f32) -> f32 {
        let span = t1 - t0;
        let s = (f - t0) / span;
        let (s2, s3) = (s * s, s * s * s);
        let dt = span / 30.0;
        (2.0 * s3 - 3.0 * s2 + 1.0) * v0
            + (-2.0 * s3 + 3.0 * s2) * v1
            + (s3 - 2.0 * s2 + s) * dt * m0
            + (s3 - s2) * dt * m1
    }

    /// libpikmin model clips: the tangent terms written as `a * (s - 1)^2` and `a * (s^2 - s)`.
    fn pikmin_model(frame: f32, f0: f32, v0: f32, m0: f32, f1: f32, v1: f32, m1: f32) -> f32 {
        let span = f1 - f0;
        let a = frame - f0;
        let s = a / span;
        let (s2, s3) = (s * s, s * s * s);
        (2.0 * s3 - 3.0 * s2 + 1.0) * v0
            + (-2.0 * s3 + 3.0 * s2) * v1
            + a * (s2 - 2.0 * s + 1.0) * m0
            + a * (s2 - s) * m1
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-4 * (1.0 + a.abs().max(b.abs()))
    }

    #[test]
    fn matches_the_standard_form_bit_for_bit() {
        let mut seed = 7;
        for _ in 0..2000 {
            let t0 = lcg(&mut seed) * 40.0 - 5.0;
            let t1 = t0 + 0.25 + lcg(&mut seed) * 30.0;
            let (p0, m0) = (lcg(&mut seed) * 8.0 - 4.0, lcg(&mut seed) * 2.0 - 1.0);
            let (p1, m1) = (lcg(&mut seed) * 8.0 - 4.0, lcg(&mut seed) * 2.0 - 1.0);
            let t = t0 + lcg(&mut seed) * (t1 - t0);
            assert_eq!(
                hermite(t, t0, p0, m0, t1, p1, m1).to_bits(),
                standard(t, t0, p0, m0, t1, p1, m1).to_bits()
            );
        }
    }

    #[test]
    fn matches_the_pikmin_forms_within_float_rounding() {
        let mut seed = 99;
        for _ in 0..2000 {
            let t0 = (lcg(&mut seed) * 60.0).floor();
            let t1 = t0 + 1.0 + (lcg(&mut seed) * 20.0).floor();
            let (p0, m0) = (lcg(&mut seed) * 8.0 - 4.0, lcg(&mut seed) * 60.0 - 30.0);
            let (p1, m1) = (lcg(&mut seed) * 8.0 - 4.0, lcg(&mut seed) * 60.0 - 30.0);
            let t = t0 + lcg(&mut seed) * (t1 - t0);
            let ours = hermite(t, t0, p0, m0 / 30.0, t1, p1, m1 / 30.0);
            assert!(near(ours, pikmin_anim(t, t0, p0, m0, t1, p1, m1)));
            let ours = hermite(t, t0, p0, m0, t1, p1, m1);
            assert!(near(ours, pikmin_model(t, t0, p0, m0, t1, p1, m1)));
        }
    }

    #[test]
    fn hits_the_keys_and_follows_the_tangents() {
        assert_eq!(hermite(2.0, 2.0, 5.0, 1.0, 6.0, 9.0, 1.0), 5.0);
        assert_eq!(hermite(6.0, 2.0, 5.0, 1.0, 6.0, 9.0, 1.0), 9.0);
        // Equal tangents of the chord's slope make a straight line.
        assert!((hermite(3.0, 2.0, 5.0, 1.0, 6.0, 9.0, 1.0) - 6.0).abs() < 1e-6);
        // Zero tangents ease in and out: the midpoint is halfway.
        assert_eq!(hermite(4.0, 2.0, 0.0, 0.0, 6.0, 10.0, 0.0), 5.0);
    }

    #[test]
    fn a_zero_span_is_a_step_to_the_first_key() {
        assert_eq!(hermite(3.0, 3.0, 4.0, 9.0, 3.0, 8.0, 9.0), 4.0);
        assert_eq!(hermite(3.0, 3.0, 4.0, 9.0, 2.0, 8.0, 9.0), 4.0);
    }
}
