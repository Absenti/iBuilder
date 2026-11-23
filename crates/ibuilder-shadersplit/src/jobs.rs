//! The containers the Python plans as jobs of their own (`linkdata_jobs`, `kpka_jobs`, `bdt_jobs`, `pkz_jobs`, `zip_jobs`, `arh_jobs` of `shader_split.main`): archives whose members are
//! scanned like loose files. Their files are taken out of the plain list, and their jobs run **before** the plain files' jobs (the Python's `big + files`), which fixes which copy of a
//! key is the first.

use crate::archive::*;
use crate::bytes::*;
use crate::ctl::*;
use crate::error::{Failure, Res, SplitError};
use crate::glslc::glslc_shaders;
use crate::scan::{FileRef, finish, scan_blob};
use crate::scan_types::{Ctx, FileResult, Shader};
use crate::source::Source;
use crate::unity::Trees;
use crate::{red, unsupported};
use std::collections::{BTreeMap, HashSet};

const PARTS: u64 = 64;

/// One job of an archive: a range of its members
#[derive(Clone)]
pub enum Special {
    /// `(file, member names)`
    Zip(FileRef, Vec<String>),
    /// `(pak, entries)`: `(path hash, offset, stored length, length, attributes)`
    Kpka(FileRef, Vec<KEntry>),
    /// `(bdt, entries)`: `(path hash, offset, size)`
    Bdt(FileRef, Vec<(u32, u64, u64)>),
    /// `(pkz, entries)`: `(name, compression, offset, stored length, length)`
    Pkz(FileRef, Vec<(String, String, u64, u64, u64)>),
    /// `(ard, entries)`: `(name, offset, size)`
    Arh(FileRef, Vec<(String, u64, u64)>),
    /// `(archive, its index, first entry, end entry)`: Koei Tecmo's LINKDATA
    Linkdata(FileRef, FileRef, usize, usize),
    /// files of an Unreal pak with inline shaders (`ue_legacy_jobs`, uepak.rs)
    Ue(crate::uepak::UeJob),
    /// a range of Game Freak's Trinity packs (`trpfs_jobs`, trinity.rs)
    Trinity(crate::trinity::TrinityJob),
    /// a range of Super Smash Bros. Ultimate's data.arc files (`smash_arc_jobs`, smash_arc.rs)
    SmashArc(crate::smash_arc::ArcJob),
    /// a range of a CRI CPK's entries (`cpk_jobs`, cpk.rs)
    Cpk(crate::cpk::CpkJob),
    /// a byte range of a Diablo III package (`d3cpk_jobs`, cpk.rs)
    D3(crate::cpk::D3Job),
    /// a range of one CASC archive's files (Diablo II: Resurrected, casc.rs)
    Casc(crate::casc::CascJob),
    /// a range of Camelot's CRA archive's files (cra.rs)
    Cra(crate::cra::CraJob),
    /// a byte range of a phx PDAT archive (Kirby Air Riders, pdat.rs)
    Pdat(crate::pdat::PdatJob),
    /// a range of a Rockstar RPF6 archive's files (Red Dead Redemption, rpf6.rs)
    Rpf(crate::rpf6::RpfJob),
    /// a range of a Larian package's BSHD members (Divinity: Original Sin 2, lspk.rs)
    Lspk(crate::lspk::LspkJob),
}

impl Special {
    pub fn file(&self) -> &FileRef {
        match self {
            Special::Zip(f, _)
            | Special::Kpka(f, _)
            | Special::Bdt(f, _)
            | Special::Pkz(f, _)
            | Special::Arh(f, _)
            | Special::Linkdata(f, _, _, _) => f,
            Special::Ue(j) => &j.file,
            Special::Trinity(j) => &j.file,
            Special::SmashArc(j) => &j.file,
            Special::Cpk(j) => &j.file,
            Special::D3(j) => &j.file,
            Special::Casc(j) => &j.file,
            Special::Cra(j) => &j.file,
            Special::Pdat(j) => &j.file,
            Special::Rpf(j) => &j.file,
            Special::Lspk(j) => &j.file,
        }
    }

    /// what the memo keys a job by
    pub fn identity(&self) -> Vec<String> {
        match self {
            Special::Zip(f, n) => {
                std::iter::once(format!("zip:{}:{}:{}", f.root, f.rel, f.loc.len()))
                    .chain(n.iter().cloned())
                    .collect()
            }
            Special::Kpka(f, e) => {
                std::iter::once(format!("kpka:{}:{}:{}", f.root, f.rel, f.loc.len()))
                    .chain(e.iter().map(|x| format!("{x:?}")))
                    .collect()
            }
            Special::Bdt(f, e) => {
                std::iter::once(format!("bdt:{}:{}:{}", f.root, f.rel, f.loc.len()))
                    .chain(e.iter().map(|x| format!("{x:?}")))
                    .collect()
            }
            Special::Pkz(f, e) => {
                std::iter::once(format!("pkz:{}:{}:{}", f.root, f.rel, f.loc.len()))
                    .chain(e.iter().map(|x| format!("{x:?}")))
                    .collect()
            }
            Special::Arh(f, e) => {
                std::iter::once(format!("arh:{}:{}:{}", f.root, f.rel, f.loc.len()))
                    .chain(e.iter().map(|x| format!("{x:?}")))
                    .collect()
            }
            Special::Linkdata(f, i, lo, hi) => vec![format!(
                "linkdata:{}:{}:{}:{}:{}:{lo}:{hi}",
                f.root,
                f.rel,
                f.loc.len(),
                i.rel,
                i.loc.len()
            )],
            Special::Ue(j) => j.identity(),
            Special::Trinity(j) => j.identity(),
            Special::SmashArc(j) => j.identity(),
            Special::Cpk(j) => j.identity(),
            Special::D3(j) => j.identity(),
            Special::Casc(j) => j.identity(),
            Special::Cra(j) => j.identity(),
            Special::Pdat(j) => j.identity(),
            Special::Rpf(j) => j.identity(),
            Special::Lspk(j) => j.identity(),
        }
    }
}

/// `(root, rel)` of the files a planner's jobs read
pub type Taken = HashSet<(usize, String)>;

pub struct Plan {
    pub jobs: Vec<Special>,
    /// `(root, rel)` of the files the jobs read: they are not scanned as loose files
    pub taken: HashSet<(usize, String)>,
    /// where split.rs puts the MVGL jobs among `jobs` (the Python's `mv_jobs`: after the CPK jobs, before the Diablo III, CASC and CRA jobs)
    pub mvgl_at: usize,
    /// where split.rs puts the Frostbite jobs among `jobs` (the Python's `fb_jobs`: after the ARH jobs, before the PDAT, RPF6 and LSPK jobs)
    pub fb_at: usize,
}

/// What the planners need besides the files: the `main` of each version (`main_v`: the AES key of an encrypted Unreal pak, an RPF6 key), `DM_NRO_DIR` (the NRO that lists a CRA
/// archive) and the zstd decoder of the tables a planner reads (Super Smash Bros. Ultimate's file system table)
pub struct PlanEnv<'a> {
    pub mains: &'a [std::path::PathBuf],
    pub nro_dir: Option<&'a std::path::Path>,
    pub zstd: &'a dyn crate::zstd::Zstd,
}

