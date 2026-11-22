//! The game's files: a `DMROMFS1` image (`dm/tools/romfs.py`'s `Image`, every check of it) or a folder (`os.walk`).
//!
//! A `DMROMFS1` image: header `<8sQQQ` (magic, index offset, index size, entry count); the index is `count` 24-byte entries `<QQIHBB` (a file: data offset, length; a
//! directory: first child, child count; then the name's offset in the name pool that follows the entries, its length, the type 0 / 1 and a damage flag) in breadth-first
//! order with the children of a directory sorted by name. An image written by the Metal source-mode build carries an `DMMSLTR1` trailer behind the base image, which is checked here as `romfs.py` does.

use crate::source::FileSource;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MAGIC: &[u8; 8] = b"DMROMFS1";
const HEAD: usize = 32;
const ENT: usize = 24;
const FOOTER: usize = 64;
const MSL_MAGIC: &[u8; 8] = b"DMMSLTR1";
const MSL_ALIGN: u64 = 16384;
const MSL_FOOTER_ALIGN: u64 = 8;
const I64_MAX: u64 = (1 << 63) - 1;

/// `romfs.is_image(path)`
pub fn is_image(path: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 8];
    match File::open(path).and_then(|mut f| f.read_exact(&mut b)) {
        Ok(()) => &b == MAGIC,
        Err(_) => false,
    }
}

fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub struct ImageFile {
    pub path: PathBuf,
    file: Arc<File>,
    index: Vec<u8>,
    count: usize,
    pool: usize,
}

impl ImageFile {
    pub fn open(path: &Path) -> Result<ImageFile, String> {
        let bad = || format!("invalid RomFS image header: {}", path.display());
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file_size = file
            .metadata()
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        let mut me = ImageFile {
            path: path.to_path_buf(),
            file: Arc::new(file),
            index: Vec::new(),
            count: 0,
            pool: 0,
        };
        let head = me.read_exact_at(0, HEAD).ok_or_else(bad)?;
        let (off, size, count) = (u64at(&head, 8), u64at(&head, 16), u64at(&head, 24));
        if &head[..8] != MAGIC || !(0 < count && count <= 0xffff_ffff) {
            return Err(bad());
        }
        me.check_bounds_and_trailer(file_size, off, size)?;
        if count > size / ENT as u64 {
            return Err(bad());
        }
        me.index = me
            .read_exact_at(off, size as usize)
            .ok_or_else(|| format!("incomplete RomFS image: {}", path.display()))?;
        me.count = count as usize;
        me.pool = me.count * ENT;
        let size = size as usize;
        let mut next_child = 1usize;
        for k in 0..me.count {
            let (a, b, no, nl, t, flags) = me.entry(k);
            if no as usize + nl as usize > size - me.pool
                || t > 1
                || flags > t
                || (k == 0 && (t != 0 || nl != 0))
            {
                return Err(format!("invalid RomFS image entry {k}: {}", path.display()));
            }
            let name = me.name(k);
            if k != 0
                && (nl == 0
                    || nl > 0x300
                    || name.contains(&0)
                    || name.contains(&b'/')
                    || name == b"."
                    || name == b"..")
            {
                return Err(format!("invalid RomFS image name: {}", path.display()));
            }
            if t != 0 {
                if a < HEAD as u64 || a > off || b > off - a {
                    return Err(format!(
                        "invalid RomFS image file range: {}",
                        path.display()
                    ));
                }
            } else {
                if a != next_child as u64
                    || a <= k as u64
                    || a > me.count as u64
                    || b > me.count as u64 - a
                {
                    return Err(format!("invalid RomFS image tree: {}", path.display()));
                }
                next_child += b as usize;
            }
        }
        if next_child != me.count {
            return Err(format!(
                "unreachable RomFS image entries: {}",
                path.display()
            ));
        }
        for k in 0..me.count {
            let (a, b, _, _, t, _) = me.entry(k);
            if t == 0 && (a + 1..a + b).any(|j| me.name(j as usize - 1) >= me.name(j as usize)) {
                return Err(format!(
                    "unsorted or duplicate RomFS image names: {}",
                    path.display()
                ));
            }
        }
        Ok(me)
    }

    fn read_exact_at(&self, off: u64, n: usize) -> Option<Vec<u8>> {
        let mut v = vec![0u8; n];
        use crate::source::Source;
        let src = FileSource::new(self.file.clone(), 0, u64::MAX / 2);
        let got = src.read_at(off, &mut v).ok()?;
        // read_at may be short: loop
        let mut got = got;
        while got < n {
            let k = src.read_at(off + got as u64, &mut v[got..]).ok()?;
            if k == 0 {
                return None;
            }
            got += k;
        }
        Some(v)
    }

