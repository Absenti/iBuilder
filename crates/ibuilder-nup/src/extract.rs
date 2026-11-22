//! `pack_nup.pack_version`, `pack_dlc`, `exefs`, `control`, `pack` and `gameapp_pipeline.extract_nup`: a NUP's version into the layout an
//! NSP extraction leaves:
//!
//! ```text
//! <extract>/ExeFS/              the version's code modules (main, main.npdm, rtld, sdk, subsdk0, ...; names starting with '.' left out)
//! <extract>/RomFS.img           its RomFS as ONE image (DMROMFS1, `romfs.rs`), written straight from the container
//! <extract>/control.nacp, icon.jpg   the package's NACP and icon when it carries them
//! <extract>/romfs_programs/<index>.img   the RomFS of each other program of the application (a Switch 2 Edition of a Switch 1 game)
//! <aoc>/<index>.img             each DLC's RomFS (index = DLC title ID minus the NACP's AddOnContentBaseId)
//! ```
//!
//! An update's RomFS is packed on its own, never laid over the base (the dumps NUPs are packed from hold the patched RomFS of an
//! update); an update with no `romfs/` uses the base's. Every file read is checked against its SHA-256. A repeat run keeps code
//! modules whose content is the same and an image made from the same container tree, or an image whose bytes check against the
//! container's hashes.

use crate::error::{Error, Result, bail};
use crate::nup::{BASE, NO_DIR, Nup, Package};
use crate::romfs::{self, Entry};
use crate::scan::{NO_BASE, Scan, program_id, scan};
use crate::sys;
use sha2::{Digest, Sha256};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// `romfs_programs`: beside a version's RomFS.img, `<index>.img` for each other program of the application
pub const PROGRAMS: &str = "romfs_programs";
const T_NACP: u16 = 2;
const T_ICON: u16 = 3;
const T_LICONS: u16 = 10;
/// the Python's headroom on top of the RomFS's bytes
pub const SPACE_MARGIN: u64 = 512 << 20;

/// where an operation is, for a progress line
#[derive(Debug, Clone)]
pub struct Progress {
    /// `checking the image`, `unpacking`, `ExeFS`, ...
    pub step: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub done_files: u64,
    pub total_files: u64,
}

pub type ProgressFn = Arc<dyn Fn(&Progress) + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    /// worker threads of the RomFS image writer (0: the cores, at most 8; the output does not depend on the count)
    pub threads: usize,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<ProgressFn>,
    /// the least time between two progress calls (the last of a step always comes); default 250 ms, 0 = after every chunk
    pub progress_ms: u64,
    /// the Python's `say`: one log line
    pub say: Option<SayFn>,
    /// free bytes on the volume of a path (default: `statvfs`); the host may know better (the app's own `free_bytes`)
    pub free_bytes: Option<FreeBytesFn>,
}

/// A log-line callback (`Options::say`).
pub type SayFn = Arc<dyn Fn(&str) + Send + Sync>;
/// A free-space probe (`Options::free_bytes`).
pub type FreeBytesFn = Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

impl Default for Options {
    fn default() -> Self {
        Options {
            threads: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            progress_ms: 250,
            say: None,
            free_bytes: None,
        }
    }
}

impl Options {
    fn say(&self, s: &str) {
        if let Some(f) = &self.say {
            f(s);
        }
    }
    fn pool_threads(&self) -> usize {
        if self.threads > 0 {
            self.threads
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .min(8)
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn free(&self, p: &Path) -> Option<u64> {
        match &self.free_bytes {
            Some(f) => f(p),
            None => sys::free_bytes(p),
        }
    }
}

/// how `pack` got an image
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// the stamp says the container tree and the packer are the same
    Kept,
    /// the stamp is from another packer, but the image checks against the container's hashes: its stamp is renewed
    Verified,
    Packed,
}

impl How {
    pub fn as_str(&self) -> &'static str {
        match self {
            How::Kept => "kept",
            How::Verified => "verified",
            How::Packed => "packed",
        }
    }
}

/// Throttled progress reporting from the worker threads.
struct Meter<'a> {
    step: String,
    opts: &'a Options,
    total_bytes: u64,
    total_files: u64,
    bytes: AtomicU64,
    files: AtomicU64,
    t0: Instant,
    last_ms: AtomicU64,
    lock: Mutex<()>,
}