impl Default for PlanEnv<'_> {
    fn default() -> Self {
        PlanEnv {
            mains: &[],
            nro_dir: None,
            zstd: &crate::zstd::Ruzstd,
        }
    }
}

fn io(e: std::io::Error) -> SplitError {
    SplitError::Io(e.to_string())
}

fn head(f: &FileRef, n: usize) -> Vec<u8> {
    f.loc
        .open()
        .ok()
        .and_then(|s| s.read(0, n).ok())
        .unwrap_or_default()
}

/// Ranges of about equal weight: `step = total / parts`, a job closes once its weight reaches it
fn chunk_by<T: Clone>(items: &[T], weight: impl Fn(&T) -> u64, parts: u64) -> Vec<Vec<T>> {
    let total: u64 = items.iter().map(&weight).sum();
    let step = total as f64 / parts as f64;
    let (mut jobs, mut cur, mut size) = (Vec::new(), Vec::new(), 0u64);
    for it in items {
        cur.push(it.clone());
        size += weight(it);
        if size as f64 >= step {
            jobs.push(std::mem::take(&mut cur));
            size = 0;
        }
    }
    if !cur.is_empty() {
        jobs.push(cur);
    }
    jobs
}

// ---- zip --------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ZipEntry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc: u32,
    pub csize: u64,
    pub size: u64,
    pub header_offset: u64,
}

/// `zipfile.ZipFile(source).infolist()`: the central directory of a classic zip (a prefix before the archive is allowed, as `zipfile` does)
pub fn zip_entries(src: &dyn Source) -> Result<Vec<ZipEntry>, String> {
    let len = src.len();
    let tail_n = (len.min(0x10000 + 22)) as usize;
    let tail = src
        .read(len - tail_n as u64, tail_n)
        .map_err(|e| e.to_string())?;
    let pos = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .ok_or("File is not a zip file")?;
    let e = &tail[pos..];
    if e.len() < 22 {
        return Err("Truncated central directory".into());
    }
    let (total, size_cd, offset_cd) = (
        u16::from_le_bytes([e[10], e[11]]) as u64,
        u32::from_le_bytes(e[12..16].try_into().unwrap()) as u64,
        u32::from_le_bytes(e[16..20].try_into().unwrap()) as u64,
    );
    if size_cd == 0xffff_ffff || offset_cd == 0xffff_ffff || total == 0xffff {
        return Err("zip64 archives are not read here".into());
    }
    let ecd_at = len - tail_n as u64 + pos as u64;
    let concat = ecd_at
        .checked_sub(size_cd + offset_cd)
        .ok_or("Bad offset for central directory")?;
    let cd = src
        .read(concat + offset_cd, size_cd as usize)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 46 <= cd.len() {
        if &cd[p..p + 4] != b"PK\x01\x02" {
            return Err("Bad magic number for central directory".into());
        }
        let u16at = |o: usize| u16::from_le_bytes([cd[p + o], cd[p + o + 1]]);
        let u32at = |o: usize| u32::from_le_bytes(cd[p + o..p + o + 4].try_into().unwrap());
        let (flags, method, crc, csize, size) = (
            u16at(8),
            u16at(10),
            u32at(16),
            u32at(20) as u64,
            u32at(24) as u64,
        );
        let (nl, xl, cl) = (u16at(28) as usize, u16at(30) as usize, u16at(32) as usize);
        let off = u32at(42) as u64;
        if csize == 0xffff_ffff || size == 0xffff_ffff || off == 0xffff_ffff {
            return Err("zip64 members are not read here".into());
        }
        let nb = cd
            .get(p + 46..p + 46 + nl)
            .ok_or("Truncated central directory")?;
        let name = if flags & 0x800 != 0 {
            String::from_utf8_lossy(nb).into_owned()
        } else {
            nb.iter().map(|&b| b as char).collect()
        };
        out.push(ZipEntry {
            name,
            method,
            flags,
            crc,
            csize,
            size,
            header_offset: off + concat,
        });
        p += 46 + nl + xl + cl;
    }
    Ok(out)
}

/// `zip_member(z, name)`: the data of the last entry of this name (`getinfo`), checked against its CRC
pub fn zip_member(src: &dyn Source, entries: &[ZipEntry], name: &str) -> Res<Vec<u8>> {
    let e = match entries.iter().rev().find(|e| e.name == name) {
        Some(e) => e,
        None => red!("KeyError: \"There is no item named '{name}' in the archive\""),
    };
    if e.method == 0 && e.csize != e.size {
        return viper_zip_member(src, e);
    }
    if e.flags & 1 != 0 {
        red!("RuntimeError: File '{name}' is encrypted, password required for extraction");
    }
    let h = src
        .read(e.header_offset, 30)
        .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
    if h.len() < 30 || &h[..4] != b"PK\x03\x04" {
        red!("BadZipFile: Bad magic number for file header");
    }
    let (nl, xl) = (
        u16::from_le_bytes([h[26], h[27]]) as u64,
        u16::from_le_bytes([h[28], h[29]]) as u64,
    );
    let lname = src
        .read(e.header_offset + 30, nl as usize)
        .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
    let lname: String = if e.flags & 0x800 != 0 {
        String::from_utf8_lossy(&lname).into_owned()
    } else {
        lname.iter().map(|&b| b as char).collect()
    };
    if lname != e.name {
        red!(
            "BadZipFile: File name in directory '{}' and header '{lname}' differ.",
            e.name
        );
    }
    let raw = src
        .read(e.header_offset + 30 + nl + xl, e.csize as usize)
        .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
    let data = match e.method {
        0 => raw,
        8 => {
            let z = crate::inflate::run(&raw, crate::inflate::Wrap::Raw, None)
                .map_err(|x| Failure::Red(format!("zlib.error: {x}")))?;
            if !z.eof {
                red!("EOFError: Compressed file ended before the end-of-stream marker was reached");
            }
            z.out
        }
        m => unsupported!("zip member compressed with method {m} ({name})"),
    };
    if data.len() as u64 != e.size || crc32fast::hash(&data) != e.crc {
        red!("BadZipFile: Bad CRC-32 for file '{name}'");
    }
    Ok(data)
}

