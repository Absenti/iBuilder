//! Camelot's CRA archive (Mario Tennis Fever; Mario Golf: Super Rush's 2021 layout) and its NARC containers: `cra.py` and `cra_jobs` / `cra_shaders` of `shader_split.py`.
//!
//! The whole game is one RomFS file (`rom:/42`, magic `CRA.`); its file table is not in it but in the read-only data of the NRO the game decodes at run time (`DM_NRO_DIR`), found by
//! the code that reads it (`index_of` / `index_2021`): paths stored with shared prefixes and a suffix table, sizes and offsets xored with constants, a checksum per file. Each file is
//! scrambled in 16-byte blocks with a key from its name (today's layout: CRC-32 / CRC-32C of the block's other word, 8 variants; the 2021 layout: two 8-byte xor keys and bit
//! reversal), and checked against its checksum when read. Most files are NARC containers whose members are packed by the kind in their first byte (`undo`: LZ4, zstd, zlib, raw
//! deflate, Camelot's LZ, chunks). Every file is read by a positioned read of its own range; sound and movies are left out by their first block.

use crate::error::{Failure, Res, SplitError};
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use std::path::Path;
use std::sync::OnceLock;

pub const MAGIC: &[u8; 4] = b"CRA.";

/// A failure: `cra.Error(msg)` (a ValueError the readers catch where the Python does), or another exception (`Type: msg`) that goes on up
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CraErr {
    Cra(String),
    Other(String),
}

impl CraErr {
    /// the text `scan_file` gives an exception: `Error: msg` for a cra.Error
    pub fn text(&self) -> String {
        match self {
            CraErr::Cra(m) => format!("Error: {m}"),
            CraErr::Other(m) => m.clone(),
        }
    }
}

type R<T> = Result<T, CraErr>;

fn index_err() -> CraErr {
    CraErr::Other("IndexError: index out of range".into())
}

/// Python's `d[o]` (a negative index counts from the end)
fn byte(d: &[u8], o: i64) -> R<u8> {
    let n = d.len() as i64;
    let p = if o < 0 { o + n } else { o };
    if p < 0 || p >= n {
        return Err(index_err());
    }
    Ok(d[p as usize])
}

/// Python's `struct.unpack_from(<n bytes>, d, o)`
fn upk(d: &[u8], o: i64, n: usize) -> R<&[u8]> {
    let len = d.len() as i64;
    let mut p = o;
    if p < 0 {
        if -p > len {
            return Err(CraErr::Other(format!(
                "error: offset {o} out of range for {len}-byte buffer"
            )));
        }
        p += len;
    }
    if p + n as i64 > len {
        return Err(CraErr::Other(format!(
            "error: unpack_from requires a buffer of at least {} bytes for unpacking {n} bytes at offset {p} (actual buffer size is {len})",
            p + n as i64
        )));
    }
    Ok(&d[p as usize..p as usize + n])
}

fn u16at(d: &[u8], o: i64) -> R<u64> {
    Ok(u16::from_le_bytes(upk(d, o, 2)?.try_into().unwrap()) as u64)
}
fn i16at(d: &[u8], o: i64) -> R<i64> {
    Ok(i16::from_le_bytes(upk(d, o, 2)?.try_into().unwrap()) as i64)
}
fn u32at(d: &[u8], o: i64) -> R<u64> {
    Ok(u32::from_le_bytes(upk(d, o, 4)?.try_into().unwrap()) as u64)
}
fn i32at(d: &[u8], o: i64) -> R<i64> {
    Ok(i32::from_le_bytes(upk(d, o, 4)?.try_into().unwrap()) as i64)
}
fn u64at(d: &[u8], o: i64) -> R<u64> {
    Ok(u64::from_le_bytes(upk(d, o, 8)?.try_into().unwrap()))
}

// ---- the CRC tables and the key ------------------------------------------------------------------------------------------------

type Tab = [[u32; 256]; 8];

#[allow(clippy::needless_range_loop)] // i is the byte the tables are indexed by, as in the Python
fn crc_table(poly: u32) -> Tab {
    let mut t = [[0u32; 256]; 8];
    for i in 0..256 {
        let mut c = i as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { poly } else { 0 };
        }
        t[0][i] = c;
    }
    for k in 1..8 {
        for i in 0..256 {
            t[k][i] = (t[k - 1][i] >> 8) ^ t[0][(t[k - 1][i] & 0xff) as usize];
        }
    }
    t
}

