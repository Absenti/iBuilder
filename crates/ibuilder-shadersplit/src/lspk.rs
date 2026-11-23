//! Larian LSPK v13 packages and their BSHD shader records (Divinity: Original Sin 2): `lspk.py` and `lspk_jobs` / `bshd_shaders` / `lspk_shaders` of `shader_split.py`.
//!
//! The package's 40-byte header is at its end, followed by its length and the magic `LSPK`; its file table is one LZ4 block of 280-byte rows (name, offset, stored length, decoded
//! length, part, compression), the members stored, zlib or LZ4, possibly in numbered part files (`<stem>_<n><ext>`). Only the `.bshd` members are read, each by a positioned read of
//! its own range (the sound and video parts are never read). A BSHD record is a big-endian length (its top bit: zlib), then a version, a stage count and {u8 stage, u32 offset,
//! u32 length} rows; an NX stage is one GLSLC output, the other platforms' stages are left alone.

use crate::error::{Failure, Res};
use crate::glslc::glslc_shaders;
use crate::scan::FileRef;
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use std::collections::{HashMap, HashSet};

pub const MAGIC: &[u8; 4] = b"LSPK";
const ENTRY_SIZE: usize = 280;
const MAX_TABLE: u64 = 256 << 20;
const MAX_MEMBER: u64 = 256 << 20;
const GLSLC: [u8; 4] = [0x91, 0x68, 0x86, 0x19];

/// `(name, part index, offset, stored length, decoded length, method)`: a member of a package (the Python keeps the part's path; the job keeps the parts beside)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub part: usize,
    pub off: u64,
    pub size: u64,
    pub raw: u64,
    pub method: u32,
}

/// One job of `lspk_jobs`: a range of one package's BSHD members, or the error that made the package unreadable (`(LSPK, None, str(e))`)
#[derive(Clone)]
pub struct LspkJob {
    pub file: FileRef,
    /// the package's parts (the package itself first)
    pub parts: Vec<FileRef>,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

impl LspkJob {
    pub fn identity(&self) -> Vec<String> {
        let mut v = vec![format!(
            "lspk:{}:{}:{}:{:?}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.error
        )];
        v.extend(
            self.parts
                .iter()
                .map(|p| format!("{}:{}", p.rel, p.loc.len())),
        );
        v.extend(self.entries.iter().map(|e| format!("{e:?}")));
        v
    }
}

/// `os.path.splitext(p)`
fn splitext(p: &str) -> (&str, &str) {
    let base_at = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &p[base_at..];
    let lead = base.len() - base.trim_start_matches('.').len();
    match base[lead..].rfind('.') {
        Some(i) => p.split_at(base_at + lead + i),
        None => (p, ""),
    }
}

/// `bytes.decode('utf-8')`: Python's `UnicodeDecodeError` text when the bytes are not UTF-8
pub fn utf8(b: &[u8]) -> Result<String, String> {
    match std::str::from_utf8(b) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => {
            let start = e.valid_up_to();
            let (end, why) = match e.error_len() {
                None => (b.len(), "unexpected end of data"),
                Some(n) => (
                    start + n,
                    if matches!(b[start], 0x80..=0xc1 | 0xf5..=0xff) {
                        "invalid start byte"
                    } else {
                        "invalid continuation byte"
                    },
                ),
            };
            Err(if end - start == 1 {
                format!(
                    "'utf-8' codec can't decode byte {:#04x} in position {start}: {why}",
                    b[start]
                )
            } else {
                format!(
                    "'utf-8' codec can't decode bytes in position {start}-{}: {why}",
                    end - 1
                )
            })
        }
    }
}

fn range(offset: u64, size: u64, end: u64, what: &str) -> Result<(), String> {
    if offset > end || size > end - offset {
        return Err(format!(
            "{what}: range {offset:#x}+{size:#x} exceeds {end:#x} bytes"
        ));
    }
    Ok(())
}

/// `lz4.block.decompress(data, uncompressed_size=size)`: at most `size` bytes (fewer when the block holds fewer); the buffer is bounded by what the block can hold (255 to 1)
fn lz4(data: &[u8], size: u64) -> Result<Vec<u8>, String> {
    let cap = size.min(256 * data.len() as u64 + 64) as usize;
    let mut out = vec![0u8; cap];
    match lz4_flex::block::decompress_into(data, &mut out) {
        Ok(k) => {
            out.truncate(k);
            Ok(out)
        }
        Err(e) => Err(format!(
            "Decompression failed: corrupt input or insufficient space in destination buffer ({e})"
        )),
    }
}

