//! TT Games' CC4 v2 DAT archives (LEGO Star Wars: The Skywalker Saga): `ttdat.py` and `ttdat_jobs`, `ttdat_shaders`, `ttdat_programs` of `shader_split.py` (nxnative 9f834e41, PR #2785;
//! 5cd38d39: the add-on's DFLT members).
//!
//! The DAT's first u32 (little-endian) is where its index is (a value with bit 31 set is the compact form: `((v ^ 0xffffffff) + 1) << 8`), the second the index's length. The index is
//! big-endian, `.CC40TAD` at +4: u32 length - 4, the magic, i32 layout (-13), u32 version (2), file count, name count, string bytes; the strings, 4 bytes, the name records (12 bytes: u32
//! string offset, u16 parent, two u16, u16 file id; a folder when its file id is 0), i32 layout and u32 count again, the file records (16 bytes: u64 offset in the low 56 bits, u32 stored
//! length, u32 length in the low 31 bits), then a FNV-1a 64 hash per file record of its full upper-case, backslash-separated name: a name finds its record by its hash only.
//! Only `.nnx_shaders` members are read. A member is stored as it is, or a run of `{'OODL' | 'DFLT', u32 packed, u32 raw}` chunks: a chunk whose packed length is its raw length is stored;
//! a DFLT chunk is one final deflate block whose dynamic type is written 0 instead of 2; an OODL chunk is Oodle, which only the game's own decoder reads (the Mac host for the Python):
//! there is none on the phone, so such a member ends the split ([`Failure::Unsupported`]) instead of giving the game fewer shaders than it has.
//! A member holds GLSLC outputs and loose control sections, each with its code area adjacent or after reflection data ([`programs`]).
//!
//! The index is read when the jobs are planned (an index this reader recognises and cannot read is one job of its own: its red line, the split goes on); each job reads a range of members.

use crate::bytes::u32le;
use crate::ctl::{CTL_MAGIC, check, glslc_stage};
use crate::error::{Failure, Res, SplitError};
use crate::glslc::{adjacent_shader, glslc_shaders_ranges};
use crate::scan::{FileRef, finish};
use crate::scan_types::{Ctx, FileResult, Shader};
use crate::source::Source;
use crate::unity::Trees;
use crate::{red, unsupported};
use std::collections::{HashMap, HashSet};

pub const MAGIC: &[u8; 8] = b".CC40TAD";
pub const MAX_TABLE: u64 = 64 << 20;
pub const MAX_MEMBER: u64 = 64 << 20;
const PARTS: u64 = 64;

/// One member: (full name, offset, stored length, decoded length)
pub type Entry = (String, u64, u64, u64);

/// `ValueError(msg)` as `scan_file` reports it
fn value(msg: String) -> Failure {
    Failure::Red(format!("ValueError: {msg}"))
}

/// `_range(offset, size, end, what)`
fn range(offset: u64, size: u64, end: u64, what: &str) -> Result<(), String> {
    if offset > end || size > end - offset {
        return Err(format!(
            "{what}: range {offset:#x}+{size:#x} exceeds {end:#x} bytes"
        ));
    }
    Ok(())
}

/// `str(UnicodeDecodeError)` of `bytes.decode('utf-8')`
fn utf8_error(b: &[u8], e: std::str::Utf8Error) -> String {
    let s = e.valid_up_to();
    let (n, why) = match e.error_len() {
        Some(n) => {
            let lead = b[s];
            let bad_start = (0x80..=0xc1).contains(&lead) || lead >= 0xf5;
            (
                n,
                if bad_start {
                    "invalid start byte"
                } else {
                    "invalid continuation byte"
                },
            )
        }
        None => (b.len() - s, "unexpected end of data"),
    };
    if n == 1 {
        format!(
            "'utf-8' codec can't decode byte {:#04x} in position {s}: {why}",
            b[s]
        )
    } else {
        format!(
            "'utf-8' codec can't decode bytes in position {s}-{}: {why}",
            s + n - 1
        )
    }
}

