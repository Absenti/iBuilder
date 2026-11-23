//! Blizzard's CASC storage as Diablo II: Resurrected ships it on the Switch: `casc.py` and `casc_jobs` / `casc_shaders` of `shader_split.py`.
//!
//! A RomFS folder holds `build_key.txt` (the current build's config key), text manifests (`*.psv`: pipe-separated, a header line naming the columns, one line per file) and the
//! archives `<chunk>-<archive>.data`. The manifest whose `## config-key` names the build key lists the current files: their archive and offset (the encoded key's last five bytes),
//! stored and decoded length, and encoding (`n` stored, `z` one zlib stream, `b:...` a BLTE container of `N` / `Z` chunks). Every listed file is read by a positioned read of its own
//! range, decoded and scanned like a loose file (the shaders are SHAD containers, `glslc::shad_shaders`). No file of this layout is encrypted.

use crate::error::{Failure, Res, SplitError};
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use std::collections::{BTreeSet, HashMap, HashSet};

const COLUMNS: [&str; 7] = [
    "FileId", "Chunk", "Archive", "CSize", "EKey", "ESize", "Encoding",
];

/// `casc.Error` (or another exception the Python reader raises: there it ends the split, as it is not caught)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CascErr {
    /// `casc.Error(msg)`
    Casc(String),
    /// an exception `casc_jobs` does not catch (`ValueError: ...`, `UnicodeDecodeError: ...`): the Python's split ends with its traceback
    Other(String),
}

/// One file of the current build: `(file ID, archive name, offset, stored length, decoded length, encoding)`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub archive: String,
    pub off: u64,
    pub stored: i128,
    pub size: i128,
    pub enc: String,
}

/// A range of one archive's files (`casc_jobs`)
#[derive(Clone)]
pub struct CascJob {
    pub file: FileRef,
    pub entries: Vec<Entry>,
}

impl CascJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "casc:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(self.entries.iter().map(|e| format!("{e:?}")))
        .collect()
    }
}

// ---- Python's text rules -----------------------------------------------------------------------------------------------------------

/// `str.strip()` (ASCII whitespace as `str.isspace` has it, and U+0085)
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(
            c,
            ' ' | '\t'
                | '\n'
                | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\x1f'
                | '\u{85}'
                | '\u{a0}'
        ) || (c as u32 > 0xa0 && c.is_whitespace())
    })
}

/// `str.splitlines()`
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if brk {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r'
                && let Some(&(_, '\n')) = it.peek()
            {
                it.next();
                next += 1;
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `repr(s)` of a `str`
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c)
            }
            c if (c as u32) < 0x20 || (0x7f..0xa1).contains(&(c as u32)) || c == '\u{ad}' => {
                o.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

/// `repr(b)` of a `bytes`
pub fn py_bytes_repr(b: &[u8]) -> String {
    let q = if b.contains(&b'\'') && !b.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut o = String::from("b");
    o.push(q as char);
    for &c in b {
        match c {
            b'\\' => o.push_str("\\\\"),
            b'\n' => o.push_str("\\n"),
            b'\r' => o.push_str("\\r"),
            b'\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c as char)
            }
            0x20..=0x7e => o.push(c as char),
            c => o.push_str(&format!("\\x{c:02x}")),
        }
    }
    o.push(q as char);
    o
}

/// `int(s)` (base 10): signs, surrounding whitespace and single underscores between digits; `Err` the `ValueError` text
pub fn py_int(s: &str) -> Result<i128, String> {
    let bad = || format!("invalid literal for int() with base 10: {}", py_repr(s));
    let t = py_strip(s);
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return Err(bad());
    }
    let mut v: i128 = 0;
    for c in body.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10).ok_or_else(bad)? as i128;
        v = v
            .checked_mul(10)
            .and_then(|x| x.checked_add(d))
            .ok_or_else(|| format!("{} is too large for this reader", py_repr(s)))?;
    }
    Ok(if neg { -v } else { v })
}

/// `bytes.fromhex(s)`; `Err` the `ValueError` text
pub fn py_fromhex(s: &str) -> Result<Vec<u8>, String> {
    let b: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < b.len() && py_strip(&b[i].to_string()).is_empty() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let hi = b[i]
            .to_digit(16)
            .filter(|_| b[i].is_ascii())
            .ok_or_else(|| {
                format!("non-hexadecimal number found in fromhex() arg at position {i}")
            })?;
        if i + 1 >= b.len() {
            return Err("fromhex() arg must contain an even number of hexadecimal digits".into());
        }
        let lo = b[i + 1]
            .to_digit(16)
            .filter(|_| b[i + 1].is_ascii())
            .ok_or_else(|| {
                if py_strip(&b[i + 1].to_string()).is_empty() {
                    "fromhex() arg must contain an even number of hexadecimal digits".to_string()
                } else {
                    format!(
                        "non-hexadecimal number found in fromhex() arg at position {}",
                        i + 1
                    )
                }
            })?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Ok(out)
}

