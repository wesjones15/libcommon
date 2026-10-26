//! Texture addressing shared by CPU previews, asset readers and native host ABIs.
//!
//! GX codes follow `dolphin/gx/GXEnum.h` (`GXTexWrapMode`). The rendering ABI's
//! historical repeat/clamp codes are reversed, so convert explicitly at boundaries.
//! Mirror reflects the entire RGBA texel: quadrant alpha masks assemble exactly
//! like their color channels, without extra opacity multiplication.

pub mod gx;

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

/// GX-style bilinear filtering of row-major RGBA8 at `(u, v)` in 0..1 texture space, each axis
/// addressed independently, returning channels in 0..1. Texel centers sit half a texel in, and
/// every one of the four neighbors is addressed on its own, so filtering is seamless across
/// repeat and mirror seams. `None` for invalid dimensions, short buffers or nonfinite UVs.
pub fn sample_rgba8_bilinear(
    width: u32,
    height: u32,
    pixels: &[u8],
    wrap: [Wrap; 2],
    u: f32,
    v: f32,
) -> Option<[f32; 4]> {
    let bytes = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || pixels.len() < bytes || !u.is_finite() || !v.is_finite() {
        return None;
    }
    let texel = |x: i64, y: i64| {
        let (x, y) = (wrap[0].index(x, width), wrap[1].index(y, height));
        let o = (y * width as usize + x) * 4;
        let p = &pixels[o..o + 4];
        [p[0], p[1], p[2], p[3]].map(|c| c as f32 / 255.0)
    };
    let (x, y) = (u * width as f32 - 0.5, v * height as f32 - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
    Some(std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        top + (bottom - top) * fy
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libpikmin-present's sampler before it moved here (its own `fix` per wrap code).
    fn old_pikmin(img: (u32, u32, &[u8]), wrap: [u8; 2], u: f32, v: f32) -> [f32; 4] {
        let (w, h) = (img.0 as i32, img.1 as i32);
        let fix = |i: i32, n: i32, mode: u8| match mode {
            1 => i.clamp(0, n - 1),
            2 => {
                let p = i.rem_euclid(2 * n);
                if p < n { p } else { 2 * n - 1 - p }
            }
            _ => i.rem_euclid(n),
        };
        let texel = |x: i32, y: i32| {
            let (x, y) = (fix(x, w, wrap[0]), fix(y, h, wrap[1]));
            let o = (y * w + x) as usize * 4;
            let p = &img.2[o..o + 4];
            [p[0], p[1], p[2], p[3]].map(|c| c as f32 / 255.0)
        };
        let (x, y) = (u * w as f32 - 0.5, v * h as f32 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }

    #[test]
    fn bilinear_matches_the_old_sampler_bit_for_bit_in_every_wrap_mode() {
        let mut s = 5u32;
        let mut next = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            s >> 8
        };
        let (w, h) = (5u32, 3u32);
        let pixels: Vec<u8> = (0..w * h * 4).map(|_| next() as u8).collect();
        for host in [[0u8, 0], [1, 1], [2, 2], [0, 2], [1, 0]] {
            let wrap = host.map(Wrap::from_host);
            for _ in 0..500 {
                let u = (next() % 6000) as f32 / 1000.0 - 2.5;
                let v = (next() % 6000) as f32 / 1000.0 - 2.5;
                let got = sample_rgba8_bilinear(w, h, &pixels, wrap, u, v).unwrap();
                let want = old_pikmin((w, h, &pixels), host, u, v);
                assert_eq!(got.map(f32::to_bits), want.map(f32::to_bits), "{host:?} {u} {v}");
            }
        }
    }

    #[test]
    fn bilinear_blends_neighbors_across_seams_and_rejects_bad_input() {
        // Two texels, black then white: the middle of the seam in repeat mode is 50% grey.
        let pixels = [0, 0, 0, 255, 255, 255, 255, 255];
        let at = |wrap, u| sample_rgba8_bilinear(2, 1, &pixels, [wrap, Wrap::Clamp], u, 0.5).unwrap();
        assert_eq!(at(Wrap::Repeat, 0.0)[0], 0.5);
        assert_eq!(at(Wrap::Clamp, 0.0)[0], 0.0);
        assert_eq!(at(Wrap::Mirror, 0.0)[0], 0.0);
        assert_eq!(at(Wrap::Mirror, 1.0)[0], 1.0);
        assert_eq!(at(Wrap::Repeat, 1.0)[0], 0.5);
        assert_eq!(at(Wrap::Clamp, 0.5)[0], 0.5);
        assert_eq!(at(Wrap::Clamp, 0.5)[3], 1.0);
        for (w, h, px, u, v) in [
            (0, 1, &[0u8; 4][..], 0., 0.),
            (1, 1, &[0u8; 3][..], 0., 0.),
            (1, 1, &[0u8; 4][..], f32::NAN, 0.),
            (1, 1, &[0u8; 4][..], 0., f32::INFINITY),
        ] {
            assert_eq!(sample_rgba8_bilinear(w, h, px, [Wrap::Clamp; 2], u, v), None);
        }
    }

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
