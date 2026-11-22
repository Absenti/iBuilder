//! Media.Vision's MVGL archives and NXVS / NXFS / NXCS shader containers (Digimon Story Cyber Sleuth, Digimon Story Time Stranger): `mvgl.py` (`doboz`, `lz4_block`, `read_entry`,
//! `Archive`) and `mvgl_jobs`, `mvgl_shaders`, `nxsh_shader`, `nxsh_embedded` of `shader_split.py` (nxnative 131bd013 + 8cc63cc9).
//!
//! MDB1, little-endian, no encryption on the Switch, in one of two layouts told apart by the header's own data start (header + tables) and archive size:
//! * legacy (Cyber Sleuth): `MDB1`, u16 tree entries, u16 name entries, u32 data entries, u32 data start, u32 archive size (0x14 bytes); tree entries of 8 bytes {u16 compare bit,
//!   u16 data index (0xffff: no file), u16 left, u16 right}; names of 0x40 bytes {char[4] extension, char[0x3c] name}; data entries of 12 bytes {u32 offset from the data start, u32 length,
//!   u32 stored length}; stored < length is doboz (version 0);
//! * wide (Time Stranger): `MDB1`, u32 x3, u64 data start, u64 archive size (0x20 bytes); tree entries of 16 bytes (u32 fields, 0xffffffff: no file); names of 0x80 bytes; data entries of
//!   16 bytes {u64 offset, u32 length, u32 stored length}; stored < length is one raw LZ4 block.
//!
//! Only the `.shad` entries are decoded (each scanned like a file: `scan_blob`), in jobs of about equal length planned after the PKZ jobs and before the zip jobs, as in the Python.
//! An NXSH container: magic (`NXVS`, `NXFS`, `NXCS`, `NXGS`, `NXHS`, `NXDS`), u32 total size, u32 reserved (0), u32 control offset, u32 control length, u32 code offset, u32 code length.

use crate::bytes::{u16le, u32le, u64le};
use crate::ctl::{CTL_MAGIC, St, check, glslc_stage};
use crate::error::{Failure, Res};
use crate::red;
use crate::scan::{FileRef, finish, scan_blob};
use crate::scan_types::{Ctx, FileResult, Shader};
use crate::source::Source;
use crate::unity::Trees;
use std::collections::{BTreeMap, HashSet};

pub const MAGIC: &[u8; 4] = b"MDB1";

// ---- NXSH ----------------------------------------------------------------------------------------------------------------------------

/// `NXSH_STAGES`
pub fn nxsh_stage(m: &[u8]) -> Option<St> {
    match m {
        b"NXVS" => Some(St::Vs),
        b"NXFS" => Some(St::Ps),
        b"NXCS" => Some(St::Cs),
        b"NXGS" => Some(St::Gs),
        b"NXHS" => Some(St::Hs),
        b"NXDS" => Some(St::Ds),
        _ => None,
    }
}

/// `NXSH_RE.search(d)` / `finditer`: the offsets of `NX[VFCGHD]S` (the pattern cannot overlap itself)
fn nxsh_hits(d: &[u8]) -> impl Iterator<Item = usize> + '_ {
    memchr::memmem::find_iter(d, b"NX")
        .filter(move |&i| i + 4 <= d.len() && b"VFCGHD".contains(&d[i + 2]) && d[i + 3] == b'S')
}