fn tabs() -> &'static (Tab, Tab) {
    static T: OnceLock<(Tab, Tab)> = OnceLock::new();
    T.get_or_init(|| (crc_table(0xedb88320), crc_table(0x82f63b78)))
}

/// the crc32x / crc32cx instruction: no inversion before or after
fn crc64(t: &Tab, c: u64, v: u64) -> u64 {
    let x = c ^ v;
    (t[7][(x & 0xff) as usize]
        ^ t[6][(x >> 8 & 0xff) as usize]
        ^ t[5][(x >> 16 & 0xff) as usize]
        ^ t[4][(x >> 24 & 0xff) as usize]
        ^ t[3][(x >> 32 & 0xff) as usize]
        ^ t[2][(x >> 40 & 0xff) as usize]
        ^ t[1][(x >> 48 & 0xff) as usize]
        ^ t[0][(x >> 56) as usize]) as u64
}

/// a path as the Python's `latin1` string: one char per byte
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn stem(path: &str) -> u64 {
    let e: Vec<u8> = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .chars()
        .map(|c| c as u8)
        .collect();
    let find = |c: u8| e.iter().position(|&x| x == c);
    let n = [find(b'.'), find(b'_'), Some(4)]
        .into_iter()
        .flatten()
        .min()
        .unwrap();
    e[..n.min(e.len())]
        .iter()
        .fold(0u64, |a, &b| (a << 8) | b as u64)
}

pub fn key(path: &str) -> u32 {
    let (c32, _) = tabs();
    let le = |s: &[u8; 8]| u64::from_le_bytes(*s);
    let k = crc64(c32, stem(path), le(b"UseUrSk1"));
    let a = crc64(
        c32,
        crc64(c32, 0xffff_ffff, le(b"Nintendo")),
        le(b"/CAMELOT"),
    );
    ((a ^ crc64(c32, k, le(b"lls4G00d")) ^ 0x0205_2636) & 0xffff_ffff) as u32
}

const VARIANT_MASK: [u64; 8] = [
    0x11111111, 0x22222223, 0x33333333, 0x44444445, 0x55555555, 0x66666667, 0x77777777, 0x88888889,
];

/// `unscramble(path, raw)`: the whole blocks of a file, unscrambled, and the sum that gives its checksum
pub fn unscramble(path: &str, raw: &[u8]) -> (Vec<u8>, u64) {
    let w = key(path) as u64;
    let v = (w & 7) as usize;
    let (c32, c32c) = tabs();
    let t = if v & 4 != 0 { c32c } else { c32 };
    let mul = (((w & 0xffff_fffe) ^ VARIANT_MASK[v]) << 32) | 1;
    let (src, oth) = if v & 2 != 0 { (1, 0) } else { (0, 1) };
    let (a, b) = if v & 1 != 0 { (1, 0) } else { (0, 1) };
    let n = raw.len() / 16;
    let mut out = vec![0u8; n * 16];
    let mut total = 0u64;
    for i in 0..n {
        let q = |k: usize| {
            u64::from_le_bytes(raw[16 * i + 8 * k..16 * i + 8 * k + 8].try_into().unwrap())
        };
        let s = q(src);
        let m = crc64(t, s, w).wrapping_mul(mul);
        let e = q(oth) ^ (m >> 3);
        let x = !(m ^ (s << 32));
        out[16 * i + 8 * a..16 * i + 8 * a + 8].copy_from_slice(&x.to_le_bytes());
        out[16 * i + 8 * b..16 * i + 8 * b + 8].copy_from_slice(&e.to_le_bytes());
        total = total.wrapping_add(e);
    }
    (out, total)
}

// ---- the NRO's file table --------------------------------------------------------------------------------------------------------

fn words(nro: &[u8]) -> R<Vec<u32>> {
    if nro.get(0x10..0x14) != Some(b"NRO0") {
        return Err(CraErr::Cra("not an NRO".into()));
    }
    let end = (u32at(nro, 0x24)? & !3) as usize;
    if end > nro.len() {
        return Err(CraErr::Other(format!(
            "error: unpack_from requires a buffer of at least {end} bytes for unpacking {end} bytes at offset 0 (actual buffer size is {})",
            nro.len()
        )));
    }
    Ok(nro[..end]
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect())
}

