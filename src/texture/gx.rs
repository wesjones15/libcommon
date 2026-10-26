//! GameCube GX texture decoding to row-major RGBA8, top row first.
//!
//! Pixel layouts are the standard GX tiled formats (`GXTexFmt`, `dolphin/gx/GXEnum.h`).
//! Only the first image level is decoded. Games number their own texture formats
//! differently (Pikmin's `TexImgFormat`); map those to [`GxFormat`] at the boundary.
//!
//! Channel expansion replicates the high bits (5-bit `v` becomes `v << 3 | v >> 2`), which
//! is what the GX hardware does and maps full scale to 255 exactly. Intensity formats fill
//! all four channels with I, as GX samples them; Z8 is opaque.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GxFormat {
    I4,
    I8,
    Ia4,
    Ia8,
    Rgb565,
    Rgb5a3,
    Rgba8,
    /// 4-bit palette index.
    C4,
    /// 8-bit palette index.
    C8,
    /// 14-bit palette index in a 16-bit word.
    C14x2,
    /// DXT1-style compression in 8x8 tiles of four 4x4 blocks.
    Cmpr,
    Z8,
}

impl GxFormat {
    /// `GXTexFmt` value (`GX_TF_Z8` is 0x11). Reserved and unsupported values give `None`.
    pub const fn from_gx(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::I4,
            1 => Self::I8,
            2 => Self::Ia4,
            3 => Self::Ia8,
            4 => Self::Rgb565,
            5 => Self::Rgb5a3,
            6 => Self::Rgba8,
            8 => Self::C4,
            9 => Self::C8,
            10 => Self::C14x2,
            14 => Self::Cmpr,
            0x11 => Self::Z8,
            _ => return None,
        })
    }

    pub const fn is_paletted(self) -> bool {
        matches!(self, Self::C4 | Self::C8 | Self::C14x2)
    }

    /// Tile size in pixels.
    pub const fn tile(self) -> (usize, usize) {
        match self {
            Self::I4 | Self::C4 | Self::Cmpr => (8, 8),
            Self::I8 | Self::Ia4 | Self::C8 | Self::Z8 => (8, 4),
            Self::Ia8 | Self::Rgb565 | Self::Rgb5a3 | Self::C14x2 | Self::Rgba8 => (4, 4),
        }
    }

    /// Bytes per tile.
    pub const fn tile_bytes(self) -> usize {
        match self {
            Self::Rgba8 => 64,
            _ => 32,
        }
    }

    /// Bytes the first level needs for `width` x `height`, padded to whole tiles.
    pub fn data_len(self, width: u32, height: u32) -> Option<usize> {
        let (tw, th) = self.tile();
        (width as usize)
            .div_ceil(tw)
            .checked_mul((height as usize).div_ceil(th))?
            .checked_mul(self.tile_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GxError {
    Truncated {
        format: GxFormat,
        width: u32,
        height: u32,
        need: usize,
        have: usize,
    },
    /// The decoded image would not fit in memory addressing.
    TooLarge { width: u32, height: u32 },
}

impl fmt::Display for GxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { format, width, height, need, have } => {
                write!(f, "{format:?} {width}x{height} needs {need} bytes, have {have}")
            }
            Self::TooLarge { width, height } => write!(f, "{width}x{height} texture is too large"),
        }
    }
}

impl std::error::Error for GxError {}

pub fn rgb565(c: u16) -> [u8; 4] {
    let r = ((c >> 11) & 0x1F) as u8;
    let g = ((c >> 5) & 0x3F) as u8;
    let b = (c & 0x1F) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2), 255]
}