/// `viper_zip_member(z, info)`: a member of Star Fox's data.zip the central directory calls stored at another size than its file size: a chain of 64 KB chunks, each a u32 length and an
/// LZ4 block compressed against the previous chunk's output as its dictionary (a chunk whose length equals what is left is stored raw); the CRC is the decoded data's
fn viper_zip_member(src: &dyn Source, e: &ZipEntry) -> Res<Vec<u8>> {
    const CHUNK: usize = 0x10000;
    let io = |x: std::io::Error| Failure::Red(format!("OSError: {x}"));
    let name = &e.name;
    let h = src.read(e.header_offset, 30).map_err(io)?;
    if h.len() < 4 || &h[..4] != b"PK\x03\x04" {
        red!("zip/{name}: no local header at {:#x}", e.header_offset);
    }
    if h.len() < 30 {
        red!(
            "error: unpack_from requires a buffer of at least 30 bytes for unpacking 4 bytes at offset 26 (actual buffer size is {})",
            h.len()
        );
    }
    let (nl, xl) = (
        u16::from_le_bytes([h[26], h[27]]) as u64,
        u16::from_le_bytes([h[28], h[29]]) as u64,
    );
    let d = src
        .read(e.header_offset + 30 + nl + xl, e.csize as usize)
        .map_err(io)?;
    let u = e.size as usize;
    let (mut out, mut off) = (Vec::<u8>::new(), 0usize);
    while off + 4 <= d.len() && out.len() < u {
        let n = u32::from_le_bytes(d[off..off + 4].try_into().unwrap()) as usize;
        off += 4;
        let want = CHUNK.min(u - out.len());
        if !(0 < n && n <= d.len() - off) {
            red!(
                "{name}: LZ4 chunk of {n} bytes at {:#x} runs past the member ({} bytes)",
                off - 4,
                d.len()
            );
        }
        if n == want {
            out.extend_from_slice(&d[off..off + n]);
        } else {
            let mut buf = vec![0u8; want];
            let dict = &out[out.len().saturating_sub(CHUNK)..];
            let r = if out.is_empty() {
                lz4_flex::block::decompress_into(&d[off..off + n], &mut buf)
            } else {
                lz4_flex::block::decompress_into_with_dict(&d[off..off + n], &mut buf, dict)
            };
            match r {
                Ok(k) => out.extend_from_slice(&buf[..k]),
                Err(x) => red!(
                    "{name}: LZ4 chunk at {:#x}: Decompression failed: corrupt input or insufficient space in destination buffer ({x})",
                    off - 4
                ),
            }
        }
        off += n;
    }
    if off != d.len() || out.len() != u {
        red!(
            "{name}: the LZ4 chunks make {} of {u} bytes from {off} of {}",
            out.len(),
            d.len()
        );
    }
    if crc32fast::hash(&out) != e.crc {
        red!("{name}: CRC mismatch after the LZ4 chunks");
    }
    Ok(out)
}

// ---- RE Engine KPKA ---------------------------------------------------------------------------------------------------------------

pub const KPKA: &[u8] = b"KPKA";
const KPKA_BLOCKS: u16 = 0x20;
const KPKA_IN_BLOCKS: i64 = 0x1000000;

/// `(path hash, offset, stored length, length, attributes)`
pub type KEntry = (u64, i64, i64, i64, i64);

/// `kpka_entries(path)`; None when the file is no KPKA. `Err` is `red()` in `kpka_jobs` itself (the whole split ends).
pub fn kpka_entries(src: &dyn Source) -> Result<Option<Vec<KEntry>>, String> {
    let h = src.read(0, 16).map_err(|e| e.to_string())?;
    if h.len() < 4 || &h[..4] != KPKA {
        return Ok(None);
    }
    if h.len() < 16 {
        return Err("PAK: the header is cut short".into());
    }
    let (ver, feat, n) = (
        h[4],
        u16::from_le_bytes([h[6], h[7]]),
        u32::from_le_bytes(h[8..12].try_into().unwrap()) as usize,
    );
    if ver != 4 || feat & !KPKA_BLOCKS != 0 {
        return Err(format!(
            "PAK version {ver}, features {feat:#x}: only version 4 with a plain entry table, or with a block table (0x20), is known"
        ));
    }
    let t = src.read(16, 0x30 * n).map_err(|e| e.to_string())?;
    if t.len() % 0x30 != 0 {
        return Err(
            "struct.error: iterative unpacking requires a buffer of a multiple of 48 bytes".into(),
        );
    }
    Ok(Some(
        t.chunks(0x30)
            .map(|c| {
                let q = |i: usize| i64::from_le_bytes(c[8 * i..8 * i + 8].try_into().unwrap());
                (
                    u64::from_le_bytes(c[..8].try_into().unwrap()),
                    q(1),
                    q(2),
                    q(3),
                    q(4),
                )
            })
            .collect(),
    ))
}

/// A KPKA index: (the data start, its blocks as (offset, length)).
pub type KpkaBlocks = (u64, Vec<(u64, u64)>);

/// `kpka_blocks(path)`: the block table of a PAK with feature KPKA_BLOCKS: (block length, [(offset, stored length)])
pub fn kpka_blocks(src: &dyn Source) -> Res<Option<KpkaBlocks>> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let h = src.read(0, 16).map_err(io)?;
    if h.len() < 16 || &h[..4] != KPKA || u16::from_le_bytes([h[6], h[7]]) & KPKA_BLOCKS == 0 {
        return Ok(None);
    }
    let n = u32::from_le_bytes(h[8..12].try_into().unwrap()) as u64;
    let b = src.read(16 + 0x30 * n, 8).map_err(io)?;
    if b.len() < 8 {
        return Err(short("kpka blocks"));
    }
    let (bl, nb) = (
        u32::from_le_bytes(b[..4].try_into().unwrap()) as u64,
        u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize,
    );
    let t = src.read(16 + 0x30 * n + 8, 8 * nb).map_err(io)?;
    if t.len() < 8 * nb {
        return Err(short("kpka blocks"));
    }
    Ok(Some((
        bl,
        t.chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .map(|b| (b & ((1 << 42) - 1), b >> 42))
            .collect(),
    )))
}

/// `kpka_data(d, e, head, blocks)`: an entry's data (head: only its first bytes)
fn kpka_data(
    cx: &Ctx<'_>,
    src: &dyn Source,
    e: &KEntry,
    head: Option<usize>,
    blocks: &Option<(u64, Vec<(u64, u64)>)>,
) -> Res<Vec<u8>> {
    let io = |x: std::io::Error| Failure::Red(format!("OSError: {x}"));
    let (hash, off, csz, usz, attr) = *e;
    if attr & KPKA_IN_BLOCKS != 0 {
        let (bl, bs) = match blocks {
            Some(b) => b,
            None => {
                red!("PAK entry {hash:016x}: its data is in blocks, but its PAK has no block table")
            }
        };
        if attr & 0xf != 0 {
            red!(
                "PAK entry {hash:016x}: compression {} inside blocks is not known",
                attr & 0xf
            );
        }
        let want = head.unwrap_or(usz.max(0) as usize).min(usz.max(0) as usize);
        let mut out: Vec<u8> = Vec::new();
        let mut i = off.max(0) as usize;
        while out.len() < want {
            if i >= bs.len() {
                red!(
                    "PAK entry {hash:016x}: block {i} is past the block table ({} blocks)",
                    bs.len()
                );
            }
            let (o, n) = bs[i];
            let raw = src.read(o, n as usize).map_err(io)?;
            if n == *bl {
                out.extend_from_slice(&raw);
            } else {
                out.extend_from_slice(
                    &cx.zstd
                        .decompress(&raw, None, None)
                        .map_err(|x| Failure::Red(format!("ZstdError: {x}")))?,
                );
            }
            i += 1;
        }
        out.truncate(want);
        return Ok(out);
    }
    let raw = src
        .read(off.max(0) as u64, csz.max(0) as usize)
        .map_err(io)?;
    match attr & 0xf {
        0 => Ok(match head {
            Some(h) if h != 0 => raw[..h.min(raw.len())].to_vec(),
            _ => raw,
        }),
        1 => {
            let z = crate::inflate::run(&raw, crate::inflate::Wrap::Raw, head.filter(|&h| h != 0))
                .map_err(|x| Failure::Red(format!("zlib.error: {x}")))?;
            Ok(z.out)
        }
        2 => cx
            .zstd
            .decompress(&raw, None, head.filter(|&h| h != 0))
            .map_err(|x| Failure::Red(format!("ZstdError: {x}"))),
        k => red!("PAK entry {hash:016x}: compression {k} is not known"),
    }
}

