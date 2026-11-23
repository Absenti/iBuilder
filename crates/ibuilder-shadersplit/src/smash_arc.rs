//! Super Smash Bros. Ultimate's `data.arc`: `smash_arc.py` (`files`, `read`) and `shader_split.smash_arc_jobs` / `smash_arc_shaders` (nxnative).
//!
//! u64 magic 0xABCDEF9876543210, then u64 offsets of the music section, the file section (+0x10), the shared file section, the file system table (+0x20) and its search table. The table is
//! `{u32 0x10, u32 size, u32 stored size, u32 section size}` and one zstd frame. Inflated: a 0x58-byte header (counts; +0x38 the region count; +0x3c version and the extra counts), the
//! regions, the stream tables, the path hash buckets, then the paths (32 bytes: four Hash40 references), path-to-info indices, directories, directory offsets (0x1c: u64 offset in
//! the file section), child hashes, file infos (16: +8 info-to-data index, +12 flags), info-to-data (12: directory offset index, file data index) and file data (16: offset / 4, stored
//! size, size, flags). Each path is followed to its file info, so only the current copy of a file is read (the update kept the superseded ones in place); a regional file (flag 0x8000)
//! gives every region's data. Every such file is read and scanned like a loose file (`scan_blob`, then the GLSLC fallback).
//!
//! Memory: the archive is 17.6 GB; only the table (one zstd frame, decoded up to the size its header gives) is read whole, at planning time. The jobs are 128 ranges of about
//! equal stored size (the Python's `parts=128`), and a job reads one file at a time.

use crate::bytes::contains;
use crate::ctl::GLSLC_MAGIC;
use crate::error::{Failure, Res, SplitError};
use crate::glslc::glslc_shaders;
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use crate::zstd::{ZSTD_MAGIC, Zstd};
use std::collections::HashSet;

/// `smash_arc.MAGIC`
pub const MAGIC: [u8; 8] = 0xABCD_EF98_7654_3210u64.to_le_bytes();
const PARTS: u64 = 128;

/// One file's data: `(path Hash40, absolute offset, stored size, size, extension Hash40)` of `smash_arc.files`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: u64,
    /// the file section's offset + the directory's + 4 × the data's: past 2^64 in a damaged table (the Python's integers do not wrap), where nothing is read
    pub at: u128,
    pub stored: u32,
    pub size: u32,
    pub ext: u64,
}

/// One job: a range of the archive's files (`(path, rel, entries)`)
#[derive(Clone)]
pub struct ArcJob {
    pub file: FileRef,
    pub entries: Vec<Entry>,
}

impl ArcJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "smash-arc:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(
            self.entries
                .iter()
                .map(|e| format!("{:x}:{:x}:{:x}:{:x}", e.path, e.at, e.stored, e.size)),
        )
        .collect()
    }
}

/// `struct.unpack_from` of a little-endian word of `n` bytes; the Python's struct.error (the whole split ends) names the read
fn word(fs: &[u8], at: u64, n: usize) -> Result<u64, String> {
    let a = usize::try_from(at).unwrap_or(usize::MAX);
    match fs.get(a..a.saturating_add(n)) {
        Some(b) if b.len() == n => {
            let mut v = [0u8; 8];
            v[..n].copy_from_slice(b);
            Ok(u64::from_le_bytes(v))
        }
        _ => Err(format!(
            "struct.error: unpack_from requires a buffer of at least {} bytes for unpacking {n} bytes at offset {at} (actual buffer size is {})",
            at as u128 + n as u128,
            fs.len()
        )),
    }
}

fn u32at(fs: &[u8], at: u64) -> Result<u64, String> {
    word(fs, at, 4)
}

