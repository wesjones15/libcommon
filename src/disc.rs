//! GameCube disc images: plain ISO/GCM and CISO, the file system table and the DOL.
//!
//! The disc header holds the 6-character game ID at 0x00, the revision at 0x07, the GameCube
//! magic `C2339F3D` at 0x1C, the DOL offset at 0x420 and the file system table (FST) offset
//! and size at 0x424/0x428. The FST is an array of 12-byte entries followed by a string
//! table: `flags(1) name_offset(3) offset(4) size(4)`. A directory (flags = 1) stores its
//! parent in `offset` and the index one past its last descendant in `size`; a file stores its
//! byte offset and length. Entry 0 is the root and its `size` is the entry count.
//!
//! CISO is a sparse container: 8-byte header (`"CISO"`, little-endian block size), a presence
//! map up to 0x8000, then only the present blocks. Absent blocks read as zeros.
//!
//! Everything here is bounds-checked: a damaged image or FST gives a [`DiscError`], never a
//! panic or an out-of-range read. What a game then does with the files (which ones it needs,
//! which revision it accepts, how it spells paths) stays with the game's library.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

const CISO_HEADER: u64 = 0x8000;

/// A file larger than this is not a game file.
const MAX_FILE: u64 = 1 << 30;

#[derive(Debug)]
pub enum DiscError {
    Io(io::Error),
    /// The image or its tables are damaged or not a GameCube disc.
    Format(&'static str),
    Unsupported(&'static str),
    /// No such file on the disc.
    Missing(String),
}

impl fmt::Display for DiscError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "disc image I/O error: {e}"),
            Self::Format(m) => write!(f, "{m}"),
            Self::Unsupported(m) => write!(f, "{m}"),
            Self::Missing(p) => write!(f, "disc file missing: {p}"),
        }
    }
}

impl std::error::Error for DiscError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for DiscError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Random access to the disc's logical bytes.
pub trait ReadAt: Send + Sync {
    /// Fills `buf` from `offset`, or fails; short reads are errors.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DiscError>;
}

const PAST_END: DiscError = DiscError::Format("read past the end of the disc image");

struct Plain<S> {
    src: Mutex<S>,
    len: u64,
}

impl<S: Read + Seek + Send> ReadAt for Plain<S> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), DiscError> {
        if offset.checked_add(buf.len() as u64).is_none_or(|end| end > self.len) {
            return Err(PAST_END);
        }
        let mut src = self.src.lock().unwrap();
        src.seek(SeekFrom::Start(offset))?;
        src.read_exact(buf)?;
        Ok(())
    }
}

struct Ciso<S> {
    src: Mutex<S>,
    block_size: u64,
    /// For each logical block, its index among the stored blocks, or `None` (zeros).
    slot: Vec<Option<u64>>,
}

impl<S: Read + Seek + Send> ReadAt for Ciso<S> {
    fn read_at(&self, mut offset: u64, mut buf: &mut [u8]) -> Result<(), DiscError> {
        let mut src = self.src.lock().unwrap();
        while !buf.is_empty() {
            let block = usize::try_from(offset / self.block_size).map_err(|_| PAST_END)?;
            let within = offset % self.block_size;
            let n = ((self.block_size - within) as usize).min(buf.len());
            let (head, rest) = buf.split_at_mut(n);
            match self.slot.get(block) {
                Some(Some(s)) => {
                    src.seek(SeekFrom::Start(CISO_HEADER + s * self.block_size + within))?;
                    src.read_exact(head)?;
                }
                Some(None) => head.fill(0),
                None => return Err(PAST_END),
            }
            offset += n as u64;
            buf = rest;
        }
        Ok(())
    }
}

/// Opens an image file as plain ISO/GCM or CISO. RVZ, WIA and WBFS are recognized and refused.
pub fn open_image(path: &Path) -> Result<Box<dyn ReadAt>, DiscError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    image_from(file, len)
}