impl<'a> Meter<'a> {
    fn new(step: &str, opts: &'a Options, total_bytes: u64, total_files: u64) -> Meter<'a> {
        Meter {
            step: step.into(),
            opts,
            total_bytes,
            total_files,
            bytes: AtomicU64::new(0),
            files: AtomicU64::new(0),
            t0: Instant::now(),
            last_ms: AtomicU64::new(0),
            lock: Mutex::new(()),
        }
    }
    fn report(&self, force: bool) {
        let Some(cb) = &self.opts.progress else {
            return;
        };
        let now = self.t0.elapsed().as_millis() as u64;
        if !force {
            let last = self.last_ms.load(Ordering::Relaxed);
            if now < last + self.opts.progress_ms
                || self
                    .last_ms
                    .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_err()
            {
                return;
            }
        } else {
            self.last_ms.store(now, Ordering::Relaxed);
        }
        let _g = self.lock.lock().unwrap();
        cb(&Progress {
            step: self.step.clone(),
            done_bytes: self.bytes.load(Ordering::Relaxed),
            total_bytes: self.total_bytes,
            done_files: self.files.load(Ordering::Relaxed),
            total_files: self.total_files,
        });
    }
    fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
        self.report(false);
    }
    fn file_done(&self) {
        self.files.fetch_add(1, Ordering::Relaxed);
    }
}

// ---- ExeFS and control data -------------------------------------------------------------------------------------------------

fn tmp_beside(dst: &Path, suffix: &str) -> PathBuf {
    let mut n = dst
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    n.push(suffix);
    dst.with_file_name(n)
}

/// `dst` becomes `data` in one step: written beside it as `<dst>.part`, then renamed over it (a reader, or a run killed in the middle, never sees half a file; the ExeFS and `RomFS.img` next to it
/// already come in this way)
fn write_whole(dst: &Path, data: &[u8]) -> std::io::Result<()> {
    let part = tmp_beside(dst, ".part");
    let r = std::fs::write(&part, data).and_then(|()| std::fs::rename(&part, dst));
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

fn sha256_file(p: &Path) -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

/// `exefs(n, k, dst)`: the package's ExeFS into `dst`, kept as it is when every file there has the same content
pub fn exefs(n: &Nup, k: &Package, dst: &Path, opts: &Options) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let mut files: Vec<(Vec<u8>, usize)> = vec![];
    for c in n.children(k.exefs) {
        if c.is_dir {
            continue;
        }
        // NUP names are labels (section 6.1): validated before they are host paths
        if c.name.is_empty()
            || c.name == b"."
            || c.name == b".."
            || c.name.contains(&0)
            || c.name.contains(&b'/')
        {
            bail!(
                "{}: invalid ExeFS name {:?}",
                n.path.display(),
                String::from_utf8_lossy(c.name)
            );
        }
        if !c.name.starts_with(b".") {
            files.push((c.name.to_vec(), c.index));
        }
    }
    let same = || -> bool {
        let Ok(rd) = std::fs::read_dir(dst) else {
            return false;
        };
        let mut have: Vec<Vec<u8>> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().as_bytes().to_vec())
            .collect();
        let mut want: Vec<Vec<u8>> = files.iter().map(|(nm, _)| nm.clone()).collect();
        have.sort();
        want.sort();
        if have != want {
            return false;
        }
        files.iter().all(|(nm, i)| {
            sha256_file(&dst.join(std::ffi::OsStr::from_bytes(nm)))
                .map(|h| h == n.file_sha(*i))
                .unwrap_or(false)
        })
    };
    if same() {
        return Ok(());
    }
    let tmp = tmp_beside(dst, ".partial");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let total: u64 = files.iter().map(|(_, i)| n.file_size(*i)).sum();
    let meter = Meter::new("ExeFS", opts, total, files.len() as u64);
    for (nm, i) in &files {
        let mut f = std::fs::File::create(tmp.join(std::ffi::OsStr::from_bytes(nm)))?;
        use std::io::Write;
        n.copy_with(*i, &mut |c| f.write_all(c), &opts.cancel, &mut |b| {
            meter.add_bytes(b)
        })?;
        meter.file_done();
    }
    meter.report(true);
    replace_dir(&tmp, dst)?;
    Ok(())
}