/// `smash_arc.files(path, regions)`: None when the file has not the magic; `Err` is the text the Python ends the split with (its `sys.exit`, or the exception a damaged table raises).
/// `path` names the file as the Python's messages do. The table is decoded up to the size its header gives (one byte more, to tell a longer one), and never past
/// [`crate::inflate::MAX_DECODED`]: a table that is larger is refused.
pub fn files(
    src: &dyn Source,
    zstd: &dyn Zstd,
    path: &str,
    regions: bool,
) -> Result<Option<Vec<Entry>>, String> {
    let io = |e: std::io::Error| format!("OSError: {e}");
    let h = src.read(0, 0x38).map_err(io)?;
    if h.len() < 8 || h[..8] != MAGIC {
        return Ok(None);
    }
    let (file_off, fs_off) = (word(&h, 0x10, 8)?, word(&h, 0x20, 8)?);
    let th = src.read(fs_off, 16).map_err(io)?;
    if th.len() < 16 {
        return Err(format!(
            "struct.error: unpack requires a buffer of 16 bytes (the file system table's header at {fs_off:#x})"
        ));
    }
    let (size, stored) = (u32at(&th, 4)?, u32at(&th, 8)?);
    let raw = src.read(fs_off + 16, stored as usize).map_err(io)?;
    let cap = (size as usize).min(crate::inflate::MAX_DECODED) + 1;
    let fs = frames(zstd, &raw, Some(cap)).map_err(|e| format!("ZstdError: {e}"))?;
    drop(raw);
    if fs.len() > crate::inflate::MAX_DECODED {
        return Err(format!(
            "{path}: the file system table decodes to more than {:#x} bytes, more than this reader decodes",
            crate::inflate::MAX_DECODED
        ));
    }
    if fs.len() as u64 != size {
        return Err(format!(
            "{path}: file system table inflates to {:#x} bytes, its header says {size:#x}",
            fs.len()
        ));
    }
    let c = |i: u64| u32at(&fs, 4 * i);
    let (tsize, n_paths, n_idx, n_dirs, n_off1, n_hfold, n_info, n_sub, n_data, n_off2, n_data2) = (
        c(0)?,
        c(1)?,
        c(2)?,
        c(3)?,
        c(4)?,
        c(5)?,
        c(6)?,
        c(7)?,
        c(8)?,
        c(9)?,
        c(10)?,
    );
    c(11)?;
    let x = |i: u64| u32at(&fs, 0x3c + 4 * i);
    let (version, extra_dirs, extra_files, extra_f2d) = (x(0)?, x(1)?, x(2)?, x(5)?);
    x(6)?;
    let nreg = fs[0x38] as u64;
    let mut o = 0x58 + nreg * 12;
    let (qd, sh, sfi, soe) = (
        u32at(&fs, o)?,
        u32at(&fs, o + 4)?,
        u32at(&fs, o + 8)?,
        u32at(&fs, o + 12)?,
    );
    o += 16 + qd * 12 + sh * 20 + sfi * 4 + soe * 16;
    let (groups, buckets) = (u32at(&fs, o)?, u32at(&fs, o + 4)?);
    o += 8 + buckets * 8 + groups * 8;
    let p = o;
    o += n_paths * 32;
    let idx = o;
    o += n_idx * 8 + n_dirs * (8 + 0x34);
    let dof = o;
    let n_do = n_off1 + n_off2 + extra_dirs;
    o += n_do * 0x1c + n_hfold * 8;
    let fi = o;
    let n_fi = n_info + n_data2 + extra_files;
    o += n_fi * 16;
    let f2d = o;
    let n_f2d = n_sub + n_data2 + extra_f2d;
    o += n_f2d * 12;
    let fd = o;
    let n_fd = n_data + n_data2 + extra_files;
    o += n_fd * 16;
    if o > tsize {
        return Err(format!(
            "{path}: version {version:#x}: the file system table does not have the layout this reader knows (it would end at {o:#x} of {tsize:#x})"
        ));
    }
    let (mut out, mut seen): (Vec<Entry>, HashSet<u128>) = (Vec::new(), HashSet::new());
    for i in 0..n_paths {
        let (cr, w) = (u32at(&fs, p + 32 * i)?, u32at(&fs, p + 32 * i + 4)?);
        let k = w >> 8;
        if k >= n_idx {
            continue;
        }
        let fii = u32at(&fs, idx + 8 * k + 4)?;
        if fii >= n_fi {
            continue;
        }
        let (f2d0, flags) = (
            u32at(&fs, fi + 16 * fii + 8)?,
            u32at(&fs, fi + 16 * fii + 12)?,
        );
        let n = if regions && flags & 0x8000 != 0 {
            nreg
        } else {
            1
        };
        for f2di in f2d0..f2d0 + n {
            if f2di >= n_f2d {
                break;
            }
            let (doi, fdi) = (
                u32at(&fs, f2d + 12 * f2di)?,
                u32at(&fs, f2d + 12 * f2di + 4)?,
            );
            if doi >= n_do || fdi >= n_fd {
                continue;
            }
            let (off_in, csz, usz) = (
                u32at(&fs, fd + 16 * fdi)?,
                u32at(&fs, fd + 16 * fdi + 4)?,
                u32at(&fs, fd + 16 * fdi + 8)?,
            );
            let at =
                file_off as u128 + word(&fs, dof + 0x1c * doi, 8)? as u128 + 4 * off_in as u128;
            if seen.contains(&at) || csz == 0 {
                continue;
            }
            let (ec, ew) = (u32at(&fs, p + 32 * i + 8)?, u32at(&fs, p + 32 * i + 12)?);
            seen.insert(at);
            out.push(Entry {
                path: cr | (w & 0xff) << 32,
                at,
                stored: csz as u32,
                size: usz as u32,
                ext: ec | (ew & 0xff) << 32,
            });
        }
    }
    out.sort_by_key(|e| e.at); // stable, as `sorted`
    Ok(Some(out))
}