/// `adrp_add(w, i, reg)`: the address adrp + add build in reg at instructions i, i + 1
fn adrp_add(w: &[u32], i: usize, reg: u32) -> R<Option<i64>> {
    let (a, b) = (
        *w.get(i).ok_or_else(index_err)?,
        *w.get(i + 1).ok_or_else(index_err)?,
    );
    if a & 0x9f00_001f != 0x9000_0000 | reg || b & 0xffc0_03ff != 0x9100_0000 | reg << 5 | reg {
        return Ok(None);
    }
    let mut hi = (((a >> 5 & 0x7ffff) << 2) | (a >> 29 & 3)) as i64;
    if hi & 1 << 20 != 0 {
        hi -= 1 << 21;
    }
    Ok(Some(
        ((i as i64 * 4) & !0xfff) + (hi << 12) + (b >> 10 & 0xfff) as i64,
    ))
}

/// `index_of(nro)`: where the file table, the checksums and the suffixes are, from the code that reads them
pub fn index_of(nro: &[u8]) -> R<(i64, i64, i64, Option<i64>)> {
    let w = words(nro)?;
    let (mut table, mut sums, mut names, mut count) = (None, None, None, 0i64);
    for i in 3..w.len().saturating_sub(3) {
        let c = w[i];
        if table.is_none()
            && c & 0xffe0_fc00 == 0x78e0_d800
            && w[i - 1] & 0xffff_ffe0 == 0x5280_0240
            && w[i + 1] & 0xffe0_8000 == 0x1b00_0000
            && w[i + 2] & 0xffe0_0000 == 0x5280_0000
        {
            if let Some(a) = adrp_add(&w, i - 3, c >> 5 & 31)? {
                table = Some(a);
                count = ((w[i + 2] >> 5 & 0xffff) / 2) as i64;
            }
        } else if sums.is_none()
            && c & 0xffff_ffe0 == 0xd28e_6aa0
            && w[i + 1] & 0xffff_ffe0 == 0xf2aa_aca0
        {
            for k in i - 3..i {
                for reg in 0..31 {
                    if let Some(a) = adrp_add(&w, k, reg)? {
                        sums = Some(a);
                    }
                }
            }
        }
    }
    let (Some(table), Some(sums)) = (table, sums) else {
        return Err(CraErr::Cra(
            "this NRO has no CRA file table (or another layout of it)".into(),
        ));
    };
    for i in 0..w.len().saturating_sub(4) {
        if w[i] & 0xffff_fc00 == 0x39c0_0c00
            && w[i + 1] & 0xff00_0000 == 0x3700_0000
            && w[i + 2] & 0xffff_fc00 == 0x9240_1c00
        {
            for reg in 0..31 {
                if let Some(a) = adrp_add(&w, i + 3, reg)?
                    && w.get(i + 5).ok_or_else(|| {
                        CraErr::Other("IndexError: tuple index out of range".into())
                    })? & 0xffe0_fc00
                        == 0xf860_7800
                {
                    names = Some(a);
                }
            }
        }
    }
    Ok((table, count, sums, names))
}

const REV8: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = (i as u8).reverse_bits();
        i += 1;
    }
    t
};
const SIZE_2021: u64 = 0xce46_2eca;
const OFF_2021: u64 = 0x46_f636_762a;
const SUM_2021: u32 = 0x1234_5678;
const KEY_A: u64 = 0x4adf_c980_eae9_731b;
const KEY_B: u64 = 0xfbcc_b4fa_c6ff_003e;

/// `index_2021(nro)`: the 2021 layout's file table, from the code that reads a record
pub fn index_2021(nro: &[u8]) -> R<i64> {
    let w = words(nro)?;
    for i in 8..w.len().saturating_sub(1) {
        if w[i] & 0xffff_ffe0 != 0x5280_0000 | 0x2eca << 5
            || w[i + 1] & 0xffff_ffe0 != 0x72a0_0000 | 0xce46 << 5
            || (w[i] ^ w[i + 1]) & 31 != 0
        {
            continue;
        }
        for k in i - 8..i {
            let reg = w[k] & 31;
            if let Some(a) = adrp_add(&w, k, reg)?
                && w[k + 2] & 0xffe0_fc00 == 0xb8a0_7800
                && w[k + 2] >> 5 & 31 == reg
            {
                return Ok(a);
            }
        }
    }
    Err(CraErr::Cra(
        "this NRO has no CRA file table (or another layout of it)".into(),
    ))
}

