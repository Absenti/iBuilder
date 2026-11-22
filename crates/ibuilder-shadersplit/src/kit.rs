//! KIT archives (Unravel TWO): `kit.py` and `shader_split.kit_jobs` / the KIT branch of `scan_file_in` (the rebased engine; the older one had no KIT reader).
//!
//! An index file `<name>.kit` and its data in numbered parts `<name>.kit.0`, `.1`, ...: magic `kit!`, +0x10 u64 the directory's offset after the 0x25-byte header, +0x18 u32 its record count;
//! 56-byte records of {u32 flags (8: chained LZ4 blocks), u16 part, u32 offset, u16 high offset word, u64 stored size, u64 decoded size, the decoded data's SHA-1, two u32 not read}.
//! Members are named only by hash, so every one is decoded and scanned (`scan_blob`: Phyre effects). A compressed member is a run of `{u32 length, LZ4 block}` that decode to 64 KB each (the last
//! one shorter), each block's matches reaching into the 64 KB decoded before it. The engine reads the index when it plans the jobs (a malformed index ends the whole split: `red`) and a range of
//! members per job (a malformed member leaves its job's shaders out, as any unreadable file).

use crate::error::{Failure, Res, SplitError};
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Extra, FileResult, Shader};
use crate::source::Source;
use std::collections::{HashMap, HashSet};

pub const MAGIC: &[u8; 4] = b"kit!";
const CHUNK: usize = 0x10000;
const HEADER: usize = 0x25;
const RECORD: usize = 56;

/// `kit.Entry`
#[derive(Clone, Debug)]
pub struct Entry {
    /// the record's number in the index
    pub index: usize,
    /// the data is in `<index path>.<part>`
    pub part: u16,
    pub offset: u64,
    /// bytes it takes there: the stored size when compressed, else the decoded size
    pub length: u64,
    /// decoded size
    pub size: u64,
    pub compressed: bool,
    /// SHA-1 of the decoded data
    pub sha1: [u8; 20],
}

/// One job: a range of one archive's members (`(path, rel, (KIT, entries))`)
#[derive(Clone)]
pub struct KitJob {
    pub file: FileRef,
    /// the path the Python's messages name: the RomFS argument, then the file's path below it
    pub path: String,
    pub entries: Vec<Entry>,
}

impl KitJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "kit:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(self.entries.iter().map(|e| format!("{e:?}")))
        .collect()
    }
}

/// `kit.entries(path)`: the index's members, each checked to lie inside its part (and the part to exist). `part_len(n)` is the size of `<path>.<n>`, None when there is no such file
pub fn entries(
    path: &str,
    d: &[u8],
    part_len: &dyn Fn(u16) -> Option<u64>,
) -> Result<Vec<Entry>, String> {
    if d.len() < HEADER || d[..4] != MAGIC[..] {
        return Err(format!("{path}: incomplete KIT header"));
    }
    let base = HEADER as u128 + u64::from_le_bytes(d[0x10..0x18].try_into().unwrap()) as u128;
    let count = u32::from_le_bytes(d[0x18..0x1c].try_into().unwrap()) as u128;
    if base > d.len() as u128 || count > (d.len() as u128 - base) / RECORD as u128 {
        return Err(format!("{path}: KIT directory is outside the index"));
    }
    let (base, count) = (base as usize, count as usize);
    let mut out = Vec::with_capacity(count);
    let mut sizes: HashMap<u16, u64> = HashMap::new();
    for i in 0..count {
        let r = &d[base + RECORD * i..base + RECORD * (i + 1)];
        let flags = u32::from_le_bytes(r[0..4].try_into().unwrap());
        let part = u16::from_le_bytes(r[4..6].try_into().unwrap());
        let off = u32::from_le_bytes(r[6..10].try_into().unwrap()) as u64;
        let high = u16::from_le_bytes(r[10..12].try_into().unwrap());
        let stored = u64::from_le_bytes(r[12..20].try_into().unwrap());
        let size = u64::from_le_bytes(r[20..28].try_into().unwrap());
        let sha: [u8; 20] = r[28..48].try_into().unwrap();
        if high != 0 {
            return Err(format!(
                "{path}: KIT entry {i} has an unsupported high offset word"
            ));
        }
        let plen = match sizes.get(&part) {
            Some(&n) => n,
            None => {
                let n = part_len(part)
                    .ok_or_else(|| format!("{path}: KIT entry {i} has no part {part}"))?;
                sizes.insert(part, n);
                n
            }
        };
        let n = if flags & 8 != 0 { stored } else { size };
        if off > plen || n > plen - off {
            return Err(format!("{path}: KIT entry {i} is outside part {part}"));
        }
        out.push(Entry {
            index: i,
            part,
            offset: off,
            length: n,
            size,
            compressed: flags & 8 != 0,
            sha1: sha,
        });
    }
    Ok(out)
}

/// The kit plan: (the jobs, the files they read as (root, rel)).
pub type KitPlan = (Vec<KitJob>, HashSet<(usize, String)>);