/// removes `p`, a folder or a file, if it is there
fn remove_any(p: &Path) {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let _ = std::fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// The folder `dst` becomes the folder `tmp`, nothing deleted before the new one stands: a folder cannot be renamed over a non-empty one, so the old
/// one is renamed aside (`<dst>.old`), `tmp` renamed in, and only then the old one removed. If the second rename fails the old one is put back.
/// (Deleting `dst` first lost it, or left half of it, whenever the delete or the rename failed.)
fn replace_dir(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    let old = tmp_beside(dst, ".old");
    remove_any(&old); // what a run killed between the two renames left
    let had = std::fs::symlink_metadata(dst).is_ok();
    if had {
        std::fs::rename(dst, &old)?;
    }
    if let Err(e) = std::fs::rename(tmp, dst) {
        if had {
            let _ = std::fs::rename(&old, dst);
        }
        return Err(e);
    }
    if had {
        remove_any(&old);
    }
    Ok(())
}

/// the localized icons record (TLV 0x000A): `{lowercase language tag: image bytes}`, first of a tag wins, in order
fn localized(payload: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = vec![];
    let mut p = 0usize;
    while p < payload.len() {
        let ln = payload[p] as usize;
        let end = (p + 1 + ln).min(payload.len());
        let tag = String::from_utf8_lossy(&payload[p + 1..end]).to_lowercase();
        p += 1 + ln;
        if p + 4 > payload.len() {
            break;
        }
        let dn = u32::from_le_bytes(payload[p..p + 4].try_into().unwrap()) as usize;
        p += 4;
        let data = payload[p.min(payload.len())..(p + dn).min(payload.len())].to_vec();
        if !out.iter().any(|(t, _)| *t == tag) {
            out.push((tag, data));
        }
        p += dn;
    }
    out
}

/// the record for a language code: the exact tag, then its primary subtag
fn pick<'a>(tags: &'a [(String, Vec<u8>)], code: &str) -> Option<&'a Vec<u8>> {
    let code = code.to_lowercase();
    let get = |c: &str| {
        tags.iter()
            .find(|(t, _)| t == c)
            .map(|(_, d)| d)
            .filter(|d| !d.is_empty())
    };
    get(&code).or_else(|| get(code.split('-').next().unwrap_or("")))
}

/// the record that holds application metadata when no base game is there (SPEC 5.1): a base record with no files, else the first package
pub(crate) fn app_package(n: &Nup) -> &Package {
    n.packages
        .iter()
        .find(|k| k.kind == BASE)
        .unwrap_or(&n.packages[0])
}

/// the TLV record of type `t` of a merged meta list, unless it is empty
fn meta_get(meta: &[(u16, Vec<u8>)], t: u16) -> Option<Vec<u8>> {
    meta.iter()
        .find(|(tt, _)| *tt == t)
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
}

/// The icon `control()` writes to `icon.jpg` for this meta: the icon record (type 3), else the `en-US` one of the localized icons (type 10), else the first localized one;
/// None when the package carries none. One function for the build and for [`crate::application_icon`] (a cover before any build), so the two can never choose differently.
pub(crate) fn choose_icon(meta: &[(u16, Vec<u8>)]) -> Option<Vec<u8>> {
    let icons = localized(&meta_get(meta, T_LICONS).unwrap_or_default());
    meta_get(meta, T_ICON)
        .or_else(|| pick(&icons, "en-US").cloned())
        .or_else(|| {
            icons
                .first()
                .map(|(_, d)| d.clone())
                .filter(|d| !d.is_empty())
        })
}

/// The meta records `control(k, …, fallback)` writes from: `k`'s, over `fallback`'s for what `k` lacks. One function for the build and for [`crate::application_icon`].
pub(crate) fn control_meta(
    n: &Nup,
    k: &Package,
    fallback: Option<&Package>,
) -> Result<Vec<(u16, Vec<u8>)>> {
    let mut meta: Vec<(u16, Vec<u8>)> = match fallback {
        Some(f) => n.meta(f)?,
        None => vec![],
    };
    for (t, v) in n.meta(k)? {
        match meta.iter_mut().find(|(tt, _)| *tt == t) {
            Some(slot) => slot.1 = v,
            None => meta.push((t, v)),
        }
    }
    Ok(meta)
}

/// The version `extract_application` packs when it is given none: the newest update, else 0 (the base).
pub(crate) fn newest_version(sc: &Scan) -> u32 {
    sc.versions.keys().next_back().copied().unwrap_or(0)
}

/// The package of version `ver` (0: the base), as `extract_application` picks it.
pub(crate) fn package_of(sc: &Scan, ver: u32) -> Option<usize> {
    if ver != 0 {
        sc.versions.get(&ver).copied()
    } else {
        sc.base
    }
}