/// `_decode(data, size, method, what)`: the member's bytes, exactly `size` of them
fn decode(data: Vec<u8>, size: u64, method: u32, what: &str) -> Result<Vec<u8>, String> {
    if size > MAX_MEMBER {
        return Err(format!("{what}: decoded size exceeds {MAX_MEMBER} bytes"));
    }
    let out = match method {
        0 => data,
        1 => {
            let lim = size as usize + 1;
            // the fast decoder first; anything but a clean end of the whole input within the limit is decided by the exact zlib model (its error texts)
            let fast = crate::inflate::run(&data, crate::inflate::Wrap::Zlib, Some(lim));
            let z = match fast {
                Ok(z) if z.eof && z.used == data.len() => crate::zlibx::Z {
                    out: z.out,
                    eof: true,
                    used: z.used,
                },
                _ => crate::zlibx::decompress(&data, Some(lim))
                    .map_err(|e| format!("{what}: damaged zlib stream: {e}"))?,
            };
            if !z.eof || z.used < data.len() {
                return Err(format!("{what}: incomplete or oversized zlib stream"));
            }
            z.out
        }
        2 => lz4(&data, size).map_err(|e| format!("{what}: damaged LZ4 block: {e}"))?,
        m => return Err(format!("{what}: unsupported compression {m}")),
    };
    if out.len() as u64 != size {
        return Err(format!(
            "{what}: decoded size {} differs from {size}",
            out.len()
        ));
    }
    Ok(out)
}

/// The header and file table of a package: `None` for another format; `Err` the `ValueError` text. `path` names the package as the Python does (its path).
/// The part lengths are asked for by `part_len` (index 1..): `Err` is the `OSError` text of a missing part.
pub fn package(
    src: &dyn Source,
    path: &str,
    part_len: &dyn Fn(usize) -> Result<u64, String>,
) -> Result<Option<(u16, Vec<Entry>)>, String> {
    let io = |e: std::io::Error| e.to_string();
    let size = src.len();
    if size < 8 {
        return Ok(None);
    }
    let t = src.read(size - 8, 8).map_err(io)?;
    let hsize = u32::from_le_bytes(t[..4].try_into().unwrap()) as u64;
    if &t[4..8] != MAGIC {
        return Ok(None);
    }
    if hsize != 40 || size < hsize {
        return Err(format!("{path}: invalid LSPK header size {hsize}"));
    }
    let header = size - hsize;
    let h = src.read(header, 16).map_err(io)?;
    let u = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap()) as u64;
    let (version, offset, stored) = (u(0), u(4), u(8));
    let (parts, flags) = (u16::from_le_bytes([h[12], h[13]]), h[14]);
    if version != 13 {
        return Err(format!("{path}: unsupported LSPK version {version}"));
    }
    if parts == 0 || flags & 4 != 0 {
        return Err(format!(
            "{path}: invalid part count or unsupported solid LSPK"
        ));
    }
    range(offset, stored, header, &format!("{path}: LSPK file table"))?;
    if !(4..=MAX_TABLE).contains(&stored) {
        return Err(format!("{path}: invalid LSPK table size {stored}"));
    }
    let c = src.read(offset, 4).map_err(io)?;
    let count = u32::from_le_bytes(c[..4].try_into().unwrap()) as u64;
    if count > MAX_TABLE / ENTRY_SIZE as u64 {
        return Err(format!("{path}: LSPK file count {count} is too large"));
    }
    let raw = src.read(offset + 4, (stored - 4) as usize).map_err(io)?;
    let table = decode(
        raw,
        count * ENTRY_SIZE as u64,
        2,
        &format!("{path}: LSPK file table"),
    )?;
    let mut lengths = vec![offset];
    for i in 1..parts as usize {
        lengths.push(part_len(i)?);
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let row = &table[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
        let end = match row[..256].iter().position(|&b| b == 0) {
            Some(e) if e > 0 => e,
            _ => return Err(format!("{path}: LSPK member {i} has an invalid name")),
        };
        let name = utf8(&row[..end])?;
        let w = |k: usize| u32::from_le_bytes(row[256 + 4 * k..260 + 4 * k].try_into().unwrap());
        let (at, n, rawn, part, comp) = (w(0) as u64, w(1) as u64, w(2) as u64, w(3), w(4));
        if part >= parts as u32 {
            return Err(format!("{path}/{name}: invalid LSPK part {part}"));
        }
        range(at, n, lengths[part as usize], &format!("{path}/{name}"))?;
        out.push(Entry {
            name,
            part: part as usize,
            off: at,
            size: n,
            raw: if comp & 15 != 0 { rawn } else { n },
            method: comp & 15,
        });
    }
    Ok(Some((parts, out)))
}