/// A planner's answer: its jobs, and the files they take out of the plain scan (`(root, rel)`)
pub type Planned<J> = (Vec<J>, Vec<(usize, String)>);

/// `size >= step` of the Python's planners (an integer against a float), exactly
pub(crate) fn reaches(size: u128, step: f64) -> bool {
    size >= step.ceil() as u128
}

/// `smash_arc_jobs(files)`: every `.arc` file with the magic (MT Framework's ARC archives share the extension, not the magic), its current files in 128 ranges of about equal stored
/// size. → (the jobs, the files taken). `path_of` names a file as the Python's messages do.
pub fn plan(
    files: &[FileRef],
    zstd: &dyn Zstd,
    path_of: &dyn Fn(&FileRef) -> String,
) -> Result<Planned<ArcJob>, SplitError> {
    let (mut jobs, mut taken) = (Vec::new(), Vec::new());
    for f in files {
        if !f.rel.ends_with(".arc") {
            continue;
        }
        let src = f.loc.open().map_err(|e| SplitError::Io(e.to_string()))?;
        if src.read(0, 8).map_err(|e| SplitError::Io(e.to_string()))? != MAGIC {
            continue;
        }
        let ents = match files_of(&src, zstd, &path_of(f))? {
            Some(e) => e,
            None => continue,
        };
        taken.push((f.root, f.rel.clone()));
        let total: u128 = ents.iter().map(|e| e.stored as u128).sum();
        let step = total as f64 / PARTS as f64;
        let (mut cur, mut size) = (Vec::new(), 0u128);
        for e in ents {
            size += e.stored as u128;
            cur.push(e);
            if reaches(size, step) {
                jobs.push(ArcJob {
                    file: f.clone(),
                    entries: std::mem::take(&mut cur),
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(ArcJob {
                file: f.clone(),
                entries: cur,
            });
        }
    }
    Ok((jobs, taken))
}

fn files_of(
    src: &dyn Source,
    zstd: &dyn Zstd,
    path: &str,
) -> Result<Option<Vec<Entry>>, SplitError> {
    // every region's files: some effects exist only in other regions'
    files(src, zstd, path, true).map_err(SplitError::Red)
}

/// `compression.zstd.decompress(data)`: every frame of `data` one after the other (skippable frames give nothing); bytes that are no frame, or a frame cut short, are its ZstdError.
/// `cap`: stop once that many bytes are out (the caller then sees a longer output than it allows)
pub fn frames(zstd: &dyn Zstd, data: &[u8], cap: Option<usize>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut p = 0usize;
    loop {
        let rest = &data[p..];
        if rest.len() >= 8 && rest[1..4] == [0x2a, 0x4d, 0x18] && rest[0] & 0xf0 == 0x50 {
            let n = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
            if rest.len() - 8 < n {
                return Err(
                    "Compressed data ended before the end-of-stream marker was reached".into(),
                );
            }
            p += 8 + n;
        } else if rest.len() >= 4 && rest[..4] == ZSTD_MAGIC {
            let Some(n) = crate::zstd::frame_len(rest, 0).filter(|&n| n <= rest.len()) else {
                return Err(
                    "Compressed data ended before the end-of-stream marker was reached".into(),
                );
            };
            let left = cap.map(|c| c.saturating_sub(out.len()));
            out.extend(zstd.decompress(&rest[..n], None, left)?);
            p += n;
            if cap.is_some_and(|c| out.len() >= c) {
                return Ok(out);
            }
        } else if rest.len() < 4 {
            // nothing at all, or too few bytes to tell a frame: the decoder waits for more
            return Err("Compressed data ended before the end-of-stream marker was reached".into());
        } else {
            return Err("Unknown frame descriptor".into());
        }
        if p >= data.len() {
            return Ok(out);
        }
    }
}

/// `smash_arc.read(m, e)`: the file's bytes, inflated when stored as a zstd frame
pub fn read(cx: &Ctx<'_>, src: &dyn Source, e: &Entry) -> Res<Vec<u8>> {
    let Ok(at) = u64::try_from(e.at) else {
        return Ok(Vec::new()); // past the end of any file: the Python's slice is empty
    };
    let d = src
        .read(at, e.stored as usize)
        .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
    if d.len() >= 4 && d[..4] == ZSTD_MAGIC {
        return frames(cx.zstd, &d, Some(crate::inflate::MAX_DECODED + 1))
            .and_then(|v| {
                if v.len() > crate::inflate::MAX_DECODED {
                    Err(crate::inflate::cap_error(crate::inflate::MAX_DECODED))
                } else {
                    Ok(v)
                }
            })
            .map_err(|x| Failure::Red(format!("ZstdError: {x}")));
    }
    let mut d = d;
    d.truncate(e.size as usize);
    Ok(d)
}

/// `b[0x13:0x0f:-1]` of an SSBH file, else `b[:4]`, as the label names it (`'data'` when not alphanumeric)
fn kind_of(b: &[u8]) -> String {
    let k: Vec<u8> = if b.starts_with(b"HBSS") {
        b[0x10.min(b.len())..0x14.min(b.len())]
            .iter()
            .rev()
            .copied()
            .collect()
    } else {
        b[..4.min(b.len())].to_vec()
    };
    if !k.is_empty() && k.iter().all(|c| c.is_ascii_alphanumeric()) {
        k.iter().map(|&c| c as char).collect()
    } else {
        "data".into()
    }
}

/// `smash_arc_shaders(path, rel, ents, out)`: each file of the range scanned as a file of its own (`scan_blob`; the GLSLC outputs of the SSBH shader databases when it found nothing).
/// Labels: `/<rel>/<kind>#<path Hash40>`
pub fn shaders(cx: &Ctx<'_>, job: &ArcJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    for e in &job.entries {
        if e.size == 0 {
            continue;
        }
        let b = read(cx, &src, e)?;
        let label = format!("/{}/{}#{:010x}", job.file.rel, kind_of(&b), e.path);
        let n = out.len();
        scan_blob(cx, &b, &label, &mut out, 0, None)?;
        if out.len() == n && contains(&b, &GLSLC_MAGIC) {
            out.extend(glslc_shaders(&b, &label, false, &[])?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_named_as_the_python_slices_them() {
        let mut b = b"HBSS".to_vec();
        b.extend_from_slice(&[0; 12]);
        b.extend_from_slice(b"RDHS");
        assert_eq!(kind_of(&b), "SHDR");
        assert_eq!(kind_of(&b[..0x12]), "DR"); // b[0x13:0x0f:-1] of a short file: the bytes there are
        assert_eq!(kind_of(&b[..0x10]), "data");
        assert_eq!(kind_of(b"EFFN...."), "EFFN");
        assert_eq!(kind_of(b"EF"), "EF");
        assert_eq!(kind_of(b""), "data");
        assert_eq!(kind_of(b"EF\x00N"), "data");
    }

    #[test]
    fn the_step_is_compared_exactly() {
        assert!(reaches(3, 2.5) && !reaches(2, 2.5) && reaches(0, 0.0));
        assert!(reaches(1 << 60, (1u128 << 60) as f64));
        assert!(!reaches((1 << 60) - 1, (1u128 << 60) as f64));
    }
}