/// `control(k, out, fallback)`: the package's control data beside its ExeFS; what it lacks comes from `fallback`
fn control(n: &Nup, k: &Package, out: &Path, fallback: Option<&Package>) -> Result<()> {
    let meta = control_meta(n, k, fallback)?;
    let nacp = meta_get(&meta, T_NACP);
    let icon = choose_icon(&meta);
    for (name, data) in [("control.nacp", nacp), ("icon.jpg", icon)] {
        match data {
            Some(d) => write_whole(&out.join(name), &d)?,
            None => {
                let _ = std::fs::remove_file(out.join(name));
            }
        }
    }
    Ok(())
}

// ---- the RomFS image --------------------------------------------------------------------------------------------------------

/// the RomFS folder a version runs with: its own, or the base game's for an update without one
fn romfs_of(_n: &Nup, k: &Package, base: Option<&Package>) -> Result<u32> {
    let d = if k.romfs != NO_DIR {
        k.romfs
    } else {
        base.map(|b| b.romfs).unwrap_or(NO_DIR)
    };
    if d == NO_DIR {
        bail!("{}: no romfs/ in this package or the base game", k.name);
    }
    Ok(d)
}

/// `{program index: RomFS folder}` of the other programs of the application (scan's siblings) at the version package `k` runs: each one's
/// newest update of no higher version, else its base (an update without romfs/ uses its base's)
fn sibling_romfs(n: &Nup, sc: &Scan, k: &Package) -> Vec<(u32, u32)> {
    let mut out = vec![];
    for (idx, s) in &sc.siblings {
        let u = s.updates.keys().copied().filter(|v| *v <= k.version).max();
        let d = match u {
            Some(v) if n.packages[s.updates[&v]].romfs != NO_DIR => n.packages[s.updates[&v]].romfs,
            _ => s.base.map(|b| n.packages[b].romfs).unwrap_or(NO_DIR),
        };
        if d != NO_DIR {
            out.push((*idx, d));
        }
    }
    out
}

/// the bytes of the images packing version `k` writes: its own RomFS and the other programs'
pub fn romfs_bytes(n: &Nup, sc: &Scan, k: &Package, base: Option<&Package>) -> Result<u64> {
    let mut t = n.tree_bytes(romfs_of(n, k, base)?);
    for (_, d) in sibling_romfs(n, sc, k) {
        t += n.tree_bytes(d);
    }
    Ok(t)
}

fn stamp_path(dst: &Path) -> PathBuf {
    tmp_beside(dst, ".inputs.json")
}

fn reusable(dst: &Path, digest: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(stamp_path(dst)) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Ok(id) = sys::file_identity(dst) else {
        return false;
    };
    v.get("source").and_then(|s| s.as_str()) == Some(digest)
        && v.get("image") == Some(&serde_json::json!(id))
}

fn stamp(dst: &Path, digest: &str, image: [u64; 4]) -> Result<()> {
    let sp = stamp_path(dst);
    let part = tmp_beside(&sp, ".part");
    std::fs::write(
        &part,
        serde_json::json!({"source": digest, "image": image}).to_string(),
    )?;
    std::fs::rename(&part, &sp)?;
    Ok(())
}