/// `repr(bytes)`
fn bytes_repr(b: &[u8]) -> String {
    let q = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut s = format!("b{q}");
    for &c in b {
        match c {
            b'\\' => s.push_str("\\\\"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            c if c as char == q => {
                s.push('\\');
                s.push(q);
            }
            0x20..=0x7e => s.push(c as char),
            c => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push(q);
    s
}

/// `name_hash(name)`: FNV-1a 64 of the name with `/` as `\`, no leading `\`, upper case
pub fn name_hash(name: &str) -> u64 {
    let n = name.replace('/', "\\");
    let n = n.trim_start_matches('\\').to_uppercase();
    let mut v: u64 = 0xcbf29ce484222325;
    for b in n.bytes() {
        v = (v ^ b as u64).wrapping_mul(1099511628211);
    }
    v
}

/// `index_header(f)`: (index offset, length, its 32-byte header), `Ok(None)` for a file of another format, `Err` (the `ValueError`'s text) for a CC4 index this reader cannot read
pub fn index_header(src: &dyn Source) -> Result<Option<(u64, u64, Vec<u8>)>, String> {
    let size = src.len();
    let h = src.read(0, 8).map_err(|e| format!("OSError: {e}"))?;
    if h.len() < 8 {
        return Ok(None);
    }
    let mut offset = u32::from_le_bytes(h[..4].try_into().unwrap()) as u64;
    let length = u32::from_le_bytes(h[4..8].try_into().unwrap()) as u64;
    if offset & 0x8000_0000 != 0 {
        offset = ((offset ^ 0xffff_ffff) + 1) << 8;
    }
    if offset < 8 || offset as i128 > size as i128 - 12 {
        return Ok(None);
    }
    let header = src.read(offset, 32).map_err(|e| format!("OSError: {e}"))?;
    if header.get(4..12) != Some(&MAGIC[..]) {
        return Ok(None);
    }
    if header.len() < 32 {
        return Err("CC4 index: truncated header".into());
    }
    range(offset, length, size, "CC4 index")?;
    if !(32..=MAX_TABLE).contains(&length) {
        return Err(format!("CC4 index: invalid length {length}"));
    }
    Ok(Some((offset, length, header)))
}

/// `entries(path)`: the `.nnx_shaders` members, `Ok(None)` for a file of another format; `Err` is the text `ttdat_jobs` puts in the archive's job (`str(e)`). `path` is the file's path as
/// the Python names it in its messages
pub fn entries(path: &str, src: &dyn Source) -> Result<Option<Vec<Entry>>, String> {
    let Some((offset, length, h)) = index_header(src)? else {
        return Ok(None);
    };
    let be = |b: &[u8], o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap());
    let (hlen, kind, version, count, names, strings) = (
        be(&h, 0) as u64,
        be(&h, 12) as i32,
        be(&h, 16),
        be(&h, 20) as u64,
        be(&h, 24) as u64,
        be(&h, 28) as u64,
    );
    if version != 2 || kind != -13 {
        return Err(format!(
            "{path}: unsupported CC4 version {version}, layout {kind}"
        ));
    }
    if hlen + 4 != length {
        return Err(format!(
            "{path}: CC4 index length disagrees with its header"
        ));
    }
    let names_at = 32 + strings + 4;
    let files_at = names_at + 12 * names + 8;
    let hashes_at = files_at + 16 * count;
    range(32, strings, length, "CC4 name strings")?;
    range(names_at, 12 * names + 8, length, "CC4 name records")?;
    range(files_at, 24 * count, length, "CC4 files and hashes")?;
    let data = src
        .read(offset, length as usize)
        .map_err(|e| format!("OSError: {e}"))?;
    if data.len() as u64 != length {
        return Err(format!("{path}: truncated CC4 index"));
    }
    let (fa, ha, na) = (files_at as usize, hashes_at as usize, names_at as usize);
    if be(&data, fa - 8) as i32 != kind || be(&data, fa - 4) as u64 != count {
        return Err(format!("{path}: CC4 file counts disagree"));
    }
    let mut hashes: HashMap<u64, usize> = HashMap::new();
    for i in 0..count as usize {
        let key = u64::from_be_bytes(data[ha + 8 * i..ha + 8 * i + 8].try_into().unwrap());
        if hashes.insert(key, i).is_some() {
            return Err(format!("{path}: duplicate CC4 file hash"));
        }
    }
    let mut folders: HashMap<u64, String> = HashMap::new();
    folders.insert(0, String::new());
    let mut out = Vec::new();
    let s_end = 32 + strings as usize;
    for i in 0..names as usize {
        let r = na + 12 * i;
        let no = be(&data, r) as u64;
        let parent = u16::from_be_bytes([data[r + 4], data[r + 5]]) as u64;
        let file_id = u16::from_be_bytes([data[r + 10], data[r + 11]]);
        if no == 0xffff_ffff {
            continue;
        }
        range(no, 1, strings, "CC4 name")?;
        let a = 32 + no as usize;
        let Some(end) = data[a..s_end].iter().position(|&b| b == 0).map(|p| p + a) else {
            return Err(format!("{path}: unterminated CC4 name"));
        };
        let Some(base) = folders.get(&parent) else {
            return Err(format!(
                "{path}: CC4 name {i} has an unknown parent {parent}"
            ));
        };
        let name = std::str::from_utf8(&data[a..end]).map_err(|e| utf8_error(&data[a..end], e))?;
        let full = format!("{base}{name}");
        if full.chars().count() > 4096 {
            return Err(format!("{path}: CC4 path exceeds 4096 characters"));
        }
        if file_id == 0 && i as u64 != names - 1 {
            folders.insert(i as u64, full + "/");
            continue;
        }
        if !full.to_lowercase().ends_with(".nnx_shaders") {
            continue;
        }
        let Some(&k) = hashes.get(&name_hash(&full)) else {
            return Err(format!("{path}/{full}: no matching CC4 file hash"));
        };
        let f = fa + 16 * k;
        let encoded = u64::from_be_bytes(data[f..f + 8].try_into().unwrap());
        let stored = be(&data, f + 8) as u64;
        let raw = (be(&data, f + 12) & 0x7fff_ffff) as u64;
        let at = encoded & 0x00ff_ffff_ffff_ffff; // the top byte carries the packing flags, not the file address
        range(at, stored, offset, &format!("{path}/{full}"))?;
        if !(0 < raw && raw <= MAX_MEMBER) {
            return Err(format!("{path}/{full}: invalid shader member length {raw}"));
        }
        if !(0 < stored && stored <= MAX_MEMBER * 2) {
            return Err(format!(
                "{path}/{full}: invalid stored shader length {stored}"
            ));
        }
        out.push((full, at, stored, raw));
    }
    Ok(Some(out))
}

/// `_inflate_dflt(data, size, name)`: one final deflate block, its dynamic type written 0 instead of 2 (a fixed block keeps type 1); exactly `size` bytes and nothing after the stream
fn inflate_dflt(data: &[u8], size: usize, name: &str) -> Res<Vec<u8>> {
    let header = data[0] & 7;
    let fixed: Vec<u8>;
    let data = if header == 1 {
        fixed = [&[data[0] | 4][..], &data[1..]].concat();
        &fixed[..]
    } else if header != 3 {
        return Err(value(format!(
            "{name}: unsupported DFLT block header {header:#x}"
        )));
    } else {
        data
    };
    let disagrees = || {
        value(format!(
            "{name}: DFLT stream disagrees with its chunk length"
        ))
    };
    // the fast decoder for a clean stream; anything else is decided by the exact zlib model (where zlib stops at max_length, and whether it sees the damage before)
    if let Ok(z) = crate::inflate::run(data, crate::inflate::Wrap::Raw, Some(size + 1))
        && z.eof
        && z.out.len() == size
        && z.used == data.len()
    {
        return Ok(z.out);
    }
    match crate::zlibx::decompress_raw(data, Some(size + 1)) {
        Err(_) => Err(value(format!("{name}: invalid DFLT stream"))),
        Ok(z) if !z.eof || z.used != data.len() || z.out.len() != size => Err(disagrees()),
        Ok(z) => Ok(z.out),
    }
}

/// `ttdat.read(f, entry, decode)`: a member's bytes. An Oodle chunk (`OODL`, packed shorter than raw) needs the game's decoder: unsupported here
pub fn read(src: &dyn Source, e: &Entry) -> Res<Vec<u8>> {
    let (name, offset, stored, size) = (e.0.as_str(), e.1, e.2, e.3);
    let io = |x: std::io::Error| Failure::Red(format!("OSError: {x}"));
    let tag = src.read(offset, 4).map_err(io)?;
    if tag != b"OODL" && tag != b"DFLT" {
        if stored != size {
            return Err(value(format!(
                "{name}: unsupported TT Games compression {}",
                bytes_repr(&tag)
            )));
        }
        let data = src.read(offset, size as usize).map_err(io)?;
        if data.len() as u64 != size {
            return Err(value(format!("{name}: truncated stored member")));
        }
        return Ok(data);
    }
    let codec = std::str::from_utf8(&tag).unwrap_or("?");
    let (mut pos, mut remaining) = (offset, stored);
    let mut out: Vec<u8> = Vec::new();
    while remaining != 0 {
        if remaining < 12 {
            return Err(value(format!("{name}: truncated {codec} header")));
        }
        let h = src.read(pos, 12).map_err(io)?;
        if h.len() != 12 {
            return Err(value(format!("{name}: truncated {codec} header")));
        }
        pos += 12;
        let (packed, raw) = (u32le(&h, 4)? as u64, u32le(&h, 8)? as u64);
        if h[..4] != tag[..]
            || packed == 0
            || raw == 0
            || packed > (remaining - 12).min(MAX_MEMBER)
            || raw > size - out.len() as u64
        {
            return Err(value(format!("{name}: invalid {codec} chunk header")));
        }
        let data = src.read(pos, packed as usize).map_err(io)?;
        if data.len() as u64 != packed {
            return Err(value(format!("{name}: truncated {codec} chunk")));
        }
        pos += packed;
        let chunk = if packed == raw {
            data
        } else if tag == b"DFLT" {
            inflate_dflt(&data, raw as usize, name)?
        } else {
            unsupported!(
                "Oodle-compressed TT Games DAT member ({name}: the game's own decoder in the Mac host)"
            )
        };
        if chunk.len() as u64 != raw {
            return Err(value(format!("{name}: Oodle returned the wrong length")));
        }
        out.extend_from_slice(&chunk);
        remaining -= 12 + packed;
    }
    if out.len() as u64 != size {
        return Err(value(format!(
            "{name}: {codec} decoded length disagrees with the index"
        )));
    }
    Ok(out)
}

/// `ttdat_programs(data, label)`: the GLSLC outputs of a member, then every control section outside them with its code area: adjacent ([`adjacent_shader`], bounded by the outputs and the next
/// control section), or else exactly one headed code area of its length after the control section and before the next one (older programs put reflection data between them)
pub fn programs(data: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut glslc_ranges: Vec<(usize, usize)> = Vec::new();
    let mut found = glslc_shaders_ranges(data, label, false, &[], Some(&mut glslc_ranges))?;
    let n = data.len();
    let mut ranges = glslc_ranges.into_iter();
    let (mut start, mut stop) = ranges.next().unwrap_or((n, n));
    let mut previous_stop = 0usize;
    let controls: Vec<usize> = crate::bytes::find_all(data, &CTL_MAGIC).collect();
    for (i, &c) in controls.iter().enumerate() {
        while c >= stop {
            previous_stop = previous_stop.max(stop);
            (start, stop) = ranges.next().unwrap_or((n, n));
        }
        if start <= c {
            continue; // already read as GLSLC, by location; another program can reuse its control bytes
        }
        if c + 0x760 > n {
            red!("{label}: truncated control section @{c:#x}");
        }
        let (cn, kn) = (
            u32le(data, c + 0x20)? as usize,
            u32le(data, c + 0x704)? as usize,
        );
        let ctl = crate::bytes::at(data, c, cn);
        let end = start.min(controls.get(i + 1).copied().unwrap_or(n));
        let mut candidate = adjacent_shader(data, c, previous_stop, Some(end))?;
        if candidate.is_none() && cn >= 0x760 && c + cn <= n && kn >= 0x80 {
            let mut cands = Vec::new();
            let from = c + cn;
            let tail = crate::bytes::sl(data, from, end);
            for m in 0..tail.len().saturating_sub(3) {
                if !matches!(tail[m], b'x' | b'y') || tail[m + 1..m + 4] != [0x56, 0x34, 0x12] {
                    continue;
                }
                let k = from + m;
                if k + kn > end {
                    continue;
                }
                let code = &data[k..k + kn];
                if let Some(st) = glslc_stage(ctl, code)?
                    && check(st, ctl, code)?.is_none()
                {
                    cands.push((k, st, ctl, code));
                }
            }
            if cands.len() == 1 {
                candidate = Some(cands[0]);
            }
        }
        let Some((_, st, ctl, code)) = candidate else {
            red!("{label} control section @{c:#x}: no unique code area in its program");
        };
        found.push(Shader::new(format!("{label}#RAW@{c:#x}"), st, ctl, code));
    }
    Ok(found)
}

/// One job of `ttdat_jobs`: a range of one DAT's members, or the error of an index this reader recognises and cannot read (`(TTDAT, None, str(e))`)
#[derive(Clone)]
pub struct TtJob {
    pub file: FileRef,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

impl TtJob {
    pub fn identity(&self) -> Vec<String> {
        let mut v = vec![format!(
            "ttdat:{}:{}:{}:{:?}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.error
        )];
        v.extend(self.entries.iter().map(|e| format!("{e:?}")));
        v
    }
}

/// The TT DAT plan: (the jobs, the files they read as (root, rel)).
pub type TtPlan = (Vec<TtJob>, HashSet<(usize, String)>);

/// `ttdat_jobs(files)`: every `.dat` whose index is a CC4 one: the ones whose index cannot be read first (one job each), then the members of the others in ranges of about equal stored
/// length. A DAT with no shader member is taken all the same (it is not scanned as a loose file). `path_of(f)` is the path the Python names the file by
pub fn plan(files: &[FileRef], path_of: &dyn Fn(&FileRef) -> String) -> Result<TtPlan, SplitError> {
    let mut jobs = Vec::new();
    let mut taken = HashSet::new();
    let mut found: Vec<(&FileRef, Vec<Entry>)> = Vec::new();
    for f in files {
        if !f.rel.to_lowercase().ends_with(".dat") {
            continue;
        }
        let src = match f.loc.open() {
            Ok(s) => s,
            Err(e) => {
                jobs.push(TtJob {
                    file: f.clone(),
                    entries: vec![],
                    error: Some(format!("{e}")),
                });
                taken.insert((f.root, f.rel.clone()));
                continue;
            }
        };
        match entries(&path_of(f), &src) {
            Ok(None) => {}
            Ok(Some(es)) => {
                taken.insert((f.root, f.rel.clone()));
                found.push((f, es));
            }
            Err(e) => {
                taken.insert((f.root, f.rel.clone()));
                jobs.push(TtJob {
                    file: f.clone(),
                    entries: vec![],
                    error: Some(e),
                });
            }
        }
    }
    let total: u64 = found.iter().flat_map(|x| x.1.iter()).map(|e| e.2).sum();
    let step = (total / PARTS).max(1);
    for (f, es) in found {
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in es {
            size += e.2;
            cur.push(e);
            if size >= step {
                jobs.push(TtJob {
                    file: f.clone(),
                    entries: std::mem::take(&mut cur),
                    error: None,
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(TtJob {
                file: f.clone(),
                entries: cur,
                error: None,
            });
        }
    }
    Ok((jobs, taken))
}

fn run(cx: &Ctx<'_>, job: &TtJob) -> Res<FileResult> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    if let Some(e) = &job.error {
        red!("{e}");
    }
    let mut found = Vec::new();
    for e in &job.entries {
        let label = format!("/{}/{}", job.file.rel, e.0);
        let d = read(&src, e)?;
        found.extend(programs(&d, &label)?);
    }
    finish(cx, &job.file.rel, found, std::iter::empty(), Trees::new())
}

/// `scan_file((path, rel, (TTDAT, entries)))`: an NXSD magic at the file's start is read as NXSD first, as `scan_file_in` does; a member that cannot be read leaves the job's shaders out
pub fn scan(cx: &Ctx<'_>, job: &TtJob) -> Result<FileResult, String> {
    if job
        .file
        .loc
        .open()
        .and_then(|s| s.read(0, 4))
        .is_ok_and(|h| h == crate::nxsd::MAGIC)
    {
        return crate::scan::scan_file(cx, &job.file);
    }
    crate::falcom::left_out(cx, &job.file.rel, run(cx, job))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_texts() {
        assert_eq!(bytes_repr(b"OO\x00\x7f\\'"), r#"b"OO\x00\x7f\\'""#);
        assert_eq!(bytes_repr(b"'\""), r#"b'\'"'"#);
        assert_eq!(bytes_repr(b"\t\n\r"), r"b'\t\n\r'");
        let cases: [(&[u8], &str); 6] = [
            (
                b"\xe4\xb8x",
                "bytes in position 0-1: invalid continuation byte",
            ),
            (
                b"\xe4x",
                "byte 0xe4 in position 0: invalid continuation byte",
            ),
            (b"\xff", "byte 0xff in position 0: invalid start byte"),
            (b"\xe4\xb8", "bytes in position 0-1: unexpected end of data"),
            (b"a\xc0\x80", "byte 0xc0 in position 1: invalid start byte"),
            (
                b"\xed\xa0\x80",
                "byte 0xed in position 0: invalid continuation byte",
            ),
        ];
        for (b, want) in cases {
            let e = std::str::from_utf8(b).unwrap_err();
            assert_eq!(
                utf8_error(b, e),
                format!("'utf-8' codec can't decode {want}")
            );
        }
        assert_eq!(
            name_hash("SHADERS\\SAMPLE.NNX_SHADERS"),
            name_hash("/shaders/sample.nnx_shaders")
        );
    }
}