/// `kit_jobs(files)`: the archives' members in ranges of about equal length, and the files they read (the index and its parts: not scanned as loose files).
/// `path_of(f)` is the path the Python names the file by; a malformed index is `red()` for the whole split
pub fn plan(
    files: &[FileRef],
    path_of: &dyn Fn(&FileRef) -> String,
) -> Result<KitPlan, SplitError> {
    const PARTS: f64 = 64.0;
    let by_rel: HashMap<(usize, &str), &FileRef> = files
        .iter()
        .map(|f| ((f.root, f.rel.as_str()), f))
        .collect();
    let mut kits: Vec<(&FileRef, String, Vec<Entry>)> = Vec::new();
    for f in files {
        if !f.rel.to_lowercase().ends_with(".kit") {
            continue;
        }
        let src = f.loc.open().map_err(|e| SplitError::Io(e.to_string()))?;
        if src.read(0, 4).map_err(|e| SplitError::Io(e.to_string()))? != MAGIC {
            continue;
        }
        let path = path_of(f);
        let d = src.read_all().map_err(|e| SplitError::Io(e.to_string()))?;
        let part_len = |n: u16| {
            by_rel
                .get(&(f.root, format!("{}.{n}", f.rel).as_str()))
                .map(|p| p.loc.len())
        };
        match entries(&path, &d, &part_len) {
            Ok(es) => kits.push((f, path, es)),
            Err(e) => return Err(SplitError::Red(e)),
        }
    }
    let mut jobs = Vec::new();
    let mut taken = HashSet::new();
    let total: u64 = kits
        .iter()
        .flat_map(|(_, _, es)| es.iter())
        .map(|e| e.length)
        .sum();
    let step = total as f64 / PARTS;
    for (f, path, es) in &kits {
        taken.insert((f.root, f.rel.clone()));
        taken.extend(es.iter().map(|e| (f.root, format!("{}.{}", f.rel, e.part))));
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in es {
            cur.push(e.clone());
            size += e.length;
            if size as f64 >= step {
                jobs.push(KitJob {
                    file: (*f).clone(),
                    path: path.clone(),
                    entries: std::mem::take(&mut cur),
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(KitJob {
                file: (*f).clone(),
                path: path.clone(),
                entries: cur,
            });
        }
    }
    Ok((jobs, taken))
}

/// FIPS 180-4 SHA-1 (the digest `kit.read` checks every member against)
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// `kit.read(path, entry)`: one member's decoded data: every LZ4 block decodes to its full length, the stored bytes are used up exactly, and the SHA-1 matches.
/// `part` is the part file's bytes from the entry's offset: the `length` bytes (fewer when the part is cut short)
pub fn read(path: &str, e: &Entry, part: &[u8]) -> Result<Vec<u8>, String> {
    let label = format!("{path}#{}", e.index);
    if part.len() as u64 != e.length {
        return Err(format!("{label}: truncated KIT member"));
    }
    let mut d = part.to_vec();
    if e.compressed {
        let size = e.size as usize;
        let mut out: Vec<u8> = Vec::new();
        let mut pos = 0usize;
        while pos + 4 <= d.len() && (out.len() as u64) < e.size {
            let stored = u32::from_le_bytes(d[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let want = CHUNK.min(size - out.len());
            if !(0 < stored && stored <= d.len() - pos) {
                return Err(format!("{label}: LZ4 block is outside the KIT member"));
            }
            let dict = &out[out.len().saturating_sub(CHUNK)..];
            let block = lz4_flex::block::decompress_with_dict(&d[pos..pos + stored], want, dict)
                .map_err(|x| {
                    format!(
                        "{label}: LZ4 block at {:#x}: Decompression failed: {x}",
                        pos - 4
                    )
                })?;
            if block.len() != want {
                return Err(format!(
                    "{label}: LZ4 block makes {} of {want} bytes",
                    block.len()
                ));
            }
            out.extend_from_slice(&block);
            pos += stored;
        }
        if pos != d.len() || out.len() as u64 != e.size {
            return Err(format!(
                "{label}: incomplete KIT LZ4 stream ({} of {} bytes, {pos} of {} stored)",
                out.len(),
                e.size,
                d.len()
            ));
        }
        d = out;
    }
    if sha1(&d) != e.sha1 {
        return Err(format!("{label}: KIT SHA1 mismatch"));
    }
    Ok(d)
}

fn run(cx: &Ctx<'_>, job: &KitJob, files: &[FileRef]) -> Res<FileResult> {
    let mut found: Vec<Shader> = Vec::new();
    let by_rel: HashMap<(usize, &str), &FileRef> = files
        .iter()
        .map(|f| ((f.root, f.rel.as_str()), f))
        .collect();
    for e in &job.entries {
        let prel = format!("{}.{}", job.file.rel, e.part);
        let part = by_rel.get(&(job.file.root, prel.as_str())).ok_or_else(|| {
            Failure::Red(format!(
                "{}: KIT entry {} has no part {}",
                job.path, e.index, e.part
            ))
        })?;
        let src = part
            .loc
            .open()
            .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
        let bytes = src
            .read(e.offset, e.length as usize)
            .map_err(|x| Failure::Red(format!("OSError: {x}")))?;
        let d = read(&job.path, e, &bytes).map_err(Failure::Red)?;
        scan_blob(
            cx,
            &d,
            &format!("/{}#{}", job.file.rel, e.index),
            &mut found,
            0,
            None,
        )?;
    }
    crate::scan::finish(cx, &job.file.rel, found, [], crate::unity::Trees::new())
}

/// The job as `scan_file` runs it: a member that cannot be read leaves the job's shaders out (`[RED] <why> (its shaders are left out)`)
pub fn scan(cx: &Ctx<'_>, job: &KitJob, files: &[FileRef]) -> Result<FileResult, String> {
    match run(cx, job, files) {
        Ok(r) => Ok(r),
        Err(Failure::Unsupported(m)) => Err(m),
        Err(e) => {
            let rel = job.file.rel.clone();
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
                extra: Extra::Unreadable { why, count: 1 },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha1_known_answers() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(&[b'a'; 1000])),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }
}
