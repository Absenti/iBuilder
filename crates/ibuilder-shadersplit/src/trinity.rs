//! Game Freak's Trinity packs (Pokémon Legends: Z-A): `shader_split.trpfs_jobs` / `trinity_shaders` (nxnative), the FlatBuffers helpers `fb_field`, `fb_vec`, `fb_refs`, `fb_root`.
//!
//! `arc/data.trpfs` holds the packs, `arc/data.trpfd` is their index, both FlatBuffers. The descriptor's root table: 0 file hashes [u64], 1 pack names [string], 2 file infos
//! [{0 pack index u64}] (in file hash order), 3 pack infos [{0 pack size u64, 1 file count u64}]. The file system: `ONEPACK\0`, u64 offset of its root table {0 pack name hashes [u64],
//! 1 pack offsets [u64]}, each pack a table of its own at its offset: 0 file hashes [u64], 1 files [{1 compression i8, 2 level u8, 3 length u64, 4 data [u8]}]. A pack is named through
//! its first file's hash. Each file is scanned like a loose one (`scan_blob`); labels `/<rel>#<pack name>#<file hash>`.
//!
//! Compression 3 is Oodle (Mermaid), which the Python decodes with the game's own decoder in the Mac host (`DM_OODLE_HOST`). There is none here: such a file ends the split as an
//! unsupported container (this port cannot read the game; leaving the pack out would give it fewer shaders than it has). A stored file (any other compression whose data is its
//! length) is read; anything else is the Python's red line for its range.
//!
//! The reads follow the Python's `struct.unpack_from` (a negative offset counts from the end of the file), so a damaged table reads, or fails, where the Python's does. Memory:
//! data.trpfs is read in small windows and one file's data at a time, never whole; the descriptor (data.trpfd) is read whole, as the Python does.

use crate::error::{Failure, Res, SplitError};
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Shader};
use crate::source::{MemSource, Source};
use crate::{red, unsupported};
use std::cell::RefCell;
use std::collections::HashMap;

/// `ONEPACK`
pub const MAGIC: &[u8; 8] = b"ONEPACK\0";
const PARTS: u64 = 64;
/// the most entries a vector may have here (a damaged count is refused before anything that size is read)
const MAX_VEC: u64 = 1 << 24;
const WIN: usize = 1 << 16;

/// One job: a range of the packs of one data.trpfs (`(path, rel, ((pack offset, pack name), ...))`)
#[derive(Clone)]
pub struct TrinityJob {
    pub file: FileRef,
    pub packs: Vec<(u64, String)>,
}

impl TrinityJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "trinity:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(self.packs.iter().map(|(o, n)| format!("{o:x}:{n}")))
        .collect()
    }
}

/// A file read as the Python's `struct.unpack_from` reads a buffer, through a small window
pub struct View<'a> {
    src: &'a dyn Source,
    len: i128,
    win: RefCell<(u64, Vec<u8>)>,
}