/// `nxsh_shader(d, base, label, strict)`: strict, a damaged container is `red()`; not strict (a container embedded in other data), it is `None`
pub fn nxsh_shader(d: &[u8], base: usize, label: &str, strict: bool) -> Res<Option<Shader>> {
    let magic = crate::bytes::at(d, base, 4);
    let wh = format!(
        "{label} {}@{base:#x}",
        magic.iter().map(|&b| b as char).collect::<String>()
    );
    macro_rules! fail {
        ($($t:tt)*) => {{
            if strict {
                red!("{wh}: {}", format!($($t)*));
            }
            return Ok(None);
        }};
    }
    if base + 0x1c > d.len() {
        fail!("truncated header");
    }
    let Some(want) = nxsh_stage(magic) else {
        fail!("unknown shader magic");
    };
    let r = |o: usize| u32le(d, base + o).map(|v| v as usize);
    let (total, reserved, coff, clen, koff, klen) = (r(4)?, r(8)?, r(12)?, r(16)?, r(20)?, r(24)?);
    if strict {
        if total != d.len() - base {
            fail!("size {total:#x} does not match {:#x} bytes", d.len() - base);
        }
    } else if reserved != 0 || total < 0x1c || base as u64 + total as u64 > d.len() as u64 {
        return Ok(None);
    }
    if coff < 0x1c || clen < 0x24 || coff as u64 + clen as u64 > total as u64 {
        fail!("control section lies outside container");
    }
    if koff < 0x1c || klen == 0 || koff as u64 + klen as u64 > total as u64 {
        fail!("code area lies outside container");
    }
    if !(coff + clen <= koff || koff + klen <= coff) {
        fail!("control section and code area overlap");
    }
    let (ctl, code) = (
        &d[base + coff..base + coff + clen],
        &d[base + koff..base + koff + klen],
    );
    if !strict && ctl[..4] != CTL_MAGIC {
        return Ok(None);
    }
    let st = glslc_stage(ctl, code)?;
    if st != Some(want) {
        let shown = match st {
            Some(s) => format!("'{}'", s.short()),
            None => "None".to_string(),
        };
        fail!(
            "stage {shown} does not match {}",
            String::from_utf8_lossy(magic)
        );
    }
    if code.len() < if want == St::Cs { 0x30 } else { 0x80 } {
        fail!("truncated code header or SPH");
    }
    if let Some(why) = check(want, ctl, code)? {
        fail!("{why}");
    }
    Ok(Some(Shader::new(
        format!("{label}#NXSH@{base:#x}"),
        want,
        ctl,
        code,
    )))
}

/// `nxsh_embedded(d, label)`: the containers embedded in other data (Time Stranger's main holds its post-processing, present and compute shaders in this form)
pub fn nxsh_embedded(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    if !crate::bytes::contains(d, &CTL_MAGIC) {
        return Ok(out);
    }
    let mut end = 0usize;
    for at in nxsh_hits(d) {
        if at < end {
            continue;
        }
        if let Some(s) = nxsh_shader(d, at, label, false)? {
            out.push(s);
            end = at + u32le(d, at + 4)? as usize;
        }
    }
    Ok(out)
}

/// `CTL_MAGIC in d and NXSH_RE.search(d)`: the last test of `scan_blob`
pub fn has_embedded(d: &[u8]) -> bool {
    crate::bytes::contains(d, &CTL_MAGIC) && nxsh_hits(d).next().is_some()
}

// ---- decoders ------------------------------------------------------------------------------------------------------------------------

/// `MATCH`: (mask, offset shift, length mask, length shift, size) by a match word's low 3 bits
const MATCH: [(u64, u32, u64, u32, usize); 8] = [
    (0xff, 2, 0, 0, 1),
    (0xffff, 2, 0, 0, 2),
    (0xffff, 6, 15, 2, 2),
    (0xffffff, 8, 31, 3, 3),
    (0xff, 2, 0, 0, 1),
    (0xffff, 2, 0, 0, 2),
    (0xffff, 6, 15, 2, 2),
    (0xffffffff, 11, 255, 3, 4),
];
const TAIL: i128 = 8;

/// Why `doboz` failed: the Python's `ValueError`, or the `IndexError` of a stream that runs out (which `mvgl_shaders` does not catch)
#[derive(Debug)]
pub enum DobozError {
    Value(String),
    Index,
}

fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |v, &x| (v << 8) | x as u64)
}