/// `lspk.read(entry)`: one member's bytes; `Err` the `ValueError` text
pub fn read(src: &dyn Source, e: &Entry) -> Result<Vec<u8>, String> {
    if e.size > MAX_MEMBER {
        return Err(format!(
            "{}: stored size exceeds {MAX_MEMBER} bytes",
            e.name
        ));
    }
    let data = src
        .read(e.off, e.size as usize)
        .map_err(|x| x.to_string())?;
    if data.len() as u64 != e.size {
        return Err(format!("{}: truncated LSPK member", e.name));
    }
    decode(data, e.raw, e.method, &e.name)
}

/// `lspk.bshd_sections(data, label)`: `(engine stage, compiled program)` of a BSHD v1 record; `Err` the `ValueError` text
pub fn bshd_sections(data: &[u8], label: &str) -> Result<Vec<(u8, Vec<u8>)>, String> {
    if data.len() < 4 {
        return Err(format!("{label}: truncated BSHD length"));
    }
    let word = u32::from_be_bytes(data[..4].try_into().unwrap());
    let d = decode(
        data[4..].to_vec(),
        (word & 0x7fff_ffff) as u64,
        if word >> 31 != 0 { 1 } else { 0 },
        label,
    )?;
    if d.len() < 6 || d[..5] != [0xff, 0xff, 0xff, 0xff, 0x01] {
        return Err(format!("{label}: unsupported BSHD header"));
    }
    let count = d[5] as usize;
    let start = 6 + 9 * count;
    if count == 0 || start > d.len() {
        return Err(format!("{label}: truncated BSHD stage table"));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let r = 6 + 9 * i;
        let stage = d[r];
        let offset = u32::from_le_bytes(d[r + 1..r + 5].try_into().unwrap()) as u64;
        let size = u32::from_le_bytes(d[r + 5..r + 9].try_into().unwrap()) as u64;
        if offset < start as u64 || size == 0 {
            return Err(format!("{label}: invalid BSHD stage {i} range"));
        }
        range(
            offset,
            size,
            d.len() as u64,
            &format!("{label}: BSHD stage {i}"),
        )?;
        out.push((stage, d[offset as usize..(offset + size) as usize].to_vec()));
    }
    Ok(out)
}

/// `bshd_shaders(data, label)`: the NX stages of a BSHD record (GLSLC outputs), labelled `<label>#BSHD#var` (one program: its vertex and pixel stages pair)
pub fn bshd_shaders(data: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    let sections =
        bshd_sections(data, label).map_err(|e| Failure::Red(format!("ValueError: {e}")))?;
    for (stage, program) in sections {
        if program.len() < 4 || program[..4] != GLSLC {
            continue;
        }
        let expected = match stage {
            1 => Some("vs"),
            3 => Some("ps"),
            4 => Some("cs"),
            _ => None,
        };
        let stages = glslc_shaders(&program, label, true, &[])?;
        if stages.len() != 1 || Some(stages[0].st.short()) != expected {
            crate::red!("{label}: BSHD stage {stage} disagrees with its GLSLC program");
        }
        let s = stages.into_iter().next().unwrap();
        out.push(Shader {
            label: format!("{label}#BSHD#var"),
            ..s
        });
    }
    Ok(out)
}