// ---- casc.py -----------------------------------------------------------------------------------------------------------------------

pub fn archive_name(chunk: u8, archive: u8) -> String {
    format!("{chunk:02}-{archive:08}.data")
}

/// `is_casc(folder)` over the names in the folder
pub fn is_casc(names: &BTreeSet<String>) -> bool {
    names.contains("build_key.txt")
        && names.iter().any(|n| n.ends_with(".psv"))
        && names
            .iter()
            .any(|n| n.ends_with(".data") && n.len() == 16 && n.as_bytes()[2] == b'-')
}

type Row = Vec<(String, String)>;

fn get<'a>(r: &'a Row, k: &str) -> &'a str {
    // `dict(zip(cols, f))`: a column named twice keeps its last field
    r.iter()
        .rev()
        .find(|(c, _)| c == k)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// `manifest(text)` → (its config key, its file lines as rows)
pub fn manifest(text: &str) -> Result<(Option<String>, Vec<Row>), String> {
    let (mut key, mut cols, mut rows): (Option<String>, Option<Vec<String>>, Vec<Row>) =
        (None, None, Vec::new());
    for line in py_splitlines(text) {
        if line.is_empty() {
            continue;
        }
        if let Some(_rest) = line.strip_prefix('#') {
            let l = line.trim_start_matches('#');
            let (k, v) = l.split_once('=').unwrap_or((l, ""));
            if py_strip(k) == "config-key" {
                key = Some(py_strip(v).to_string());
            }
            continue;
        }
        let f: Vec<&str> = line.split('|').collect();
        let short: String = line.chars().take(80).collect();
        match &cols {
            None => {
                if !f[0].contains('!') {
                    return Err(format!(
                        "no header line before the first file line: {}",
                        py_repr(&short)
                    ));
                }
                let c: Vec<String> = f
                    .iter()
                    .map(|c| c.split('!').next().unwrap_or("").to_string())
                    .collect();
                let missing: Vec<&str> = COLUMNS
                    .iter()
                    .copied()
                    .filter(|m| !c.iter().any(|x| x == m))
                    .collect();
                if !missing.is_empty() {
                    return Err(format!(
                        "manifest without the columns {}",
                        missing.join(", ")
                    ));
                }
                cols = Some(c);
            }
            Some(c) => {
                if f.len() != c.len() {
                    return Err(format!(
                        "{} fields where the header names {}: {}",
                        f.len(),
                        c.len(),
                        py_repr(&short)
                    ));
                }
                rows.push(
                    c.iter()
                        .cloned()
                        .zip(f.iter().map(|s| s.to_string()))
                        .collect(),
                );
            }
        }
    }
    if cols.is_none() {
        return Err("no header line".into());
    }
    Ok((key, rows))
}