/// The identity of the tree this crate would pack: the TOC's hash covers every name and file hash. (The Python hashes its own scripts too;
/// a stamp of the other packer never matches, and the image is then checked against the container's hashes: `Verified`.)
fn image_digest(n: &Nup, d: u32) -> String {
    let mut h = Sha256::new();
    h.update(b"ibuilder-nup romfs image 1\0");
    h.update(n.toc_sha);
    h.update(d.to_le_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `verified(n, ents, dst)`: the identity of the image at `dst` when it is, byte for byte, what `write` makes of `ents` now (the header and the index,
/// and each file's bytes with the container's SHA-256), and it stayed the same while being read; else None.
pub fn verify_image(
    n: &Nup,
    ents: &[Entry],
    dst: &Path,
    opts: &Options,
    step: &str,
) -> Result<Option<[u64; 4]>> {
    use rayon::prelude::*;
    let mut ents: Vec<Entry> = ents.to_vec();
    let end = romfs::assign_offsets(n, &mut ents);
    let (head, idx) = romfs::layout(&ents, end)?;
    let before = match sys::file_identity(dst) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    if before[1] != end + idx.len() as u64 {
        return Ok(None);
    }
    let Ok(f) = std::fs::File::open(dst) else {
        return Ok(None);
    };
    let mut hb = vec![0u8; head.len()];
    if f.read_exact_at(&mut hb, 0).is_err() || hb != head {
        return Ok(None);
    }
    let mut ib = vec![0u8; idx.len()];
    if f.read_exact_at(&mut ib, end).is_err() || ib != idx {
        return Ok(None);
    }
    let files: Vec<(u32, u64, u64)> = ents
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| (e.node, e.a, e.b))
        .collect();
    let total: u64 = files.iter().map(|x| x.2).sum();
    let meter = Meter::new(step, opts, total, files.len() as u64);
    let bad = AtomicBool::new(false);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.pool_threads())
        .build()
        .map_err(|e| Error::Msg(format!("thread pool: {e}")))?;
    let halt = AtomicBool::new(false);
    let r: Result<()> = pool.install(|| {
        files
            .par_iter()
            .try_for_each(|&(node, off, size)| -> Result<()> {
                if bad.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if opts.cancelled() {
                    halt.store(true, Ordering::Relaxed);
                    return Err(Error::Cancelled);
                }
                match romfs::hash_range(&f, off, size, &opts.cancel)? {
                    Some(h) if h == n.file_sha(node as usize) => {}
                    _ => bad.store(true, Ordering::Relaxed),
                }
                meter.add_bytes(size);
                meter.file_done();
                Ok(())
            })
    });
    r?;
    meter.report(true);
    if bad.load(Ordering::Relaxed) {
        return Ok(None);
    }
    Ok(if sys::file_identity(dst).ok() == Some(before) {
        Some(before)
    } else {
        None
    })
}

/// A worker's failure: the first real one is kept (the others fail with `Cancelled` because the halt flag is set), and the rest stop.
fn note_error(first: &Mutex<Option<Error>>, halt: &AtomicBool, e: &Error) {
    if !matches!(e, Error::Cancelled) {
        let mut g = first.lock().unwrap();
        if g.is_none() {
            *g = Some(match e {
                Error::Msg(m) => Error::Msg(m.clone()),
                Error::Io(io) => Error::Io(std::io::Error::new(io.kind(), io.to_string())),
                Error::Cancelled => Error::Cancelled,
            });
        }
    }
    halt.store(true, Ordering::Relaxed);
}

/// The outcome of a parallel loop: the first real error, else a user's cancellation, else Ok.
fn finish_parallel(r: Result<()>, first: Mutex<Option<Error>>, opts: &Options) -> Result<()> {
    if let Some(e) = first.into_inner().unwrap() {
        return Err(e);
    }
    if opts.cancelled() {
        return Err(Error::Cancelled);
    }
    r
}

/// `romfs_image.write(ents, dst, copy)` with several threads: the image's data section is laid out from the container's sizes, so every file
/// goes to its own offset (positioned writes) whichever thread gets it; the index and the header are written last.
fn write_image(
    n: &Nup,
    ents: &mut [Entry],
    part: &Path,
    opts: &Options,
    step: &str,
) -> Result<u64> {
    use rayon::prelude::*;
    let end = romfs::assign_offsets(n, ents);
    let (head, idx) = romfs::layout(ents, end)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(part)?;
    file.set_len(end + idx.len() as u64)?;
    let files: Vec<(u32, u64)> = ents
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| (e.node, e.a))
        .collect();
    let total: u64 = files
        .iter()
        .map(|&(node, _)| n.file_size(node as usize))
        .sum();
    let meter = Meter::new(step, opts, total, files.len() as u64);
    let halt = AtomicBool::new(false);
    let first_err: Mutex<Option<Error>> = Mutex::new(None);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.pool_threads())
        .build()
        .map_err(|e| Error::Msg(format!("thread pool: {e}")))?;
    let r: Result<()> = pool.install(|| {
        files.par_iter().try_for_each(|&(node, off)| -> Result<()> {
            if halt.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let mut pos = off;
            let r = n.copy_with(
                node as usize,
                &mut |c| {
                    file.write_all_at(c, pos)?;
                    pos += c.len() as u64;
                    Ok(())
                },
                &halt,
                &mut |b| {
                    meter.add_bytes(b);
                    if opts.cancelled() {
                        halt.store(true, Ordering::Relaxed);
                    }
                },
            );
            match r {
                Ok(_) => {
                    meter.file_done();
                    Ok(())
                }
                Err(e) => {
                    note_error(&first_err, &halt, &e);
                    Err(e)
                }
            }
        })
    });
    finish_parallel(r, first_err, opts)?;
    meter.report(true);
    file.write_all_at(&idx, end)?;
    file.write_all_at(&head, 0)?;
    file.sync_all().ok();
    Ok(end + idx.len() as u64)
}

