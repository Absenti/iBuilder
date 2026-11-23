//! Nihon Falcom's `.pkg` packages and PH3/Falcom's deduplicated `.pka` archives (The Legend of Heroes: Trails into Reverie, `01008CB0156BC000`): `falcom_decompress`, `falcom_pkg`,
//! `falcom_pkg_shaders`, `falcom_pka_index`, `falcom_pka_jobs` and `falcom_pka_shaders` of `shader_split.py` (nxnative 930e9195).
//!
//! `.pkg` (no magic, recognised by its table): u32 0, u32 entry count n, n x 0x50-byte entries {name (64 bytes NUL-padded), u32 length, u32 stored length, u32 offset from the file's start,
//! u32 flags}, the first entry's data right after the table. Flags 0 stored, 1 one raw LZ4 block, 0x10 one zstd frame; any other (9 with stored length 0: external data) is counted in one
//! `[WARN]` line per package. `.pka` (magic 0x7ff7cf0d): u32 magic, u32 package count, per package {char[32] name, u32 entry count, per entry {char[64] name, u8[32] SHA-256}}, u32 file count,
//! per file {u8[32] SHA-256, u64 offset, u32 stored length, u32 length, u32 flags}, then the data. Each blob is read once, under the first package entry that names it; textures, meshes and
//! manifests (`.dds.phyre`, `.dae.phyre`, `.xml`) are not read. Every entry read is scanned like a file (`scan_blob`, then whole GLSLC outputs). A `.pka` is never read whole: its entries
//! are planned in jobs of about equal stored size after the KIT jobs, as the Python's `pk_jobs` come last.

use crate::bytes::{u32le, u64le};
use crate::ctl::GLSLC_MAGIC;
use crate::error::{Failure, Res};
use crate::glslc::glslc_shaders;
use crate::red;
use crate::scan::{FileRef, finish, scan_blob};
use crate::scan_types::{Ctx, Extra, FileResult, Shader};
use crate::source::Source;
use crate::unity::Trees;
use crate::zstd::ZSTD_MAGIC;
use std::collections::{BTreeMap, HashMap, HashSet};

/// `FALCOM_PKA`: 0x7ff7cf0d
pub const PKA_MAGIC: [u8; 4] = [0x0d, 0xcf, 0xf7, 0x7f];
/// `FALCOM_SKIP_EXT`
const SKIP_EXT: [&str; 3] = [".dds.phyre", ".dae.phyre", ".xml"];

fn skipped(name: &str) -> bool {
    let l = name.to_lowercase();
    SKIP_EXT.iter().any(|e| l.ends_with(e))
}

/// `falcom_decompress(raw, usz, flags, where)`: `None` for a storage flag this reader does not know
pub fn falcom_decompress(
    cx: &Ctx<'_>,
    raw: Vec<u8>,
    usz: usize,
    flags: u32,
    wh: &str,
) -> Res<Option<Vec<u8>>> {
    match flags {
        0 => {
            if raw.len() != usz {
                red!(
                    "{wh}: {:#x} stored bytes, the entry says {usz:#x}",
                    raw.len()
                );
            }
            Ok(Some(raw))
        }
        1 => {
            if usz as u128 > 255 * raw.len() as u128 {
                red!(
                    "{wh}: {usz:#x} bytes from {:#x} stored is more than an LZ4 block can hold (255 to 1)",
                    raw.len()
                );
            }
            if usz > crate::inflate::MAX_DECODED {
                red!(
                    "{wh}: {}",
                    crate::inflate::cap_error(crate::inflate::MAX_DECODED)
                );
            }
            let mut m = vec![0u8; usz];
            match lz4_flex::block::decompress_into(&raw, &mut m) {
                Ok(k) => m.truncate(k),
                Err(e) => red!(
                    "{wh}: its {:#x} stored bytes are not an LZ4 block (Decompression failed: corrupt input or insufficient space in destination buffer ({e}))",
                    raw.len()
                ),
            }
            if m.len() != usz {
                red!("{wh}: {:#x} bytes, the entry says {usz:#x}", m.len());
            }
            Ok(Some(m))
        }
        0x10 => {
            if !raw.starts_with(&ZSTD_MAGIC) {
                red!("{wh}: flags 0x10 entry does not start with a zstd frame");
            }
            if usz > crate::inflate::MAX_DECODED {
                red!(
                    "{wh}: {}",
                    crate::inflate::cap_error(crate::inflate::MAX_DECODED)
                );
            }
            // `ZstdDecompressor().decompress(raw, max_length=usz)` reads one frame: what follows it is unused data. A frame cut short is no error there, only not at its end
            let mismatch =
                || Failure::Red(format!("{wh}: zstd stream length does not match {usz:#x}"));
            let Some(n) = crate::zstd::frame_len(&raw, 0).filter(|&n| n <= raw.len()) else {
                return Err(mismatch());
            };
            let m = cx
                .zstd
                .decompress(&raw[..n], None, Some(usz.saturating_add(1)))
                .map_err(|e| Failure::Red(format!("{wh}: invalid zstd frame ({e})")))?;
            if m.len() != usz {
                return Err(mismatch());
            }
            Ok(Some(m))
        }
        _ => Ok(None),
    }
}