/// `files(folder)`: the current build's files, in manifest order. `names` are the folder's entries (sorted), `read` gives a file's bytes, `folder` names it as the Python does
pub fn files(
    folder: &str,
    names: &BTreeSet<String>,
    read: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<Entry>, CascErr> {
    let other = CascErr::Other;
    let bk = read("build_key.txt").map_err(other)?;
    if let Some(i) = bk.iter().position(|b| !b.is_ascii()) {
        return Err(CascErr::Other(format!(
            "UnicodeDecodeError: 'ascii' codec can't decode byte {:#04x} in position {i}: ordinal not in range(128)",
            bk[i]
        )));
    }
    let want = py_strip(std::str::from_utf8(&bk).unwrap()).to_string();
    for n in names {
        if !n.ends_with(".psv") {
            continue;
        }
        let b = read(n).map_err(other)?;
        let text = crate::lspk::utf8(&b)
            .map_err(|e| CascErr::Other(format!("UnicodeDecodeError: {e}")))?;
        let head: String = text.chars().take(4096).collect();
        if !(text.starts_with('#') || text.starts_with("FileId")) || !head.contains("FileId!") {
            continue; // archive_manifest.psv lists the archives, not files
        }
        let (key, rows) = manifest(&text).map_err(CascErr::Casc)?;
        if key.as_deref() != Some(want.as_str()) {
            continue;
        }
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let ek = get(r, "EKey");
            let e = py_fromhex(ek).map_err(|x| CascErr::Other(format!("ValueError: {x}")))?;
            let chunk =
                py_int(get(r, "Chunk")).map_err(|x| CascErr::Other(format!("ValueError: {x}")))?;
            let arch = py_int(get(r, "Archive"))
                .map_err(|x| CascErr::Other(format!("ValueError: {x}")))?;
            if e.len() != 16 || (e[9] as i128, e[10] as i128) != (chunk, arch) {
                return Err(CascErr::Casc(format!(
                    "{n}: {}: encoded key {ek} does not end in chunk {chunk}, archive {arch} and an offset",
                    get(r, "FileId")
                )));
            }
            let off = e[11..].iter().fold(0u64, |a, &b| (a << 8) | b as u64);
            out.push(Entry {
                id: get(r, "FileId").to_string(),
                archive: archive_name(e[9], e[10]),
                off,
                stored: py_int(get(r, "ESize"))
                    .map_err(|x| CascErr::Other(format!("ValueError: {x}")))?,
                size: py_int(get(r, "CSize"))
                    .map_err(|x| CascErr::Other(format!("ValueError: {x}")))?,
                enc: get(r, "Encoding").to_string(),
            });
        }
        return Ok(out);
    }
    Err(CascErr::Casc(format!(
        "no manifest in {folder} has the config key {want} that build_key.txt names"
    )))
}

/// `zlib.decompress(d)`: the stream (bytes after it ignored), or the `zlib.error` text
pub fn zlib_decompress(d: &[u8]) -> Result<Vec<u8>, String> {
    use crate::inflate::{MAX_DECODED, Wrap, cap_error, run};
    match run(d, Wrap::Zlib, None) {
        Ok(r) if r.eof => return Ok(r.out),
        Err(e) if e == cap_error(MAX_DECODED) => return Err(e),
        _ => {}
    }
    // the exact zlib model for the error text
    let z = crate::zlibx::decompress(d, Some(MAX_DECODED + 1))?;
    if !z.eof {
        if z.out.len() > MAX_DECODED {
            return Err(cap_error(MAX_DECODED));
        }
        return Err("Error -5 while decompressing data: incomplete or truncated stream".into());
    }
    Ok(z.out)
}

/// A failure of reading one file: `casc.Error` / `zlib.error` (the file is not scanned, the job goes on) or another exception (the job ends)
pub enum ReadErr {
    Skip(String),
    Job(String),
}

/// `struct.error` of `unpack_from(fmt, d, at)` that needs `n` bytes
fn struct_err(d: &[u8], at: usize, n: usize) -> ReadErr {
    ReadErr::Job(format!(
        "error: unpack_from requires a buffer of at least {} bytes for unpacking {n} bytes at offset {at} (actual buffer size is {})",
        at + n,
        d.len()
    ))
}

fn decode_chunk(c: &[u8]) -> Result<Vec<u8>, ReadErr> {
    match c.first() {
        Some(b'N') => Ok(c[1..].to_vec()),
        Some(b'Z') => zlib_decompress(&c[1..]).map_err(ReadErr::Skip),
        _ => Err(ReadErr::Skip(format!(
            "BLTE chunk mode {}",
            py_bytes_repr(&c[..c.len().min(1)])
        ))),
    }
}

/// `blte(d)`: a BLTE container's decoded bytes
pub fn blte(d: &[u8]) -> Result<Vec<u8>, ReadErr> {
    if d.len() < 4 || &d[..4] != b"BLTE" {
        return Err(ReadErr::Skip(format!(
            "not BLTE: {}",
            crate::bytes::hex(&d[..d.len().min(4)])
        )));
    }
    if d.len() < 8 {
        return Err(struct_err(d, 4, 4));
    }
    let hl = u32::from_be_bytes(d[4..8].try_into().unwrap()) as usize;
    if hl == 0 {
        return decode_chunk(&d[8..]);
    }
    let Some(&flags) = d.get(8) else {
        return Err(ReadErr::Job("IndexError: index out of range".into()));
    };
    if flags != 0x0f {
        return Err(ReadErr::Skip(format!("BLTE chunk table flags {flags:#x}")));
    }
    let n = d[9..d.len().min(12)]
        .iter()
        .fold(0usize, |a, &b| (a << 8) | b as usize);
    if 12 + 24 * n != hl {
        return Err(ReadErr::Skip(format!(
            "BLTE header of {hl} bytes for {n} chunks"
        )));
    }
    let (mut at, mut out) = (hl, Vec::new());
    for i in 0..n {
        let o = 12 + 24 * i;
        if o + 8 > d.len() {
            return Err(struct_err(d, o, 8));
        }
        let stored = u32::from_be_bytes(d[o..o + 4].try_into().unwrap()) as usize;
        let size = u32::from_be_bytes(d[o + 4..o + 8].try_into().unwrap()) as usize;
        let c = &d[at.min(d.len())..at.saturating_add(stored).min(d.len())];
        let x = decode_chunk(c)?;
        if x.len() != size {
            return Err(ReadErr::Skip(format!(
                "BLTE chunk {i}: {} bytes where the table says {size}",
                x.len()
            )));
        }
        out.extend_from_slice(&x);
        if out.len() > crate::inflate::MAX_DECODED {
            return Err(ReadErr::Skip(crate::inflate::cap_error(
                crate::inflate::MAX_DECODED,
            )));
        }
        at += stored;
    }
    if at != d.len() {
        return Err(ReadErr::Skip(format!(
            "BLTE chunks end at {at}, the container at {}",
            d.len()
        )));
    }
    Ok(out)
}