/// `doboz(b)`: Attila T. Afra's doboz, version 0. The decoded data is bounded by [`crate::inflate::MAX_DECODED`] (the Python's by its memory)
pub fn doboz(b: &[u8]) -> Result<Vec<u8>, DobozError> {
    let v = |m: &str| DobozError::Value(m.to_string());
    if b.is_empty() {
        return Err(v("empty doboz stream"));
    }
    let a = b[0];
    if a & 7 != 0 {
        return Err(DobozError::Value(format!("doboz version {}", a & 7)));
    }
    let w = (((a >> 3) & 7) + 1) as usize;
    if b.len() < 1 + 2 * w {
        return Err(v("truncated doboz header"));
    }
    let (size, csize) = (le(&b[1..1 + w]), le(&b[1 + w..1 + 2 * w]));
    let mut i = 1 + 2 * w;
    if a & 0x80 != 0 {
        if (b.len() as u64) < i as u64 + size {
            return Err(v("truncated stored doboz payload"));
        }
        return Ok(b[i..i + size as usize].to_vec());
    }
    let mut src = b.to_vec();
    src.extend_from_slice(&[0; 8]);
    let byte = |i: usize| src.get(i).copied().ok_or(DobozError::Index);
    let word = |i: usize| le(crate::bytes::at(&src, i, 4));
    let cap = crate::inflate::MAX_DECODED as u64;
    let mut out: Vec<u8> = Vec::new();
    let mut ctl: u64 = 1;
    while (out.len() as u64) < size {
        if out.len() as u64 > cap {
            return Err(DobozError::Value(crate::inflate::cap_error(
                crate::inflate::MAX_DECODED,
            )));
        }
        if ctl == 1 {
            ctl = word(i);
            i += 4;
        }
        if ctl & 1 == 0 {
            if (out.len() as i128) < size as i128 - TAIL {
                out.push(byte(i)?);
                i += 1;
            } else {
                // the tail: every remaining byte is a literal, whatever the control bits say (doboz's decoder does the same)
                while (out.len() as u64) < size {
                    if ctl == 1 {
                        ctl = word(i);
                        i += 4;
                    }
                    out.push(byte(i)?);
                    i += 1;
                    ctl >>= 1;
                }
                break;
            }
        } else {
            let wd = word(i);
            let m = MATCH[(wd & 7) as usize];
            let off = ((wd & m.0) >> m.1) as usize;
            let n = (((wd >> m.3) & m.2) + 3) as usize;
            i += m.4;
            if off == 0 || off > out.len() {
                return Err(v("doboz match out of range"));
            }
            let s = out.len() - off;
            if off >= n {
                out.extend_from_within(s..s + n);
            } else {
                for k in 0..n {
                    out.push(out[s + k]);
                }
            }
        }
        ctl >>= 1;
    }
    if (i - (1 + 2 * w)) as u64 > csize + 4 {
        return Err(v("doboz stream overrun"));
    }
    Ok(out)
}

/// `lz4_block(b, size)`
pub fn lz4_block(b: &[u8], size: usize) -> Result<Vec<u8>, String> {
    if size != 0 && (b.is_empty() || size as u128 > 255 * b.len() as u128) {
        return Err(format!(
            "impossible LZ4 length {size:#x} for {:#x} bytes",
            b.len()
        ));
    }
    if size > crate::inflate::MAX_DECODED {
        return Err(crate::inflate::cap_error(crate::inflate::MAX_DECODED));
    }
    let mut out = vec![0u8; size];
    match lz4_flex::block::decompress_into(b, &mut out) {
        Ok(k) => out.truncate(k),
        Err(e) => {
            return Err(format!(
                "invalid LZ4 block: Decompression failed: corrupt input or insufficient space in destination buffer ({e})"
            ));
        }
    }
    if out.len() != size {
        return Err(format!(
            "LZ4 decoded {:#x} bytes, expected {size:#x}",
            out.len()
        ));
    }
    Ok(out)
}

/// `read_entry(f, off, n, cn, wide)`: `Err` is the Python's `ValueError` (`Ok(Err(Index))` the `IndexError` of a doboz stream that runs out)
pub fn read_entry(
    src: &dyn Source,
    off: u64,
    n: u64,
    cn: u64,
    wide: bool,
) -> Result<Vec<u8>, DobozError> {
    let b = src
        .read(off, cn as usize)
        .map_err(|e| DobozError::Value(format!("OSError: {e}")))?;
    if b.len() as u64 != cn {
        return Err(DobozError::Value(format!(
            "truncated MVGL entry ({:#x} of {cn:#x} bytes)",
            b.len()
        )));
    }
    if cn == n {
        return Ok(b);
    }
    if cn > n {
        return Err(DobozError::Value(format!(
            "MVGL entry stored length {cn:#x} exceeds length {n:#x}"
        )));
    }
    if wide {
        lz4_block(&b, n as usize).map_err(DobozError::Value)
    } else {
        doboz(&b)
    }
}

/// An archive's files: `name.ext` to (offset in the file, length, stored length)
pub type MvglFiles = BTreeMap<String, (u64, u64, u64)>;
/// One `.shad` entry of a job: (name, offset, length, stored length)
pub type ShadEntry = (String, u64, u64, u64);