/// `lspk_jobs(files)`: the packages among `files` (a `.pak` whose last 8 bytes end in `LSPK`), their BSHD members in ranges of about equal stored length; an unreadable package
/// is one job of its error, before the others. Returns the jobs and every file a package took (its parts too).
pub fn plan(
    files: &[FileRef],
    path_of: &dyn Fn(&FileRef) -> String,
) -> (Vec<LspkJob>, HashSet<(usize, String)>) {
    const PARTS: u64 = 64;
    let by: HashMap<(usize, &str), &FileRef> = files
        .iter()
        .map(|f| ((f.root, f.rel.as_str()), f))
        .collect();
    let mut consumed: HashSet<(usize, String)> = HashSet::new();
    let mut errors: Vec<LspkJob> = Vec::new();
    let mut found: Vec<(FileRef, Vec<FileRef>, Vec<Entry>)> = Vec::new();
    for f in files {
        if consumed.contains(&(f.root, f.rel.clone())) || !f.rel.to_lowercase().ends_with(".pak") {
            continue;
        }
        let path = path_of(f);
        let (pstem, pext) = splitext(&path);
        let (rstem, rext) = splitext(&f.rel);
        let part_rel = |i: usize| format!("{rstem}_{i}{rext}");
        let part_len = |i: usize| -> Result<u64, String> {
            match by.get(&(f.root, part_rel(i).as_str())) {
                Some(p) => Ok(p.loc.len()),
                None => Err(format!(
                    "[Errno 2] No such file or directory: '{pstem}_{i}{pext}'"
                )),
            }
        };
        let r = match f.loc.open() {
            Ok(src) => package(&src, &path, &part_len),
            Err(e) => Err(e.to_string()),
        };
        match r {
            Err(e) => {
                errors.push(LspkJob {
                    file: f.clone(),
                    parts: vec![],
                    entries: vec![],
                    error: Some(e),
                });
                consumed.insert((f.root, f.rel.clone()));
            }
            Ok(None) => {}
            Ok(Some((nparts, entries))) => {
                let mut parts = vec![f.clone()];
                consumed.insert((f.root, f.rel.clone()));
                for i in 1..nparts as usize {
                    let rel = part_rel(i);
                    // every part exists: `package` asked for its length
                    if let Some(p) = by.get(&(f.root, rel.as_str())) {
                        parts.push((*p).clone());
                    }
                    consumed.insert((f.root, rel));
                }
                let es: Vec<Entry> = entries
                    .into_iter()
                    .filter(|e| e.name.to_lowercase().ends_with(".bshd"))
                    .collect();
                found.push((f.clone(), parts, es));
            }
        }
    }
    let total: u64 = found.iter().flat_map(|x| x.2.iter()).map(|e| e.size).sum();
    let step = (total / PARTS).max(1);
    let mut jobs = errors;
    for (f, parts, es) in found {
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in es {
            size += e.size;
            cur.push(e);
            if size >= step {
                jobs.push(LspkJob {
                    file: f.clone(),
                    parts: parts.clone(),
                    entries: std::mem::take(&mut cur),
                    error: None,
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(LspkJob {
                file: f.clone(),
                parts: parts.clone(),
                entries: cur,
                error: None,
            });
        }
    }
    (jobs, consumed)
}

/// `lspk_shaders(entries, rel, out)` (and the red line of a package that could not be read): the shaders of the job's members, labelled `/<rel>/<member>`
pub fn shaders(_cx: &Ctx<'_>, job: &LspkJob) -> Res<Vec<Shader>> {
    if let Some(e) = &job.error {
        crate::red!("{e}");
    }
    let mut out = Vec::new();
    let mut opened: HashMap<usize, crate::source::FileSource> = HashMap::new();
    for e in &job.entries {
        let src = match opened.get(&e.part) {
            Some(s) => s.clone(),
            None => {
                let s = job.parts[e.part]
                    .loc
                    .open()
                    .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
                opened.insert(e.part, s.clone());
                s
            }
        };
        let d = read(&src, e).map_err(|x| Failure::Red(format!("ValueError: {x}")))?;
        out.extend(bshd_shaders(&d, &format!("/{}/{}", job.file.rel, e.name))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_names() {
        assert_eq!(splitext("/a/b.pak"), ("/a/b", ".pak"));
        assert_eq!(splitext("/a/.pak"), ("/a/.pak", ""));
        assert_eq!(splitext("x.y/b"), ("x.y/b", ""));
        assert_eq!(
            utf8(b"ab\xe2\x82").unwrap_err(),
            "'utf-8' codec can't decode bytes in position 2-3: unexpected end of data"
        );
        assert_eq!(
            utf8(b"\xf0\x90(").unwrap_err(),
            "'utf-8' codec can't decode bytes in position 0-1: invalid continuation byte"
        );
        assert_eq!(
            utf8(b"ab\xff").unwrap_err(),
            "'utf-8' codec can't decode byte 0xff in position 2: invalid start byte"
        );
    }
}