/// One file of the archive: `(index, path, size, offset, checksum, layout)`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub i: u64,
    pub path: String,
    pub size: u64,
    pub off: u64,
    pub sum: u64,
    pub y2021: bool,
}

/// a loop the Python would not end (a damaged prefix chain): an error here
fn stuck() -> CraErr {
    CraErr::Other(
        "RecursionError: a path prefix chain that does not end (a damaged file table)".into(),
    )
}

struct Index<'a> {
    d: &'a [u8],
    table: i64,
    count: i64,
    sums: i64,
    names: Option<i64>,
}

impl Index<'_> {
    fn rec(&self, i: i64) -> R<i64> {
        Ok(self.table + i16at(self.d, self.table + i * 2)? + i * 18 + self.count * 2)
    }

    fn name(&self, i: i64) -> R<String> {
        let d = self.d;
        let r = self.rec(i)?;
        let mut need = byte(d, r + 2)? as usize;
        let mut out = vec![0u8; need];
        if need != 0 {
            let mut k = i - u16at(d, r)? as i64;
            let mut steps = 0usize;
            loop {
                steps += 1;
                if steps > 1 << 20 {
                    return Err(stuck());
                }
                let s = self.rec(k)?;
                let have = need.min(byte(d, s + 2)? as usize);
                for j in 0..need - have {
                    out[have + j] = ((k - byte(d, s + 0xd + j as i64)? as i64) & 0x7f) as u8;
                }
                need = have;
                if need == 0 {
                    break;
                }
                k -= u16at(d, s)? as i64;
            }
        }
        let mut o = r + 0xd;
        while byte(d, o)? != 0 {
            out.push(((i - byte(d, o)? as i64) & 0x7f) as u8);
            o += 1;
        }
        let sfx = byte(d, r + 3)? as i64;
        if sfx < 0x80
            && let Some(names) = self.names
        {
            let mut o = u64at(d, names + sfx * 8)? as i64;
            while byte(d, o)? != 0 {
                out.push(((sfx - byte(d, o)? as i64) & 0x7f) as u8);
                o += 1;
            }
        }
        let text = latin1(&out);
        let mut parts: Vec<String> = Vec::new();
        for c in text.split('/') {
            let p = match parts.last() {
                Some(last) => c.replace('$', last),
                None => c.to_string(),
            };
            parts.push(p);
        }
        Ok(parts.join("/"))
    }

    fn files(&self) -> R<Vec<Entry>> {
        let mut out = Vec::new();
        for i in 0..self.count {
            let r = self.rec(i)?;
            let size = u32at(self.d, r + 4)? ^ 0xff00_ff00;
            if size == 0 {
                continue;
            }
            let off = (u64at(self.d, r + 8)? & 0xff_ffff_ffff) ^ 0xff_00ff_00ff;
            out.push(Entry {
                i: i as u64,
                path: self.name(i)?,
                size,
                off,
                sum: u32at(self.d, self.sums + i * 4)?,
                y2021: false,
            });
        }
        Ok(out)
    }
}

struct Index2021<'a> {
    d: &'a [u8],
    table: i64,
    count: i64,
}