/// One `.pkg` entry: `(name, length, stored length, offset, flags)`
pub type PkgEntry = (String, u32, u32, u32, u32);

/// `falcom_pkg(d)` over a file read through its table: the entries when the table holds, else `None` (the file goes on through the other readers)
pub fn falcom_pkg(src: &dyn Source) -> Res<Option<Vec<PkgEntry>>> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let len = src.len();
    if len < 8 + 0x50 {
        return Ok(None);
    }
    let h = src.read(0, 8).map_err(io)?;
    let (zero, n) = (u32le(&h, 0)?, u32le(&h, 4)? as u64);
    if zero != 0 || n == 0 || n > (len - 8) / 0x50 {
        return Ok(None);
    }
    let table_end = 8 + 0x50 * n;
    // checked before the table is read: plain files that start with a u32 0 leave here
    if u32le(&src.read(8 + 0x48, 4).map_err(io)?, 0)? as u64 != table_end {
        return Ok(None);
    }
    let t = src.read(8, (0x50 * n) as usize).map_err(io)?;
    let mut es = Vec::with_capacity(n as usize);
    for o in (0..t.len()).step_by(0x50) {
        let (usz, csz, off, fl) = (
            u32le(&t, o + 0x40)?,
            u32le(&t, o + 0x44)?,
            u32le(&t, o + 0x48)?,
            u32le(&t, o + 0x4c)?,
        );
        if off as u64 > len
            || csz as u64 > len - off as u64
            || (csz != 0 && (off as u64) < table_end)
        {
            return Ok(None);
        }
        let raw = &t[o..o + 0x40];
        let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
        es.push((
            String::from_utf8_lossy(&raw[..end]).into_owned(),
            usz,
            csz,
            off,
            fl,
        ));
    }
    Ok(Some(es))
}

