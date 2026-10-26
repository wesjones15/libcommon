//! GX TEV stage arithmetic for CPU evaluators (hosts whose shaders can't run TEV).
//!
//! Colors are RGBA in 0..1. A stage computes `(d ± lerp(a, b, c) + bias) * scale`, or a
//! compare (`GX_TEV_COMP_*`: `d + (a OP b ? c : 0)`), and writes PREV or a register. Which
//! texture, rasterized color and konst a stage reads, and how a material is turned into
//! stages, stay with the game's material reader.

pub type C4 = [f32; 4];

/// The arithmetic fields of a stage's combiner (`GXSetTevColorOp` / `GXSetTevAlphaOp`):
/// `op` 0 add, 1 subtract, 8..=15 compare; `bias` 0/1/2 = 0/+0.5/-0.5; `scale` 0..3 =
/// x1/x2/x4/x0.5. Inputs a, b, c and d are chosen by the caller (see [`color_in`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Combine {
    pub op: u8,
    pub bias: u8,
    pub scale: u8,
    pub clamp: bool,
}

/// PREV and the three color registers (`GX_TEVPREV`, `GX_TEVREG0..2`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Regs {
    pub prev: C4,
    pub r: [C4; 3],
}

impl Regs {
    /// The register a stage writes: 0 is PREV, 1..=3 are REG0..2 (larger values use REG2).
    pub fn slot(&mut self, out: u8) -> &mut C4 {
        match out {
            0 => &mut self.prev,
            n => &mut self.r[(n as usize - 1).min(2)],
        }
    }
}

/// GX konst selection (`GXTevKColorSel` / `GXTevKAlphaSel`): 0..7 are fractions 8/8..1/8,
/// 12..15 a konst color, then its R, G, B, A replicated.
pub fn konst(sel: u8, k: &[C4; 4]) -> C4 {
    if sel < 8 {
        return [1.0 - sel as f32 / 8.0; 4];
    }
    let c = k[(sel as usize).wrapping_sub(12) % 4];
    match sel {
        12..=15 => c,
        16..=19 => [c[0]; 4],
        20..=23 => [c[1]; 4],
        24..=27 => [c[2]; 4],
        _ => [c[3]; 4],
    }
}

/// `GXTevColorArg`: PREV, REG0, REG1, REG2 (each as RGB then as alpha replicated), texture,
/// raster, 1, 1/2, konst, 0.
pub fn color_in(a: u8, g: &Regs, tex: C4, ras: C4, k: C4) -> [f32; 3] {
    let rgb = |c: C4| [c[0], c[1], c[2]];
    let rep = |v: f32| [v; 3];
    match a {
        0 => rgb(g.prev),
        1 => rep(g.prev[3]),
        2 => rgb(g.r[0]),
        3 => rep(g.r[0][3]),
        4 => rgb(g.r[1]),
        5 => rep(g.r[1][3]),
        6 => rgb(g.r[2]),
        7 => rep(g.r[2][3]),
        8 => rgb(tex),
        9 => rep(tex[3]),
        10 => rgb(ras),
        11 => rep(ras[3]),
        12 => rep(1.0),
        13 => rep(0.5),
        14 => rgb(k),
        _ => rep(0.0),
    }
}

/// `GXTevAlphaArg`: PREV, REG0..2, texture, raster, konst alpha, 0.
pub fn alpha_in(a: u8, g: &Regs, tex: C4, ras: C4, k: C4) -> f32 {
    match a {
        0 => g.prev[3],
        1 => g.r[0][3],
        2 => g.r[1][3],
        3 => g.r[2][3],
        4 => tex[3],
        5 => ras[3],
        6 => k[3],
        _ => 0.0,
    }
}

/// A channel as the 8-bit value the hardware compares.
fn b8(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32
}

fn bias(b: u8) -> f32 {
    match b {
        1 => 0.5,
        2 => -0.5,
        _ => 0.0,
    }
}

fn scale(s: u8) -> f32 {
    match s {
        1 => 2.0,
        2 => 4.0,
        3 => 0.5,
        _ => 1.0,
    }
}

/// `(d ± lerp(a, b, c) + bias) * scale`, clamped to 0..1 or, unclamped, to the hardware's
/// -4..4 range. Inputs a, b and c are clamped to 0..1 first.
fn arithmetic(c: &Combine, a: f32, b: f32, cc: f32, d: f32) -> f32 {
    let l = a + (b - a) * cc;
    let v = (d + if c.op == 1 { -l } else { l } + bias(c.bias)) * scale(c.scale);
    if c.clamp { v.clamp(0.0, 1.0) } else { v.clamp(-4.0, 4.0) }
}

