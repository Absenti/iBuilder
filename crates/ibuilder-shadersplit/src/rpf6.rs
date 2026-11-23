//! Rockstar RPF6 archives (Red Dead Redemption's Switch port): `rpf6.py` and `rpf_jobs` / `rpf_shaders` of `shader_split.py`.
//!
//! Big-endian: a 16-byte header ('RPF6', entry count, name table offset / 8, encryption tag 0xfffffffd), 20-byte entries (directories, files, RAGE resources) and a name table, both
//! AES-256 ECB decrypted 16 times over with a key that is in the game's `main` (never written anywhere: it is found at split time, `find_key`, as the Python finds it). A name is found
//! by the Jenkins one-at-a-time hash of every NUL-terminated printable string of the name table. A file is a zstd frame, a zlib stream, raw deflate, or stored; a script is in addition
//! encrypted like the tables. Each file is read by a positioned read of its own range and scanned like a loose file (the shaders are the `rgxd` effects of shaders.rpf).

use crate::aes::Aes256;
use crate::error::{Failure, Res, SplitError};
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use crate::zstd::Zstd;
use std::collections::HashMap;
use std::path::Path;

pub const MAGIC: &[u8; 4] = b"RPF6";
pub const ENCRYPTED: u32 = 0xffff_fffd;
const RSC5: &[u8; 4] = b"RSC\x05";
const RSC85: &[u8; 4] = b"RSC\x85";
const ZSTD: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
pub const ROUNDS: usize = 16;
/// the key setup's first instructions: cmp w1, #0; mov w8, #-3; csel w8, w8, w1, eq; cmn w8, #3; str w8, [x0]
const SETUP: [u32; 5] = [0x7100003f, 0x12800048, 0x1a810108, 0x31000d1f, 0xb9000008];

/// `(path, offset, stored length, length (None for a resource), compressed, resource flags (None for a plain file))`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub off: u64,
    pub stored: u64,
    pub size: Option<u64>,
    pub comp: bool,
    pub rflags: Option<u32>,
}

/// A range of one archive's files (`rpf_jobs`): the key came with it
#[derive(Clone)]
pub struct RpfJob {
    pub file: FileRef,
    /// the archive as the Python names it in its messages (its path)
    pub path: String,
    pub key: [u8; 32],
    pub entries: Vec<Entry>,
}

impl RpfJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "rpf6:{}:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            crate::bytes::hex(&self.key)
        ))
        .chain(self.entries.iter().map(|e| format!("{e:?}")))
        .collect()
    }
}