/// The `[WARN]` of the entries whose storage flags are not known, one per package
fn warn_other(cx: &Ctx<'_>, label: &str, other: &BTreeMap<u32, usize>) {
    if other.is_empty() {
        return;
    }
    cx.print(&format!(
        "[WARN] {label}: {} entries not read, their storage flags ({}) are not ones this reader knows (0 = stored, 1 = LZ4, 0x10 = zstd)",
        other.values().sum::<usize>(),
        other
            .iter()
            .map(|(f, n)| format!("{f:#x} \u{d7} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

/// scan_blob, then whole GLSLC outputs when it found nothing in an entry that starts with one
fn scan_entry(cx: &Ctx<'_>, m: &[u8], label: &str, out: &mut Vec<Shader>) -> Res<()> {
    let before = out.len();
    scan_blob(cx, m, label, out, 0, None)?;
    if out.len() == before && m.starts_with(&GLSLC_MAGIC) {
        out.extend(glslc_shaders(m, label, false, &[])?);
    }
    Ok(())
}

/// `falcom_pkg_shaders(d, entries, label, out)`
pub fn falcom_pkg_shaders(
    cx: &Ctx<'_>,
    src: &dyn Source,
    entries: &[PkgEntry],
    label: &str,
    out: &mut Vec<Shader>,
) -> Res<()> {
    let mut other: BTreeMap<u32, usize> = BTreeMap::new();
    for (name, usz, csz, off, fl) in entries {
        if *csz == 0 || skipped(name) {
            continue;
        }
        let wh = format!("{label}/{name}");
        let raw = src
            .read(*off as u64, *csz as usize)
            .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
        let Some(m) = falcom_decompress(cx, raw, *usz as usize, *fl, &wh)? else {
            *other.entry(*fl).or_insert(0) += 1;
            continue;
        };
        scan_entry(cx, &m, &wh, out)?;
    }
    warn_other(cx, label, &other);
    Ok(())
}

// ---- .pka ----------------------------------------------------------------------------------------------------------------------------

/// One blob to read: `(package name, entry name, offset, stored length, length, flags)`
pub type PkaEntry = (String, String, u64, u32, u32, u32);

/// `falcom_pka_index(path, label)`: every blob a package entry names, each once, after checking the archive's bounds
pub fn falcom_pka_index(src: &dyn Source, label: &str) -> Res<Vec<PkaEntry>> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let size = src.len();
    if size < 12 {
        red!("{label}: PKA archive is shorter than its header ({size} bytes)");
    }
    let h = src.read(0, 8).map_err(io)?;
    if h[..4] != PKA_MAGIC {
        red!(
            "{label}: PKA header magic {}, expected {}",
            crate::bytes::hex(&h[..4]),
            crate::bytes::hex(&PKA_MAGIC)
        );
    }
    let pkg_count = u32le(&h, 4)? as u64;
    if pkg_count > (size - 12) / 36 {
        red!("{label}: PKA header lists {pkg_count} packages, more than fit in the archive");
    }
    let lossy = |b: &[u8]| {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        String::from_utf8_lossy(&b[..end]).into_owned()
    };
    let mut pos = 8u64;
    let mut names: HashMap<Vec<u8>, (String, String)> = HashMap::new();
    for i in 0..pkg_count {
        let hdr = src.read(pos, 36).map_err(io)?;
        if hdr.len() != 36 {
            red!("{label}: PKA package {i} header runs past the end of the archive");
        }
        pos += 36;
        let pnm = lossy(&hdr[..32]);
        let ecnt = u32le(&hdr, 32)? as u64;
        if ecnt > size.saturating_sub(pos) / 96 {
            red!("{label}: PKA package {pnm} lists {ecnt} entries, more than fit in the archive");
        }
        let raw = src.read(pos, (ecnt * 96) as usize).map_err(io)?;
        if raw.len() as u64 != ecnt * 96 {
            red!("{label}: PKA package {pnm} entries run past the end of the archive");
        }
        pos += ecnt * 96;
        for o in (0..raw.len()).step_by(96) {
            let enm = lossy(&raw[o..o + 64]);
            if skipped(&enm) {
                continue;
            }
            names
                .entry(raw[o + 64..o + 96].to_vec())
                .or_insert((pnm.clone(), enm));
        }
    }
    let cnt = src.read(pos, 4).map_err(io)?;
    if cnt.len() != 4 {
        red!("{label}: PKA file table count runs past the end of the archive");
    }
    pos += 4;
    let file_count = u32le(&cnt, 0)? as u64;
    if file_count > size.saturating_sub(pos) / 52 {
        red!("{label}: PKA file table lists {file_count} files, more than fit in the archive");
    }
    let ft = src.read(pos, (file_count * 52) as usize).map_err(io)?;
    if ft.len() as u64 != file_count * 52 {
        red!("{label}: PKA file table runs past the end of the archive");
    }
    let ft_end = pos + file_count * 52;
    let mut entries = Vec::new();
    for o in (0..ft.len()).step_by(52) {
        let (off, csz, usz, fl) = (
            u64le(&ft, o + 32)?,
            u32le(&ft, o + 40)?,
            u32le(&ft, o + 44)?,
            u32le(&ft, o + 48)?,
        );
        if csz != 0 && (off < ft_end || off > size || csz as u64 > size - off) {
            red!(
                "{label}: PKA file entry {} ({csz:#x} bytes at {off:#x}) lies outside archive data ({ft_end:#x}..{size:#x})",
                o / 52
            );
        }
        if csz == 0 {
            continue;
        }
        if let Some((p, e)) = names.get(&ft[o..o + 32]) {
            entries.push((p.clone(), e.clone(), off, csz, usz, fl));
        }
    }
    Ok(entries)
}

/// `falcom_pka_shaders(path, rel, entries, out)`: one shard of a `.pka`'s blobs
pub fn falcom_pka_shaders(
    cx: &Ctx<'_>,
    src: &dyn Source,
    rel: &str,
    entries: &[PkaEntry],
    out: &mut Vec<Shader>,
) -> Res<()> {
    let mut other: BTreeMap<u32, usize> = BTreeMap::new();
    for (pnm, enm, off, csz, usz, fl) in entries {
        let wh = format!("/{rel}/{pnm}/{enm}");
        let raw = src
            .read(*off, *csz as usize)
            .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
        if raw.len() != *csz as usize {
            red!("{wh}: short read ({:#x} of {csz:#x} bytes)", raw.len());
        }
        let Some(m) = falcom_decompress(cx, raw, *usz as usize, *fl, &wh)? else {
            *other.entry(*fl).or_insert(0) += 1;
            continue;
        };
        scan_entry(cx, &m, &wh, out)?;
    }
    warn_other(cx, &format!("/{rel}"), &other);
    Ok(())
}

/// One job of `falcom_pka_jobs`: a shard of a `.pka`'s blobs, or the archive's error (`(FALCOM_PKA, None, why)`)
#[derive(Clone)]
pub struct PkaJob {
    pub file: FileRef,
    pub entries: Vec<PkaEntry>,
    pub error: Option<String>,
}

impl PkaJob {
    pub fn identity(&self) -> Vec<String> {
        let mut v = vec![format!(
            "falcom-pka:{}:{}:{}:{:?}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.error
        )];
        v.extend(self.entries.iter().map(|e| format!("{e:?}")));
        v
    }
}

/// `unreadable_why(e, name)`: a reason that does not name the file gets its name in front
fn named(why: String, name: &str) -> String {
    if why
        .trim_start_matches('/')
        .starts_with(name.trim_start_matches('/'))
    {
        why
    } else {
        format!("{name}: {why}")
    }
}

/// `falcom_pka_jobs(starting(FALCOM_PKA))`: the archives whose index cannot be read first (one job each, its red line), then the shards of the others
pub fn plan(files: &[FileRef]) -> (Vec<PkaJob>, HashSet<(usize, String)>) {
    const PARTS: u64 = 64;
    let mut jobs = Vec::new();
    let mut found: Vec<(&FileRef, Vec<PkaEntry>)> = Vec::new();
    let mut taken = HashSet::new();
    for f in files {
        let Ok(src) = f.loc.open() else { continue };
        if src.read(0, 4).ok().as_deref() != Some(&PKA_MAGIC[..]) {
            continue;
        }
        taken.insert((f.root, f.rel.clone()));
        match falcom_pka_index(&src, &format!("/{}", f.rel)) {
            Ok(es) => found.push((f, es)),
            Err(e) => jobs.push(PkaJob {
                file: f.clone(),
                entries: vec![],
                error: Some(named(e.to_string(), &f.rel)),
            }),
        }
    }
    let total: u64 = found
        .iter()
        .flat_map(|x| x.1.iter())
        .map(|e| e.3 as u64)
        .sum();
    let step = (total / PARTS).max(1);
    for (f, es) in found {
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in es {
            size += e.3 as u64;
            cur.push(e);
            if size >= step {
                jobs.push(PkaJob {
                    file: f.clone(),
                    entries: std::mem::take(&mut cur),
                    error: None,
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(PkaJob {
                file: f.clone(),
                entries: cur,
                error: None,
            });
        }
    }
    (jobs, taken)
}

fn run(cx: &Ctx<'_>, job: &PkaJob) -> Res<FileResult> {
    if let Some(e) = &job.error {
        red!("{e}");
    }
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    falcom_pka_shaders(cx, &src, &job.file.rel, &job.entries, &mut out)?;
    finish(cx, &job.file.rel, out, std::iter::empty(), Trees::new())
}

/// `scan_file((path, rel, (FALCOM_PKA, entries)))`
pub fn scan(cx: &Ctx<'_>, job: &PkaJob) -> Result<FileResult, String> {
    left_out(cx, &job.file.rel, run(cx, job))
}

/// `scan_file` around a job of its own: a red job is left out with its line (`[RED] <why> (its shaders are left out)`), an unsupported container ends the split
pub fn left_out(cx: &Ctx<'_>, rel: &str, r: Res<FileResult>) -> Result<FileResult, String> {
    match r {
        Ok(r) => Ok(r),
        Err(Failure::Unsupported(m)) => Err(m),
        Err(e) => {
            let why = named(e.to_string(), rel);
            cx.print(&format!("[RED] {why} (its shaders are left out)"));
            Ok(FileResult {
                rel: rel.to_string(),
                uniq: vec![],
                pairs: vec![],
                extra: Extra::Unreadable { why, count: 1 },
            })
        }
    }
}