impl Index2021<'_> {
    fn rec(&self, i: i64) -> R<i64> {
        Ok(self.table - i32at(self.d, self.table + i * 4)?)
    }

    fn chars(&self, i: i64, r: i64) -> R<Vec<u8>> {
        let (d, k) = (self.d, ((i & 15) + 1) as u8);
        let (mut o, mut out) = (r + 0x11, Vec::new());
        while byte(d, o)? != 0 {
            out.push(REV8[byte(d, o)? as usize] ^ k);
            o += 1;
        }
        Ok(out)
    }

    fn name(&self, i: i64) -> R<String> {
        let d = self.d;
        let mut r = self.rec(i)?;
        let mut need = byte(d, r + 2)? as usize;
        let mut out = vec![0u8; need];
        out.extend(self.chars(i, r)?);
        let mut k = i;
        let mut steps = 0usize;
        while need != 0 {
            steps += 1;
            if steps > 1 << 20 {
                return Err(stuck());
            }
            let back = u16at(d, r)? as i64;
            if back == 0 {
                return Err(CraErr::Cra(format!(
                    "file {i}: a path prefix with no record behind it"
                )));
            }
            k -= back;
            r = self.rec(k)?;
            let have = need.min(byte(d, r + 2)? as usize);
            let c = self.chars(k, r)?;
            // `out[have:need] = c[:need - have]` (a shorter slice shortens out)
            let take = &c[..(need - have).min(c.len())];
            let tail = out.split_off(need.min(out.len()));
            out.truncate(have);
            out.extend_from_slice(take);
            out.extend(tail);
            need = have;
        }
        Ok(latin1(&out))
    }

    fn files(&self) -> R<Vec<Entry>> {
        let mut out = Vec::new();
        for i in 0..self.count {
            let r = self.rec(i)?;
            let raw = u32at(self.d, r + 4)?;
            let size = raw ^ SIZE_2021;
            if raw == 0 || size == 0 {
                continue;
            }
            let off = (u64at(self.d, r + 0xc)? & 0xff_ffff_ffff) ^ OFF_2021;
            out.push(Entry {
                i: i as u64,
                path: self.name(i)?,
                size,
                off,
                sum: u32at(self.d, r + 8)?,
                y2021: true,
            });
        }
        Ok(out)
    }
}

/// `index(nro).files()`: the file table of the layout this NRO's code reads (today's, else the 2021 one). `Err(Cra(..))` from finding the table is what `cra_jobs` skips an NRO for
pub fn index_files(nro: &[u8]) -> Result<Vec<Entry>, (bool, CraErr)> {
    match index_of(nro) {
        Ok((table, count, sums, names)) => Index {
            d: nro,
            table,
            count,
            sums,
            names,
        }
        .files()
        .map_err(|e| (false, e)),
        Err(CraErr::Cra(_)) => {
            let table = index_2021(nro).map_err(|e| (true, e))?;
            let first = i32at(nro, table).map_err(|e| (true, e))?;
            if first >= 0 || first.rem_euclid(4) != 0 {
                return Err((
                    true,
                    CraErr::Cra(
                        "the 2021 file table does not start with its records' distance".into(),
                    ),
                ));
            }
            Index2021 {
                d: nro,
                table,
                count: -first / 4,
            }
            .files()
            .map_err(|e| (false, e))
        }
        Err(e) => Err((true, e)),
    }
}

fn keys_2021(path: &[u8]) -> R<[u8; 16]> {
    let st = path
        .iter()
        .rposition(|&c| c == b'/')
        .map(|p| p + 1)
        .unwrap_or(0);
    if st >= path.len() {
        return Err(CraErr::Cra(format!(
            "{}: no last path element to take the key from",
            latin1(path)
        )));
    }
    let (mut q, mut k) = (st, 0u64);
    for _ in 0..8 {
        k = (k << 8) | REV8[path[q] as usize] as u64;
        q += 1;
        if q >= path.len() || path[q] == b'.' {
            q = st;
        }
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&(k ^ KEY_A).to_le_bytes());
    out[8..].copy_from_slice(&(k ^ KEY_B).to_le_bytes());
    Ok(out)
}

fn path_bytes(p: &str) -> Vec<u8> {
    p.chars().map(|c| c as u8).collect()
}

fn plain_2021(path: &str, raw: &[u8]) -> R<Vec<u8>> {
    let ks = keys_2021(&path_bytes(path))?;
    Ok(raw
        .iter()
        .enumerate()
        .map(|(i, &b)| REV8[(b ^ ks[i % 16]) as usize])
        .collect())
}

/// `read(f, entry)`: a file's bytes, unscrambled and checked
pub fn read(src: &dyn Source, e: &Entry) -> R<Vec<u8>> {
    let io = |x: std::io::Error| CraErr::Other(format!("OSError: {x}"));
    if e.y2021 {
        let plain = plain_2021(&e.path, &src.read(e.off, e.size as usize).map_err(io)?)?;
        if plain.len() as u64 != e.size || (crc32fast::hash(&plain) ^ SUM_2021) as u64 != e.sum {
            return Err(CraErr::Cra(format!("{}: the checksum differs", e.path)));
        }
        return Ok(plain);
    }
    let raw = src
        .read(e.off, ((e.size + 15) & !15) as usize)
        .map_err(io)?;
    let (mut plain, total) = unscramble(&e.path, &raw);
    if ((total >> 32) ^ total) & 0xffff_ffff != e.sum {
        return Err(CraErr::Cra(format!("{}: the checksum differs", e.path)));
    }
    plain.truncate(e.size as usize);
    Ok(plain)
}