/// One color combiner. Compare ops (`op` 8..=15) test `a OP b` on the 8-bit R channel, GR16,
/// BGR24 or per channel (`GX_TEV_COMP_R8/GR16/BGR24/RGB8_GT/EQ`) and add `c` when true; they
/// always clamp to 0..1. Reserved ops other than 0 and 1 and 8..=15 act as add.
pub fn combine_color(c: &Combine, a: [f32; 3], b: [f32; 3], cc: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    let cl = |v: [f32; 3]| v.map(|x| x.clamp(0.0, 1.0));
    let (a, b, cc) = (cl(a), cl(b), cl(cc));
    if c.op <= 1 {
        return std::array::from_fn(|i| arithmetic(c, a[i], b[i], cc[i], d[i]));
    }
    let gr16 = |v: [f32; 3]| b8(v[1]) << 8 | b8(v[0]);
    let bgr24 = |v: [f32; 3]| b8(v[2]) << 16 | b8(v[1]) << 8 | b8(v[0]);
    let hit = match c.op {
        8 => Some(b8(a[0]) > b8(b[0])),
        9 => Some(b8(a[0]) == b8(b[0])),
        10 => Some(gr16(a) > gr16(b)),
        11 => Some(gr16(a) == gr16(b)),
        12 => Some(bgr24(a) > bgr24(b)),
        13 => Some(bgr24(a) == bgr24(b)),
        _ => None,
    };
    match hit {
        Some(h) => std::array::from_fn(|i| (d[i] + if h { cc[i] } else { 0.0 }).clamp(0.0, 1.0)),
        None => std::array::from_fn(|i| {
            let m = if c.op == 14 { b8(a[i]) > b8(b[i]) } else { b8(a[i]) == b8(b[i]) };
            (d[i] + if m { cc[i] } else { 0.0 }).clamp(0.0, 1.0)
        }),
    }
}