    /// `_check_bounds_and_trailer`
    fn check_bounds_and_trailer(&self, file_size: u64, ioff: u64, isz: u64) -> Result<(), String> {
        let p = self.path.display();
        if file_size > I64_MAX || ioff < HEAD as u64 || ioff > file_size || isz > file_size - ioff {
            return Err(format!("invalid RomFS image header: {p}"));
        }
        let base_end = ioff + isz;
        if file_size == base_end {
            return Ok(());
        }
        let trailer = || format!("incomplete or invalid RomFS trailer: {p}");
        if file_size - base_end < FOOTER as u64 {
            return Err(trailer());
        }
        let footer_off = file_size - FOOTER as u64;
        let raw = self.read_exact_at(footer_off, FOOTER).ok_or_else(trailer)?;
        // <8sIBBHQQQ24s
        let (t_magic, ver, gfx_api, _flags, chain_len) = (
            &raw[..8],
            u32at(&raw, 8),
            raw[12],
            raw[13],
            u16::from_le_bytes([raw[14], raw[15]]),
        );
        let (base_sz, msl_off, msl_sz) = (u64at(&raw, 16), u64at(&raw, 24), u64at(&raw, 32));
        let chain_raw = &raw[40..64];
        if t_magic != MSL_MAGIC
            || ver != 1
            || !(1..=3).contains(&gfx_api)
            || base_sz != base_end
            || base_end > u64::MAX - (MSL_ALIGN - 1)
            || msl_off != ((base_end + (MSL_ALIGN - 1)) & !(MSL_ALIGN - 1))
            || msl_sz < HEAD as u64
            || msl_off > footer_off
            || msl_sz > footer_off - msl_off
        {
            return Err(trailer());
        }
        let raw_end = msl_off + msl_sz;
        let cl = chain_len as usize;
        if raw_end > u64::MAX - (MSL_FOOTER_ALIGN - 1)
            || ((raw_end + (MSL_FOOTER_ALIGN - 1)) & !(MSL_FOOTER_ALIGN - 1)) != footer_off
            || cl > 24
            || chain_raw[..cl].contains(&0)
            || chain_raw[cl..].iter().any(|&b| b != 0)
        {
            return Err(trailer());
        }
        Ok(())
    }

    /// `(a, b, name offset, name length, type, flags)` of entry k
    fn entry(&self, k: usize) -> (u64, u64, u32, u16, u8, u8) {
        let o = k * ENT;
        let x = &self.index;
        (
            u64at(x, o),
            u64at(x, o + 8),
            u32at(x, o + 16),
            u16::from_le_bytes([x[o + 20], x[o + 21]]),
            x[o + 22],
            x[o + 23],
        )
    }

    fn name(&self, k: usize) -> &[u8] {
        let (_, _, no, nl, _, _) = self.entry(k);
        let s = self.pool + no as usize;
        &self.index[s..s + nl as usize]
    }

    /// Every file of the image, `(path below the root, offset in the image file, length)`, sorted by path
    pub fn files(self: &Arc<Self>) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut stack: Vec<(usize, String)> = vec![(0, String::new())];
        while let Some((k, prefix)) = stack.pop() {
            let (a, b, _, _, t, _) = self.entry(k);
            if t != 0 {
                continue;
            }
            for j in a as usize..(a + b) as usize {
                let nm = String::from_utf8_lossy(self.name(j)).into_owned();
                let rel = if prefix.is_empty() {
                    nm
                } else {
                    format!("{prefix}/{nm}")
                };
                let (fa, fb, _, _, ft, _) = self.entry(j);
                if ft != 0 {
                    out.push(Entry {
                        rel,
                        loc: Loc::Image {
                            file: self.file.clone(),
                            off: fa,
                            len: fb,
                        },
                    });
                } else {
                    stack.push((j, rel));
                }
            }
        }
        out.sort_by(|x, y| x.rel.as_bytes().cmp(y.rel.as_bytes()));
        out
    }
}

/// Where a file's bytes are
#[derive(Clone)]
pub enum Loc {
    Image { file: Arc<File>, off: u64, len: u64 },
    Path(PathBuf, u64),
}

impl Loc {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn len(&self) -> u64 {
        match self {
            Loc::Image { len, .. } => *len,
            Loc::Path(_, n) => *n,
        }
    }
    pub fn open(&self) -> io::Result<FileSource> {
        match self {
            Loc::Image { file, off, len } => Ok(FileSource::new(file.clone(), *off, *len)),
            Loc::Path(p, _) => FileSource::open(p),
        }
    }
}

#[derive(Clone)]
pub struct Entry {
    /// the path below the root (`os.path.relpath(path, romfs)`), `/`-separated
    pub rel: String,
    pub loc: Loc,
}

/// `os.walk` over a folder: every file (not through symlinked folders), sorted by path
pub fn walk_dir(root: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for e in std::fs::read_dir(&dir)? {
            let e = e?;
            let nm = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                nm
            } else {
                format!("{prefix}/{nm}")
            };
            let ft = e.file_type()?;
            if ft.is_dir() {
                stack.push((e.path(), rel));
            } else {
                // a symlink to a file counts as a file (os.walk lists it in `files`); one to a folder is a `dirs` entry that is not followed
                let md = std::fs::metadata(e.path());
                match md {
                    Ok(m) if m.is_dir() => {}
                    Ok(m) => out.push(Entry {
                        rel,
                        loc: Loc::Path(e.path(), m.len()),
                    }),
                    Err(_) => out.push(Entry {
                        rel,
                        loc: Loc::Path(e.path(), 0),
                    }),
                }
            }
        }
    }
    out.sort_by(|x, y| x.rel.as_bytes().cmp(y.rel.as_bytes()));
    Ok(out)
}

/// The files of a root given as an image or a folder (`romfs.os.walk`)
pub fn walk_root(root: &Path) -> Result<Vec<Entry>, String> {
    if is_image(root) {
        let img = Arc::new(ImageFile::open(root)?);
        Ok(img.files())
    } else {
        walk_dir(root).map_err(|e| format!("{}: {e}", root.display()))
    }
}