const SDF_STAGES: [St; 6] = [St::Vs, St::Hs, St::Ds, St::Gs, St::Ps, St::Cs];
/// (first entry, entry length, the stage record's lengths at, its control section at)
const SDF_LAYOUTS: [(usize, usize, usize, usize); 3] = [
    (0x10, 0x108, 0, 8),
    (0x18, 0x110, 8, 0x58),
    (0x18, 0x110, 8, 0x60),
];

fn sdf_layout(d: &[u8], n: usize, label: &str) -> Res<(usize, usize, usize, usize)> {
    let fits = |lay: &(usize, usize, usize, usize)| -> bool {
        let (base, size, _, at) = *lay;
        if base + size * n > d.len() {
            return false;
        }
        let mut offs = Vec::new();
        for i in 0..n {
            for k in 0..6 {
                match u64le(d, base + size * i + 0x10 + 8 * k) {
                    Ok(o) if o != 0 => offs.push(o as usize),
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
        }
        !offs.is_empty()
            && offs
                .iter()
                .all(|&o| o.saturating_add(at + 4) <= d.len() && eq_at(d, o + at, &CTL_MAGIC))
    };
    let lay: Vec<_> = SDF_LAYOUTS.iter().filter(|l| fits(l)).collect();
    if lay.len() != 1 {
        red!(
            "{label}: the SDF entries fit {} of the {} known layouts",
            lay.len(),
            SDF_LAYOUTS.len()
        );
    }
    Ok(*lay[0])
}

/// `sdf_shaders(d, label)`: RE Engine shader definition ('SDF\0')
pub fn sdf_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    field_ok(d, 4, 4, &format!("{label} SDF header"))?;
    let (ver, n) = (u16le(d, 4)?, u16le(d, 6)? as usize);
    let mut out = Vec::new();
    if ver == 0 && n == 0 {
        return Ok(out); // an empty definition: 16 bytes, version 0, no entries
    }
    if ver != 1 {
        red!("{label}: SDF version {ver}");
    }
    if n == 0 {
        return Ok(out);
    }
    let (base, size, lens, at) = sdf_layout(d, n, label)?;
    for i in 0..n {
        field_ok(
            d,
            base + size * i + 0x10,
            48,
            &format!("{label} SDF entry {i}"),
        )?;
        for (k, st0) in SDF_STAGES.iter().enumerate() {
            let o = u64le(d, base + size * i + 0x10 + 8 * k)? as usize;
            if o == 0 {
                continue;
            }
            let wh = format!("{label} SDF entry {i} {}", st0.short());
            field_ok(d, o + lens, 8, &wh)?;
            let (nctl, ncode) = (
                u32le(d, o + lens)? as usize,
                u32le(d, o + lens + 4)? as usize,
            );
            let b = span(d, o + at, nctl + ncode, &wh)?;
            let (ctl, code) = (&b[..nctl], &b[nctl..]);
            let mut st = *st0;
            let mut why = check(st, ctl, code)?.map(str::to_string);
            if why.is_none() && nvn2_ctl(ctl) {
                // NVN2: the stage from the control section or the SPH (nvn2_stage); PRAGMATA keeps its 1,407 mesh shaders in the hull slot
                let got = nvn2_stage(ctl, code);
                if got == Some(st) || (st == St::Hs && got == Some(St::Ms)) {
                    st = got.unwrap();
                } else {
                    why = Some(format!(
                        "the stage ({}) does not match the entry slot",
                        got.map(|s| s.short()).unwrap_or("None")
                    ));
                }
            } else if why.is_none() && st != St::Cs && sph_stage(code) != Some(st) {
                why = Some("the SPH stage does not match the entry slot".to_string());
            }
            if let Some(w) = why {
                red!("{label} SDF entry {i} {}: {w}", st.short());
            }
            out.push(Shader::new(format!("{label}#SDF#var{i}"), st, ctl, code));
        }
    }
    Ok(out)
}

/// `ocio_shaders(d, label)`: RE Engine's OpenColorIO pipelines ('OCIO'): stage records found by scanning for NVN2 control sections whose record header names their length
pub fn ocio_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let (mut out, mut other) = (Vec::new(), 0usize);
    for c in find_all(d, &CTL_MAGIC) {
        // the record header's length: its lengths are at c - hl + 8 (the Python's `for hl in (0x58, 0x60)`, a `break` on the first that fits)
        let mut chosen: Option<(usize, &[u8], &[u8])> = None;
        for hl in [0x58usize, 0x60] {
            if c < hl {
                continue;
            }
            let (nctl, ncode) = (
                u32le(d, c - hl + 8)? as usize,
                u32le(d, c - hl + 12)? as usize,
            );
            let (ctl, code) = (at(d, c, nctl), at(d, c + nctl, ncode));
            if code.len() == ncode
                && nctl != 0
                && (!nvn2_ctl(ctl) || u32le(ctl, 0x44)? as usize == ncode)
            {
                chosen = Some((hl, ctl, code));
                break;
            }
        }
        let Some((hl, ctl, code)) = chosen else {
            continue;
        };
        if !nvn2_ctl(ctl) {
            other += 1;
            continue;
        }
        match nvn2_stage(ctl, code) {
            Some(st) => {
                if let Some(w) = check(st, ctl, code)? {
                    red!("{label} OCIO record @{:#x} {}: {w}", c - hl, st.short());
                }
                out.push(Shader::new(
                    format!("{label}#OCIO@{:x}", c - hl),
                    st,
                    ctl,
                    code,
                ));
            }
            // a record of no stage would end the split's key table with a None: the Python fails there
            None => red!("{label} OCIO record @{:#x}: no stage", c - hl),
        }
    }
    if other != 0 {
        cx.print(&format!("[RED] {label}: {other} control sections of a GPU other than NVN2 in an OCIO file are left out (no such file has been read yet)"));
    }
    Ok(out)
}

// ---- FromSoftware dvdbnd ---------------------------------------------------------------------------------------------------------

fn undcx(d: &[u8], label: &str) -> Res<Vec<u8>> {
    let (n, csz) = (u32be(d, 0x1c)? as usize, u32be(d, 0x20)? as usize);
    if !eq_at(d, 0x28, b"DFLT") || !eq_at(d, 0x44, b"DCA\0") {
        red!(
            "{label}: DCX method {:?}: only DFLT is known",
            at(d, 0x28, 4)
        );
    }
    let out = crate::inflate::zlib_decompress(at(d, 0x4c, csz))
        .map_err(|e| Failure::Red(format!("zlib.error: {e}")))?;
    if out.len() != n {
        red!(
            "{label}: DCX holds {:#x} bytes, its header says {n:#x}",
            out.len()
        );
    }
    Ok(out)
}