impl<'a> View<'a> {
    pub fn new(src: &'a dyn Source) -> View<'a> {
        View {
            src,
            len: src.len() as i128,
            win: RefCell::new((0, Vec::new())),
        }
    }

    /// `n` bytes at `off` (negative: from the end), or the Python's struct.error
    fn bytes(&self, off: i128, n: usize) -> Result<Vec<u8>, String> {
        let o = if off < 0 { off + self.len } else { off };
        if o < 0 {
            return Err(format!(
                "struct.error: offset {off} out of range for {}-byte buffer",
                self.len
            ));
        }
        if o + n as i128 > self.len {
            return Err(if off < 0 {
                format!("struct.error: not enough data to unpack {n} bytes at offset {off}")
            } else {
                format!(
                    "struct.error: unpack_from requires a buffer of at least {} bytes for unpacking {n} bytes at offset {off} (actual buffer size is {})",
                    o + n as i128,
                    self.len
                )
            });
        }
        let o = o as u64;
        // a window starts at most WIN / 2 before what it serves
        if n > WIN / 2 {
            let b = self.src.read(o, n).map_err(|e| format!("OSError: {e}"))?;
            if b.len() < n {
                return Err(format!("OSError: short read at {o:#x}"));
            }
            return Ok(b);
        }
        let mut w = self.win.borrow_mut();
        if o < w.0 || o + n as u64 > w.0 + w.1.len() as u64 {
            let start = o & !(WIN as u64 / 2 - 1);
            w.1 = self
                .src
                .read(start, WIN)
                .map_err(|e| format!("OSError: {e}"))?;
            w.0 = start;
            if o + n as u64 > start + w.1.len() as u64 {
                return Err(format!("OSError: short read at {o:#x}"));
            }
        }
        let a = (o - w.0) as usize;
        Ok(w.1[a..a + n].to_vec())
    }

    fn word<const N: usize>(&self, off: i128) -> Result<[u8; N], String> {
        Ok(self.bytes(off, N)?.try_into().unwrap())
    }
    fn u16(&self, off: i128) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.word(off)?))
    }
    fn i32(&self, off: i128) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.word(off)?))
    }
    fn u32(&self, off: i128) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.word(off)?))
    }
    fn u64(&self, off: i128) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.word(off)?))
    }
    fn i8(&self, off: i128) -> Result<i8, String> {
        Ok(i8::from_le_bytes(self.word(off)?))
    }

    /// `d[st:st + n]` (st >= 0): what there is
    fn slice(&self, st: i128, n: u64) -> Result<Vec<u8>, String> {
        if st >= self.len {
            return Ok(Vec::new());
        }
        let n = (n as i128).min(self.len - st) as usize;
        self.src
            .read(st as u64, n)
            .map_err(|e| format!("OSError: {e}"))
    }

    /// `fb_field(d, t, i)`: the absolute offset of field i of the table at t, None when the field is absent
    pub fn field(&self, t: i128, i: usize) -> Result<Option<i128>, String> {
        let vt = t - self.i32(t)? as i128;
        if 4 + 2 * i as i128 >= self.u16(vt)? as i128 {
            return Ok(None);
        }
        let off = self.u16(vt + 4 + 2 * i as i128)?;
        Ok((off != 0).then_some(t + off as i128))
    }

    /// `fb_vec(d, t, i)`: (start, count) of the vector in field i
    pub fn vec(&self, t: i128, i: usize) -> Result<(i128, u64), String> {
        let Some(p) = self.field(t, i)? else {
            return Ok((0, 0));
        };
        let p = p + self.u32(p)? as i128;
        Ok((p + 4, self.u32(p)? as u64))
    }

    /// `struct.unpack_from(f'<{n}Q', d, st)`
    fn u64s(&self, st: i128, n: u64) -> Result<Vec<u64>, String> {
        too_long(n)?;
        let b = self.bytes(st, 8 * n as usize)?;
        Ok(b.chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect())
    }

    /// `fb_refs(d, t, i)`: the tables / strings a vector of offsets points at (all of them, before any is used, as the Python's list)
    pub fn refs(&self, t: i128, i: usize) -> Result<Vec<i128>, String> {
        let (st, n) = self.vec(t, i)?;
        if n == 0 {
            return Ok(Vec::new());
        }
        too_long(n)?;
        let b = self.bytes(st, 4 * n as usize)?;
        Ok(b.chunks(4)
            .enumerate()
            .map(|(k, c)| st + 4 * k as i128 + u32::from_le_bytes(c.try_into().unwrap()) as i128)
            .collect())
    }

    /// `fb_root(d, at)`
    pub fn root(&self, at: i128) -> Result<i128, String> {
        Ok(at + self.u32(at)? as i128)
    }
}

fn too_long(n: u64) -> Result<(), String> {
    if n > MAX_VEC {
        return Err(format!(
            "a FlatBuffers vector of {n} entries, more than this reader reads ({MAX_VEC})"
        ));
    }
    Ok(())
}

/// `size >= step` of the Python's planner, exactly
fn reaches(size: u128, step: f64) -> bool {
    crate::smash_arc::reaches(size, step)
}