/// Top bit set: opaque 5-5-5. Clear: 3-bit alpha and 4-4-4 color.
pub fn rgb5a3(c: u16) -> [u8; 4] {
    if c & 0x8000 != 0 {
        let r = ((c >> 10) & 0x1F) as u8;
        let g = ((c >> 5) & 0x1F) as u8;
        let b = (c & 0x1F) as u8;
        [(r << 3) | (r >> 2), (g << 3) | (g >> 2), (b << 3) | (b >> 2), 255]
    } else {
        let a = ((c >> 12) & 0x7) as u8;
        let r = ((c >> 8) & 0xF) as u8;
        let g = ((c >> 4) & 0xF) as u8;
        let b = (c & 0xF) as u8;
        [r * 0x11, g * 0x11, b * 0x11, (a << 5) | (a << 2) | (a >> 1)]
    }
}

/// IA8 as stored in palettes and textures: alpha in the high byte, intensity in the low.
pub fn ia8(c: u16) -> [u8; 4] {
    let (a, i) = ((c >> 8) as u8, c as u8);
    [i, i, i, a]
}

/// The four colors of a CMPR block. With `c0 <= c1` the fourth is transparent.
fn cmpr_palette(c0: u16, c1: u16) -> [[u8; 4]; 4] {
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u16, wb: u16| -> [u8; 4] {
        std::array::from_fn(|i| {
            if i == 3 {
                255
            } else {
                ((p0[i] as u16 * wa + p1[i] as u16 * wb) / (wa + wb)) as u8
            }
        })
    };
    if c0 > c1 {
        [p0, p1, mix(2, 1), mix(1, 2)]
    } else {
        [p0, p1, mix(1, 1), [0; 4]]
    }
}

