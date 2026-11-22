//! `pack_nup.Nup` and `Package`: a validated NUP (SPEC section 6.1): the superblock, the TOC's SHA-256, every record's bounds. A file's
//! content is checked against its size and SHA-256 as it is read.
//!
//! Layout (little-endian): superblock 0x80 bytes (`NUP` + format version byte, header size, TOC offset/size/SHA-256, data blob offset,
//! file size, package count, flags, uuid); the TOC (`TOC\x01`, counts, name pool, then the package records of 0x40 bytes, the folder
//! records of 0x18, the file records of 0x50, the u32 child-slot array, the name pool); file data in the blob, every extent 16-aligned.

use crate::error::{Error, Result, bail};
use crate::io::{Extent, FileSource, ReadAt};
use crate::zstd::{ZstdBackend, default_backend};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const NO_DIR: u32 = 0xFFFF_FFFF;
pub const BASE: u8 = 0;
pub const UPDATE: u8 = 1;
pub const ADDON: u8 = 2;
pub const SYSTEM: u8 = 4;
pub const KINDS: [&str; 5] = ["base", "update", "addon", "addon_update", "system"];
/// package flags bit 0: an update in a container without a base
pub const STANDALONE: u8 = 1;
/// dump-tool files a producer may have kept (SPEC section 6.3): not game files (`romfs_image.ARTIFACTS`)
pub const ARTIFACTS: [&[u8]; 1] = [b".listing"];

const SUPER_SIZE: usize = 0x80;
const TOC_SIZE: usize = 0x30;
const PKG_SIZE: usize = 0x40;
const DIR_SIZE: usize = 0x18;
const FILE_SIZE: usize = 0x50;
/// a TOC this large is not a NUP's (Ocarina's, 186,587 files, is a few MB): refused before it is read into memory
const TOC_MAX: u64 = 1 << 30;
/// bytes of one raw copy chunk (the Python's 8 MiB)
const RAW_CHUNK: u64 = 8 << 20;

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}
fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}
fn u64le(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

#[derive(Debug, Clone)]
pub struct Package {
    pub index: usize,
    pub title_id: u64,
    pub root: u32,
    pub exefs: u32,
    pub romfs: u32,
    pub version: u32,
    pub kind: u8,
    pub flags: u8,
    pub meta_off: u64,
    pub meta_size: u32,
    /// what `pick_nsps` prints, e.g. `Game.nup (update 131072)`
    pub name: String,
}

#[derive(Debug, Clone, Copy)]
struct Dir {
    name_off: u32,
    name_len: u32,
    first: u32,
    count: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FileRec {
    pub name_off: u32,
    pub name_len: u32,
    pub size: u64,
    pub eoff: u64,
    pub esz: u64,
    pub sha: [u8; 32],
    pub codec: u8,
}

/// An entry of a folder: its name, whether it is a folder, and its index into the folders or the files
#[derive(Debug, Clone, Copy)]
pub struct Child<'a> {
    pub name: &'a [u8],
    pub is_dir: bool,
    pub index: usize,
}

pub struct Nup {
    pub path: PathBuf,
    /// the file name, for messages
    pub label: String,
    src: Box<dyn ReadAt>,
    backend: Arc<dyn ZstdBackend>,
    pub toc_sha: [u8; 32],
    pub uuid: [u8; 16],
    pub blob: u64,
    pub fsize: u64,
    pub packages: Vec<Package>,
    dirs: Vec<Dir>,
    pub(crate) files: Vec<FileRec>,
    slots: Vec<u32>,
    pool: Vec<u8>,
}

pub fn is_nup(path: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 3];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
        && &b == b"NUP"
}

impl Nup {
    pub fn open(path: &Path) -> Result<Nup> {
        let src =
            FileSource::open(path).map_err(|e| Error::Msg(format!("{}: {e}", path.display())))?;
        Nup::from_source(Box::new(src), path)
    }