/// `head(f, entry)`: the first block of a file, unscrambled
pub fn head(src: &dyn Source, e: &Entry) -> R<Vec<u8>> {
    let io = |x: std::io::Error| CraErr::Other(format!("OSError: {x}"));
    if e.y2021 {
        return plain_2021(
            &e.path,
            &src.read(e.off, e.size.min(16) as usize).map_err(io)?,
        );
    }
    Ok(unscramble(&e.path, &src.read(e.off, 16).map_err(io)?).0)
}

// ---- NARC and the members' packing -------------------------------------------------------------------------------------------------

/// `narc_members(d, where)`
pub fn narc_members<'a>(d: &'a [u8], wh: &str) -> Result<Vec<&'a [u8]>, String> {
    if d.len() < 4 || &d[..4] != b"NARC" {
        return Err(format!("{wh}: not a NARC"));
    }
    if d.len() < 16 {
        return Err(format!("{wh}: NARC header cut short"));
    }
    let u16_ = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]) as usize;
    let u32_ = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
    let (mut o, n) = (u16_(0xc), u16_(0xe));
    if o < 16 {
        return Err(format!("{wh}: NARC sections overlap the header"));
    }
    let mut secs: Vec<(&[u8], (usize, usize))> = Vec::new();
    for _ in 0..n {
        if o + 8 > d.len() {
            return Err(format!("{wh}: a section past the end"));
        }
        let s = u32_(o + 4);
        if s < 8 || o + s > d.len() {
            return Err(format!("{wh}: invalid NARC section size at {o:#x}"));
        }
        let tag = &d[o..o + 4];
        match secs.iter_mut().find(|x| x.0 == tag) {
            Some(x) => x.1 = (o, s),
            None => secs.push((tag, (o, s))),
        }
        o += s;
    }
    let get = |t: &[u8]| secs.iter().find(|x| x.0 == t).map(|x| x.1);
    let (Some((fat, size)), Some((image, image_size))) = (get(b"BTAF"), get(b"GMIF")) else {
        return Err(format!("{wh}: no BTAF or GMIF"));
    };
    let data = image + 8;
    if size < 12 {
        return Err(format!("{wh}: NARC BTAF header cut short"));
    }
    let count = u32_(fat + 8);
    if count > (size - 12) / 8 {
        return Err(format!("{wh}: NARC BTAF member table cut short"));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (a, b) = (u32_(fat + 12 + 8 * i), u32_(fat + 16 + 8 * i));
        if !(a <= b && b <= image_size - 8) {
            return Err(format!("{wh}: member {i} past the GMIF end"));
        }
        out.push(&d[data + a..data + b]);
    }
    Ok(out)
}

fn cap() -> CraErr {
    CraErr::Other(format!(
        "MemoryError: {}",
        crate::inflate::cap_error(crate::inflate::MAX_DECODED)
    ))
}

/// `lz_undo(d, at)`: kinds 0x50 and 0x60
pub fn lz_undo(d: &[u8], mut at: usize) -> R<Vec<u8>> {
    let n = d.len();
    let mut out: Vec<u8> = Vec::new();
    let g = |i: usize| d.get(i).copied().ok_or_else(index_err);
    while at < n {
        let f = d[at];
        at += 1;
        if f == 0 {
            out.extend_from_slice(&d[at.min(n)..(at + 8).min(n)]);
            at += 8;
            continue;
        }
        let f = f.wrapping_neg();
        for bit in (0..8).rev() {
            if f >> bit & 1 == 0 {
                out.push(g(at)?);
                at += 1;
                continue;
            }
            let (lo, hi) = (g(at)? as usize, g(at + 1)? as usize);
            let dist = lo >> 4 | hi << 4;
            if dist == 0 {
                return Ok(out);
            }
            let mut ln = lo & 0xf;
            if ln > 1 {
                at += 2;
            } else if ln == 1 {
                ln = g(at + 2)? as usize + 0x11 + ((g(at + 3)? as usize) << 8) + 0xff;
                at += 4;
            } else {
                ln = g(at + 2)? as usize + 0x10;
                at += 3;
            }
            if dist > out.len() {
                return Err(CraErr::Cra("an LZ distance before the start".into()));
            }
            if out.len() + ln > crate::inflate::MAX_DECODED {
                return Err(cap());
            }
            let s = out.len() - dist;
            for k in 0..ln {
                let c = out[s + k];
                out.push(c);
            }
        }
    }
    Ok(out)
}