fn bnd3_members(d: &[u8], label: &str) -> Res<Vec<(String, Vec<u8>)>> {
    let (f, big) = (u8at(d, 12)?, u8at(d, 13)?);
    if (f != 0x5c && f != 0x7c) || big != 0 {
        red!(
            "{label}: BND3 format {f:#x}, big-endian {big}: only formats 0x5c and 0x7c, little-endian, are known"
        );
    }
    let mut out = Vec::new();
    for i in 0..i32le(d, 0x10)?.max(0) as usize {
        let b = 0x20 + 28 * i;
        // <B3xiQiIi: flags, 3 pad, i32 length, u64 offset, i32 id, u32 name offset, i32 unpacked length
        let (n, off, nm, usz) = (
            i32le(d, b + 4)?,
            u64le(d, b + 8)? as usize,
            u32le(d, b + 20)? as usize,
            i32le(d, b + 24)?,
        );
        if n != usz {
            red!("{label}: BND3 entry {i} is compressed ({n:#x} of {usz:#x} bytes)");
        }
        let e = match find(d, &[0], nm) {
            Some(e) => e,
            None => red!("ValueError: subsection not found"),
        };
        // Shift JIS paths: ASCII names decode as they are
        let name = String::from_utf8_lossy(&d[nm..e]).replace('\\', "/");
        let name = name
            .rsplit_once("/Source/")
            .map(|x| x.1.to_string())
            .unwrap_or(name);
        out.push((name, at(d, off, n.max(0) as usize).to_vec()));
    }
    Ok(out)
}

// ---- Monolith ARH ------------------------------------------------------------------------------------------------------------------

/// `arh_entries(d)`: Monolith ARH1/ARH2 index → (path, ARD offset, stored size)
pub fn arh_entries(d: &[u8]) -> Result<Vec<(String, u64, u64)>, String> {
    let err = |m: &str| Err(m.to_string());
    let rd = |o: usize| u32le(d, o).map_err(|e| e.to_string());
    if starts(d, b"arh2") {
        if d.len() < 16 {
            return err("ARH2: truncated header");
        }
        let (count, align) = (rd(4)? as usize, rd(8)? as u64);
        if align == 0 || align & (align - 1) != 0 {
            return err("ARH2: invalid file alignment");
        }
        if count > (d.len() - 16) / 16 {
            return err("ARH2: index table out of bounds");
        }
        let (mut out, mut off) = (Vec::new(), 0u64);
        for i in 0..count {
            let key = u64le(d, 16 + i * 16).map_err(|e| e.to_string())?;
            let size = rd(24 + i * 16)? as u64;
            if size != 0 {
                out.push((format!("/#{key:016x}"), off, size));
            }
            off += (size + align - 1) & !(align - 1);
        }
        return Ok(out);
    }
    if d.len() < 48 || !starts(d, b"arh1") {
        return Ok(vec![]);
    }
    let (count, so, sn, no, nn, fo, fn_, key) = (
        rd(8)? as usize,
        rd(12)? as usize,
        rd(16)? as usize,
        rd(20)? as usize,
        rd(24)? as usize,
        rd(28)? as usize,
        rd(32)? as usize,
        rd(36)?,
    );
    let oob = |o: usize, n: usize| o > d.len() || n > d.len() - o;
    if nn != count * 8 || oob(so, sn) || oob(no, nn) || oob(fo, fn_ * 24) {
        return err("ARH1: index table out of bounds");
    }
    let key = (key ^ 0xf3f3_5353).to_le_bytes();
    // the format encrypts whole u32 words; a final partial word stays plain
    let dec = |o: usize, n: usize| -> Vec<u8> {
        let mut v: Vec<u8> = d[o..o + (n & !3)]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key[i & 3])
            .collect();
        v.extend_from_slice(&d[o + (n & !3)..o + n]);
        v
    };
    let strings = dec(so, sn);
    let nodes: Vec<(i32, i32)> = dec(no, nn)
        .chunks(8)
        .map(|c| {
            (
                i32::from_le_bytes(c[..4].try_into().unwrap()),
                i32::from_le_bytes(c[4..8].try_into().unwrap()),
            )
        })
        .collect();
    let mut names: BTreeMap<u32, String> = BTreeMap::new();
    for (i, &(nxt, prev)) in nodes.iter().enumerate() {
        if nxt >= 0 || prev < 0 {
            continue;
        }
        let off = (-(nxt as i64)) as usize;
        let end = match strings
            .get(off..)
            .and_then(|s| s.iter().position(|&b| b == 0))
            .map(|p| p + off)
        {
            Some(e) if e + 5 <= strings.len() && strings[off..e].is_ascii() => e,
            _ => return err("ARH1: invalid path suffix"),
        };
        let fid = u32::from_le_bytes(strings[end + 1..end + 5].try_into().unwrap());
        let (mut prefix, mut cur, mut seen) = (Vec::new(), i, HashSet::new());
        while cur != 0 {
            if !seen.insert(cur) || cur >= nodes.len() {
                return err("ARH1: invalid path trie");
            }
            let parent = nodes[cur].1;
            if parent < 0 || parent as usize >= nodes.len() {
                return err("ARH1: invalid path parent");
            }
            let ch = cur as i64 ^ nodes[parent as usize].0 as i64;
            if !(0..128).contains(&ch) {
                return err("ARH1: invalid path character");
            }
            prefix.push(ch as u8);
            cur = parent as usize;
        }
        prefix.reverse();
        prefix.extend_from_slice(&strings[off..end]);
        names.insert(fid, String::from_utf8_lossy(&prefix).into_owned());
    }
    let mut out = Vec::new();
    for i in 0..fn_ {
        let (off, size, flags, fid) = (
            u64le(d, fo + i * 24).map_err(|e| e.to_string())?,
            rd(fo + i * 24 + 8)? as u64,
            rd(fo + i * 24 + 16)?,
            rd(fo + i * 24 + 20)?,
        );
        // HasXbc1Header (bit 1): stored size excludes the 48-byte wrapper
        if size != 0 {
            out.push((
                names
                    .get(&fid)
                    .cloned()
                    .unwrap_or_else(|| format!("#{fid}")),
                off,
                size + if flags & 2 != 0 { 48 } else { 0 },
            ));
        }
    }
    Ok(out)
}

// ---- planning --------------------------------------------------------------------------------------------------------------------

/// The jobs of the archives among `files`, in the Python's `plan_jobs` order (linkdata, [ea], kpka, trpfs, ue, bdt, smash_arc, pkz, cpk, [mvgl], d3cpk, casc, cra, zip, arh,
/// [frostbite], pdat, rpf, lspk; then kit, ttdat, falcom_pka); the bracketed ones are planned in split.rs and put at their place there ([`Plan::mvgl_at`], [`Plan::fb_at`])
pub fn plan(files: &[FileRef], log: &(dyn Fn(&str) + Sync)) -> Result<Plan, SplitError> {
    plan_env(
        files,
        log,
        &|f: &FileRef| f.rel.clone(),
        &PlanEnv::default(),
    )
}