/// One alpha combiner; compare ops test the 8-bit alpha (even `op` greater-than, odd equal).
pub fn combine_alpha(c: &Combine, a: f32, b: f32, cc: f32, d: f32) -> f32 {
    let (a, b, cc) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0), cc.clamp(0.0, 1.0));
    if c.op <= 1 {
        return arithmetic(c, a, b, cc, d);
    }
    let hit = if c.op.is_multiple_of(2) { b8(a) > b8(b) } else { b8(a) == b8(b) };
    (d + if hit { cc } else { 0.0 }).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The old per-crate code, kept as the reference the shared one must match bit for bit.
    mod old {
        use super::*;

        /// libpikmin-present `combine` (add and subtract only).
        pub fn pikmin(c: &Combine, a: f32, b: f32, cc: f32, d: f32) -> f32 {
            let (a, b, cc) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0), cc.clamp(0.0, 1.0));
            let l = a + (b - a) * cc;
            let bias = match c.bias {
                1 => 0.5,
                2 => -0.5,
                _ => 0.0,
            };
            let scale = match c.scale {
                1 => 2.0,
                2 => 4.0,
                3 => 0.5,
                _ => 1.0,
            };
            let v = (d + if c.op == 1 { -l } else { l } + bias) * scale;
            if c.clamp { v.clamp(0.0, 1.0) } else { v.clamp(-4.0, 4.0) }
        }

        /// libpikmin-present `konst`, taking 8-bit konst colors.
        pub fn pikmin_konst(sel: u8, k: &[[u8; 4]; 4]) -> C4 {
            if sel < 8 {
                let v = 1.0 - sel as f32 / 8.0;
                return [v; 4];
            }
            let idx = (sel as usize).wrapping_sub(12) % 4;
            let c = k[idx].map(|v| v as f32 / 255.0);
            match sel {
                12..=15 => c,
                16..=19 => [c[0]; 4],
                20..=23 => [c[1]; 4],
                24..=27 => [c[2]; 4],
                _ => [c[3]; 4],
            }
        }
    }

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f32 / (1u32 << 24) as f32
    }

    #[test]
    fn add_and_subtract_match_the_old_pikmin_combiner_bit_for_bit() {
        let mut s = 11;
        for _ in 0..5000 {
            let c = Combine {
                op: (lcg(&mut s) * 2.0) as u8,
                bias: (lcg(&mut s) * 3.0) as u8,
                scale: (lcg(&mut s) * 4.0) as u8,
                clamp: lcg(&mut s) < 0.5,
            };
            // Out-of-range inputs too: the a, b, c clamp and the unclamped -4..4 range.
            let mut v = || lcg(&mut s) * 3.0 - 1.0;
            let (a, b, cc, d) = (v(), v(), v(), v());
            let want = old::pikmin(&c, a, b, cc, d).to_bits();
            assert_eq!(combine_alpha(&c, a, b, cc, d).to_bits(), want);
            let rgb = combine_color(&c, [a, b, cc], [b, cc, d], [cc, d, a], [d, a, b]);
            let per_channel = [
                old::pikmin(&c, a, b, cc, d),
                old::pikmin(&c, b, cc, d, a),
                old::pikmin(&c, cc, d, a, b),
            ];
            assert_eq!(rgb.map(f32::to_bits), per_channel.map(f32::to_bits));
        }
    }

    #[test]
    fn compare_ops_add_c_when_the_test_passes() {
        let c = |op| Combine { op, ..Default::default() };
        let (lo, hi) = ([0.2, 0.9, 0.1], [0.4, 0.1, 0.1]);
        let add = [0.25; 3];
        let d = [0.5; 3];
        // R8_GT: lo.r < hi.r so a>b is false for (lo, hi), true for (hi, lo).
        assert_eq!(combine_color(&c(8), lo, hi, add, d), [0.5; 3]);
        assert_eq!(combine_color(&c(8), hi, lo, add, d), [0.75; 3]);
        // R8_EQ
        assert_eq!(combine_color(&c(9), lo, lo, add, d), [0.75; 3]);
        // GR16_GT compares G before R: lo.g > hi.g.
        assert_eq!(combine_color(&c(10), lo, hi, add, d), [0.75; 3]);
        // BGR24_EQ and per-channel RGB8_GT / RGB8_EQ.
        assert_eq!(combine_color(&c(13), hi, hi, add, d), [0.75; 3]);
        assert_eq!(combine_color(&c(14), lo, hi, add, d), [0.5, 0.75, 0.5]);
        assert_eq!(combine_color(&c(15), lo, [0.2, 0.0, 0.1], add, d), [0.75, 0.5, 0.75]);
        // Alpha: even op greater-than, odd equal; results clamp.
        assert_eq!(combine_alpha(&c(8), 0.9, 0.1, 0.75, 0.5), 1.0);
        assert_eq!(combine_alpha(&c(9), 0.9, 0.1, 0.75, 0.5), 0.5);
    }

    #[test]
    fn konst_selection_matches_the_old_code() {
        let bytes = [[255u8, 0, 51, 255], [10, 20, 30, 40], [0, 0, 0, 0], [255; 4]];
        let floats = bytes.map(|c| c.map(|v| v as f32 / 255.0));
        for sel in 0..32u8 {
            assert_eq!(
                konst(sel, &floats).map(f32::to_bits),
                old::pikmin_konst(sel, &bytes).map(f32::to_bits)
            );
        }
        assert_eq!(konst(0, &floats), [1.0; 4]);
        assert_eq!(konst(7, &floats), [0.125; 4]);
        assert_eq!(konst(13, &floats), floats[1]);
        assert_eq!(konst(21, &floats), [floats[1][1]; 4]);
    }

    #[test]
    fn inputs_registers_and_slots() {
        let mut g = Regs { prev: [0.1, 0.2, 0.3, 0.4], r: [[0.5; 4], [0.6; 4], [0.7, 0.8, 0.9, 1.0]] };
        let (tex, ras, k) = ([0.0, 0.1, 0.2, 0.3], [0.9; 4], [0.4, 0.5, 0.6, 0.7]);
        assert_eq!(color_in(0, &g, tex, ras, k), [0.1, 0.2, 0.3]);
        assert_eq!(color_in(1, &g, tex, ras, k), [0.4; 3]);
        assert_eq!(color_in(6, &g, tex, ras, k), [0.7, 0.8, 0.9]);
        assert_eq!(color_in(7, &g, tex, ras, k), [1.0; 3]);
        assert_eq!(color_in(9, &g, tex, ras, k), [0.3; 3]);
        assert_eq!(color_in(12, &g, tex, ras, k), [1.0; 3]);
        assert_eq!(color_in(13, &g, tex, ras, k), [0.5; 3]);
        assert_eq!(color_in(14, &g, tex, ras, k), [0.4, 0.5, 0.6]);
        assert_eq!(color_in(15, &g, tex, ras, k), [0.0; 3]);
        assert_eq!(alpha_in(3, &g, tex, ras, k), 1.0);
        assert_eq!(alpha_in(6, &g, tex, ras, k), 0.7);
        assert_eq!(alpha_in(7, &g, tex, ras, k), 0.0);
        g.slot(0)[0] = 9.0;
        g.slot(2)[0] = 8.0;
        g.slot(200)[0] = 7.0;
        assert_eq!((g.prev[0], g.r[1][0], g.r[2][0]), (9.0, 8.0, 7.0));
    }
}
