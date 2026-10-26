//! Texture addressing shared by CPU previews, asset readers and native host ABIs.
//!
//! GX codes follow `dolphin/gx/GXEnum.h` (`GXTexWrapMode`). The rendering ABI's
//! historical repeat/clamp codes are reversed, so convert explicitly at boundaries.
//! Mirror reflects the entire RGBA texel: quadrant alpha masks assemble exactly
//! like their color channels, without extra opacity multiplication.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    Clamp,
    Repeat,
    Mirror,
}

impl Wrap {
    /// GX_CLAMP=0, GX_REPEAT=1, GX_MIRROR=2. Reserved values clamp.
    pub const fn from_gx(code: u8) -> Self {
        match code {
            1 => Self::Repeat,
            2 => Self::Mirror,
            _ => Self::Clamp,
        }
    }

    pub const fn to_gx(self) -> u8 {
        match self {
            Self::Clamp => 0,
            Self::Repeat => 1,
            Self::Mirror => 2,
        }
    }

    /// Shared native-view ABI: repeat=0, clamp=1, mirror=2.
    pub const fn to_host(self) -> u8 {
        match self {
            Self::Repeat => 0,
            Self::Clamp => 1,
            Self::Mirror => 2,
        }
    }

    /// Shared native-view ABI. Reserved values retain the host's repeat fallback.
    pub const fn from_host(code: u8) -> Self {
        match code {
            1 => Self::Clamp,
            2 => Self::Mirror,
            _ => Self::Repeat,
        }
    }

    /// Address a texel, including negative coordinates and both sides of a seam.
    /// Bilinear samplers apply this independently to each neighbor texel.
    ///
    /// # Panics
    /// Panics for zero-sized images.
    pub fn index(self, texel: i64, size: u32) -> usize {
        assert!(size != 0, "texture dimension must be nonzero");
        let n = i64::from(size);
        match self {
            Self::Clamp => texel.clamp(0, n - 1) as usize,
            Self::Repeat => texel.rem_euclid(n) as usize,
            Self::Mirror => {
                let at = texel.rem_euclid(n * 2);
                if at < n {
                    at as usize
                } else {
                    (n * 2 - 1 - at) as usize
                }
            }
        }
    }
}

/// Sample row-major RGBA8 with independent U/V addressing, without color/alpha
/// conversion. Returns `None` for invalid dimensions, short buffers or nonfinite UVs.
pub fn sample_rgba8_nearest(
    width: u32,
    height: u32,
    pixels: &[u8],
    wrap: [Wrap; 2],
    u: f32,
    v: f32,
) -> Option<[u8; 4]> {
    let bytes = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || pixels.len() < bytes || !u.is_finite() || !v.is_finite() {
        return None;
    }
    let x = wrap[0].index((u * width as f32).floor() as i64, width);
    let y = wrap[1].index((v * height as f32).floor() as i64, height);
    let start = (y * width as usize + x) * 4;
    Some(pixels[start..start + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gx_and_host_codes_are_explicit_and_round_trip() {
        for mode in [Wrap::Clamp, Wrap::Repeat, Wrap::Mirror] {
            assert_eq!(Wrap::from_gx(mode.to_gx()), mode);
            assert_eq!(Wrap::from_host(mode.to_host()), mode);
        }
        assert_eq!(Wrap::from_gx(0), Wrap::Clamp);
        assert_eq!(Wrap::from_host(0), Wrap::Repeat);
        assert_eq!(Wrap::from_gx(255), Wrap::Clamp);
        assert_eq!(Wrap::from_host(255), Wrap::Repeat);
    }

    #[test]
    fn addressing_handles_negative_coordinates_and_mirror_seams() {
        let coords = [-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5];
        assert_eq!(
            coords.map(|v| Wrap::Mirror.index(v, 2)),
            [0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1]
        );
        assert_eq!(
            coords.map(|v| Wrap::Repeat.index(v, 2)),
            [1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]
        );
        assert_eq!(Wrap::Clamp.index(-20, 2), 0);
        assert_eq!(Wrap::Clamp.index(20, 2), 1);
        for v in coords {
            assert_eq!(Wrap::Mirror.index(v, 1), 0);
        }
    }

    #[test]
    fn mirrored_quadrants_join_color_and_alpha_without_changing_opacity() {
        let pixels = [
            10, 11, 12, 13, 20, 21, 22, 23, 30, 31, 32, 33, 40, 41, 42, 200,
        ];
        let sample = |x, y| sample_rgba8_nearest(2, 2, &pixels, [Wrap::Mirror; 2], x, y).unwrap();
        let center = [40, 41, 42, 200];
        for (u, v) in [(0.75, 0.75), (1.25, 0.75), (0.75, 1.25), (1.25, 1.25)] {
            assert_eq!(sample(u, v), center);
        }
        assert_eq!(sample(0.25, 0.25), sample(1.75, 1.75));
        assert_eq!(sample(-0.25, -0.25), sample(0.25, 0.25));
    }

    #[test]
    fn axes_keep_distinct_addressing_modes() {
        let pixels = [
            10, 11, 12, 13, 20, 21, 22, 23, 30, 31, 32, 33, 40, 41, 42, 43,
        ];
        assert_eq!(
            sample_rgba8_nearest(2, 2, &pixels, [Wrap::Mirror, Wrap::Clamp], 1.25, -4.),
            Some([20, 21, 22, 23])
        );
        assert_eq!(
            sample_rgba8_nearest(2, 2, &pixels, [Wrap::Repeat, Wrap::Mirror], 1.25, 1.25),
            Some([30, 31, 32, 33])
        );
    }

    #[test]
    fn invalid_texture_inputs_are_rejected() {
        for (w, h, pixels, u, v) in [
            (0, 1, &[0u8; 4][..], 0., 0.),
            (1, 0, &[0u8; 4][..], 0., 0.),
            (1, 1, &[0u8; 3][..], 0., 0.),
            (1, 1, &[0u8; 4][..], f32::NAN, 0.),
            (1, 1, &[0u8; 4][..], 0., f32::INFINITY),
        ] {
            assert_eq!(
                sample_rgba8_nearest(w, h, pixels, [Wrap::Clamp; 2], u, v),
                None
            );
        }
    }
}