/// `zlib.decompress(d, wbits)` for 47 (a zlib or gzip header) and -15 (raw), or None where it raises
fn zlib_or_none(d: &[u8], raw: bool) -> Option<Vec<u8>> {
    if raw {
        // the exact zlib model: a raw stream has no check value, and miniz takes some damaged streams zlib refuses (a distance too far back)
        return crate::zlibx::decompress_raw(d, Some(crate::inflate::MAX_DECODED + 1))
            .ok()
            .filter(|z| z.eof && z.out.len() <= crate::inflate::MAX_DECODED)
            .map(|z| z.out);
    }
    if d.len() >= 2 && d[0] == 0x1f && d[1] == 0x8b {
        use std::io::Read;
        let mut out = Vec::new();
        let mut r = flate2::read::GzDecoder::new(d).take(crate::inflate::MAX_DECODED as u64 + 1);
        r.read_to_end(&mut out).ok()?;
        return (out.len() <= crate::inflate::MAX_DECODED).then_some(out);
    }
    crate::casc::zlib_decompress(d).ok()
}

/// `undo(d)`: a member unpacked by the kind in its first byte (anything else is the member itself)
pub fn undo(zstd: &dyn crate::zstd::Zstd, d: &[u8]) -> R<Vec<u8>> {
    undo_at(zstd, d, 0)
}

fn undo_at(zstd: &dyn crate::zstd::Zstd, d: &[u8], depth: usize) -> R<Vec<u8>> {
    if depth > 900 {
        return Err(CraErr::Other(
            "RecursionError: maximum recursion depth exceeded".into(),
        ));
    }
    let Some(&k) = d.first() else {
        return Ok(d.to_vec());
    };
    if k == 0x4d && d.starts_with(b"MMMM") {
        let h = upk(d, 4, 8)?; // `total, n = unpack_from('<II', d, 4)`
        let n = u32::from_le_bytes(h[4..8].try_into().unwrap()) as usize;
        let mut at = (12 + 4 * n + 15) & !15;
        let mut out = Vec::new();
        for i in 0..n {
            let s = u32at(d, 12 + 4 * i as i64)? as usize;
            let part = &d[at.min(d.len())..at.saturating_add(s).min(d.len())];
            out.extend(undo_at(zstd, part, depth + 1)?);
            if out.len() > crate::inflate::MAX_DECODED {
                return Err(cap());
            }
            at = at.saturating_add((s + 15) & !15);
        }
        return Ok(out);
    }
    if k == 0x94 && d.len() >= 8 {
        let size = u32at(d, 4)?;
        let b = &d[8..];
        let lim = size.min(256 * b.len() as u64 + 64) as usize;
        let mut out = vec![0u8; lim];
        return match lz4_flex::block::decompress_into(b, &mut out) {
            Ok(n) => {
                out.truncate(n);
                Ok(out)
            }
            Err(e) => Err(CraErr::Other(format!(
                "LZ4BlockError: Decompression failed: corrupt input or insufficient space in destination buffer ({e})"
            ))),
        };
    }
    if k == 0x5a && d.starts_with(b"Z\0\0\0") {
        return zstd
            .decompress(&d[8.min(d.len())..], None, None)
            .map_err(|e| CraErr::Other(format!("ZstdError: {e}")));
    }
    if k == 0x50 && d.len() >= 8 {
        return lz_undo(d, 8);
    }
    if k == 0x60 && d.len() >= 4 {
        return lz_undo(d, 4);
    }
    if (k == 0x70 || k == 0x80) && d.len() >= 0x10 {
        return Ok(d[0x10..].to_vec());
    }
    if matches!(k, 0 | 0x10 | 0x20 | 0x30) && d.len() >= 0x10 {
        return Ok(zlib_or_none(&d[0x10..], k == 0x30).unwrap_or_else(|| d.to_vec()));
    }
    Ok(d.to_vec())
}