    pub fn from_source(src: Box<dyn ReadAt>, path: &Path) -> Result<Nup> {
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut n = Nup {
            path: path.to_path_buf(),
            label,
            src,
            backend: default_backend(),
            toc_sha: [0; 32],
            uuid: [0; 16],
            blob: 0,
            fsize: 0,
            packages: vec![],
            dirs: vec![],
            files: vec![],
            slots: vec![],
            pool: vec![],
        };
        n.read_toc()?;
        Ok(n)
    }

    /// use another zstd decoder (the pure-Rust one, the host app's binding)
    pub fn with_backend(mut self, b: Arc<dyn ZstdBackend>) -> Nup {
        self.backend = b;
        self
    }

    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    fn p(&self) -> String {
        self.path.display().to_string()
    }

    fn read_toc(&mut self) -> Result<()> {
        let len = self.src.len();
        if len == 0 {
            bail!("{}: empty file", self.p());
        }
        if (len as usize) < SUPER_SIZE {
            bail!("{}: not a NUP (too short)", self.p());
        }
        let mut sb = [0u8; SUPER_SIZE];
        self.src.read_exact_at(&mut sb, 0)?;
        let magic = &sb[..4];
        let hsize = u16le(&sb, 4) as u64;
        let toc_off = u64le(&sb, 8);
        let toc_size = u64le(&sb, 16);
        let mut toc_sha = [0u8; 32];
        toc_sha.copy_from_slice(&sb[24..56]);
        self.blob = u64le(&sb, 56);
        let fsize = u64le(&sb, 64);
        let npkg = u32le(&sb, 72) as u64;
        let flags = u32le(&sb, 76);
        self.uuid.copy_from_slice(&sb[80..96]);
        self.fsize = fsize;
        if &magic[..3] != b"NUP" {
            bail!("{}: not a NUP", self.p());
        }
        if magic[3] != 1 {
            bail!(
                "{}: NUP format version {}; this reads version 1",
                self.p(),
                magic[3]
            );
        }
        if hsize < SUPER_SIZE as u64 || toc_off != hsize || flags != 0 {
            bail!("{}: unsupported NUP header", self.p());
        }
        if fsize != len {
            bail!(
                "{}: {} bytes, its header says {} (copied or downloaded partly?)",
                self.p(),
                thousands(len),
                thousands(fsize)
            );
        }
        if self.blob < toc_off.saturating_add(toc_size)
            || !self.blob.is_multiple_of(16)
            || self.blob > fsize
        {
            bail!("{}: TOC and data regions overlap", self.p());
        }
        if toc_size > TOC_MAX {
            bail!(
                "{}: table of contents of {} bytes (implausible)",
                self.p(),
                thousands(toc_size)
            );
        }
        let mut toc = vec![0u8; toc_size as usize];
        self.src.read_exact_at(&mut toc, toc_off)?;
        self.toc_sha = toc_sha;
        if Sha256::digest(&toc).as_slice() != toc_sha {
            bail!(
                "{}: table of contents does not match its SHA-256 (damaged file)",
                self.p()
            );
        }
        if toc.len() < TOC_SIZE {
            bail!("{}: malformed table of contents", self.p());
        }
        let tmagic = &toc[..4];
        let tpkg = u32le(&toc, 4) as u64;
        let ndirs = u64le(&toc, 8);
        let nfiles = u64le(&toc, 16);
        let pool_off = u64le(&toc, 24);
        let pool_size = u64le(&toc, 32);
        let need = (TOC_SIZE as u128)
            + tpkg as u128 * PKG_SIZE as u128
            + ndirs as u128 * DIR_SIZE as u128
            + nfiles as u128 * FILE_SIZE as u128;
        if tmagic != b"TOC\x01"
            || tpkg != npkg
            || npkg == 0
            || (toc.len() as u128) < need
            || (pool_off as u128) < need
            || pool_off as u128 + pool_size as u128 > toc.len() as u128
            || !(pool_off as u128 - need).is_multiple_of(4)
        {
            bail!("{}: malformed table of contents", self.p());
        }
        let (tpkg, ndirs, nfiles) = (tpkg as usize, ndirs as usize, nfiles as usize);
        let (pool_off, pool_size) = (pool_off as usize, pool_size as usize);
        self.pool = toc[pool_off..pool_off + pool_size].to_vec();
        let pool_len = self.pool.len() as u64;
        let check_name = |off: u32, n: u32, p: &str| -> Result<()> {
            if off as u64 + n as u64 > pool_len {
                bail!("{p}: name out of bounds");
            }
            Ok(())
        };
        let mut p = TOC_SIZE;
        let mut packages = Vec::with_capacity(tpkg);
        for i in 0..tpkg {
            let r = &toc[p + i * PKG_SIZE..p + (i + 1) * PKG_SIZE];
            let (title_id, root, exefs, romfs, version) = (
                u64le(r, 0),
                u32le(r, 8),
                u32le(r, 12),
                u32le(r, 16),
                u32le(r, 20),
            );
            let (kind, pflags, r1, r2, meta_off, meta_size) = (
                r[24],
                r[25],
                u16le(r, 26),
                u32le(r, 28),
                u64le(r, 32),
                u32le(r, 40),
            );
            if kind as usize >= KINDS.len() {
                bail!("{}: package {i} of unknown kind {kind}", self.p());
            }
            if r1 != 0 || r2 != 0 || pflags & !STANDALONE != 0 {
                bail!("{}: package {i} has reserved fields or flags set", self.p());
            }
            let mut name = format!("{} ({}", self.label, KINDS[kind as usize]);
            if kind == UPDATE {
                name += &format!(" {version}");
            } else if kind == ADDON {
                name += &format!(" {title_id:016X}");
            }
            name.push(')');
            packages.push(Package {
                index: i,
                title_id,
                root,
                exefs,
                romfs,
                version,
                kind,
                flags: pflags,
                meta_off,
                meta_size,
                name,
            });
        }
        p += tpkg * PKG_SIZE;
        let sp = self.p();
        let mut dirs = Vec::with_capacity(ndirs);
        for _ in 0..ndirs {
            let (o, n, first, count) = (
                u32le(&toc, p),
                u32le(&toc, p + 4),
                u32le(&toc, p + 8),
                u32le(&toc, p + 12),
            );
            p += DIR_SIZE;
            check_name(o, n, &sp)?;
            dirs.push(Dir {
                name_off: o,
                name_len: n,
                first,
                count,
            });
        }
        let mut files = Vec::with_capacity(nfiles);
        for _ in 0..nfiles {
            let r = &toc[p..p + FILE_SIZE];
            p += FILE_SIZE;
            let (o, n, size, eoff, esz) = (
                u32le(r, 0),
                u32le(r, 4),
                u64le(r, 8),
                u64le(r, 16),
                u64le(r, 24),
            );
            let mut sha = [0u8; 32];
            sha.copy_from_slice(&r[32..64]);
            let (codec, cflags) = (r[64], r[65]);
            check_name(o, n, &sp)?;
            let nm = String::from_utf8_lossy(&self.pool[o as usize..(o + n) as usize]).into_owned();
            let shown = format!("{:?}", nm);
            if (codec != 0 && codec != 1) || cflags != 0 {
                bail!(
                    "{}: file {} uses codec {codec}; this reads raw and zstd",
                    sp,
                    shown
                );
            }
            if size != 0
                && (eoff < self.blob
                    || eoff.saturating_add(esz) > fsize
                    || eoff % 16 != 0
                    || esz == 0
                    || (codec == 0 && esz != size))
            {
                bail!("{}: file {} out of bounds", sp, shown);
            }
            if size == 0 && (eoff != 0 || esz != 0 || codec != 0) {
                bail!("{}: empty file {} with data", sp, shown);
            }
            files.push(FileRec {
                name_off: o,
                name_len: n,
                size,
                eoff,
                esz,
                sha,
                codec,
            });
        }
        let nslots = (pool_off - p) / 4;
        let mut slots = Vec::with_capacity(nslots);
        for i in 0..nslots {
            slots.push(u32le(&toc, p + 4 * i));
        }
        for d in &dirs {
            if d.first as u64 + d.count as u64 > slots.len() as u64 {
                bail!(
                    "{}: folder {:?} out of bounds",
                    sp,
                    String::from_utf8_lossy(
                        &self.pool[d.name_off as usize..(d.name_off + d.name_len) as usize]
                    )
                );
            }
        }
        for &s in &slots {
            if s as u64 >= ndirs as u64 + nfiles as u64 {
                bail!("{}: child {s} out of bounds", sp);
            }
        }
        self.packages = packages;
        self.dirs = dirs;
        self.files = files;
        self.slots = slots;
        self.acyclic()?;
        for k in &self.packages {
            let bad_dir = |d: u32| d != NO_DIR && d as usize >= ndirs;
            if k.root as usize >= ndirs
                || bad_dir(k.exefs)
                || bad_dir(k.romfs)
                || (k.meta_size != 0 && k.meta_off.saturating_add(k.meta_size as u64) > fsize)
            {
                bail!("{}: package {} out of bounds", sp, k.index);
            }
        }
        Ok(())
    }