/// `mvgl.Archive(path)`: `(wide, {name.ext: (offset in the file, length, stored length)})`; `Err` is the `ValueError` text (it names the file by `path`, as the Python does)
pub fn archive(src: &dyn Source, path: &str) -> Result<(bool, MvglFiles), String> {
    let io = |e: std::io::Error| format!("{e}");
    let fsize = src.len() as u128;
    let head = src.read(0, 0x20).map_err(io)?;
    if head.len() < 0x14 || &head[..4] != MAGIC {
        return Err(format!("{path}: not an MVGL archive"));
    }
    let mut layout: Option<(bool, u64, u64, u64, u64, u64)> = None;
    if head.len() >= 0x20 {
        let (nt, nn, nd) = (
            u32le(&head, 4).unwrap() as u64,
            u32le(&head, 8).unwrap() as u64,
            u32le(&head, 12).unwrap() as u64,
        );
        let (start, size) = (u64le(&head, 16).unwrap(), u64le(&head, 24).unwrap());
        if 0x20 + nt as u128 * 16 + nn as u128 * 0x80 + nd as u128 * 16 == start as u128
            && start <= size
            && size as u128 <= fsize
        {
            layout = Some((true, nt, nn, nd, start, size));
        }
    }
    if layout.is_none() {
        let (nt, nn, nd) = (
            u16le(&head, 4).unwrap() as u64,
            u16le(&head, 6).unwrap() as u64,
            u32le(&head, 8).unwrap() as u64,
        );
        let (start, size) = (
            u32le(&head, 12).unwrap() as u64,
            u32le(&head, 16).unwrap() as u64,
        );
        if 0x14 + nt as u128 * 8 + nn as u128 * 0x40 + nd as u128 * 12 == start as u128
            && start <= size
            && size as u128 <= fsize
        {
            layout = Some((false, nt, nn, nd, start, size));
        }
    }
    let Some((wide, nt, nn, nd, start, size)) = layout else {
        return Err(format!("{path}: unrecognized MVGL header layout"));
    };
    let (hdr, tree_sz, name_sz, data_sz) = if wide {
        (0x20u64, 16u64, 0x80u64, 16u64)
    } else {
        (0x14, 8, 0x40, 12)
    };
    // the tables lie inside the file: their sum is the data start, which is at most the file's size
    let tree = src.read(hdr, (nt * tree_sz) as usize).map_err(io)?;
    let names = src
        .read(hdr + nt * tree_sz, (nn * name_sz) as usize)
        .map_err(io)?;
    let data = src
        .read(hdr + nt * tree_sz + nn * name_sz, (nd * data_sz) as usize)
        .map_err(io)?;
    if tree.len() as u64 != nt * tree_sz
        || names.len() as u64 != nn * name_sz
        || data.len() as u64 != nd * data_sz
    {
        return Err(format!("{path}: truncated MVGL tables"));
    }
    let latin1 = |b: &[u8]| b.iter().map(|&c| c as char).collect::<String>();
    let mut files = BTreeMap::new();
    for k in 0..nt as usize {
        let (idx, none) = if wide {
            (u32le(&tree, k * 16 + 4).unwrap() as u64, 0xffff_ffffu64)
        } else {
            (u16le(&tree, k * 8 + 2).unwrap() as u64, 0xffff)
        };
        if idx == none {
            continue;
        }
        if k as u64 >= nn || idx >= nd {
            return Err(format!("{path}: MVGL table index out of range"));
        }
        let o = (idx * data_sz) as usize;
        let (off, n, cn) = if wide {
            (
                u64le(&data, o).unwrap(),
                u32le(&data, o + 8).unwrap() as u64,
                u32le(&data, o + 12).unwrap() as u64,
            )
        } else {
            (
                u32le(&data, o).unwrap() as u64,
                u32le(&data, o + 4).unwrap() as u64,
                u32le(&data, o + 8).unwrap() as u64,
            )
        };
        if cn > n || off as u128 + cn as u128 > (size - start) as u128 {
            return Err(format!(
                "{path}: MVGL entry {idx} lies outside archive data"
            ));
        }
        let base = k * name_sz as usize;
        let ext_raw = &names[base..base + 4];
        let ext_end = ext_raw
            .iter()
            .rposition(|&c| c != 0 && c != b' ')
            .map(|p| p + 1)
            .unwrap_or(0);
        let ext = latin1(&ext_raw[..ext_end]);
        let nm_raw = &names[base + 4..base + name_sz as usize];
        let nm_end = nm_raw.iter().position(|&c| c == 0).unwrap_or(nm_raw.len());
        let name = latin1(&nm_raw[..nm_end]);
        files.insert(format!("{name}.{ext}"), (start + off, n, cn));
    }
    Ok((wide, files))
}

