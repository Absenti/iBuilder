//! The RomFS file listing `profile_guess.romfs_files` reads: `(path, size, first 32 bytes)` of every file, from
//! * nx_extract's `romfs_manifest.tsv` (`path<TAB>size<TAB>hex of the first 32 bytes`),
//! * a `DMROMFS1` image (`dm/tools/romfs.py`: read in place, validated like the Python), or
//! * a RomFS folder (entries sorted by name; the Python's `os.walk` order is the file system's).
//!
//! `.listing` (a dump tool's file, `romfs_image.ARTIFACTS`) is not a game file and is left out.
use crate::error::{Error, Result, fmt_err};
use crate::pyutil::py_splitlines;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

const MAGIC: &[u8; 8] = b"DMROMFS1";
const MSL_MAGIC: &[u8; 8] = b"DMMSLTR1";
const MSL_ALIGN: u64 = 16384;
const MSL_FOOTER_ALIGN: u64 = 8;
const HEAD: usize = 32;
const ENT: usize = 24;
const FOOTER: usize = 64;
const ARTIFACTS: [&str; 1] = [".listing"];

/// `is_image(path)`: the file starts with `DMROMFS1`.
pub fn is_image(path: &Path) -> bool {
    let mut b = [0u8; 8];
    File::open(path)
        .and_then(|f| f.read_exact_at(&mut b, 0))
        .is_ok()
        && &b == MAGIC
}

/// Call `f(path, size, head)` for every file in listing order.
pub fn romfs_for_each(romfs: &Path, f: &mut dyn FnMut(&str, u64, &[u8])) -> Result<()> {
    if romfs.is_file() && !is_image(romfs) {
        let text = String::from_utf8_lossy(&std::fs::read(romfs)?).into_owned();
        for line in py_splitlines(&text) {
            if line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() != 3 {
                return fmt_err(format!(
                    "{}: a manifest line has 3 tab-separated fields, found {}: {line:?}",
                    romfs.display(),
                    cols.len()
                ));
            }
            let size: u64 = cols[1]
                .trim()
                .parse()
                .map_err(|_| Error::Format(format!("invalid literal for int(): {:?}", cols[1])))?;
            let head = from_hex(cols[2])?;
            f(cols[0], size, &head);
        }
        return Ok(());
    }
    if romfs.is_dir() {
        return walk_dir(romfs, romfs, f);
    }
    let img = Image::open(romfs)?;
    img.walk(0, "", f)
}

/// `bytes.fromhex` (ASCII whitespace between bytes is skipped).
fn from_hex(s: &str) -> Result<Vec<u8>> {
    let d: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !d.len().is_multiple_of(2) {
        return fmt_err("non-hexadecimal number found in fromhex() arg");
    }
    let nib = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
    d.chunks(2)
        .map(|p| match (nib(p[0]), nib(p[1])) {
            (Some(a), Some(b)) => Ok(a << 4 | b),
            _ => fmt_err("non-hexadecimal number found in fromhex() arg"),
        })
        .collect()
}

fn walk_dir(root: &Path, dir: &Path, f: &mut dyn FnMut(&str, u64, &[u8])) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    let mut dirs = Vec::new();
    for e in &entries {
        let p = e.path();
        if std::fs::metadata(&p)?.is_dir() {
            dirs.push(p);
        } else {
            let name = e.file_name().to_string_lossy().into_owned();
            if ARTIFACTS.contains(&name.as_str()) {
                continue;
            }
            let md = std::fs::metadata(&p)?;
            let mut head = Vec::new();
            use std::io::Read;
            File::open(&p)?.take(32).read_to_end(&mut head)?;
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            f(&format!("/{rel}"), md.len(), &head);
        }
    }
    for d in dirs {
        walk_dir(root, &d, f)?;
    }
    Ok(())
}

struct Image {
    file: File,
    index: Vec<u8>,
    count: usize,
    pool: usize,
}

fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Image {
    fn entry(&self, k: usize) -> (u64, u64, usize, usize, u8, u8) {
        let e = &self.index[k * ENT..(k + 1) * ENT];
        (
            le64(e, 0),
            le64(e, 8),
            u32::from_le_bytes(e[16..20].try_into().unwrap()) as usize,
            u16::from_le_bytes(e[20..22].try_into().unwrap()) as usize,
            e[22],
            e[23],
        )
    }
    fn name(&self, k: usize) -> &[u8] {
        let (_, _, no, nl, _, _) = self.entry(k);
        &self.index[self.pool + no..self.pool + no + nl]
    }

    fn open(path: &Path) -> Result<Image> {
        let bad = |what: &str| Error::Format(format!("{what}: {}", path.display()));
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let mut head = [0u8; HEAD];
        if file.read_exact_at(&mut head, 0).is_err() {
            return Err(bad("invalid RomFS image header"));
        }
        let (off, size, count) = (le64(&head, 8), le64(&head, 16), le64(&head, 24));
        if &head[..8] != MAGIC || count == 0 || count > 0xffff_ffff {
            return Err(bad("invalid RomFS image header"));
        }
        // _check_bounds_and_trailer
        if file_size > i64::MAX as u64
            || off < HEAD as u64
            || off > file_size
            || size > file_size - off
        {
            return Err(bad("invalid RomFS image header"));
        }
        let base_end = off + size;
        if file_size != base_end {
            if file_size - base_end < FOOTER as u64 {
                return Err(bad("incomplete or invalid RomFS trailer"));
            }
            let footer_off = file_size - FOOTER as u64;
            let mut raw = [0u8; FOOTER];
            file.read_exact_at(&mut raw, footer_off)
                .map_err(|_| bad("incomplete or invalid RomFS trailer"))?;
            let ver = u32::from_le_bytes(raw[8..12].try_into().unwrap());
            let gfx_api = raw[12];
            let chain_len = u16::from_le_bytes(raw[14..16].try_into().unwrap()) as usize;
            let base_sz = le64(&raw, 16);
            let (msl_off, msl_sz) = (le64(&raw, 24), le64(&raw, 32));
            let chain_raw = &raw[40..64];
            let align_up = |x: u64, a: u64| (x + (a - 1)) & !(a - 1);
            if &raw[..8] != MSL_MAGIC
                || ver != 1
                || !(1..=3).contains(&gfx_api)
                || base_sz != base_end
                || base_end > u64::MAX - (MSL_ALIGN - 1)
                || msl_off != align_up(base_end, MSL_ALIGN)
                || msl_sz < HEAD as u64
                || msl_off > footer_off
                || msl_sz > footer_off - msl_off
            {
                return Err(bad("incomplete or invalid RomFS trailer"));
            }
            let raw_end = msl_off + msl_sz;
            if raw_end > u64::MAX - (MSL_FOOTER_ALIGN - 1)
                || align_up(raw_end, MSL_FOOTER_ALIGN) != footer_off
                || chain_len > 24
                || chain_raw[..chain_len].contains(&0)
                || chain_raw[chain_len..].iter().any(|&b| b != 0)
            {
                return Err(bad("incomplete or invalid RomFS trailer"));
            }
        }
        if count > size / ENT as u64 {
            return Err(bad("invalid RomFS image header"));
        }
        let mut index = vec![0u8; size as usize];
        file.read_exact_at(&mut index, off)
            .map_err(|_| bad("incomplete RomFS image"))?;
        let count = count as usize;
        let img = Image {
            file,
            index,
            count,
            pool: count * ENT,
        };
        let mut next_child = 1u64;
        for k in 0..count {
            let (a, b, no, nl, t, flags) = img.entry(k);
            if (no + nl) as u64 > size - img.pool as u64
                || t > 1
                || flags > t
                || (k == 0 && (t != 0 || nl != 0))
            {
                return Err(bad(&format!("invalid RomFS image entry {k}")));
            }
            let name = img.name(k);
            if k != 0
                && (nl == 0
                    || nl > 0x300
                    || name.contains(&0)
                    || name.contains(&b'/')
                    || name == b"."
                    || name == b"..")
            {
                return Err(bad("invalid RomFS image name"));
            }
            if t != 0 {
                if a < HEAD as u64 || a > off || b > off - a {
                    return Err(bad("invalid RomFS image file range"));
                }
            } else {
                if a != next_child || a <= k as u64 || a > count as u64 || b > count as u64 - a {
                    return Err(bad("invalid RomFS image tree"));
                }
                next_child += b;
            }
        }
        if next_child != count as u64 {
            return Err(bad("unreachable RomFS image entries"));
        }
        for k in 0..count {
            let (a, b, _, _, t, _) = img.entry(k);
            if t == 0 && (a + 1..a + b).any(|j| img.name(j as usize - 1) >= img.name(j as usize)) {
                return Err(bad("unsorted or duplicate RomFS image names"));
            }
        }
        Ok(img)
    }

    /// os.walk order: the files of a directory (index order), then its subdirectories, recursively.
    fn walk(&self, k: usize, prefix: &str, f: &mut dyn FnMut(&str, u64, &[u8])) -> Result<()> {
        let (a, b, ..) = self.entry(k);
        let mut dirs = Vec::new();
        for j in a as usize..(a + b) as usize {
            let (fa, fb, _, _, t, _) = self.entry(j);
            let name = String::from_utf8_lossy(self.name(j)).into_owned();
            if t == 0 {
                dirs.push((j, name));
                continue;
            }
            if ARTIFACTS.contains(&name.as_str()) {
                continue;
            }
            let mut head = vec![0u8; fb.min(32) as usize];
            self.file.read_exact_at(&mut head, fa)?;
            f(&format!("{prefix}/{name}"), fb, &head);
        }
        for (j, name) in dirs {
            self.walk(j, &format!("{prefix}/{name}"), f)?;
        }
        let _ = self.count;
        Ok(())
    }
}
