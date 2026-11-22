//! The RomFS image the host reads in place of a folder (`dm/tools/romfs_image.py`, hle_fs.c "RomFS image"): one file instead of hundreds
//! of thousands. Layout (little-endian):
//!
//! * header, 32 bytes: magic `DMROMFS1`, u64 index offset, u64 index size, u64 entry count
//! * data: each file's bytes, one after another, in entry order
//! * index: entry-count entries of 24 bytes (u64 a, u64 b, u32 name offset, u16 name length, u8 type 0 dir / 1 file, u8 flags), then the
//!   names, one pool. A directory: a = its first child's entry, b = its child count; a file: a = data offset in the image, b = size.
//!   Entry 0 is the root (empty name); each directory's children are consecutive entries sorted by name (bytes); directories are
//!   numbered breadth-first.
//!
//! The index comes last, so a copy cut off partway has no valid index (the header points past the end of the file) and the host refuses it.

use crate::error::{Error, Result, bail};
use crate::nup::Nup;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"DMROMFS1";
pub const HEAD_SIZE: usize = 32;
pub const ENT_SIZE: usize = 24;

/// `[name, container node, is dir, a, b]` of `romfs_image.entries`
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: Vec<u8>,
    /// index into the container's folders (a directory) or files
    pub node: u32,
    pub is_dir: bool,
    pub a: u64,
    pub b: u64,
}

/// `pack_nup.entries`: the entries of folder `d` of the container, breadth-first (each directory's children land as one run)
pub fn entries(n: &Nup, d: u32) -> Vec<Entry> {
    let mut ents = vec![Entry {
        name: vec![],
        node: d,
        is_dir: true,
        a: 0,
        b: 0,
    }];
    let mut i = 0;
    while i < ents.len() {
        if ents[i].is_dir {
            let kids = n.children(ents[i].node);
            ents[i].a = ents.len() as u64;
            ents[i].b = kids.len() as u64;
            ents.extend(kids.into_iter().map(|c| Entry {
                name: c.name.to_vec(),
                node: c.index as u32,
                is_dir: c.is_dir,
                a: 0,
                b: 0,
            }));
        }
        i += 1;
    }
    ents
}

/// Fills in each file's data offset and size from the container's own sizes (the data starts after the header, in entry order);
/// returns the offset where the data ends (= where the index starts)
pub fn assign_offsets(n: &Nup, ents: &mut [Entry]) -> u64 {
    let mut off = HEAD_SIZE as u64;
    for e in ents.iter_mut() {
        if !e.is_dir {
            e.a = off;
            e.b = n.file_size(e.node as usize);
            off += e.b;
        }
    }
    off
}

/// The header and the index (entries, then the name pool) of an image of `ents` whose files' a and b are filled in, the file data
/// ending at `end`. The writer writes these bytes and a check of an existing image compares them, so the two cannot disagree.
pub fn layout(ents: &[Entry], end: u64) -> Result<([u8; HEAD_SIZE], Vec<u8>)> {
    let mut pool: Vec<u8> = vec![];
    let mut idx: Vec<u8> = Vec::with_capacity(ents.len() * ENT_SIZE);
    for e in ents {
        if e.name.len() > u16::MAX as usize {
            bail!(
                "a name of {} bytes does not fit the image's index",
                e.name.len()
            );
        }
        if pool.len() > u32::MAX as usize {
            bail!("the name pool outgrows the image's index");
        }
        idx.extend_from_slice(&e.a.to_le_bytes());
        idx.extend_from_slice(&e.b.to_le_bytes());
        idx.extend_from_slice(&(pool.len() as u32).to_le_bytes());
        idx.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        idx.push(if e.is_dir { 0 } else { 1 });
        idx.push(0);
        pool.extend_from_slice(&e.name);
    }
    idx.extend_from_slice(&pool);
    let mut head = [0u8; HEAD_SIZE];
    head[..8].copy_from_slice(MAGIC);
    head[8..16].copy_from_slice(&end.to_le_bytes());
    head[16..24].copy_from_slice(&(idx.len() as u64).to_le_bytes());
    head[24..32].copy_from_slice(&(ents.len() as u64).to_le_bytes());
    Ok((head, idx))
}

/// The largest index this reader will hold in memory (a real one is ~100 bytes per file: Ocarina of Time's 372 587 files are about 30 MB). The header's size is the file's word, not a promise.
pub const MAX_INDEX: u64 = 1 << 30;