/// [`open_image`] over any seekable source of `len` bytes.
pub fn image_from<S: Read + Seek + Send + 'static>(
    mut src: S,
    len: u64,
) -> Result<Box<dyn ReadAt>, DiscError> {
    let mut head = [0u8; 8];
    src.read_exact(&mut head)?;
    if &head[..4] == b"CISO" {
        let block_size = u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
        if !(0x800..=64 * 1024 * 1024).contains(&block_size) {
            return Err(DiscError::Format("CISO block size is implausible"));
        }
        let mut map = vec![0u8; (CISO_HEADER - 8) as usize];
        src.read_exact(&mut map)?;
        let mut stored = 0u64;
        let mut slot = Vec::with_capacity(map.len());
        for present in map {
            slot.push(match present {
                0 => None,
                1 => {
                    stored += 1;
                    Some(stored - 1)
                }
                _ => return Err(DiscError::Format("invalid CISO block map")),
            });
        }
        if CISO_HEADER + stored * block_size > len {
            return Err(DiscError::Format("truncated CISO blocks"));
        }
        return Ok(Box::new(Ciso { src: Mutex::new(src), block_size, slot }));
    }
    if matches!(&head[..4], b"RVZ\x01" | b"WIA\x01") || head.starts_with(b"WBFS") {
        return Err(DiscError::Unsupported(
            "compressed Dolphin formats (RVZ/WIA/WBFS) are not supported; convert to ISO or CISO",
        ));
    }
    Ok(Box::new(Plain { src: Mutex::new(src), len }))
}

/// One file in the FST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscFile {
    /// Path as spelled on the disc, `/`-separated, no leading slash (`data/mario.szs`).
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

pub struct Disc {
    reader: Box<dyn ReadAt>,
    /// Game ID from the disc header, e.g. `GPIE01`.
    pub game_id: String,
    /// Disc revision (header byte 7), e.g. 1 for "Rev 1".
    pub version: u8,
    files: Vec<DiscFile>,
    by_path: HashMap<String, usize>,
}

fn be(v: &[u8], at: usize) -> Result<u32, DiscError> {
    v.get(at..at + 4)
        .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
        .ok_or(DiscError::Format("truncated FST"))
}

impl Disc {
    pub fn open(path: &Path) -> Result<Disc, DiscError> {
        Disc::from_reader(open_image(path)?)
    }

    pub fn from_reader(reader: Box<dyn ReadAt>) -> Result<Disc, DiscError> {
        let mut head = [0u8; 0x440];
        reader.read_at(0, &mut head)?;
        if head[0x1C..0x20] != [0xC2, 0x33, 0x9F, 0x3D] {
            return Err(DiscError::Format("not a GameCube disc image (bad magic)"));
        }
        let game_id = String::from_utf8_lossy(&head[0..6]).into_owned();
        let version = head[7];
        let fst_off = be(&head, 0x424)? as u64;
        let fst_size = be(&head, 0x428)? as usize;
        if !(12..=64 * 1024 * 1024).contains(&fst_size) {
            return Err(DiscError::Format("file system table size is implausible"));
        }
        let mut fst = vec![0u8; fst_size];
        reader.read_at(fst_off, &mut fst)?;

        let count = be(&fst, 8)? as usize;
        if count == 0 || count > fst_size / 12 {
            return Err(DiscError::Format("file system table is corrupt"));
        }
        let names = count * 12;
        let mut files = Vec::new();
        let mut by_path = HashMap::new();
        // (index one past the directory's last entry, its path prefix)
        let mut dirs: Vec<(usize, String)> = vec![(count, String::new())];
        for i in 1..count {
            while dirs.len() > 1 && i >= dirs.last().unwrap().0 {
                dirs.pop();
            }
            let at = i * 12;
            let name_at = names + (be(&fst, at)? & 0x00FF_FFFF) as usize;
            let s = fst.get(name_at..).ok_or(DiscError::Format("FST name is out of range"))?;
            let end = s
                .iter()
                .position(|&b| b == 0)
                .ok_or(DiscError::Format("unterminated FST name"))?;
            // Some discs keep Shift-JIS names; lossy decoding keeps them distinct and listable.
            let name = String::from_utf8_lossy(&s[..end]);
            if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
                return Err(DiscError::Format("unsafe FST name"));
            }
            let path = format!("{}{name}", dirs.last().unwrap().1);
            match fst[at] {
                1 => {
                    let end = be(&fst, at + 8)? as usize;
                    if end <= i || end > dirs.last().unwrap().0 {
                        return Err(DiscError::Format("invalid FST directory extent"));
                    }
                    dirs.push((end, format!("{path}/")));
                }
                0 => {
                    let file = DiscFile {
                        path: path.clone(),
                        offset: be(&fst, at + 4)? as u64,
                        size: be(&fst, at + 8)? as u64,
                    };
                    if by_path.insert(path, files.len()).is_some() {
                        return Err(DiscError::Format("duplicate FST name"));
                    }
                    files.push(file);
                }
                _ => return Err(DiscError::Format("invalid FST entry kind")),
            }
        }
        Ok(Disc { reader, game_id, version, files, by_path })
    }

    /// Every file, in FST (disc) order.
    pub fn files(&self) -> &[DiscFile] {
        &self.files
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Looks a file up by its exact path on the disc.
    pub fn find(&self, path: &str) -> Option<&DiscFile> {
        self.by_path.get(path).map(|&i| &self.files[i])
    }

    pub fn contains(&self, path: &str) -> bool {
        self.by_path.contains_key(path)
    }

    /// Every file path starting with `prefix` (`""` for all), sorted.
    pub fn paths_under(&self, prefix: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .files
            .iter()
            .filter(|f| f.path.starts_with(prefix))
            .map(|f| f.path.clone())
            .collect();
        v.sort();
        v
    }

    /// Raw bytes of the disc at `offset`.
    pub fn read_range(&self, offset: u64, len: usize) -> Result<Vec<u8>, DiscError> {
        let mut buf = vec![0u8; len];
        self.reader.read_at(offset, &mut buf)?;
        Ok(buf)
    }

    /// A whole file.
    pub fn read(&self, file: &DiscFile) -> Result<Vec<u8>, DiscError> {
        if file.size > MAX_FILE {
            return Err(DiscError::Format("file size is implausible"));
        }
        self.read_range(file.offset, file.size as usize)
    }

    /// A whole file by its exact path on the disc, e.g. `data/mario.szs`.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, DiscError> {
        let file = self.find(path).ok_or_else(|| DiscError::Missing(path.to_owned()))?;
        self.read(file)
    }

    /// The game executable (`sys/main.dol`): its offset is at 0x420 in the disc header and its
    /// size is the end of its furthest section (DOL header: 7 text + 11 data section offsets
    /// at 0x00, sizes at 0x90).
    pub fn read_dol(&self) -> Result<Vec<u8>, DiscError> {
        let off = be(&self.read_range(0x420, 4)?, 0)? as u64;
        let dol = self.read_range(off, 0x100)?;
        let mut size = 0u32;
        for i in 0..18 {
            size = size.max(be(&dol, i * 4)?.saturating_add(be(&dol, 0x90 + i * 4)?));
        }
        self.read_range(off, (size as usize).clamp(0x100, 64 << 20))
    }
}

