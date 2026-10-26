//! Bounds-checked big-endian readers for GameCube file formats.
//!
//! [`Be`] reads at absolute offsets (header fields, tables); [`Cursor`] walks forward through
//! a stream. Neither panics on short data or on offsets that overflow `usize`: every read
//! returns [`Truncated`] carrying the offset where the read started. Callers convert that
//! into their own error type with `From<Truncated>`.
//!
//! Format-specific structures (name tables, length-prefixed strings, vector types) belong to
//! the crates that parse those formats.

use std::fmt;

/// A read began at `offset` and ran past the end of the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Truncated {
    pub offset: usize,
}

impl fmt::Display for Truncated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "data ends early (at {:#x})", self.offset)
    }
}

impl std::error::Error for Truncated {}

fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8], Truncated> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(Truncated { offset })
}

/// Lossy UTF-8 up to the first NUL, or to the end of `raw` when there is none.
fn nul_terminated(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// Big-endian reads at absolute offsets.
#[derive(Clone, Copy)]
pub struct Be<'a>(pub &'a [u8]);

impl<'a> Be<'a> {
    pub fn bytes(&self, o: usize, n: usize) -> Result<&'a [u8], Truncated> {
        slice(self.0, o, n)
    }

    pub fn u8(&self, o: usize) -> Result<u8, Truncated> {
        self.0.get(o).copied().ok_or(Truncated { offset: o })
    }

    pub fn u16(&self, o: usize) -> Result<u16, Truncated> {
        Ok(u16::from_be_bytes(self.bytes(o, 2)?.try_into().unwrap()))
    }

    pub fn i16(&self, o: usize) -> Result<i16, Truncated> {
        Ok(self.u16(o)? as i16)
    }

    pub fn u32(&self, o: usize) -> Result<u32, Truncated> {
        Ok(u32::from_be_bytes(self.bytes(o, 4)?.try_into().unwrap()))
    }

    pub fn f32(&self, o: usize) -> Result<f32, Truncated> {
        Ok(f32::from_bits(self.u32(o)?))
    }

    /// A NUL-terminated string (lossy UTF-8). An offset at the very end reads as empty.
    pub fn cstr(&self, o: usize) -> Result<String, Truncated> {
        let rest = self.0.get(o..).ok_or(Truncated { offset: o })?;
        Ok(nul_terminated(rest))
    }
}

/// A forward cursor over big-endian data. A failed read leaves the position unchanged.
#[derive(Clone)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Moves anywhere, including past the end; the next read then fails.
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Truncated> {
        let s = slice(self.data, self.pos, n)?;
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), Truncated> {
        self.take(n).map(|_| ())
    }

    /// Advances to the next multiple of `align` (a no-op when already aligned). Does not
    /// check the end; the next read does.
    ///
    /// # Panics
    /// Panics when `align` is zero.
    pub fn align(&mut self, align: usize) {
        self.pos = self.pos.next_multiple_of(align);
    }

    pub fn u8(&mut self) -> Result<u8, Truncated> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Truncated> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn i16(&mut self) -> Result<i16, Truncated> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32, Truncated> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32, Truncated> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32, Truncated> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// `n` bytes read as a NUL-terminated string (lossy UTF-8); the cursor moves past all `n`.
    pub fn cstr_in(&mut self, n: usize) -> Result<String, Truncated> {
        Ok(nul_terminated(self.take(n)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: [u8; 11] = [0x12, 0x34, 0x56, 0x78, 0xFF, 0xFE, 0x3F, 0x80, 0x00, 0x00, 0xAB];

    #[test]
    fn random_access_reads_are_big_endian_and_report_the_start_offset() {
        let r = Be(&DATA);
        assert_eq!(r.u8(0).unwrap(), 0x12);
        assert_eq!(r.u16(0).unwrap(), 0x1234);
        assert_eq!(r.u32(0).unwrap(), 0x1234_5678);
        assert_eq!(r.i16(4).unwrap(), -2);
        assert_eq!(r.f32(6).unwrap(), 1.0);
        assert_eq!(r.bytes(2, 3).unwrap(), &[0x56, 0x78, 0xFF]);
        assert_eq!(r.u32(7).unwrap(), 0x8000_00AB);
        assert_eq!(r.u8(11), Err(Truncated { offset: 11 }));
        assert_eq!(r.u16(10), Err(Truncated { offset: 10 }));
        assert_eq!(r.u32(8), Err(Truncated { offset: 8 }));
        assert_eq!(r.bytes(11, 0).unwrap(), &[] as &[u8]);
    }

    #[test]
    fn offsets_that_overflow_are_errors_not_panics() {
        let r = Be(&DATA);
        assert_eq!(r.bytes(usize::MAX, 2), Err(Truncated { offset: usize::MAX }));
        assert_eq!(r.bytes(3, usize::MAX), Err(Truncated { offset: 3 }));
        assert_eq!(r.u32(usize::MAX - 1), Err(Truncated { offset: usize::MAX - 1 }));
        let mut c = Cursor::new(&DATA);
        c.seek(usize::MAX);
        assert_eq!(c.u8(), Err(Truncated { offset: usize::MAX }));
        c.seek(2);
        assert_eq!(c.take(usize::MAX), Err(Truncated { offset: 2 }));
    }

    #[test]
    fn cursor_advances_and_failed_reads_keep_position() {
        let mut c = Cursor::new(&DATA);
        assert_eq!(c.u8().unwrap(), 0x12);
        assert_eq!(c.u16().unwrap(), 0x3456);
        c.seek(0);
        assert_eq!(c.u32().unwrap(), 0x1234_5678);
        assert_eq!(c.i16().unwrap(), -2);
        assert_eq!(c.f32().unwrap(), 1.0);
        assert_eq!((c.pos(), c.remaining()), (10, 1));
        assert_eq!(c.u16(), Err(Truncated { offset: 10 }));
        assert_eq!(c.pos(), 10);
        assert_eq!(c.skip(1), Ok(()));
        assert_eq!(c.skip(1), Err(Truncated { offset: 11 }));
        c.seek(100);
        assert_eq!(c.remaining(), 0);
        assert_eq!(c.u8(), Err(Truncated { offset: 100 }));
    }

    #[test]
    fn align_rounds_up_without_moving_aligned_positions() {
        let mut c = Cursor::new(&[0; 16]);
        c.skip(3).unwrap();
        c.align(4);
        assert_eq!(c.pos(), 4);
        c.align(4);
        assert_eq!(c.pos(), 4);
        c.align(32);
        assert_eq!(c.pos(), 32);
        assert_eq!(c.u8(), Err(Truncated { offset: 32 }));
    }

    #[test]
    fn strings_stop_at_nul_and_replace_invalid_utf8() {
        let d = b"ab\0cd\0\xFFx";
        let r = Be(d);
        assert_eq!(r.cstr(0).unwrap(), "ab");
        assert_eq!(r.cstr(3).unwrap(), "cd");
        assert_eq!(r.cstr(6).unwrap(), "\u{FFFD}x");
        assert_eq!(r.cstr(d.len()).unwrap(), "");
        assert_eq!(r.cstr(d.len() + 1), Err(Truncated { offset: d.len() + 1 }));
        let mut c = Cursor::new(d);
        assert_eq!(c.cstr_in(5).unwrap(), "ab");
        assert_eq!(c.pos(), 5);
        assert_eq!(c.cstr_in(10), Err(Truncated { offset: 5 }));
    }
}