/// `joaat(s)`: Jenkins one-at-a-time over the lower-case name
pub fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for ch in s.to_lowercase().bytes() {
        h = h.wrapping_add(ch as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// `decrypt(key, data)`: 16 rounds of AES-256 ECB over the whole blocks (the tail is dropped)
pub fn decrypt(key: &Aes256, data: &[u8]) -> Vec<u8> {
    let mut d = data[..data.len() / 16 * 16].to_vec();
    for _ in 0..ROUNDS {
        key.decrypt(&mut d);
    }
    d
}

fn be(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(d[o..o + 4].try_into().unwrap())
}

/// `is_root(d)`: hash 0, 0, first child 1, at least one child
pub fn is_root(d: &[u8]) -> bool {
    d.len() >= 16 && be(d, 0) == 0 && be(d, 4) == 0 && be(d, 8) == 0x8000_0001 && be(d, 12) > 0
}

/// `header(path)`: (entry count, name table offset, encrypted), None when the source is no RPF6
pub fn header(src: &dyn Source) -> Option<(u32, u64, bool)> {
    let h = src.read(0, 16).ok()?;
    if h.len() < 16 || &h[..4] != MAGIC {
        return None;
    }
    Some((be(&h, 4), be(&h, 8) as u64 * 8, be(&h, 12) == ENCRYPTED))
}

/// `img.find(SETUP, a, b)` over whole words of the pattern
fn find_setup(img: &[u8], a: usize, b: usize) -> Option<usize> {
    let pat: Vec<u8> = SETUP.iter().flat_map(|w| w.to_le_bytes()).collect();
    crate::bytes::find_in(img, &pat, a, b)
}

/// Python's `img[o:o + 16]` for any integer o (a negative one counts from the end)
fn py_slice16(img: &[u8], o: i128) -> Vec<u8> {
    let n = img.len() as i128;
    let norm = |x: i128| -> usize {
        if x < 0 {
            (x + n).max(0) as usize
        } else {
            x.min(n) as usize
        }
    };
    let (a, b) = (norm(o), norm(o + 16));
    if a >= b {
        Vec::new()
    } else {
        img[a..b].to_vec()
    }
}

/// `key_from_setup(img, text)`: the candidate keys of each key setup in main's code, in order (see rpf6.py: adrp, then ldr q of 16-byte constants, stored at x0+8 through
/// registers copied from x0 and moved by add; the eight key words are big-endian u32s zero-extended to 64 bits)
/// Returns the candidates and, when a setup's constants are cut short by the image's end, the `struct.error` that ends the Python's generator there.
pub fn key_from_setup(img: &[u8], text: (u32, u32)) -> (Vec<[u8; 32]>, Option<String>) {
    let (ts, tl) = (text.0 as usize, text.1 as usize);
    let end = ts + tl;
    let mut out = Vec::new();
    let mut at = find_setup(img, ts, end);
    while let Some(s) = at {
        let mut page: HashMap<u32, i128> = HashMap::new();
        let mut ctx: HashMap<u32, i128> = HashMap::from([(0, 0)]);
        let mut q: HashMap<u32, Vec<u8>> = HashMap::new();
        let mut mem: HashMap<i128, Vec<u8>> = HashMap::new();
        let mut a = s + 20 + 4;
        while a < (s + 0x80).min(end) {
            if a + 4 > img.len() {
                break;
            }
            let w = u32::from_le_bytes(img[a..a + 4].try_into().unwrap());
            let (rd, rn) = (w & 31, (w >> 5) & 31);
            if w & 0x9f00_0000 == 0x9000_0000 {
                let imm = ((w >> 29) & 3) as i128 | ((((w >> 5) & 0x7ffff) as i128) << 2);
                let imm = if imm & (1 << 20) != 0 {
                    imm - (1 << 21)
                } else {
                    imm
                };
                page.insert(rd, (a as i128 & !0xfff) + (imm << 12));
                ctx.remove(&rd);
            } else if w & 0xffe0_ffe0 == 0xaa00_03e0 && ctx.contains_key(&((w >> 16) & 31)) {
                let v = ctx[&((w >> 16) & 31)];
                ctx.insert(rd, v);
            } else if w & 0xffc0_0000 == 0x9100_0000 && ctx.contains_key(&rn) {
                let v = ctx[&rn] + ((w >> 10) & 0xfff) as i128;
                ctx.insert(rd, v);
            } else if w & 0xffc0_0000 == 0x3dc0_0000 && page.contains_key(&rn) {
                let o = page[&rn] + ((w >> 10) & 0xfff) as i128 * 16;
                q.insert(rd, py_slice16(img, o));
            } else if w & 0xffe0_0c00 == 0x3c80_0c00 && ctx.contains_key(&rn) && q.contains_key(&rd)
            {
                let imm = ((w >> 12) & 0x1ff) as i128;
                let v = ctx[&rn] + if imm & 0x100 != 0 { imm - 0x200 } else { imm };
                ctx.insert(rn, v);
                mem.insert(v, q[&rd].clone());
            } else if w & 0xffc0_0000 == 0x3d80_0000 && ctx.contains_key(&rn) && q.contains_key(&rd)
            {
                mem.insert(ctx[&rn] + ((w >> 10) & 0xfff) as i128 * 16, q[&rd].clone());
            } else if w & 0xffc0_0000 == 0xad00_0000
                && ctx.contains_key(&rn)
                && q.contains_key(&rd)
                && q.contains_key(&((w >> 10) & 31))
            {
                let imm = ((w >> 15) & 0x7f) as i128;
                let o = ctx[&rn] + (if imm & 0x40 != 0 { imm - 0x80 } else { imm }) * 16;
                mem.insert(o, q[&rd].clone());
                mem.insert(o + 16, q[&((w >> 10) & 31)].clone());
            } else if matches!(w & 0xfc00_0000, 0x1400_0000 | 0x9400_0000)
                || w & 0xff00_0010 == 0x5400_0000
            {
                break;
            }
            a += 4;
        }
        let offs = [8i128, 24, 40, 56];
        if offs.iter().all(|o| mem.contains_key(o)) {
            let b: Vec<u8> = offs.iter().flat_map(|o| mem[o].clone()).collect();
            // `struct.unpack('<8Q', ...)`: a short constant (one read past the image) is a struct.error that ends the search
            if b.len() != 64 {
                return (
                    out,
                    Some("error: unpack requires a buffer of 64 bytes".into()),
                );
            }
            {
                let words: Vec<u64> = b
                    .chunks(8)
                    .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                    .collect();
                if words.iter().all(|v| v >> 32 == 0) {
                    let mut k = [0u8; 32];
                    for (i, v) in words.iter().enumerate() {
                        k[4 * i..4 * i + 4].copy_from_slice(&(*v as u32).to_be_bytes());
                    }
                    out.push(k);
                }
            }
        }
        at = find_setup(img, s + 4, end);
    }
    (out, None)
}

/// `find_key(nso, rpf)` on a loaded main: the key that decrypts the archive's first entry block `blk` to the root directory: the key setup's candidates first, then every
/// 4-aligned 32-byte window (at least 24 distinct bytes) of the read-only and data segments
/// (`Err`: the struct.error of a key setup cut short, which ends the Python's split)
pub fn find_key(
    img: &[u8],
    segments: &[(u32, u32); 3],
    blk: &[u8],
) -> Result<Option<[u8; 32]>, String> {
    let opens = |k: &[u8; 32]| is_root(&decrypt(&Aes256::new(k), blk));
    let (cands, err) = key_from_setup(img, segments[0]);
    for k in cands {
        if opens(&k) {
            return Ok(Some(k));
        }
    }
    if let Some(e) = err {
        return Err(e);
    }
    if blk.len() < 16 {
        return Ok(None); // nothing decrypts to a root of 16 bytes
    }
    for &(mo, sz) in &segments[1..] {
        let (mo, sz) = (mo as usize, sz as usize);
        let mut a = mo;
        while a + 32 < mo + sz && a + 32 <= img.len() {
            let w: &[u8; 32] = img[a..a + 32].try_into().unwrap();
            let mut seen = [false; 256];
            let mut distinct = 0;
            for &b in w {
                if !seen[b as usize] {
                    seen[b as usize] = true;
                    distinct += 1;
                }
            }
            if distinct >= 24 && opens(w) {
                return Ok(Some(*w));
            }
            a += 4;
        }
    }
    Ok(None)
}

/// `re.findall(rb'[\x20-\x7e]{1,255}(?=\0)', nt)`
pub fn names_of(nt: &[u8]) -> Vec<&[u8]> {
    let pr = |b: u8| (0x20..=0x7e).contains(&b);
    let mut out = Vec::new();
    let mut i = 0;
    while i < nt.len() {
        if !pr(nt[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < nt.len() && pr(nt[j]) {
            j += 1;
        }
        // the run nt[i..j]: the matches are its last 255 characters when a NUL follows it, none otherwise
        if j < nt.len() && nt[j] == 0 {
            let a = if j - i > 255 { j - 255 } else { i };
            out.push(&nt[a..j]);
        }
        i = j;
    }
    out
}

/// An opened archive: its files (`Rpf(path, key).files`)
pub struct Rpf {
    pub files: Vec<Entry>,
}

/// `Rpf(path, key)`; `Err` is the exception's text (`ValueError: ...`, `error: ...` for struct.error, `RecursionError: ...`)
pub fn open(src: &dyn Source, path: &str, key: &[u8; 32]) -> Result<Rpf, String> {
    let io = |e: std::io::Error| format!("OSError: {e}");
    let Some((n, names_at, enc)) = header(src) else {
        return Err(format!("ValueError: {path}: no RPF6 archive"));
    };
    let n = n as usize;
    let want = (n * 20).div_ceil(16) * 16;
    let mut t = src.read(16, want).map_err(io)?;
    let mut nt = src
        .read(names_at, src.len().saturating_sub(names_at) as usize)
        .map_err(io)?;
    if enc {
        let k = Aes256::new(key);
        t = decrypt(&k, &t);
        nt = decrypt(&k, &nt);
    }
    if !is_root(&t) {
        return Err(format!(
            "ValueError: {path}: the first entry is no root directory (wrong key?)"
        ));
    }
    if t.len() < 20 * n {
        let i = t.len() / 20;
        return Err(format!(
            "error: unpack_from requires a buffer of at least {} bytes for unpacking 20 bytes at offset {} (actual buffer size is {})",
            20 * i + 20,
            20 * i,
            t.len()
        ));
    }
    let raw: Vec<[u32; 5]> = (0..n)
        .map(|i| std::array::from_fn(|k| be(&t, 20 * i + 4 * k)))
        .collect();
    let mut names: HashMap<u32, String> = HashMap::new();
    for s in names_of(&nt) {
        let s = String::from_utf8_lossy(s).into_owned();
        names.insert(joaat(&s), s);
    }
    let mut files = Vec::new();
    // walk(i, prefix), the Python's recursion as a stack (its depth limit: RecursionError)
    fn walk(
        raw: &[[u32; 5]],
        names: &HashMap<u32, String>,
        path: &str,
        i: usize,
        prefix: &str,
        depth: usize,
        files: &mut Vec<Entry>,
    ) -> Result<(), String> {
        if depth > 900 {
            return Err("RecursionError: maximum recursion depth exceeded".into());
        }
        if files.len() > 1 << 24 {
            return Err(format!(
                "ValueError: {path}: more than 2^24 files (a directory that holds itself?)"
            ));
        }
        let [h, a, b, c, _] = raw[i];
        let name = names
            .get(&h)
            .cloned()
            .unwrap_or_else(|| format!("#{h:08x}"));
        if b & 0x8000_0000 != 0 {
            let (first, count) = ((b & 0x7fff_ffff) as u64, c as u64);
            if first + count > raw.len() as u64 {
                return Err(format!(
                    "ValueError: {path}: directory entry {i} runs past the table"
                ));
            }
            let p = if i != 0 {
                format!("{prefix}{name}/")
            } else {
                String::new()
            };
            for j in first..first + count {
                walk(raw, names, path, j as usize, &p, depth + 1, files)?;
            }
        } else if c & 0x8000_0000 != 0 {
            files.push(Entry {
                path: format!("{prefix}{name}"),
                off: (b & 0x7fff_ff00) as u64 * 8,
                stored: (a & 0x7fff_ffff) as u64,
                size: None,
                comp: c & 0x4000_0000 != 0,
                rflags: Some(c),
            });
        } else {
            files.push(Entry {
                path: format!("{prefix}{name}"),
                off: (b & 0x7fff_ffff) as u64 * 8,
                stored: (a & 0x7fff_ffff) as u64,
                size: Some((c & 0x3fff_ffff) as u64),
                comp: c & 0x4000_0000 != 0,
                rflags: None,
            });
        }
        Ok(())
    }
    walk(&raw, &names, path, 0, "", 0, &mut files)?;
    Ok(Rpf { files })
}

/// `unpack(d, zstd)`: the decoded data of a zstd frame or a zlib stream, else None; `Err` the ZstdError (it is not caught)
fn unpack(zstd: &dyn Zstd, d: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if d.len() >= 4 && d[..4] == ZSTD {
        return zstd
            .decompress(d, None, None)
            .map(Some)
            .map_err(|e| format!("ZstdError: {e}"));
    }
    if d.len() >= 2
        && d[0] & 0xf == 8
        && d[0] >> 4 <= 7
        && ((d[0] as u32) << 8 | d[1] as u32).is_multiple_of(31)
    {
        // `zlib.decompressobj().decompress(d)`: a stream cut short gives what it holds; a damaged one None
        return Ok(crate::inflate::run(d, crate::inflate::Wrap::Zlib, None)
            .ok()
            .map(|r| r.out));
    }
    Ok(None)
}

/// `Rpf.read(f, entry, zstd)`; `Err` the exception's text
pub fn read(
    zstd: &dyn Zstd,
    src: &dyn Source,
    path: &str,
    key: &[u8; 32],
    e: &Entry,
) -> Result<Vec<u8>, String> {
    let name = &e.path;
    let mut d = src
        .read(e.off, e.stored as usize)
        .map_err(|x| format!("OSError: {x}"))?;
    if d.len() as u64 != e.stored {
        return Err(format!(
            "ValueError: {path}/{name}: cut off ({} of {} bytes)",
            d.len(),
            e.stored
        ));
    }
    if let Some(rf) = e.rflags {
        if d.len() < 4 || (&d[..4] != RSC5 && &d[..4] != RSC85) {
            return Err(format!(
                "ValueError: {path}/{name}: a resource entry, but no RSC5 / RSC85 header at {:#x}",
                e.off
            ));
        }
        if d.len() < 12 {
            return Err(format!(
                "error: unpack_from requires a buffer of at least 12 bytes for unpacking 4 bytes at offset 8 (actual buffer size is {})",
                d.len()
            ));
        }
        if u32::from_le_bytes(d[8..12].try_into().unwrap()) != rf {
            return Err(format!(
                "ValueError: {path}/{name}: the resource header's flags differ from the entry's"
            ));
        }
        let cut = if &d[..4] == RSC85 { 16 } else { 12 };
        d = d[cut.min(d.len())..].to_vec();
    }
    if !e.comp && e.rflags.is_none() {
        return Ok(d);
    }
    let mut u = unpack(zstd, &d)?;
    if u.is_none()
        && e.comp
        && let Some(size) = e.size
    {
        // raw deflate (audio_switch.rpf's config files), kept only at the entry's length; the exact zlib model (no check value: miniz takes some streams zlib refuses)
        u = crate::zlibx::decompress_raw(&d, Some(crate::inflate::MAX_DECODED + 1))
            .ok()
            .map(|z| z.out)
            .filter(|x| x.len() as u64 == size);
    }
    if u.is_none() {
        // a script: encrypted like the tables, then compressed
        let mut x = decrypt(&Aes256::new(key), &d);
        x.extend_from_slice(&d[d.len() / 16 * 16..]);
        u = unpack(zstd, &x)?;
    }
    let Some(u) = u else {
        if !e.comp {
            return Ok(d); // a resource stored as it is
        }
        return Err(format!(
            "ValueError: {path}/{name}: compressed, but neither a zstd frame nor a zlib stream, also after decryption"
        ));
    };
    if let Some(size) = e.size
        && u.len() as u64 != size
    {
        return Err(format!(
            "ValueError: {path}/{name}: {} bytes decoded, the entry says {size}",
            u.len()
        ));
    }
    Ok(u)
}

/// `rpf_jobs(starting(RPF6), main_v)`: every file of every archive, in ranges of about equal stored length; the key from the first main that opens the first archive
pub fn plan(
    files: &[FileRef],
    mains: &[&Path],
    path_of: &dyn Fn(&FileRef) -> String,
) -> Result<(Vec<RpfJob>, crate::jobs::Taken), SplitError> {
    let mut rp: Vec<(&FileRef, crate::source::FileSource)> = Vec::new();
    for f in files {
        if let Ok(s) = f.loc.open()
            && header(&s).is_some()
        {
            rp.push((f, s));
        }
    }
    if rp.is_empty() {
        return Ok((Vec::new(), Default::default()));
    }
    let blk = rp[0]
        .1
        .read(16, 16)
        .map_err(|e| SplitError::Io(e.to_string()))?;
    let mut key = None;
    for m in mains {
        let module = ibuilder_nso::read(m).map_err(|e| SplitError::Red(e.to_string()))?;
        if let Some(k) = find_key(&module.image, &module.segments, &blk).map_err(SplitError::Red)? {
            key = Some(k);
            break;
        }
    }
    let Some(key) = key else {
        return Err(SplitError::Red(format!(
            "/{}: an encrypted RPF6 archive, and no 32-byte window of main decrypts its first entry to the root directory",
            rp[0].0.rel
        )));
    };
    let mut arcs = Vec::new();
    for (f, s) in &rp {
        let path = path_of(f);
        let r = open(s, &path, &key).map_err(SplitError::Red)?;
        arcs.push((*f, path, r.files));
    }
    let total: u64 = arcs.iter().flat_map(|a| a.2.iter()).map(|e| e.stored).sum();
    let step = (total as f64 / 64.0).max(1.0);
    let mut jobs = Vec::new();
    for (f, path, es) in arcs {
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in es {
            size += e.stored;
            cur.push(e);
            if size as f64 >= step {
                jobs.push(RpfJob {
                    file: f.clone(),
                    path: path.clone(),
                    key,
                    entries: std::mem::take(&mut cur),
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(RpfJob {
                file: f.clone(),
                path: path.clone(),
                key,
                entries: cur,
            });
        }
    }
    let taken = rp.iter().map(|(f, _)| (f.root, f.rel.clone())).collect();
    Ok((jobs, taken))
}

/// `rpf_shaders(path, rel, key, ents, out)`: each file decoded and scanned like a loose file, labelled `/<rel>/<path in the archive>`
pub fn shaders(cx: &Ctx<'_>, job: &RpfJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    for e in &job.entries {
        let d = read(cx.zstd, &src, &job.path, &job.key, e).map_err(Failure::Red)?;
        scan_blob(
            cx,
            &d,
            &format!("/{}/{}", job.file.rel, e.path),
            &mut out,
            0,
            None,
        )?;
    }
    Ok(out)
}