// ---- jobs ----------------------------------------------------------------------------------------------------------------------------

/// One job of `mvgl_jobs`: a batch of an archive's `.shad` entries `(name, offset, length, stored length)`, or the archive's error (`(MDB1, None, why)`)
#[derive(Clone)]
pub struct MvglJob {
    pub file: FileRef,
    pub wide: bool,
    pub entries: Vec<ShadEntry>,
    pub error: Option<String>,
}

impl MvglJob {
    pub fn identity(&self) -> Vec<String> {
        let mut v = vec![format!(
            "mvgl:{}:{}:{}:{}:{:?}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.wide,
            self.error
        )];
        v.extend(self.entries.iter().map(|e| format!("{e:?}")));
        v
    }
}

/// `mvgl_jobs(starting(MDB1))`: the jobs and the `(root, rel)` of every archive they take (an archive that cannot be read is taken too: its job is its red line)
pub fn plan(
    files: &[FileRef],
    path_of: &dyn Fn(&FileRef) -> String,
) -> (Vec<MvglJob>, HashSet<(usize, String)>) {
    const PARTS: u64 = 64;
    let mut found: Vec<(&FileRef, bool, Vec<ShadEntry>)> = Vec::new();
    let mut jobs = Vec::new();
    let mut taken = HashSet::new();
    for f in files {
        let Ok(src) = f.loc.open() else { continue };
        if src.read(0, 4).ok().as_deref() != Some(&MAGIC[..]) {
            continue;
        }
        taken.insert((f.root, f.rel.clone()));
        match archive(&src, &path_of(f)) {
            Ok((wide, files)) => {
                let es: Vec<ShadEntry> = files
                    .into_iter()
                    .filter(|(nm, _)| nm.to_lowercase().ends_with(".shad"))
                    .map(|(nm, (o, n, c))| (nm, o, n, c))
                    .collect();
                if !es.is_empty() {
                    found.push((f, wide, es));
                }
            }
            Err(e) => jobs.push(MvglJob {
                file: f.clone(),
                wide: false,
                entries: vec![],
                error: Some(format!("/{}: the MVGL cannot be read ({e})", f.rel)),
            }),
        }
    }
    let total: u64 = found.iter().flat_map(|x| x.2.iter()).map(|e| e.2).sum();
    let step = (total / PARTS).max(1);
    for (f, wide, es) in found {
        let (mut batch, mut size) = (Vec::new(), 0u64);
        for e in es {
            size += e.2;
            batch.push(e);
            if size >= step {
                jobs.push(MvglJob {
                    file: f.clone(),
                    wide,
                    entries: std::mem::take(&mut batch),
                    error: None,
                });
                size = 0;
            }
        }
        if !batch.is_empty() {
            jobs.push(MvglJob {
                file: f.clone(),
                wide,
                entries: batch,
                error: None,
            });
        }
    }
    (jobs, taken)
}

fn run(cx: &Ctx<'_>, job: &MvglJob) -> Res<FileResult> {
    if let Some(e) = &job.error {
        red!("{e}");
    }
    let f = &job.file;
    let src = f
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    for (nm, off, n, cn) in &job.entries {
        let d = match read_entry(&src, *off, *n, *cn, job.wide) {
            Ok(d) => d,
            Err(DobozError::Value(e)) => red!("/{}/{nm}: {e}", f.rel),
            Err(DobozError::Index) => red!("IndexError: index out of range"),
        };
        scan_blob(cx, &d, &format!("/{}/{nm}", f.rel), &mut out, 0, None)?;
    }
    finish(cx, &f.rel, out, std::iter::empty(), Trees::new())
}

/// `scan_file((path, rel, (MDB1, wide, entries)))`: an entry that does not decode ends the job, not the split
pub fn scan(cx: &Ctx<'_>, job: &MvglJob) -> Result<FileResult, String> {
    crate::falcom::left_out(cx, &job.file.rel, run(cx, job))
}
