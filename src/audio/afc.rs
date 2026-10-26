//! AFC ADPCM, the format of JAudio banks and CPU-decoded streams (Pikmin, Sunshine).
//! Melee's DSP-ADPCM has its own header, coefficients and nibble order; it is not this.
//!
//! The 16 predictor pairs are `filter_table` (Pikmin `syncstream.c`, Sunshine
//! `JAIGFrameStream.cpp`), in 1/2048ths. A 4-bit frame is a header byte (scale in the high
//! nibble, predictor in the low) and 8 bytes of 16 signed nibbles. The 2-bit form packs
//! 16 codes into 4 bytes after its header; the DSP scales them like nibbles by shifting two
//! bits further (`JASystem::Driver` wave formats 0-3: 4-bit ADPCM, 2-bit ADPCM, PCM8, PCM16).
//!
//! The DSP saturates each sample to 16 bits. The CPU stream decoder (`Jac_Decode_ADPCM`,
//! `__DecodeADPCM`) does not: it stores the `int` into an `s16`, wrapping, and that wrapped
//! value is its history. [`Overflow`] selects which one a caller means.

pub const COEFS: [[i32; 2]; 16] = [
    [0, 0],
    [2048, 0],
    [0, 2048],
    [1024, 1024],
    [4096, -2048],
    [3584, -1536],
    [3072, -1024],
    [4608, -2560],
    [4200, -2248],
    [4800, -2300],
    [5120, -3072],
    [2048, -2048],
    [1024, -1024],
    [-1024, 1024],
    [-1024, 0],
    [-2048, 0],
];

/// What happens when a decoded sample leaves the 16-bit range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Saturate, as the DSP does for bank waves.
    Clamp,
    /// Truncate to 16 bits, as the CPU stream decoder does.
    Wrap,
}

/// Decoder history: the last two samples, after any clamping or wrapping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct History {
    pub hist1: i16,
    pub hist2: i16,
}

fn predict(code: i32, shift: u32, coef: [i32; 2], h: &mut History, overflow: Overflow) -> i16 {
    let s = (code << shift) + ((coef[0] * h.hist1 as i32 + coef[1] * h.hist2 as i32) >> 11);
    let s = match overflow {
        Overflow::Clamp => s.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
        Overflow::Wrap => s as i16,
    };
    h.hist2 = h.hist1;
    h.hist1 = s;
    s
}

/// Decodes one 9-byte 4-bit frame into 16 samples.
///
/// # Panics
/// Panics if `frame` is shorter than 9 bytes.
pub fn decode_frame4(frame: &[u8], h: &mut History, out: &mut [i16; 16], overflow: Overflow) {
    let shift = (frame[0] >> 4) as u32;
    let coef = COEFS[(frame[0] & 0xF) as usize];
    for (i, o) in out.iter_mut().enumerate() {
        let b = frame[1 + i / 2];
        let nib = if i % 2 == 0 { b >> 4 } else { b & 0xF };
        let code = ((nib as i8) << 4 >> 4) as i32;
        *o = predict(code, shift, coef, h, overflow);
    }
}

/// Decodes one 5-byte 2-bit frame into 16 samples.
///
/// # Panics
/// Panics if `frame` is shorter than 5 bytes.
pub fn decode_frame2(frame: &[u8], h: &mut History, out: &mut [i16; 16], overflow: Overflow) {
    let shift = (frame[0] >> 4) as u32 + 2;
    let coef = COEFS[(frame[0] & 0xF) as usize];
    for (i, o) in out.iter_mut().enumerate() {
        let b = frame[1 + i / 4];
        let bits = (b >> (6 - 2 * (i % 4))) & 3;
        let code = ((bits as i8) << 6 >> 6) as i32;
        *o = predict(code, shift, coef, h, overflow);
    }
}