/// Decodes the first level into `width * height * 4` bytes.
///
/// `palette` serves C4/C8/C14x2 and holds already-converted colors (see [`rgb565`],
/// [`rgb5a3`], [`ia8`]); an index past its end decodes as transparent black. Texels in the
/// padding of partial edge tiles are dropped.
pub fn decode(
    format: GxFormat,
    width: u32,
    height: u32,
    data: &[u8],
    palette: &[[u8; 4]],
) -> Result<Vec<u8>, GxError> {
    let too_large = || GxError::TooLarge { width, height };
    let (w, h) = (width as usize, height as usize);
    let out_len = w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or_else(too_large)?;
    let need = format.data_len(width, height).ok_or_else(too_large)?;
    if data.len() < need {
        return Err(GxError::Truncated { format, width, height, need, have: data.len() });
    }

    let (tw, th) = format.tile();
    let tile_bytes = format.tile_bytes();
    let tiles_x = w.div_ceil(tw);
    let tiles_y = h.div_ceil(th);
    let mut rgba = vec![0u8; out_len];
    let mut put = |x: usize, y: usize, px: [u8; 4]| {
        if x < w && y < h {
            rgba[(y * w + x) * 4..][..4].copy_from_slice(&px);
        }
    };
    let pal = |i: usize| palette.get(i).copied().unwrap_or([0; 4]);

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let tile = &data[(ty * tiles_x + tx) * tile_bytes..][..tile_bytes];
            let (x0, y0) = (tx * tw, ty * th);
            let be16 = |o: usize| u16::from_be_bytes([tile[o], tile[o + 1]]);
            match format {
                GxFormat::Cmpr => {
                    for sub in 0..4 {
                        let block = &tile[sub * 8..][..8];
                        let colors = cmpr_palette(
                            u16::from_be_bytes([block[0], block[1]]),
                            u16::from_be_bytes([block[2], block[3]]),
                        );
                        let (sx, sy) = (x0 + (sub % 2) * 4, y0 + (sub / 2) * 4);
                        for row in 0..4 {
                            let bits = block[4 + row];
                            for col in 0..4 {
                                let idx = (bits >> (6 - col * 2)) & 3;
                                put(sx + col, sy + row, colors[idx as usize]);
                            }
                        }
                    }
                }
                GxFormat::Rgba8 => {
                    // AR pairs for the 16 pixels, then GB pairs.
                    for i in 0..16 {
                        let (a, r) = (tile[i * 2], tile[i * 2 + 1]);
                        let (g, b) = (tile[32 + i * 2], tile[32 + i * 2 + 1]);
                        put(x0 + i % 4, y0 + i / 4, [r, g, b, a]);
                    }
                }
                _ => {
                    let nibble = |i: usize| {
                        let byte = tile[i / 2];
                        if i % 2 == 0 { byte >> 4 } else { byte & 0xF }
                    };
                    for i in 0..tw * th {
                        let px = match format {
                            GxFormat::I4 => {
                                let v = nibble(i) * 0x11;
                                [v; 4]
                            }
                            GxFormat::I8 => [tile[i]; 4],
                            GxFormat::Z8 => [tile[i], tile[i], tile[i], 255],
                            GxFormat::Ia4 => {
                                let (a, v) = ((tile[i] >> 4) * 0x11, (tile[i] & 0xF) * 0x11);
                                [v, v, v, a]
                            }
                            GxFormat::Ia8 => ia8(be16(i * 2)),
                            GxFormat::Rgb565 => rgb565(be16(i * 2)),
                            GxFormat::Rgb5a3 => rgb5a3(be16(i * 2)),
                            GxFormat::C4 => pal(nibble(i) as usize),
                            GxFormat::C8 => pal(tile[i] as usize),
                            GxFormat::C14x2 => pal((be16(i * 2) & 0x3FFF) as usize),
                            GxFormat::Cmpr | GxFormat::Rgba8 => unreachable!(),
                        };
                        put(x0 + i % tw, y0 + i / tw, px);
                    }
                }
            }
        }
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_expansion_reaches_full_scale() {
        assert_eq!(rgb565(0xFFFF), [255; 4]);
        assert_eq!(rgb565(0x0000), [0, 0, 0, 255]);
        assert_eq!(rgb5a3(0xFFFF), [255; 4]);
        assert_eq!(rgb5a3(0x7F00), [255, 0, 0, 255]);
        assert_eq!(rgb5a3(0x0000), [0; 4]);
        assert_eq!(rgb5a3(0x0F00), [255, 0, 0, 0]);
        assert_eq!(rgb5a3(0x7000)[3], 255);
        // High-bit replication, not scaling: 5-bit 13 is 107.
        assert_eq!(rgb565(13 << 11)[0], 107);
        assert_eq!(ia8(0x80FF), [255, 255, 255, 0x80]);
    }

    #[test]
    fn format_codes_and_tile_geometry() {
        for (code, tile, bytes) in [
            (0, (8, 8), 32),
            (1, (8, 4), 32),
            (2, (8, 4), 32),
            (3, (4, 4), 32),
            (4, (4, 4), 32),
            (5, (4, 4), 32),
            (6, (4, 4), 64),
            (8, (8, 8), 32),
            (9, (8, 4), 32),
            (10, (4, 4), 32),
            (14, (8, 8), 32),
            (0x11, (8, 4), 32),
        ] {
            let f = GxFormat::from_gx(code).unwrap();
            assert_eq!((f.tile(), f.tile_bytes()), (tile, bytes), "code {code}");
        }
        for code in [7, 11, 12, 13, 15, 0x10, 0x12] {
            assert_eq!(GxFormat::from_gx(code), None);
        }
        assert_eq!(GxFormat::I8.data_len(16, 4), Some(64));
        assert_eq!(GxFormat::Cmpr.data_len(9, 9), Some(4 * 32));
    }

    #[test]
    fn i8_tiles_are_8x4_and_edges_are_cropped() {
        let mut data = vec![0u8; 64];
        data[32] = 200;
        let img = decode(GxFormat::I8, 16, 4, &data, &[]).unwrap();
        assert_eq!(&img[8 * 4..8 * 4 + 4], &[200; 4]);
        // 3x2 of a 8x4 tile: only the visible corner is kept.
        let data: Vec<u8> = (0..32).collect();
        let img = decode(GxFormat::I8, 3, 2, &data, &[]).unwrap();
        assert_eq!(img.len(), 3 * 2 * 4);
        assert_eq!(img[(1 * 3 + 2) * 4], 10);
    }

    #[test]
    fn intensity_alpha_and_z8() {
        assert_eq!(decode(GxFormat::I4, 1, 1, &[0x88; 32], &[]).unwrap(), [136; 4]);
        assert_eq!(decode(GxFormat::I8, 1, 1, &[0x20; 32], &[]).unwrap(), [32; 4]);
        assert_eq!(decode(GxFormat::Z8, 1, 1, &[0x20; 32], &[]).unwrap(), [32, 32, 32, 255]);
        let mut ia4 = [0u8; 32];
        ia4[0] = 0x8F;
        assert_eq!(decode(GxFormat::Ia4, 1, 1, &ia4, &[]).unwrap(), [255, 255, 255, 136]);
    }

    #[test]
    fn rgba8_reorders_ar_gb_pairs() {
        let mut t = [0u8; 64];
        t[0..2].copy_from_slice(&[0xA0, 0x10]); // pixel 0: A, R
        t[32..34].copy_from_slice(&[0x20, 0x30]); // pixel 0: G, B
        let img = decode(GxFormat::Rgba8, 4, 4, &t, &[]).unwrap();
        assert_eq!(&img[..4], &[0x10, 0x20, 0x30, 0xA0]);
    }

    #[test]
    fn paletted_formats_use_the_palette_and_clear_out_of_range() {
        let pal = [[0; 4], [9, 8, 7, 6]];
        let c8 = decode(GxFormat::C8, 8, 4, &[1u8; 32], &pal).unwrap();
        assert_eq!(&c8[..4], &[9, 8, 7, 6]);
        let c8 = decode(GxFormat::C8, 8, 4, &[200u8; 32], &pal).unwrap();
        assert_eq!(&c8[..4], &[0; 4]);
        let c4 = decode(GxFormat::C4, 8, 8, &[0x11u8; 32], &pal).unwrap();
        assert_eq!(&c4[..4], &[9, 8, 7, 6]);
        // C14x2 masks the top two bits of the word.
        let mut t = [0u8; 32];
        t[0..2].copy_from_slice(&[0xC0, 0x01]);
        let c14 = decode(GxFormat::C14x2, 4, 4, &t, &pal).unwrap();
        assert_eq!(&c14[..4], &[9, 8, 7, 6]);
    }

    #[test]
    fn cmpr_blends_and_makes_transparent_black() {
        let mut t = [0u8; 32];
        // c0 = white, c1 = black: opaque, thirds; indices 0,1,2,3 across the first row.
        t[0..4].copy_from_slice(&[0xFF, 0xFF, 0x00, 0x00]);
        t[4] = 0b00_01_10_11;
        let img = decode(GxFormat::Cmpr, 8, 8, &t, &[]).unwrap();
        let px = |x: usize| img[x * 4..x * 4 + 4].to_vec();
        assert_eq!(px(0), [255, 255, 255, 255]);
        assert_eq!(px(1), [0, 0, 0, 255]);
        assert_eq!(px(2), [170, 170, 170, 255]);
        assert_eq!(px(3), [85, 85, 85, 255]);
        // c0 <= c1: midpoint, and index 3 is transparent.
        let mut t = [0u8; 32];
        t[2..4].copy_from_slice(&[0xFF, 0xFF]);
        t[4] = 0b10_11_00_00;
        let img = decode(GxFormat::Cmpr, 4, 4, &t, &[]).unwrap();
        assert_eq!(&img[..4], &[127, 127, 127, 255]);
        assert_eq!(&img[4..8], &[0; 4]);
    }

    #[test]
    fn short_data_is_rejected_and_empty_images_decode() {
        assert_eq!(
            decode(GxFormat::Cmpr, 8, 8, &[0; 31], &[]),
            Err(GxError::Truncated {
                format: GxFormat::Cmpr,
                width: 8,
                height: 8,
                need: 32,
                have: 31
            })
        );
        assert_eq!(decode(GxFormat::Rgba8, 0, 7, &[], &[]).unwrap(), Vec::<u8>::new());
    }
}