/// `decode(d, enc, size)`
pub fn decode(d: Vec<u8>, enc: &str, size: i128) -> Result<Vec<u8>, ReadErr> {
    let x = if enc == "n" {
        d
    } else if enc == "z" {
        zlib_decompress(&d).map_err(ReadErr::Skip)?
    } else if enc.starts_with("b:") {
        blte(&d)?
    } else {
        return Err(ReadErr::Skip(format!("encoding {}", py_repr(enc))));
    };
    if x.len() as i128 != size {
        return Err(ReadErr::Skip(format!(
            "{} bytes where the manifest says {size}",
            x.len()
        )));
    }
    Ok(x)
}

/// `read(f, ent)`: one file of an open archive
pub fn read(src: &dyn Source, e: &Entry) -> Result<Vec<u8>, ReadErr> {
    // `f.read(stored)` with a negative length reads to the end
    let want = if e.stored < 0 {
        src.len().saturating_sub(e.off) as usize
    } else {
        e.stored.min(usize::MAX as i128) as usize
    };
    if e.stored > src.len() as i128 {
        return Err(ReadErr::Skip(format!(
            "{} bytes at {:#x} run past the archive's end",
            e.stored, e.off
        )));
    }
    let d = src
        .read(e.off, want)
        .map_err(|x| ReadErr::Job(format!("OSError: {x}")))?;
    if d.len() as i128 != e.stored {
        return Err(ReadErr::Skip(format!(
            "{} bytes at {:#x} run past the archive's end",
            e.stored, e.off
        )));
    }
    decode(d, &e.enc, e.size)
}

// ---- the split's jobs --------------------------------------------------------------------------------------------------------------