    /// Refuses a folder that contains itself, at any depth (the bounds checks pass it, and every walk of the tree would loop for ever), and
    /// a folder listed by more than one slot (two folders, or twice by one): the tree is then a DAG, and every walk of it (`tree_bytes`,
    /// `romfs::entries`) visits a shared folder once per path, 2^K times for K stacked "diamonds" in a TOC of a few hundred bytes. The
    /// `DMROMFS1` reader (`romfs::read_index`) refuses the same thing. (The Python accepts it and its walks take the same exponential time.)
    fn acyclic(&self) -> Result<()> {
        let nd = self.dirs.len();
        let kids = |d: usize| -> &[u32] {
            let dd = &self.dirs[d];
            &self.slots[dd.first as usize..(dd.first + dd.count) as usize]
        };
        let mut listed = vec![false; nd];
        for d in 0..nd {
            for &c in kids(d) {
                let c = c as usize;
                if c < nd && std::mem::replace(&mut listed[c], true) {
                    bail!(
                        "{}: folder {:?} (index {c}) is listed by more than one folder: the index is not a tree (damaged file)",
                        self.p(),
                        String::from_utf8_lossy(self.dir_name(c))
                    );
                }
            }
        }
        let mut state = vec![0u8; nd]; // 0 not seen, 1 on the current path, 2 done
        for top in 0..nd {
            if state[top] != 0 {
                continue;
            }
            state[top] = 1;
            let mut stack: Vec<(usize, usize)> = vec![(top, 0)];
            while let Some(&(d, i)) = stack.last() {
                // children that are folders, in order
                let ks = kids(d);
                let mut idx = i;
                let mut next = None;
                while idx < ks.len() {
                    if (ks[idx] as usize) < nd {
                        next = Some(ks[idx] as usize);
                        idx += 1;
                        break;
                    }
                    idx += 1;
                }
                match next {
                    None => {
                        state[d] = 2;
                        stack.pop();
                    }
                    Some(c) => {
                        stack.last_mut().unwrap().1 = idx;
                        if state[c] == 1 {
                            bail!(
                                "{}: folder {:?} (index {c}) contains itself (damaged file)",
                                self.p(),
                                String::from_utf8_lossy(self.dir_name(c))
                            );
                        }
                        if state[c] == 0 {
                            state[c] = 1;
                            stack.push((c, 0));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn dir_name(&self, d: usize) -> &[u8] {
        let dd = &self.dirs[d];
        &self.pool[dd.name_off as usize..(dd.name_off + dd.name_len) as usize]
    }

    pub fn file_name(&self, i: usize) -> &[u8] {
        let f = &self.files[i];
        &self.pool[f.name_off as usize..(f.name_off + f.name_len) as usize]
    }

    pub fn file_size(&self, i: usize) -> u64 {
        self.files[i].size
    }

    pub fn file_sha(&self, i: usize) -> [u8; 32] {
        self.files[i].sha
    }

    /// the stored (compressed) bytes of file `i` and its codec (0 raw, 1 zstd)
    pub fn file_extent(&self, i: usize) -> (u64, u64, u8) {
        let f = &self.files[i];
        (f.eoff, f.esz, f.codec)
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn dir_count(&self) -> usize {
        self.dirs.len()
    }

    /// the container's bytes at `off` (for tests and tools)
    pub fn raw(&self, buf: &mut [u8], off: u64) -> Result<()> {
        Ok(self.src.read_exact_at(buf, off)?)
    }

    /// `{TLV type: payload}` of the package's meta block (first record of a type wins)
    pub fn meta(&self, k: &Package) -> Result<Vec<(u16, Vec<u8>)>> {
        let mut out: Vec<(u16, Vec<u8>)> = vec![];
        if k.meta_size == 0 {
            return Ok(out);
        }
        let mut b = vec![0u8; k.meta_size as usize];
        self.src.read_exact_at(&mut b, k.meta_off)?;
        let mut p = 0usize;
        while p + 8 <= b.len() {
            let t = u16le(&b, p);
            let n = u32le(&b, p + 4) as usize;
            p += 8;
            if p + n > b.len() {
                bail!(
                    "{}: package {}: meta record {t:#x} runs past its block",
                    self.p(),
                    k.index
                );
            }
            if !out.iter().any(|(tt, _)| *tt == t) {
                out.push((t, b[p..p + n].to_vec()));
            }
            p += n;
        }
        Ok(out)
    }

    /// `[(name, is dir, index into dirs or files)]` of folder `d`, sorted by name (bytes), artifacts left out
    pub fn children(&self, d: u32) -> Vec<Child<'_>> {
        let dd = &self.dirs[d as usize];
        let nd = self.dirs.len();
        let mut out = Vec::with_capacity(dd.count as usize);
        for &s in &self.slots[dd.first as usize..(dd.first + dd.count) as usize] {
            let s = s as usize;
            if s < nd {
                out.push(Child {
                    name: self.dir_name(s),
                    is_dir: true,
                    index: s,
                });
            } else {
                let fi = s - nd;
                let nm = self.file_name(fi);
                if !ARTIFACTS.contains(&nm) {
                    out.push(Child {
                        name: nm,
                        is_dir: false,
                        index: fi,
                    });
                }
            }
        }
        // Python sorts the tuples (name, is dir, index)
        out.sort_by(|a, b| (a.name, a.is_dir, a.index).cmp(&(b.name, b.is_dir, b.index)));
        out
    }

    /// the index of the file `name` in folder `d`, or None
    pub fn child(&self, d: u32, name: &[u8]) -> Option<usize> {
        self.children(d)
            .into_iter()
            .find(|c| c.name == name && !c.is_dir)
            .map(|c| c.index)
    }

    /// the uncompressed size of everything under folder `d`
    pub fn tree_bytes(&self, d: u32) -> u64 {
        let mut total = 0u64;
        let mut stack = vec![d];
        while let Some(x) = stack.pop() {
            for c in self.children(x) {
                if c.is_dir {
                    stack.push(c.index as u32);
                } else {
                    total = total.saturating_add(self.files[c.index].size);
                }
            }
        }
        total
    }

    /// File `i`'s content to `out`, streamed and checked against its size and SHA-256; returns its size. `cancel` is looked at between chunks.
    pub fn copy(
        &self,
        i: usize,
        out: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
        cancel: &AtomicBool,
    ) -> Result<u64> {
        self.copy_with(i, out, cancel, &mut |_| {})
    }

    /// as [`Nup::copy`], `tick(n)` after each chunk of n uncompressed bytes (progress)
    pub fn copy_with(
        &self,
        i: usize,
        out: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
        cancel: &AtomicBool,
        tick: &mut dyn FnMut(u64),
    ) -> Result<u64> {
        let f = self.files[i];
        let damaged = || {
            Error::Msg(format!(
                "{}: {} does not match its size or SHA-256 (damaged file)",
                self.p(),
                String::from_utf8_lossy(self.file_name(i))
            ))
        };
        let mut h = Sha256::new();
        let mut n: u64 = 0;
        if f.codec == 0 {
            let mut buf = vec![0u8; f.esz.min(RAW_CHUNK) as usize];
            let mut p = f.eoff;
            let end = f.eoff + f.esz;
            while p < end {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let k = (end - p).min(RAW_CHUNK) as usize;
                self.src.read_exact_at(&mut buf[..k], p)?;
                h.update(&buf[..k]);
                out(&buf[..k])?;
                n += k as u64;
                tick(k as u64);
                p += k as u64;
            }
        } else if f.size != 0 {
            let mut src = Extent {
                src: &*self.src,
                pos: f.eoff,
                end: f.eoff + f.esz,
            };
            let mut over = false;
            let mut cancelled = false;
            let mut io_err: Option<std::io::Error> = None;
            let r = self.backend.decode_frame(&mut src, &mut |chunk: &[u8]| {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    return Err(std::io::Error::other("cancelled"));
                }
                h.update(chunk);
                n += chunk.len() as u64;
                if n > f.size {
                    over = true;
                    return Err(std::io::Error::other("more than its size"));
                }
                if let Err(e) = out(chunk) {
                    let m = e.to_string();
                    io_err = Some(e);
                    return Err(std::io::Error::other(m));
                }
                tick(chunk.len() as u64);
                Ok(())
            });
            if cancelled {
                return Err(Error::Cancelled);
            }
            if let Some(e) = io_err {
                return Err(Error::Io(e));
            }
            if over {
                return Err(damaged());
            }
            if let Err(e) = r {
                bail!(
                    "{}: {}: {e}",
                    self.p(),
                    String::from_utf8_lossy(self.file_name(i))
                );
            }
        }
        if n != f.size || h.finalize().as_slice() != f.sha {
            return Err(damaged());
        }
        Ok(n)
    }

    /// file `i`, whole (small files: the NPDM, the control data)
    pub fn read(&self, i: usize) -> Result<Vec<u8>> {
        let mut v = Vec::with_capacity(self.files[i].size.min(1 << 30) as usize);
        self.copy(
            i,
            &mut |c| {
                v.extend_from_slice(c);
                Ok(())
            },
            &AtomicBool::new(false),
        )?;
        Ok(v)
    }

    /// file `i`'s first `n` bytes (fewer for a shorter file), not checked against the SHA-256 (which covers the whole file)
    pub fn head(&self, i: usize, n: usize) -> Result<Vec<u8>> {
        let f = self.files[i];
        let want = (f.size.min(n as u64)) as usize;
        if f.codec == 0 || f.size == 0 {
            let mut b = vec![0u8; want];
            if want > 0 {
                self.src.read_exact_at(&mut b, f.eoff)?;
            }
            return Ok(b);
        }
        let mut src = Extent {
            src: &*self.src,
            pos: f.eoff,
            end: f.eoff + f.esz,
        };
        let mut v: Vec<u8> = vec![];
        let _ = self.backend.decode_frame(&mut src, &mut |c: &[u8]| {
            let take = (want - v.len()).min(c.len());
            v.extend_from_slice(&c[..take]);
            if v.len() >= want {
                return Err(std::io::Error::other("enough"));
            }
            Ok(())
        });
        v.truncate(want);
        Ok(v)
    }
}

/// `f'{n:,}'`
pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