/// scan_blob's NARC branch: each member unpacked and scanned (a fresh budget: a member's size says nothing about what it unpacks to)
pub fn narc_scan(
    cx: &Ctx<'_>,
    d: &[u8],
    label: &str,
    out: &mut Vec<Shader>,
    depth: usize,
) -> Res<()> {
    let members = narc_members(d, label).map_err(Failure::Red)?;
    for (i, m) in members.iter().enumerate() {
        let wh = format!("{label}/{i}");
        let u = match undo(cx.zstd, m) {
            Ok(u) => u,
            Err(CraErr::Cra(e)) => crate::red!("{wh}: {e}"),
            Err(CraErr::Other(e)) => return Err(Failure::Red(e)),
        };
        scan_blob(cx, &u, &wh, out, depth + 1, None)?;
    }
    Ok(())
}

// ---- the split's jobs --------------------------------------------------------------------------------------------------------------

/// A range of the archive's files (`cra_jobs`)
#[derive(Clone)]
pub struct CraJob {
    pub file: FileRef,
    pub entries: Vec<Entry>,
}

impl CraJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "cra:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(self.entries.iter().map(|e| format!("{e:?}")))
        .collect()
    }
}

/// `boot_nros()`: the `*.nro` of the folder (not the hidden ones), sorted
fn boot_nros(dir: Option<&Path>) -> Vec<std::path::PathBuf> {
    let Some(d) = dir else { return Vec::new() };
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(d)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.ends_with(".nro") && !n.starts_with('.')
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// `cra_jobs(starting(CRA))`: every file of each CRA archive in ranges of about equal length, the file table from the first NRO in `DM_NRO_DIR` that holds one
pub fn plan(
    files: &[FileRef],
    nro_dir: Option<&Path>,
) -> Result<(Vec<CraJob>, crate::jobs::Taken), SplitError> {
    let (mut jobs, mut taken) = (Vec::new(), crate::jobs::Taken::new());
    for f in files {
        if f.loc.open().ok().and_then(|s| s.read(0, 4).ok()).as_deref() != Some(&MAGIC[..]) {
            continue;
        }
        let mut es = None;
        for n in boot_nros(nro_dir) {
            let d = std::fs::read(&n).map_err(|e| SplitError::Io(e.to_string()))?;
            match index_files(&d) {
                Ok(v) => {
                    es = Some(v);
                    break;
                }
                Err((true, CraErr::Cra(_))) => continue,
                Err((_, e)) => {
                    return Err(SplitError::Red(format!("{}: {}", n.display(), e.text())));
                }
            }
        }
        let Some(es) = es else {
            return Err(SplitError::Red(format!(
                "/{}: a CRA archive, and no NRO in DM_NRO_DIR ({}) holds its file table",
                f.rel,
                nro_dir
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|| "None".into())
            )));
        };
        let step = es.iter().map(|e| e.size).sum::<u64>() as f64 / 64.0;
        let (mut cur, mut size) = (Vec::new(), 0u64);
        for e in es {
            size += e.size;
            cur.push(e);
            if size as f64 >= step {
                jobs.push(CraJob {
                    file: f.clone(),
                    entries: std::mem::take(&mut cur),
                });
                size = 0;
            }
        }
        if !cur.is_empty() {
            jobs.push(CraJob {
                file: f.clone(),
                entries: cur,
            });
        }
        taken.insert((f.root, f.rel.clone()));
    }
    Ok((jobs, taken))
}

/// `cra_shaders(path, rel, ents, out)`: each file unscrambled, unpacked and scanned like a loose file, labelled `/<rel>/<path>`; sound (FSTM) and movies (EBML) are left out by their
/// first block
pub fn shaders(cx: &Ctx<'_>, job: &CraJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let t = |e: CraErr| Failure::Red(e.text());
    let mut out = Vec::new();
    for e in &job.entries {
        let h = head(&src, e).map_err(t)?;
        if h.len() >= 4 && (&h[..4] == b"FSTM" || h[..4] == [0x1a, 0x45, 0xdf, 0xa3]) {
            continue;
        }
        let d = undo(cx.zstd, &read(&src, e).map_err(t)?).map_err(t)?;
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