fn dirname(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => "",
    }
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `casc_jobs(files)`: the CASC folders (a `build_key.txt` beside a manifest and an archive), their current files by archive in ranges of about equal stored length. Returns
/// the jobs and the archives taken (every `.data` file of the folder). A folder whose manifests do not give the current files is logged and its archives scanned as loose files.
pub fn plan(
    files: &[FileRef],
    log: &(dyn Fn(&str) + Sync),
    path_of: &dyn Fn(&FileRef) -> String,
) -> Result<(Vec<CascJob>, crate::jobs::Taken), SplitError> {
    const PARTS: f64 = 64.0;
    // the folders, by the path the Python names them by (sorted as it sorts them)
    let mut dirs: Vec<(String, usize, String)> = files
        .iter()
        .filter(|f| basename(&f.rel) == "build_key.txt")
        .map(|f| {
            (
                dirname(&path_of(f)).to_string(),
                f.root,
                dirname(&f.rel).to_string(),
            )
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    let (mut jobs, mut taken) = (Vec::new(), HashSet::new());
    for (d, root, rdir) in dirs {
        let inside = |f: &FileRef| f.root == root && dirname(&f.rel) == rdir;
        let below = |f: &FileRef| {
            f.root == root && (rdir.is_empty() || f.rel.starts_with(&format!("{rdir}/")))
        };
        // `os.listdir(d)`: the files in it and the folders below it
        let names: BTreeSet<String> = files
            .iter()
            .filter(|f| below(f))
            .map(|f| {
                let rest = if rdir.is_empty() {
                    f.rel.as_str()
                } else {
                    &f.rel[rdir.len() + 1..]
                };
                rest.split('/').next().unwrap_or(rest).to_string()
            })
            .collect();
        if !is_casc(&names) {
            continue;
        }
        let rel_of: HashMap<String, &FileRef> = files
            .iter()
            .filter(|f| inside(f))
            .map(|f| (basename(&f.rel).to_string(), f))
            .collect();
        let read = |n: &str| -> Result<Vec<u8>, String> {
            match rel_of.get(n) {
                Some(f) => f
                    .loc
                    .open()
                    .and_then(|s| s.read_all())
                    .map_err(|e| format!("OSError: {e}")),
                None => Err(format!(
                    "IsADirectoryError: [Errno 21] Is a directory: '{d}/{n}'"
                )),
            }
        };
        let es = match files_of(&d, &names, &read) {
            Ok(es) => es,
            Err(CascErr::Casc(e)) => {
                log(&format!(
                    "[RED] {d}: {e}; its archives are scanned as loose files"
                ));
                continue;
            }
            Err(CascErr::Other(e)) => return Err(SplitError::Red(format!("{d}: {e}"))),
        };
        // `by`: the archives in the order the manifest first names them
        let mut by: Vec<(String, Vec<Entry>)> = Vec::new();
        for e in es.iter() {
            match by.iter_mut().find(|(a, _)| *a == e.archive) {
                Some((_, v)) => v.push(e.clone()),
                None => by.push((e.archive.clone(), vec![e.clone()])),
            }
        }
        if let Some((a, _)) = by.iter().find(|(a, _)| !rel_of.contains_key(a)) {
            return Err(SplitError::Red(format!(
                "{d}: the manifest names archive {a}, which the RomFS lacks"
            )));
        }
        let step = es.iter().map(|e| e.stored).sum::<i128>() as f64 / PARTS;
        by.sort_by(|a, b| a.0.cmp(&b.0));
        for (a, mut v) in by {
            let f = rel_of[&a];
            v.sort_by_key(|e| e.off);
            let (mut cur, mut size) = (Vec::new(), 0i128);
            for e in v {
                size += e.stored;
                cur.push(e);
                if size as f64 >= step {
                    jobs.push(CascJob {
                        file: (*f).clone(),
                        entries: std::mem::take(&mut cur),
                    });
                    size = 0;
                }
            }
            if !cur.is_empty() {
                jobs.push(CascJob {
                    file: (*f).clone(),
                    entries: cur,
                });
            }
        }
        for f in files
            .iter()
            .filter(|f| inside(f) && f.rel.ends_with(".data"))
        {
            taken.insert((f.root, f.rel.clone()));
        }
    }
    Ok((jobs, taken))
}

fn files_of(
    d: &str,
    names: &BTreeSet<String>,
    read: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Vec<Entry>, CascErr> {
    files(d, names, read)
}

/// `casc_shaders(path, rel, ents, out)`: each file of the range decoded and scanned like a loose file, labelled `/<folder>/<file ID>`; a file that does not decode is logged and
/// not scanned
pub fn shaders(cx: &Ctx<'_>, job: &CascJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    for e in &job.entries {
        let label = format!("/{}/{}", dirname(&job.file.rel), e.id);
        match read(&src, e) {
            Ok(d) => scan_blob(cx, &d, &label, &mut out, 0, None)?,
            Err(ReadErr::Skip(x)) => cx.print(&format!("[RED] {label}: {x}; not scanned")),
            Err(ReadErr::Job(x)) => return Err(Failure::Red(x)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_text_rules() {
        assert_eq!(py_int(" +1_0 ").unwrap(), 10);
        assert!(py_int("1x").is_err());
        assert_eq!(
            py_int("1x").unwrap_err(),
            "invalid literal for int() with base 10: '1x'"
        );
        assert_eq!(py_fromhex("00 11  22").unwrap(), vec![0, 0x11, 0x22]);
        assert_eq!(
            py_fromhex("0g").unwrap_err(),
            "non-hexadecimal number found in fromhex() arg at position 1"
        );
        assert_eq!(
            py_fromhex("012").unwrap_err(),
            "fromhex() arg must contain an even number of hexadecimal digits"
        );
        assert_eq!(py_repr("a'b"), "\"a'b\"");
        assert_eq!(py_repr("a\"b'c"), "'a\"b\\'c'");
        assert_eq!(py_repr("\x7f\t"), "'\\x7f\\t'");
        assert_eq!(py_bytes_repr(b""), "b''");
        assert_eq!(py_bytes_repr(b"\0"), "b'\\x00'");
        assert_eq!(
            py_splitlines("a\r\nb\rc\n\nd"),
            vec!["a", "b", "c", "", "d"]
        );
    }
}