/// Synthetic images for testing code that reads discs, without a real disc.
pub mod synthetic {
    use super::*;

    /// Builds a minimal disc image: header (game `id`, revision 2), an FST at 0x1000 and file
    /// data after 0x2000. Directories come from each path's first component (one level).
    pub fn disc_with(id: &[u8; 6], files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut dirs: Vec<&str> = Vec::new();
        for (p, _) in files {
            if let Some((d, _)) = p.split_once('/') {
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
        let mut entries: Vec<(bool, String, u32, u32)> = Vec::new();
        let mut data_at = 0x2000u32;
        let mut blobs: Vec<(u32, &[u8])> = Vec::new();
        for (p, d) in files.iter().filter(|(p, _)| !p.contains('/')) {
            entries.push((false, (*p).to_owned(), data_at, d.len() as u32));
            blobs.push((data_at, d));
            data_at += (d.len() as u32 + 31) & !31;
        }
        for dir in &dirs {
            let in_dir: Vec<_> = files
                .iter()
                .filter(|(p, _)| p.split_once('/').map(|x| x.0) == Some(*dir))
                .collect();
            let index = entries.len() + 1;
            entries.push((true, (*dir).to_owned(), 0, (index + 1 + in_dir.len()) as u32));
            for (p, d) in in_dir {
                entries.push((false, p.split_once('/').unwrap().1.to_owned(), data_at, d.len() as u32));
                blobs.push((data_at, d));
                data_at += (d.len() as u32 + 31) & !31;
            }
        }
        let mut names = Vec::new();
        let mut fst = vec![0u8; 12 * (entries.len() + 1)];
        fst[0] = 1;
        fst[8..12].copy_from_slice(&((entries.len() + 1) as u32).to_be_bytes());
        for (i, (is_dir, name, a, b)) in entries.iter().enumerate() {
            let at = (i + 1) * 12;
            fst[at] = *is_dir as u8;
            fst[at + 1..at + 4].copy_from_slice(&(names.len() as u32).to_be_bytes()[1..]);
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            fst[at + 4..at + 8].copy_from_slice(&a.to_be_bytes());
            fst[at + 8..at + 12].copy_from_slice(&b.to_be_bytes());
        }
        fst.extend_from_slice(&names);

        let mut img = vec![0u8; data_at.max(0x2000) as usize];
        img[..6].copy_from_slice(id);
        img[7] = 2;
        img[0x1C..0x20].copy_from_slice(&[0xC2, 0x33, 0x9F, 0x3D]);
        img[0x424..0x428].copy_from_slice(&0x1000u32.to_be_bytes());
        img[0x428..0x42C].copy_from_slice(&(fst.len() as u32).to_be_bytes());
        img[0x1000..0x1000 + fst.len()].copy_from_slice(&fst);
        for (at, d) in blobs {
            img[at as usize..at as usize + d.len()].copy_from_slice(d);
        }
        img
    }

    /// Wraps `img` as a CISO with `block` bytes per block, storing only blocks with data.
    pub fn ciso(img: &[u8], block: usize) -> Vec<u8> {
        let mut out = vec![0u8; CISO_HEADER as usize];
        out[..4].copy_from_slice(b"CISO");
        out[4..8].copy_from_slice(&(block as u32).to_le_bytes());
        for (i, chunk) in img.chunks(block).enumerate() {
            if chunk.iter().any(|&b| b != 0) {
                out[8 + i] = 1;
                let mut stored = chunk.to_vec();
                stored.resize(block, 0);
                out.extend_from_slice(&stored);
            }
        }
        out
    }

}

#[cfg(test)]
mod tests {
    use super::synthetic::{ciso, disc_with};
    use super::*;
    use std::io::Cursor;

    fn plain(img: Vec<u8>) -> Box<dyn ReadAt> {
        let len = img.len() as u64;
        image_from(Cursor::new(img), len).unwrap()
    }

    fn sample() -> Vec<u8> {
        disc_with(
            b"GMSP01",
            &[("opening.bnr", b"banner"), ("data/mario.szs", b"szs-bytes"), ("data/params.szs", b"p")],
        )
    }

    #[test]
    fn lists_and_reads_files_in_directories() {
        let d = Disc::from_reader(plain(sample())).unwrap();
        assert_eq!((d.game_id.as_str(), d.version, d.len()), ("GMSP01", 2, 3));
        assert_eq!(d.read_file("data/mario.szs").unwrap(), b"szs-bytes");
        assert_eq!(d.read_file("data/params.szs").unwrap(), b"p");
        assert_eq!(d.read_file("opening.bnr").unwrap(), b"banner");
        assert!(matches!(d.read_file("data/nope"), Err(DiscError::Missing(p)) if p == "data/nope"));
        assert_eq!(d.paths_under("data/"), ["data/mario.szs", "data/params.szs"]);
        assert_eq!(d.paths_under("").len(), 3);
        assert!(d.contains("opening.bnr") && !d.contains("Opening.bnr"));
        assert_eq!(d.find("opening.bnr").unwrap().size, 6);
        assert_eq!(d.files().len(), 3);
    }

    #[test]
    fn ciso_reads_like_the_plain_image_and_zero_fills_absent_blocks() {
        let img = sample();
        let c = ciso(&img, 0x800);
        let len = c.len() as u64;
        let d = Disc::from_reader(image_from(Cursor::new(c), len).unwrap()).unwrap();
        assert_eq!(d.read_file("data/mario.szs").unwrap(), b"szs-bytes");
        assert_eq!(d.read_file("opening.bnr").unwrap(), b"banner");
        // A range across stored and absent blocks matches the original image.
        assert_eq!(d.read_range(0x7F0, img.len() - 0x7F0).unwrap(), img[0x7F0..]);
        // Past the block map's coverage is an error, not zeros.
        assert!(d.read_range(0x8000 * 0x800, 4).is_err());
    }

    #[test]
    fn rejects_non_discs_and_damaged_images() {
        assert!(Disc::from_reader(plain(vec![0; 0x1000])).is_err());
        assert!(Disc::from_reader(plain(vec![0; 0x100])).is_err());
        let mut bad_size = sample();
        bad_size[0x428..0x42C].copy_from_slice(&4u32.to_be_bytes());
        assert!(Disc::from_reader(plain(bad_size)).is_err());
        let img = sample();
        let good = ciso(&img, 0x800);
        let len = good.len() as u64;
        assert!(image_from(Cursor::new(good.clone()), len).is_ok());
        // Truncated stored blocks, bad map byte, implausible block sizes.
        assert!(image_from(Cursor::new(good[..0x9000].to_vec()), 0x9000).is_err());
        let mut map = good.clone();
        map[8] = 2;
        assert!(image_from(Cursor::new(map), len).is_err());
        for size in [0u32, 0x7FF, 65 * 1024 * 1024] {
            let mut c = good.clone();
            c[4..8].copy_from_slice(&size.to_le_bytes());
            assert!(image_from(Cursor::new(c), len).is_err(), "block {size}");
        }
        for magic in [&b"RVZ\x01"[..], b"WIA\x01", b"WBFS"] {
            let mut c = vec![0u8; 0x1000];
            c[..4].copy_from_slice(magic);
            assert!(matches!(
                image_from(Cursor::new(c), 0x1000).map(|_| ()),
                Err(DiscError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn rejects_damaged_file_system_tables() {
        // Entries: root, dir "data" (index 1, ends at 3), files at 2.
        let img = disc_with(b"GMSP01", &[("data/a", b"a"), ("b", b"b")]);
        assert!(Disc::from_reader(plain(img.clone())).is_ok());
        let entry = |i: usize| 0x1000 + i * 12;
        // The directory ends before it starts.
        let mut bad = img.clone();
        let dir = entry(2);
        assert_eq!(bad[dir], 1);
        bad[dir + 8..dir + 12].copy_from_slice(&1u32.to_be_bytes());
        assert!(Disc::from_reader(plain(bad)).is_err());
        // The directory claims entries past its parent's end.
        let mut bad = img.clone();
        bad[dir + 8..dir + 12].copy_from_slice(&99u32.to_be_bytes());
        assert!(Disc::from_reader(plain(bad)).is_err());
        // Unknown entry kind.
        let mut bad = img.clone();
        bad[entry(1)] = 7;
        assert!(Disc::from_reader(plain(bad)).is_err());
        // Name offset past the table.
        let mut bad = img.clone();
        bad[entry(1) + 1..entry(1) + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
        assert!(Disc::from_reader(plain(bad)).is_err());
        // Entry count larger than the table.
        let mut bad = img;
        bad[0x1008..0x100C].copy_from_slice(&9999u32.to_be_bytes());
        assert!(Disc::from_reader(plain(bad)).is_err());
    }

    #[test]
    fn rejects_unsafe_and_duplicate_names() {
        for name in ["..", ".", "a:b", "a\\b"] {
            let img = disc_with(b"GMSP01", &[(name, b"x")]);
            assert!(Disc::from_reader(plain(img)).is_err(), "{name}");
        }
        let img = disc_with(b"GMSP01", &[("same", b"1"), ("same", b"2")]);
        assert!(Disc::from_reader(plain(img)).is_err());
        // Names that aren't UTF-8 are listed lossily instead of refused.
        let mut img = disc_with(b"GMSP01", &[("abc", b"x")]);
        let names = 0x1000 + 12 * 2;
        img[names] = 0x83;
        let d = Disc::from_reader(plain(img)).unwrap();
        assert_eq!(d.files()[0].path, "\u{FFFD}bc");
    }

    #[test]
    fn reads_never_leave_the_image() {
        let img = sample();
        let len = img.len();
        let d = Disc::from_reader(plain(img)).unwrap();
        assert!(d.read_range(len as u64 - 2, 4).is_err());
        assert!(d.read_range(u64::MAX, 2).is_err());
        let huge = DiscFile { path: "x".into(), offset: 0, size: 2 << 30 };
        assert!(d.read(&huge).is_err());
    }

    #[test]
    fn dol_size_is_the_furthest_section_end() {
        let mut img = disc_with(b"GMSP01", &[("a", b"x")]);
        img.resize(0x4000, 0);
        let dol = 0x3000usize;
        img[0x420..0x424].copy_from_slice(&(dol as u32).to_be_bytes());
        // Text section 0: offset 0x100, size 0x200; data section 2: offset 0x300, size 0x80.
        img[dol..dol + 4].copy_from_slice(&0x100u32.to_be_bytes());
        img[dol + 0x90..dol + 0x94].copy_from_slice(&0x200u32.to_be_bytes());
        img[dol + 8 * 4..dol + 8 * 4 + 4].copy_from_slice(&0x300u32.to_be_bytes());
        img[dol + 0x90 + 8 * 4..dol + 0x90 + 8 * 4 + 4].copy_from_slice(&0x80u32.to_be_bytes());
        let d = Disc::from_reader(plain(img)).unwrap();
        assert_eq!(d.read_dol().unwrap().len(), 0x380);
    }

    #[test]
    fn opens_files_from_disk() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join(format!("disc-test-{}.iso", std::process::id()));
        std::fs::write(&path, sample()).unwrap();
        let d = Disc::open(&path).unwrap();
        assert_eq!(d.read_file("data/mario.szs").unwrap(), b"szs-bytes");
        drop(d);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(Disc::open(&path), Err(DiscError::Io(_))));
    }
}