/// Decodes a bank wave's bytes (wave format 0-3, as the DSP plays them, so saturating) into
/// exactly `samples` samples. A few waves state a sample count slightly past their bytes;
/// the rest is silence. Formats above 3 read as PCM16.
pub fn decode_wave(format: u8, data: &[u8], samples: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(samples);
    let mut h = History::default();
    let mut buf = [0i16; 16];
    match format {
        0 | 1 => {
            let size = if format == 0 { 9 } else { 5 };
            for frame in data.chunks_exact(size) {
                if out.len() >= samples {
                    break;
                }
                if format == 0 {
                    decode_frame4(frame, &mut h, &mut buf, Overflow::Clamp);
                } else {
                    decode_frame2(frame, &mut h, &mut buf, Overflow::Clamp);
                }
                out.extend_from_slice(&buf[..16.min(samples - out.len())]);
            }
        }
        2 => out.extend(data.iter().take(samples).map(|&b| ((b as i8) as i16) << 8)),
        _ => out.extend(
            data.as_chunks::<2>()
                .0
                .iter()
                .take(samples)
                .map(|c| i16::from_be_bytes(*c)),
        ),
    }
    out.resize(samples, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame4(header: u8, first: u8) -> [u8; 9] {
        let mut f = [0u8; 9];
        f[0] = header;
        f[1] = first;
        f
    }

    #[test]
    fn silent_frame_is_silent() {
        let mut h = History::default();
        let mut out = [1i16; 16];
        decode_frame4(&[0; 9], &mut h, &mut out, Overflow::Clamp);
        assert_eq!(out, [0; 16]);
    }

    #[test]
    fn nibbles_are_signed_and_scaled() {
        // Predictor 0, scale 2: nibble 1 -> 4, nibble 0xF (-1) -> -4.
        let mut h = History::default();
        let mut out = [0i16; 16];
        decode_frame4(&frame4(0x20, 0x1F), &mut h, &mut out, Overflow::Clamp);
        assert_eq!(&out[..3], &[4, -4, 0]);
    }

    #[test]
    fn predictor_uses_history() {
        // Predictor 1 = hist1 * 2048 / 2048: a single impulse holds.
        let mut h = History::default();
        let mut out = [0i16; 16];
        decode_frame4(&frame4(0x01, 0x70), &mut h, &mut out, Overflow::Clamp);
        assert!(out.iter().all(|&s| s == 7));
        assert_eq!(h, History { hist1: 7, hist2: 7 });
    }

    #[test]
    fn overflow_clamps_for_the_dsp_and_wraps_for_the_cpu() {
        // Predictor 1, scale 12: nibble 7 = 0x7000, then 0x7000 + 0x7000 = 0xE000.
        let f = frame4(0xC1, 0x77);
        let (mut hc, mut hw) = (History::default(), History::default());
        let (mut c, mut w) = ([0i16; 16], [0i16; 16]);
        decode_frame4(&f, &mut hc, &mut c, Overflow::Clamp);
        decode_frame4(&f, &mut hw, &mut w, Overflow::Wrap);
        assert_eq!((c[0], w[0]), (0x7000, 0x7000));
        assert_eq!(c[1], i16::MAX);
        assert_eq!(w[1], 0xE000u16 as i16);
        // History keeps the stored value, so the two paths diverge from here on.
        assert_eq!(hc.hist1, c[15]);
        assert_eq!(hw.hist1, w[15]);
        assert_ne!(c, w);
    }

    #[test]
    fn two_bit_frames_shift_two_further() {
        // Predictor 0, scale 0 -> shift 2; codes 01, 11 (-1), 10 (-2), 00.
        let mut h = History::default();
        let mut out = [0i16; 16];
        decode_frame2(&[0x00, 0b01_11_10_00, 0, 0, 0], &mut h, &mut out, Overflow::Clamp);
        assert_eq!(&out[..4], &[4, -4, -8, 0]);
    }

    #[test]
    fn waves_fill_exactly_the_requested_samples() {
        // Two silent 4-bit frames, 20 samples requested: the second is cut short.
        assert_eq!(decode_wave(0, &[0; 18], 20), vec![0; 20]);
        // Stated count past the bytes: the rest is silence.
        let w = decode_wave(0, &[0x20, 0x11, 0, 0, 0, 0, 0, 0, 0], 40);
        assert_eq!(w.len(), 40);
        assert_eq!(&w[..2], &[4, 4]);
        assert!(w[16..].iter().all(|&s| s == 0));
        assert_eq!(decode_wave(2, &[0x80, 0x7F, 1], 3), vec![-32768, 0x7F00, 0x0100]);
        assert_eq!(decode_wave(3, &[0x12, 0x34, 0xFF, 0xFE], 3), vec![0x1234, -2, 0]);
    }
}