/// The RomFS folder `d` of the container to the image `img`. Returns how: `Kept` (the stamp says the container tree and the packer are the
/// same), `Verified` (the stamp is from another packer, but the image checks against the container's hashes: its stamp is renewed) or `Packed`.
pub fn pack(n: &Nup, d: u32, img: &Path, opts: &Options) -> Result<How> {
    if let Some(parent) = img.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut ents = romfs::entries(n, d);
    let digest = image_digest(n, d);
    if reusable(img, &digest) {
        return Ok(How::Kept);
    }
    if img.is_file() {
        // a packer update changes the digest, not the image: check its bytes before unpacking minutes of data again
        opts.say(&format!("checking {} against {}", img.display(), n.label));
        if let Some(identity) = verify_image(n, &ents, img, opts, "checking the image")? {
            stamp(img, &digest, identity)?;
            opts.say("  kept: the image matches the container");
            return Ok(How::Verified);
        }
    }
    opts.say(&format!(
        "packing {} folder {d} \u{2192} {}",
        n.label,
        img.display()
    ));
    let part = tmp_beside(img, ".part");
    let size = match write_image(n, &mut ents, &part, opts, "unpacking") {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    std::fs::rename(&part, img)?;
    stamp(img, &digest, sys::file_identity(img)?)?;
    opts.say(&format!(
        "  {} entries, {:.2} GB",
        crate::nup::thousands(ents.len() as u64),
        size as f64 / (1u64 << 30) as f64
    ));
    Ok(How::Packed)
}

/// the number of bytes the image of folder `d` will have
fn image_size(n: &Nup, d: u32) -> Result<u64> {
    let mut ents = romfs::entries(n, d);
    let end = romfs::assign_offsets(n, &mut ents);
    let (_, idx) = romfs::layout(&ents, end)?;
    Ok(end + idx.len() as u64)
}

/// `pack_version`: one version (the base package, or an update package) into `out/ExeFS`, `out/RomFS.img` and its control data, and
/// `out/romfs_programs/<index>.img` for each other program of the application; returns how this program's image was got
pub fn pack_version(
    n: &Nup,
    sc: &Scan,
    k: &Package,
    base: Option<&Package>,
    out: &Path,
    opts: &Options,
) -> Result<How> {
    if k.exefs == NO_DIR
        || n.child(k.exefs, b"main").is_none()
        || n.child(k.exefs, b"main.npdm").is_none()
    {
        bail!("{}: no exefs/main and exefs/main.npdm", k.name);
    }
    let npdm = n.read(n.child(k.exefs, b"main.npdm").unwrap())?;
    let pid = program_id(&npdm, &format!("{} exefs/main.npdm", k.name))?;
    if pid != sc.program_id {
        bail!(
            "{}: program ID {pid:016X}, the base is {:016X}",
            k.name,
            sc.program_id
        );
    }
    let d = romfs_of(n, k, base)?;
    std::fs::create_dir_all(out)?;
    exefs(n, k, &out.join("ExeFS"), opts)?;
    let how = pack(n, d, &out.join("RomFS.img"), opts)?;
    let others: Vec<(String, u32, u32)> = sibling_romfs(n, sc, k)
        .into_iter()
        .map(|(idx, sd)| (format!("{idx}.img"), sd, idx))
        .collect();
    let progs = out.join(PROGRAMS);
    if progs.is_dir() {
        for e in std::fs::read_dir(&progs)? {
            let e = e?;
            let nm = e.file_name().to_string_lossy().into_owned();
            // an image and the files beside it (its stamp)
            if !others
                .iter()
                .any(|(w, _, _)| nm == *w || nm.starts_with(&format!("{w}.")))
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
        if others.is_empty() {
            let _ = std::fs::remove_dir(&progs);
        }
    }
    for (name, sd, idx) in &others {
        opts.say(&format!(
            "program {} of the application ({:016X}): its RomFS \u{2192} {PROGRAMS}/{name}",
            name.trim_end_matches(".img"),
            sc.siblings[idx].program_id
        ));
        std::fs::create_dir_all(&progs)?;
        pack(n, *sd, &progs.join(name), opts)?;
    }
    control(
        n,
        k,
        out,
        if base.is_none() {
            Some(app_package(n))
        } else {
            None
        },
    )?;
    Ok(how)
}

/// `pack_version` without the RomFS: the ExeFS and the control data only (what a build needs to look at the code before it unpacks 25 GB)
pub fn pack_exefs_only(
    n: &Nup,
    sc: &Scan,
    k: &Package,
    base: Option<&Package>,
    out: &Path,
    opts: &Options,
) -> Result<()> {
    if k.exefs == NO_DIR
        || n.child(k.exefs, b"main").is_none()
        || n.child(k.exefs, b"main.npdm").is_none()
    {
        bail!("{}: no exefs/main and exefs/main.npdm", k.name);
    }
    let npdm = n.read(n.child(k.exefs, b"main.npdm").unwrap())?;
    let pid = program_id(&npdm, &format!("{} exefs/main.npdm", k.name))?;
    if pid != sc.program_id {
        bail!(
            "{}: program ID {pid:016X}, the base is {:016X}",
            k.name,
            sc.program_id
        );
    }
    std::fs::create_dir_all(out)?;
    exefs(n, k, &out.join("ExeFS"), opts)?;
    control(
        n,
        k,
        out,
        if base.is_none() {
            Some(app_package(n))
        } else {
            None
        },
    )
}

/// `pack_dlc`: `{DLC title ID: package}` into `aoc/<index>.img` (index = title ID minus the add-on content base ID scan found); images of DLC no
/// longer in the container go
pub fn pack_dlc(n: &Nup, sc: &Scan, aoc: &Path, opts: &Options) -> Result<()> {
    let first = sc.aoc_base;
    let want: Vec<(String, u32)> = sc
        .dlc
        .iter()
        .map(|(tid, k)| (format!("{}.img", tid - first), n.packages[*k].romfs))
        .collect();
    if aoc.is_dir() {
        for e in std::fs::read_dir(aoc)? {
            let e = e?;
            let nm = e.file_name().to_string_lossy().into_owned();
            let stem = nm.strip_suffix(".inputs.json").unwrap_or(&nm).to_string();
            if !want.iter().any(|(w, _)| *w == stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for (name, d) in &want {
        std::fs::create_dir_all(aoc)?;
        pack(n, *d, &aoc.join(name), opts)?;
    }
    Ok(())
}

/// the nearest existing ancestor of `p` (a volume to ask about free space)
fn existing_ancestor(p: &Path) -> PathBuf {
    let mut q = p.to_path_buf();
    while !q.exists() {
        if !q.pop() {
            return PathBuf::from(".");
        }
    }
    q
}

/// The Python's `extract_nup` check: the bytes the images need plus 512 MiB against the free space of the volume. An image that is already there at
/// the size this container's RomFS gives is not asked for again (a repeat run keeps or verifies it; the Python would refuse it when the volume is
/// nearly full, though nothing is written).
pub fn check_space(
    n: &Nup,
    sc: &Scan,
    k: &Package,
    base: Option<&Package>,
    extract: &Path,
    opts: &Options,
) -> Result<()> {
    let need = romfs_bytes(n, sc, k, base)? + SPACE_MARGIN;
    let Some(have) = opts.free(&existing_ancestor(extract)) else {
        return Ok(());
    };
    if have >= need {
        return Ok(());
    }
    let img = extract.join("RomFS.img");
    if let (Ok(m), Ok(want)) = (
        std::fs::metadata(&img),
        image_size(n, romfs_of(n, k, base)?),
    ) {
        let others_exist = sibling_romfs(n, sc, k).iter().all(|(idx, d)| {
            std::fs::metadata(extract.join(PROGRAMS).join(format!("{idx}.img")))
                .map(|m| image_size(n, *d).map(|w| w == m.len()).unwrap_or(false))
                .unwrap_or(false)
        });
        if m.len() == want && others_exist {
            return Ok(());
        }
    }
    bail!(
        "not enough space for {}: about {:.1} GB needed, {:.1} GB free",
        n.label,
        need as f64 / (1u64 << 30) as f64,
        have as f64 / (1u64 << 30) as f64
    )
}

/// What [`extract_application`] did
#[derive(Debug, Clone)]
pub struct Extracted {
    /// the title ID (the program ID packed), `%016X`
    pub title_id: String,
    pub program_id: u64,
    pub application: u64,
    pub version: u32,
    pub package: String,
    pub has_base: bool,
    pub dlc: usize,
    pub how: How,
}

/// `gameapp_pipeline.extract_nup`: the newest version of the container into `extract` (and its DLC into `aoc`, when given), as the app's stage 1 does.
///
/// `aoc`: where the DLC go, given the title id (`Documents/games/<title id>/aoc`: the id is only known once the container is read). `nup` is a container already
/// opened (to choose its zstd decoder); `program` / `version` pick a program of a Switch 2 Edition and an update (default: the highest program, the newest update).
pub fn extract_application(
    nup_path: &Path,
    extract: &Path,
    aoc: Option<&dyn Fn(&str) -> PathBuf>,
    program: Option<u32>,
    version: Option<u32>,
    nup: Option<Nup>,
    opts: &Options,
) -> Result<Extracted> {
    let n = match nup {
        Some(n) => n,
        None => Nup::open(nup_path)?,
    };
    let sc = scan(&n, program)?;
    let ver = match version {
        Some(v) => {
            if v != 0 && !sc.versions.contains_key(&v) {
                let have: Vec<String> = sc.versions.keys().map(|v| v.to_string()).collect();
                bail!(
                    "no update {v} in {} (have: {})",
                    n.label,
                    if have.is_empty() {
                        "none".into()
                    } else {
                        have.join(", ")
                    }
                );
            }
            v
        }
        None => newest_version(&sc),
    };
    let ki = package_of(&sc, ver);
    let Some(ki) = ki else {
        bail!(
            "{} holds no base game and no standalone update to run",
            n.label
        )
    };
    let k = &n.packages[ki];
    let base = sc.base.map(|b| &n.packages[b]);
    if base.is_none() {
        opts.say(NO_BASE);
    }
    check_space(&n, &sc, k, base, extract, opts)?;
    // what an earlier game left that this one may not carry: its control data and its other programs
    for leaf in ["control.nacp", "icon.jpg", "RomFS.img.id"] {
        let _ = std::fs::remove_file(extract.join(leaf));
    }
    let _ = std::fs::remove_dir_all(extract.join(PROGRAMS));
    opts.say(&format!(
        "nup: {:016X} version {ver} ({}){}, {} DLC",
        sc.program_id,
        k.name,
        if base.is_some() { "" } else { ", no base game" },
        sc.dlc.len()
    ));
    let how = pack_version(&n, &sc, k, base, extract, opts)?;
    if let (false, Some(aoc)) = (sc.dlc.is_empty(), aoc) {
        pack_dlc(&n, &sc, &aoc(&format!("{:016X}", sc.program_id)), opts)?;
    }
    Ok(Extracted {
        title_id: format!("{:016X}", sc.program_id),
        program_id: sc.program_id,
        application: sc.application,
        version: ver,
        package: k.name.clone(),
        has_base: base.is_some(),
        dlc: sc.dlc.len(),
        how,
    })
}

/// Reads and checks every file of the container against its SHA-256 without writing anything (the container's own hashes are the oracle:
/// no second copy of the game is needed to check it). Returns (files, uncompressed bytes).
pub fn verify_container(n: &Nup, opts: &Options) -> Result<(u64, u64)> {
    use rayon::prelude::*;
    let total: u64 = (0..n.file_count()).map(|i| n.file_size(i)).sum();
    let meter = Meter::new("verifying", opts, total, n.file_count() as u64);
    let halt = AtomicBool::new(false);
    let first_err: Mutex<Option<Error>> = Mutex::new(None);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(opts.pool_threads())
        .build()
        .map_err(|e| Error::Msg(format!("thread pool: {e}")))?;
    let r: Result<()> = pool.install(|| {
        (0..n.file_count())
            .into_par_iter()
            .try_for_each(|i| -> Result<()> {
                if halt.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let r = n.copy_with(i, &mut |_| Ok(()), &halt, &mut |b| {
                    meter.add_bytes(b);
                    if opts.cancelled() {
                        halt.store(true, Ordering::Relaxed);
                    }
                });
                match r {
                    Ok(_) => {
                        meter.file_done();
                        Ok(())
                    }
                    Err(e) => {
                        note_error(&first_err, &halt, &e);
                        Err(e)
                    }
                }
            })
    });
    finish_parallel(r, first_err, opts)?;
    meter.report(true);
    Ok((n.file_count() as u64, total))
}

pub use crate::romfs::read_index;

#[cfg(test)]
mod write_whole_tests {
    use super::write_whole;

    /// `control.nacp` / `icon.jpg` beside an extraction: replaced whole (a `.part` beside, then a rename) or not at all
    #[test]
    fn a_file_is_replaced_whole_or_not_at_all() {
        let base = std::env::var("IBUILDER_TEST_TMP")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        let d = base.join(format!("ibuilder-nup-write-whole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("control.nacp");
        write_whole(&f, b"first").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"first");
        assert!(!d.join("control.nacp.part").exists());
        // the .part cannot be written (a folder is in its place): the old file stays, and nothing is left behind
        std::fs::create_dir(d.join("control.nacp.part")).unwrap();
        assert!(write_whole(&f, b"second").is_err());
        assert_eq!(std::fs::read(&f).unwrap(), b"first");
        let _ = std::fs::remove_dir_all(&d);
    }
}