/// [`plan`], `path_of` naming a file as the Python's messages do (the RomFS argument and the path below it: the Unreal pak reader's), with what [`PlanEnv`] gives the planners
pub fn plan_env(
    files: &[FileRef],
    log: &(dyn Fn(&str) + Sync),
    path_of: &dyn Fn(&FileRef) -> String,
    env: &PlanEnv<'_>,
) -> Result<Plan, SplitError> {
    let mut jobs: Vec<Special> = Vec::new();
    let mut taken: HashSet<(usize, String)> = HashSet::new();
    let key = |f: &FileRef| (f.root, f.rel.clone());

    // linkdata_jobs: a LINKDATA archive's index (32-byte entries {u64 offset, u64 inflated length, u64 stored length, u64 compressed}) split into ranges of about equal stored size
    for f in files {
        let base = f.rel.rsplit('/').next().unwrap_or(&f.rel);
        let dir = &f.rel[..f.rel.len() - base.len()];
        let idx_name = if base == "DATA1.bin" {
            Some("DATA0.bin".to_string())
        } else if base == "LINKDATA.BIN" {
            Some("LINKDATA.IDX".to_string())
        } else if base.starts_with("LD_") && base.ends_with(".BIN") {
            Some(format!("LI_{}.IDX", &base[3..base.len() - 4]))
        } else {
            None
        };
        let Some(idx_name) = idx_name else { continue };
        let Some(il) = files
            .iter()
            .find(|x| x.root == f.root && x.rel == format!("{dir}{idx_name}"))
        else {
            continue;
        };
        let n = (il.loc.len() / 32) as usize;
        let step = f.loc.len() as f64 / PARTS as f64;
        let raw = il.loc.open().and_then(|s| s.read(0, n * 32)).map_err(io)?;
        // `read` clamps to what the file holds (a truncated image, a file cut after it was listed): fewer bytes than the length said is a damaged index, as `struct.unpack_from` raises in the Python
        if raw.len() < n * 32 {
            return Err(SplitError::Input(format!(
                "{}: the index holds {} bytes, {} short of the {} its length says",
                il.rel,
                raw.len(),
                n * 32 - raw.len(),
                n * 32
            )));
        }
        let first = |i: usize| u64::from_le_bytes(raw[32 * i..32 * i + 8].try_into().unwrap());
        let (mut lo, mut njobs) = (0usize, 0usize);
        for i in 1..=n {
            if i == n || first(i) as f64 >= (njobs + 1) as f64 * step {
                jobs.push(Special::Linkdata(f.clone(), il.clone(), lo, i));
                njobs += 1;
                lo = i;
            }
        }
        if njobs > 0 {
            taken.insert(key(f)); // `not linkdata_jobs(p, r)`: an empty list leaves the file in the plain list
        }
    }

    // kpka_jobs: the game reads its PAKs as one view: a path in a later patch PAK replaces the same path hash in earlier ones
    let mut paks: Vec<(&FileRef, Vec<KEntry>)> = Vec::new();
    for f in files {
        if f.rel.ends_with(".pak") {
            let src = f.loc.open().map_err(io)?;
            if let Some(e) =
                kpka_entries(&src).map_err(|m| SplitError::Red(format!("{}: {m}", f.rel)))?
            {
                paks.push((f, e));
            }
        }
    }
    if !paks.is_empty() {
        let mut order: Vec<usize> = (0..paks.len()).collect();
        order.sort_by(|&a, &b| paks[a].0.rel.cmp(&paks[b].0.rel));
        let mut view: BTreeMap<u64, (usize, KEntry)> = BTreeMap::new();
        for &pi in &order {
            for e in &paks[pi].1 {
                view.insert(e.0, (pi, *e));
            }
        }
        let mut items: Vec<(usize, KEntry)> = view.into_values().collect();
        items.sort_by(|a, b| (&paks[a.0].0.rel, a.1.1).cmp(&(&paks[b.0].0.rel, b.1.1)));
        let total: i64 = items.iter().map(|x| x.1.2).sum();
        let step = total as f64 / PARTS as f64;
        let (mut cur, mut size): (Vec<(usize, Vec<KEntry>)>, i64) = (Vec::new(), 0);
        let flush = |cur: &mut Vec<(usize, Vec<KEntry>)>, jobs: &mut Vec<Special>| {
            for (pi, es) in cur.drain(..) {
                jobs.push(Special::Kpka(paks[pi].0.clone(), es));
            }
        };
        for (pi, e) in items {
            match cur.iter_mut().find(|(p, _)| *p == pi) {
                Some((_, v)) => v.push(e),
                None => cur.push((pi, vec![e])),
            }
            size += e.2;
            if size as f64 >= step {
                flush(&mut cur, &mut jobs);
                size = 0;
            }
        }
        flush(&mut cur, &mut jobs);
        for (f, _) in &paks {
            taken.insert(key(f));
        }
    }

    // trpfs_jobs (after kpka_jobs): Game Freak's Trinity packs, data.trpfs read through data.trpfd
    {
        let (tr, tr_taken) = crate::trinity::plan(files)?;
        jobs.extend(tr.into_iter().map(Special::Trinity));
        taken.extend(tr_taken);
    }

    // ue_legacy_jobs (after kpka_jobs and trpfs_jobs, before bdt_jobs, as the Python plans them): Unreal paks with inline shaders
    {
        let (ue, ue_taken) = crate::uepak::legacy_jobs(files, path_of, log, env.mains)?;
        jobs.extend(ue.into_iter().map(Special::Ue));
        taken.extend(ue_taken);
    }

    // bdt_jobs: the entries of every dvdbnd archive by its .bhd5 index
    for f in files {
        if !f.rel.ends_with(".bdt") {
            continue;
        }
        let idx = format!("{}.bhd5", &f.rel[..f.rel.len() - 4]);
        let Some(il) = files.iter().find(|x| x.root == f.root && x.rel == idx) else {
            continue;
        };
        let d = il.loc.open().and_then(|s| s.read_all()).map_err(io)?;
        if !starts(&d, b"BHD5\xff") {
            return Err(SplitError::Red(format!(
                "{idx}: not a little-endian BHD5 index"
            )));
        }
        let rd = |r: Res<u64>| r.map_err(|e| SplitError::Red(e.to_string()));
        let (nb, tab) = (
            u32le(&d, 0x10).map_err(|e| SplitError::Red(e.to_string()))? as usize,
            rd(u64le(&d, 0x18))? as usize,
        );
        let mut ents: Vec<(u32, u64, u64)> = Vec::new();
        for i in 0..nb {
            let (n, at_) = (
                u32le(&d, tab + 16 * i).map_err(|e| SplitError::Red(e.to_string()))? as usize,
                rd(u64le(&d, tab + 16 * i + 8))? as usize,
            );
            for k in 0..n {
                let o = at_ + 16 * k;
                if o + 16 > d.len() {
                    break; // iter_unpack over a short slice drops the tail
                }
                ents.push((
                    u32le(&d, o).unwrap(),
                    u32le(&d, o + 4).unwrap() as u64,
                    u64le(&d, o + 8).unwrap(),
                ));
            }
        }
        taken.insert(key(f));
        ents.sort_by_key(|e| e.2);
        // the entries are (hash, size, offset) in the index; the job keeps (hash, offset, size)
        let ents: Vec<(u32, u64, u64)> = ents
            .into_iter()
            .map(|(h, size, off)| (h, off, size))
            .collect();
        for c in chunk_by(&ents, |e| e.2, PARTS) {
            jobs.push(Special::Bdt(f.clone(), c));
        }
    }

    // smash_arc_jobs (after bdt_jobs, before pkz_jobs): Super Smash Bros. Ultimate's data.arc, its current files in ranges
    {
        let (arc, arc_taken) = crate::smash_arc::plan(files, env.zstd, path_of)?;
        jobs.extend(arc.into_iter().map(Special::SmashArc));
        taken.extend(arc_taken);
    }

    // pkz_jobs
    for f in files {
        if !f.rel.ends_with(".pkz") {
            continue;
        }
        let src = f.loc.open().map_err(io)?;
        let h = src.read(0, 0x20).map_err(io)?;
        if !starts(&h, b"pkzl") {
            continue;
        }
        let w = |o: usize| u32le(&h, o).map_err(|e| SplitError::Red(e.to_string()));
        let (ver, n, eo, nl) = (w(4)?, w(16)? as usize, w(20)? as u64, w(24)? as usize);
        if ver != 0x10000 && ver != 0x20000 {
            return Err(SplitError::Red(format!(
                "/{}: PKZ version {ver:#x}: only 0x10000 and 0x20000 are known",
                f.rel
            )));
        }
        let t = src.read(eo, 0x20 * n + nl).map_err(io)?;
        let nm = |o: usize| -> Result<String, SplitError> {
            let s = 0x20 * n + o;
            let e = find(&t, &[0], s)
                .ok_or_else(|| SplitError::Red("ValueError: subsection not found".into()))?;
            Ok(String::from_utf8_lossy(&t[s..e]).into_owned())
        };
        let mut ents = Vec::new();
        for i in 0..n {
            let c = t
                .get(0x20 * i..0x20 * i + 0x20)
                .ok_or_else(|| SplitError::Red("struct.error: PKZ entry table".into()))?;
            let (a, b) = (
                u32::from_le_bytes(c[..4].try_into().unwrap()) as usize,
                u32::from_le_bytes(c[4..8].try_into().unwrap()) as usize,
            );
            let (usz, off, csz) = (
                u64::from_le_bytes(c[8..16].try_into().unwrap()),
                u64::from_le_bytes(c[16..24].try_into().unwrap()),
                u64::from_le_bytes(c[24..32].try_into().unwrap()),
            );
            ents.push((nm(a)?, nm(b)?, off, csz, usz));
        }
        taken.insert(key(f));
        let total: u64 = ents.iter().map(|e| e.3).sum();
        let step = (total as f64 / PARTS as f64).max(0.0);
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in ents {
            size += e.3;
            cur.push(e);
            if size as f64 >= step {
                jobs.push(Special::Pkz(f.clone(), std::mem::take(&mut cur)));
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(Special::Pkz(f.clone(), cur));
        }
    }

    // cpk_jobs; mvgl_jobs go here (split.rs); then d3cpk_jobs (cpk.rs), casc_jobs and cra_jobs
    let mvgl_at;
    {
        let (cj, ct) = crate::cpk::cpk_jobs(files, path_of, log)?;
        jobs.extend(cj.into_iter().map(Special::Cpk));
        taken.extend(ct);
        mvgl_at = jobs.len();
        let (dj, dt) = crate::cpk::d3cpk_jobs(files)?;
        jobs.extend(dj.into_iter().map(Special::D3));
        taken.extend(dt);
        let (cj, ct) = crate::casc::plan(files, log, path_of)?;
        jobs.extend(cj.into_iter().map(Special::Casc));
        taken.extend(ct);
        let (rj, rt) = crate::cra::plan(files, env.nro_dir)?;
        jobs.extend(rj.into_iter().map(Special::Cra));
        taken.extend(rt);
    }

    // zip_jobs
    let mut zs: Vec<(&FileRef, Vec<(String, u64)>)> = Vec::new();
    for f in files {
        let h = head(f, 4);
        if h != b"PK\x03\x04" && h != b"VZIP" {
            continue;
        }
        let src = f.loc.open().map_err(io)?;
        match zip_entries(&src) {
            Ok(es) => zs.push((
                f,
                es.iter()
                    .filter(|e| !e.name.ends_with('/'))
                    .map(|e| (e.name.clone(), e.size))
                    .collect(),
            )),
            Err(e) => log(&format!(
                "[RED] /{}: the zip cannot be read ({e}); scanned as one file",
                f.rel
            )),
        }
    }
    if !zs.is_empty() {
        let total: u64 = zs.iter().flat_map(|z| z.1.iter().map(|e| e.1)).sum();
        let step = total as f64 / PARTS as f64;
        for (f, es) in &zs {
            let (mut cur, mut size) = (Vec::new(), 0u64);
            for (nm, n) in es {
                cur.push(nm.clone());
                size += n;
                if size as f64 >= step {
                    jobs.push(Special::Zip((*f).clone(), std::mem::take(&mut cur)));
                    size = 0;
                }
            }
            if !cur.is_empty() {
                jobs.push(Special::Zip((*f).clone(), cur));
            }
            taken.insert(key(f));
        }
    }

    // arh_jobs
    for f in files {
        if !f.rel.ends_with(".arh") {
            continue;
        }
        let d = f.loc.open().and_then(|s| s.read_all()).map_err(io)?;
        if !starts(&d, b"arh1") && !starts(&d, b"arh2") {
            continue;
        }
        let ard_rel = format!("{}.ard", &f.rel[..f.rel.len() - 4]);
        let Some(ard) = files.iter().find(|x| x.root == f.root && x.rel == ard_rel) else {
            return Err(SplitError::Red(format!(
                "{}: ARH index has no ARD data file",
                f.rel
            )));
        };
        let entries = arh_entries(&d).map_err(SplitError::Red)?;
        let length = ard.loc.len();
        if entries
            .iter()
            .any(|(_, o, n)| *o > length || *n > length - *o)
        {
            return Err(SplitError::Red(format!(
                "{}: ARD entry out of bounds",
                f.rel
            )));
        }
        let step = entries.len().div_ceil(PARTS as usize).max(1);
        for c in entries.chunks(step) {
            jobs.push(Special::Arh(ard.clone(), c.to_vec()));
        }
        taken.insert(key(f));
        taken.insert(key(ard));
    }

    // frostbite_jobs go here (split.rs); then pdat_jobs, rpf_jobs, lspk_jobs; kit_jobs, ttdat_jobs and falcom_pka_jobs follow in split.rs
    let fb_at = jobs.len();
    {
        let (pj, pt) = crate::pdat::plan(files);
        jobs.extend(pj.into_iter().map(Special::Pdat));
        taken.extend(pt);
        let mains: Vec<&std::path::Path> = env.mains.iter().map(|m| m.as_path()).collect();
        let (rj, rt) = crate::rpf6::plan(files, &mains, path_of)?;
        jobs.extend(rj.into_iter().map(Special::Rpf));
        taken.extend(rt);
        let (lj, lt) = crate::lspk::plan(files, path_of);
        jobs.extend(lj.into_iter().map(Special::Lspk));
        taken.extend(lt);
    }
    Ok(Plan {
        jobs,
        taken,
        mvgl_at,
        fb_at,
    })
}

// ---- running a job -------------------------------------------------------------------------------------------------------------------

/// `scan_file(args)` of a special job: an unreadable member ends the job, not the split; an unsupported one ends the split
pub fn scan_special(cx: &Ctx<'_>, job: &Special) -> Result<FileResult, String> {
    match run(cx, job) {
        Ok(r) => Ok(r),
        Err(Failure::Unsupported(m)) => Err(m),
        Err(e) => {
            let rel = job.file().rel.clone();
            let msg = e.to_string();
            let why = if msg
                .trim_start_matches('/')
                .starts_with(rel.trim_start_matches('/'))
            {
                msg
            } else {
                format!("{rel}: {msg}")
            };
            cx.print(&format!("[RED] {why} (its shaders are left out)"));
            Ok(FileResult {
                rel,
                uniq: vec![],
                pairs: vec![],
                extra: crate::scan_types::Extra::Unreadable { why, count: 1 },
            })
        }
    }
}

/// the result of one special job (`scan_file_in` with a job tag): the shaders of its members, as one file's result named after the archive
pub fn run(cx: &Ctx<'_>, job: &Special) -> Result<FileResult, Failure> {
    if let Special::Ue(j) = job {
        // `lost or trees`: the packages that did not read are counted, the shaders of the others kept
        let mut out = Vec::new();
        let lost = crate::uepak::legacy_shaders(cx, j, &mut out)?;
        let mut r = finish(cx, &j.file.rel, out, std::iter::empty(), Trees::new())?;
        if let Some((why, count)) = lost {
            r.extra = crate::scan_types::Extra::Unreadable { why, count };
        }
        return Ok(r);
    }
    if let Special::Cpk(j) = job {
        let out = crate::cpk::cpk_shaders(cx, j)?;
        return finish(cx, &j.file.rel, out, std::iter::empty(), Trees::new());
    }
    if let Special::D3(j) = job {
        let out = crate::cpk::d3cpk_shaders(cx, j)?;
        return finish(cx, &j.file.rel, out, std::iter::empty(), Trees::new());
    }
    let (f, found) = match job {
        Special::Zip(f, names) => {
            let src = f
                .loc
                .open()
                .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
            let entries =
                zip_entries(&src).map_err(|e| Failure::Red(format!("BadZipFile: {e}")))?;
            let mut out = Vec::new();
            for nm in names {
                let d = zip_member(&src, &entries, nm)?;
                let n = out.len();
                scan_blob(cx, &d, &format!("/{}/{nm}", f.rel), &mut out, 0, None)?;
                if out.len() == n && !starts(&d, b"MSHD") && contains(&d, &GLSLC_MAGIC) {
                    out.extend(glslc_shaders(&d, &format!("/{}/{nm}", f.rel), false, &[])?);
                }
            }
            (f, out)
        }
        Special::Kpka(f, ents) => {
            let src = f
                .loc
                .open()
                .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
            let blocks = kpka_blocks(&src)?;
            let mut out = Vec::new();
            for e in ents {
                if e.3 < 0x10 {
                    continue;
                }
                let h = kpka_data(cx, &src, e, Some(4), &blocks)?;
                let label = format!("/{}#{:016x}", f.rel, e.0);
                if h == b"SDF\0" {
                    out.extend(sdf_shaders(
                        &kpka_data(cx, &src, e, None, &blocks)?,
                        &label,
                    )?);
                } else if h == b"OCIO" {
                    out.extend(ocio_shaders(
                        cx,
                        &kpka_data(cx, &src, e, None, &blocks)?,
                        &label,
                    )?);
                }
            }
            (f, out)
        }
        Special::Bdt(f, ents) => {
            let src = f
                .loc
                .open()
                .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
            let mut out = Vec::new();
            for (h, off, n) in ents {
                let label = format!("/{}#{h:08x}", f.rel);
                let mut d = src
                    .read(*off, *n as usize)
                    .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
                if starts(&d, b"DCX\0") {
                    d = undcx(&d, &label)?;
                }
                if !starts(&d, b"BND3") || !contains(&d, &GLSLC_MAGIC) {
                    continue; // the game's other BND3 archives use other formats (0x78) and hold no shaders
                }
                for (nm, b) in bnd3_members(&d, &label)? {
                    if starts(&b, &GLSLC_MAGIC) {
                        out.extend(glslc_shaders(&b, &format!("{label}/{nm}"), true, &[])?);
                    }
                }
            }
            (f, out)
        }
        Special::Pkz(f, ents) => {
            let src = f
                .loc
                .open()
                .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
            let mut out = Vec::new();
            for (name, comp, off, csz, usz) in ents {
                let c = src
                    .read(*off, *csz as usize)
                    .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
                let d: Vec<u8> = match comp.as_str() {
                    "None" => c,
                    "OodleKraken" => unsupported!(
                        "Oodle Kraken (PKZ entry /{}/{name}: the game's own decoder in the Mac host)",
                        f.rel
                    ),
                    "ZStandard" => cx
                        .zstd
                        .decompress(&c, None, None)
                        .map_err(|e| Failure::Red(format!("ZstdError: {e}")))?,
                    other => red!("/{}/{name}: compression {other:?} is not known", f.rel),
                };
                if d.len() as u64 != *usz {
                    red!(
                        "/{}/{name}: {} bytes decoded, the entry says {usz}",
                        f.rel,
                        d.len()
                    );
                }
                scan_blob(cx, &d, &format!("/{}/{name}", f.rel), &mut out, 0, None)?;
            }
            (f, out)
        }
        Special::Linkdata(f, il, lo, hi) => {
            let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
            let idx = il.loc.open().and_then(|s| s.read_all()).map_err(io)?;
            let src = f.loc.open().map_err(io)?;
            let mut out = Vec::new();
            for i in *lo..*hi {
                let (off, n, stored, comp) = (
                    u64le(&idx, i * 32)?,
                    u64le(&idx, i * 32 + 8)?,
                    u64le(&idx, i * 32 + 16)?,
                    u64le(&idx, i * 32 + 24)?,
                );
                if n != 0 {
                    let d = src
                        .read(off, if comp != 0 { stored } else { n } as usize)
                        .map_err(io)?;
                    kt_shaders(cx, &d, &format!("/{}#{i}", f.rel), &mut out)?;
                }
            }
            (f, out)
        }
        Special::Arh(f, ents) => {
            let src = f
                .loc
                .open()
                .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
            let mut out = Vec::new();
            for (name, off, size) in ents {
                let d = src
                    .read(*off, *size as usize)
                    .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
                monolith_shaders(cx, &d, &format!("/{}{name}", f.rel), &mut out, 0)?;
            }
            (f, out)
        }
        Special::Trinity(j) => (&j.file, crate::trinity::shaders(cx, j)?),
        Special::SmashArc(j) => (&j.file, crate::smash_arc::shaders(cx, j)?),
        Special::Casc(j) => (&j.file, crate::casc::shaders(cx, j)?),
        Special::Cra(j) => (&j.file, crate::cra::shaders(cx, j)?),
        Special::Pdat(j) => (&j.file, crate::pdat::shaders(cx, j)?),
        Special::Rpf(j) => (&j.file, crate::rpf6::shaders(cx, j)?),
        Special::Lspk(j) => (&j.file, crate::lspk::shaders(cx, j)?),
        Special::Ue(_) | Special::Cpk(_) | Special::D3(_) => {
            red!("an Unreal pak or CPK job reached the archive readers") // handled above
        }
    };
    // the arh job is named after its ARD: `rel[:-4] + '.ard'` is the file's own rel here
    finish(cx, &f.rel, found, std::iter::empty(), Trees::new())
}