/// `read_index`: `{guest path: (is dir, offset, size)}` of an image (tests and checks)
///
/// Every number comes from the image: the index must lie inside the file, the entry count must fit in the index, a directory's children are a run of entries inside the table, a name is a
/// range of the pool, and an entry is listed by at most one directory (a directory that lists itself or its ancestor would otherwise never end). All of it is checked before it is used,
/// in `u128` where it is a sum or a product (review finding 4.1: no allocation or slice of a number the header merely claims).
pub fn read_index(img: &Path) -> Result<Vec<(String, bool, u64, u64)>> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::File::open(img)?;
    let len = f.metadata()?.len();
    let mut h = [0u8; HEAD_SIZE];
    f.read_exact_at(&mut h, 0)?;
    if &h[..8] != MAGIC {
        bail!("{}: not a RomFS image", img.display());
    }
    let ioff = u64::from_le_bytes(h[8..16].try_into().unwrap());
    let isz = u64::from_le_bytes(h[16..24].try_into().unwrap());
    let cnt = u64::from_le_bytes(h[24..32].try_into().unwrap());
    if (ioff as u128) < HEAD_SIZE as u128 || ioff as u128 + isz as u128 > len as u128 {
        bail!(
            "{}: the index (at {ioff:#x}, {isz:#x} bytes) is not inside the image's {len:#x} bytes",
            img.display()
        );
    }
    if isz > MAX_INDEX {
        bail!(
            "{}: the index claims {isz:#x} bytes, more than the {MAX_INDEX:#x} this reader accepts",
            img.display()
        );
    }
    if cnt == 0 || cnt as u128 * ENT_SIZE as u128 > isz as u128 {
        bail!(
            "{}: {cnt:#x} entries do not fit in an index of {isz:#x} bytes",
            img.display()
        );
    }
    let mut raw = vec![0u8; isz as usize];
    f.read_exact_at(&mut raw, ioff)?;
    let cnt = cnt as usize;
    let pool = &raw[cnt * ENT_SIZE..];
    let ent = |k: usize| {
        let r = &raw[k * ENT_SIZE..(k + 1) * ENT_SIZE];
        (
            u64::from_le_bytes(r[..8].try_into().unwrap()),
            u64::from_le_bytes(r[8..16].try_into().unwrap()),
            u32::from_le_bytes(r[16..20].try_into().unwrap()) as usize,
            u16::from_le_bytes(r[20..22].try_into().unwrap()) as usize,
            r[22],
        )
    };
    let mut seen = vec![false; cnt];
    seen[0] = true;
    let mut out = vec![];
    let mut todo: Vec<(usize, String)> = vec![(0, String::new())];
    while let Some((k, path)) = todo.pop() {
        let (a, b, _, _, t) = ent(k);
        if t == 0 {
            if !path.is_empty() {
                out.push((path.clone(), true, 0, 0));
            }
            if a as u128 + b as u128 > cnt as u128 {
                bail!(
                    "{}: entry {k} lists {b} children from entry {a}, past the {cnt} entries of the index",
                    img.display()
                );
            }
            for (c, s) in seen[a as usize..(a + b) as usize].iter_mut().enumerate() {
                let c = c + a as usize;
                if std::mem::replace(s, true) {
                    bail!(
                        "{}: entry {c} is listed by more than one directory (or by itself): the index is not a tree",
                        img.display()
                    );
                }
                let (_, _, co, cl, _) = ent(c);
                if co as u64 + cl as u64 > pool.len() as u64 {
                    bail!(
                        "{}: the name of entry {c} (at {co:#x}, {cl} bytes) is outside the name pool",
                        img.display()
                    );
                }
                let nm = String::from_utf8_lossy(&pool[co..co + cl]).into_owned();
                todo.push((
                    c,
                    if path.is_empty() {
                        nm
                    } else {
                        format!("{path}/{nm}")
                    },
                ));
            }
        } else {
            out.push((path, false, a, b));
        }
    }
    Ok(out)
}

/// A format's first bytes and the mask that is compared (`FORMAT_HEADS`).
pub type HeadMask = ([u8; 4], [u8; 4]);
/// One file that does not start as its name promises: `(guest path, size, first 4 bytes)`.
pub type FormatMismatch = (String, u64, [u8; 4]);

/// `romfs_image.FORMAT_HEADS`: file names that promise a format and the first bytes that format starts with: `(suffix, [(bytes, mask)])`. A zstd stream starts with a frame
/// (`28 b5 2f fd`) or a skippable frame (`5? 2a 4d 18`). A NUP carries no NCA hashes, so this is the check its extraction has for a file that was damaged before it was packed.
pub const FORMAT_HEADS: [(&str, [HeadMask; 2]); 1] = [(
    ".zs",
    [
        ([0x28, 0xb5, 0x2f, 0xfd], [0xff; 4]),
        ([0x50, 0x2a, 0x4d, 0x18], [0xf0, 0xff, 0xff, 0xff]),
    ],
)];

/// `romfs_image.format_mismatches(img)`: `[(guest path, size, first 4 bytes)]` of the image's files whose bytes do not start as their name promises; empty files are left out.
/// Returns how many files were looked at too.
pub fn format_mismatches(img: &Path) -> Result<(usize, Vec<FormatMismatch>)> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::File::open(img)?;
    let mut bad = vec![];
    let mut looked = 0;
    for (path, is_dir, off, size) in read_index(img)? {
        if is_dir || size == 0 {
            continue;
        }
        for (suffix, heads) in FORMAT_HEADS {
            if !path.ends_with(suffix) {
                continue;
            }
            looked += 1;
            let mut b = [0u8; 4];
            let n = (size as usize).min(4);
            f.read_exact_at(&mut b[..n], off)?;
            if !heads
                .iter()
                .any(|(h, m)| (0..4).all(|i| b[i] & m[i] == h[i]))
            {
                bad.push((path.clone(), size, b));
            }
        }
    }
    Ok((looked, bad))
}

/// SHA-256 of the file's bytes `[off, off+size)` of an image, read with positioned reads
pub(crate) fn hash_range(
    f: &std::fs::File,
    off: u64,
    size: u64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Option<[u8; 32]>> {
    use std::os::unix::fs::FileExt;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; size.min(8 << 20) as usize];
    let mut left = size;
    let mut p = off;
    while left > 0 {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let k = left.min(buf.len() as u64) as usize;
        let mut got = 0;
        while got < k {
            let r = f.read_at(&mut buf[got..k], p + got as u64)?;
            if r == 0 {
                return Ok(None); // the image is shorter than its index says
            }
            got += r;
        }
        h.update(&buf[..k]);
        p += k as u64;
        left -= k as u64;
    }
    Ok(Some(h.finalize().into()))
}