/// `trpfs_jobs(files)`: every `.trpfs` with the magic and a `.trpfd` beside it, its packs in 64 ranges of about equal size (the descriptor's pack sizes), in offset order.
/// → (the jobs, the files taken: the pack file and its descriptor). A pack the descriptor does not list ends the split, as a damaged descriptor does (the Python's `red`, or its exception).
pub fn plan(files: &[FileRef]) -> Result<crate::smash_arc::Planned<TrinityJob>, SplitError> {
    let io = |e: std::io::Error| SplitError::Io(e.to_string());
    let (mut jobs, mut taken) = (Vec::new(), Vec::new());
    for f in files {
        if !f.rel.ends_with(".trpfs") {
            continue;
        }
        let drel = format!("{}d", &f.rel[..f.rel.len() - 1]);
        let Some(df) = files.iter().find(|x| x.root == f.root && x.rel == drel) else {
            continue;
        };
        let src = f.loc.open().map_err(io)?;
        if src.read(0, 8).map_err(io)? != MAGIC {
            continue;
        }
        let fd = df.loc.open().and_then(|s| s.read_all()).map_err(io)?;
        let packs = packs_of(&f.rel, &MemSource(&fd), &src).map_err(SplitError::Red)?;
        drop(fd);
        let total: u128 = packs.iter().map(|p| p.2 as u128).sum();
        let step = total as f64 / PARTS as f64;
        let (mut cur, mut size) = (Vec::new(), 0u128);
        for (off, name, s) in packs {
            cur.push((off, name));
            size += s as u128;
            if reaches(size, step) {
                jobs.push(TrinityJob {
                    file: f.clone(),
                    packs: std::mem::take(&mut cur),
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(TrinityJob {
                file: f.clone(),
                packs: cur,
            });
        }
        taken.push((f.root, f.rel.clone()));
        taken.push((df.root, df.rel.clone()));
    }
    Ok((jobs, taken))
}

/// The packs of one data.trpfs: (offset, name, size) sorted as the Python's `sorted(packs)`
pub fn packs_of(
    rel: &str,
    desc: &dyn Source,
    fs: &dyn Source,
) -> Result<Vec<(u64, String, u64)>, String> {
    let d = View::new(desc);
    let fr = d.root(0)?;
    let (hst, hn) = d.vec(fr, 0)?;
    let hashes = d.u64s(hst, hn)?;
    let mut names = Vec::new();
    for s in d.refs(fr, 1)? {
        let n = d.u32(s)?;
        let b = d.slice(s + 4, n as u64)?;
        names.push(String::from_utf8(b).map_err(|e| format!("UnicodeDecodeError: {e}"))?);
    }
    let mut pack_of: HashMap<u64, u64> = HashMap::new();
    let infos = d.refs(fr, 2)?;
    for (h, t) in hashes.iter().zip(infos) {
        let v = match d.field(t, 0)? {
            Some(p) => d.u64(p)?,
            None => 0,
        };
        pack_of.insert(*h, v);
    }
    let mut sizes = Vec::new();
    for t in d.refs(fr, 3)? {
        match d.field(t, 0)? {
            Some(p) => sizes.push(d.u64(p)?),
            None => {
                return Err("TypeError: a pack info without its size (unpack_from of None)".into());
            }
        }
    }
    let v = View::new(fs);
    let sr = v.root(v.u64(8)? as i128)?;
    let (st, n) = v.vec(sr, 1)?;
    let mut packs = Vec::new();
    for off in v.u64s(st, n)? {
        let pr = v.root(off as i128)?;
        let (h0, hn) = v.vec(pr, 0)?;
        let i = if hn == 0 {
            None
        } else {
            pack_of.get(&v.u64(h0)?).copied()
        };
        let Some(i) = i else {
            return Err(format!(
                "{rel}: the pack at {off:#x} has no file the descriptor lists"
            ));
        };
        let (Some(name), Some(size)) = (
            usize::try_from(i).ok().and_then(|i| names.get(i)),
            usize::try_from(i).ok().and_then(|i| sizes.get(i)),
        ) else {
            return Err("IndexError: list index out of range".into());
        };
        packs.push((off, name.clone(), *size));
    }
    packs.sort();
    Ok(packs)
}

/// a read's error as the Python's exception in a job: the range's shaders are left out
fn r<T>(x: Result<T, String>) -> Res<T> {
    x.map_err(Failure::Red)
}

/// `trinity_shaders(path, rel, packs, out)`: each file of each pack of the range scanned like a loose one
pub fn shaders(cx: &Ctx<'_>, job: &TrinityJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let v = View::new(&src);
    let rel = &job.file.rel;
    let mut out = Vec::new();
    for (off, name) in &job.packs {
        let pr = r(v.root(*off as i128))?;
        let hs = r(v.vec(pr, 0))?;
        for (k, t) in r(v.refs(pr, 1))?.into_iter().enumerate() {
            let (c, ln) = (r(v.field(t, 1))?, r(v.field(t, 3))?);
            let comp = match c {
                Some(c) => r(v.i8(c))?,
                None => 0,
            };
            let n = match ln {
                Some(l) => r(v.u64(l))?,
                None => 0,
            };
            let (st, sz) = r(v.vec(t, 4))?;
            let data = if comp == 3 {
                unsupported!(
                    "Oodle Mermaid (Trinity pack /{rel}#{name}: file {k}, {sz} → {n} bytes; the game's own decoder in the Mac host)"
                )
            } else if sz == n {
                r(v.slice(st, sz))?
            } else {
                red!(
                    "/{rel}#{name}: file {k} has compression {comp} ({sz} → {n} bytes), which is not known"
                )
            };
            let h = r(v.u64(hs.0 + 8 * k as i128))?;
            scan_blob(
                cx,
                &data,
                &format!("/{rel}#{name}#{h:016x}"),
                &mut out,
                0,
                None,
            )?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_follow_struct_unpack_from() {
        let d: Vec<u8> = (0u8..16).collect();
        let s = MemSource(&d);
        let v = View::new(&s);
        assert_eq!(v.u32(-4).unwrap(), u32::from_le_bytes([12, 13, 14, 15]));
        assert_eq!(
            v.u32(-20).unwrap_err(),
            "struct.error: offset -20 out of range for 16-byte buffer"
        );
        assert_eq!(
            v.u32(-2).unwrap_err(),
            "struct.error: not enough data to unpack 4 bytes at offset -2"
        );
        assert!(v.u32(13).is_err());
        assert_eq!(v.slice(14, 9).unwrap(), vec![14, 15]);
        assert!(v.slice(99, 9).unwrap().is_empty());
    }
}
