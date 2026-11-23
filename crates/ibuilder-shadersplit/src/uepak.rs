//! Unreal Engine paks: the engine's `uepak.py` (footer, index, entries, block decoding, shader code archives, LSLG / NVN2 records) and the parts of `shader_split.py` that read them
//! (`ue_legacy_jobs`, `ue_legacy_shaders`, `ue_packages`, `ue_inline_shaders`, `ue_shaders`), upstream seedds/nxnative origin/main (2026-11-23: 22222648 Zstandard blocks, b3e0d8c5 single-block
//! version 11 entries).
//!
//! Paks are read by their footer (magic `0x5A6F12E1` near the end), whatever their name ends with `.pak`. Versions 4, 8 (ordinary and short), 9 (serialized and frozen index), 11 and 12.
//! Blocks: stored, Zlib, Zstd, LZ4. Every other method (Oodle, LZ5, ...) needs the game's own decoder, which the Mac host runs for the Python (`DM_OODLE_HOST`); there is none on the phone, so
//! such a package is left out with the Python's own red line when it runs without a host (an archive or IoStore shader library that needs Oodle ends the split: [`OODLE_WHY`]).
//! An encrypted index (and encrypted entries) are read with the AES-256 key found in main as upstream finds it ([`Keys`]: two 16-byte literals main's code stores together, else a
//! 32-byte window of its read-only or data segment that decrypts the index start to a mount point). The key stays in memory.
//! IoStore containers (`uestore.rs`): the export bundles of the containers beside a pak with inline shaders are scanned like its packages; Unreal Engine 5 shader libraries are read
//! into archives with the paks' ones.
//!
//! Two routes, as upstream decides per pak (`pak_shader_sources`):
//! * inline shaders (version 4 and ordinary version 8 paks, DRAGON BALL Z: KAKAROT; version 11 paks with `.uexp` and no `.ushaderbytecode`; the inline half of short v8 / v9 paks): every file of
//!   the paks' merged view (a `_P.pak` replaces the same path) is decoded and searched for the shader records the engine keeps inline (`ue_inline_shaders`), as jobs of their own;
//! * shader code archives (`.ushaderbytecode`): upstream extracts them on the Mac (`uepak.py job` / `finish` into `<version dir>/ue_shaders`) and main() reads them (`ue_shaders`). Here they are
//!   read from the paks in process ([`archives`]), with the same result when every compressed shader is an LZ4 block (or the archive is version 1); Oodle shaders end the split.

use crate::bytes::*;
use crate::ctl::*;
use crate::error::{Failure, Res, SplitError};
use crate::image::Loc;
use crate::inflate::{self, Wrap};
use crate::scan::FileRef;
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use crate::uestore::{IO_SHADER_LIBRARY, Toc, mount_order};
use crate::zstd::Zstd;
use crate::{red, unsupported};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const PAK_MAGIC: u32 = 0x5A6F12E1;
pub const ARCHIVE_VERSION: u32 = 2;
const SHORT_V8_MAGIC_BACK: u64 = 0xac;
const PARTS: f64 = 64.0;
/// `oodle_ask` with no Mac host: the Python's red text
pub const OODLE_RED: &str = "Oodle- or LZ5-compressed game files: set DM_OODLE_HOST (the Mac host, make -C dm/host GAME=<id> dmhost) and DM_OODLE_MODULES (the rewritten modules) to decode them with the game's own decoder";
/// LSLG records in the archives and packages: 'LSLG' + this stage tag
pub fn ue_tag(t: &str) -> Option<St> {
    match t {
        "SV" => Some(St::Vs),
        "SH" => Some(St::Hs),
        "SD" => Some(St::Ds),
        "SG" => Some(St::Gs),
        "SP" => Some(St::Ps),
        "SC" => Some(St::Cs),
        _ => None,
    }
}
/// `FREQ`: EShaderFrequency in Unreal 4's numbering
pub fn freq(f: u8) -> Option<St> {
    match f {
        0 => Some(St::Vs),
        1 => Some(St::Hs),
        2 => Some(St::Ds),
        3 => Some(St::Ps),
        4 => Some(St::Gs),
        5 => Some(St::Cs),
        _ => None,
    }
}

/// Python's `f'{n:#x}'` for any integer (a negative one is `-0x..`)
pub fn hx(n: i128) -> String {
    if n < 0 {
        format!("-{:#x}", -n)
    } else {
        format!("{n:#x}")
    }
}

/// A Python `ValueError` raised in `uepak.py`: kept apart from `red()` so that `unreadable_why` can prefix its type as the Python does
#[derive(Debug, Clone)]
pub enum UeErr {
    /// `ValueError(msg)` (and `zlib.error` / `ZstdError` / `LZ4BlockError` wrapped by `Pak.read`)
    Value(String),
    /// `red(msg)` (a `SystemExit`)
    Red(String),
    /// a container this port does not read: ends the split
    Unsupported(String),
    /// another Python exception, already as `<type name>: <message>` (`zlib.error`, `IndexError`, `OSError`, ...)
    Other(String),
}

impl From<Failure> for UeErr {
    fn from(f: Failure) -> UeErr {
        match f {
            Failure::Red(s) | Failure::NoTree(s) => UeErr::Red(s),
            Failure::Unsupported(s) => UeErr::Unsupported(s),
        }
    }
}

impl UeErr {
    /// `unreadable_why(e, name)`
    pub fn why(&self, name: &str) -> String {
        let w = match self {
            UeErr::Red(s) => s.clone(),
            UeErr::Value(s) => format!("ValueError: {s}"),
            UeErr::Unsupported(s) | UeErr::Other(s) => s.clone(),
        };
        if w.trim_start_matches('/')
            .starts_with(name.trim_start_matches('/'))
        {
            w
        } else {
            format!("{name}: {w}")
        }
    }
    pub fn text(&self) -> &str {
        match self {
            UeErr::Value(s) | UeErr::Red(s) | UeErr::Unsupported(s) | UeErr::Other(s) => s,
        }
    }
}

type UeRes<T> = Result<T, UeErr>;

fn val<T>(m: impl Into<String>) -> UeRes<T> {
    Err(UeErr::Value(m.into()))
}

// ---- R: a table reader whose reads past the end are a ValueError ----------------------------------------------------------------

pub struct R<'a> {
    b: &'a [u8],
    o: i128,
    what: String,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8], o: i128, what: impl Into<String>) -> R<'a> {
        R {
            b,
            o,
            what: what.into(),
        }
    }
    pub fn need(&self, n: i128) -> UeRes<()> {
        if n < 0 || !(0 <= self.o && self.o <= self.b.len() as i128 - n) {
            return val(format!(
                "{}: {} bytes at {} do not fit in its {}",
                self.what,
                hx(n),
                hx(self.o),
                hx(self.b.len() as i128)
            ));
        }
        Ok(())
    }
    pub fn raw(&mut self, n: i128) -> UeRes<&'a [u8]> {
        self.need(n)?;
        let a = self.o as usize;
        self.o += n;
        Ok(&self.b[a..a + n as usize])
    }
    pub fn u8(&mut self) -> UeRes<u8> {
        Ok(self.raw(1)?[0])
    }
    pub fn u32(&mut self) -> UeRes<u32> {
        Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> UeRes<i32> {
        Ok(i32::from_le_bytes(self.raw(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> UeRes<u64> {
        Ok(u64::from_le_bytes(self.raw(8)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> UeRes<i64> {
        Ok(i64::from_le_bytes(self.raw(8)?.try_into().unwrap()))
    }
    /// `fstr()`: i32 length with the NUL; UTF-8 (`'replace'`), or UTF-16 when negative
    pub fn fstr(&mut self) -> UeRes<String> {
        let n = self.i32()? as i128;
        if n < 0 {
            let b = self.raw(-2 * n)?;
            let u: Vec<u16> = b
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let mut s = String::from_utf16(&u).map_err(|_| {
                UeErr::Value(
                    "'utf-16-le' codec can't decode bytes: illegal UTF-16 surrogate".into(),
                )
            })?;
            s.pop();
            return Ok(s);
        }
        if n == 0 {
            return Ok(String::new());
        }
        let b = self.raw(n)?;
        Ok(String::from_utf8_lossy(&b[..b.len() - 1]).into_owned())
    }
    /// `count()`: a TArray's length (i32)
    pub fn count(&mut self) -> UeRes<i32> {
        self.i32()
    }
    /// `[r.take(<size bytes>) for _ in range(count)]`: fails where the Python's loop would, at the first item that does not fit
    pub fn items(&mut self, count: i128, size: i128) -> UeRes<()> {
        if count <= 0 {
            return Ok(());
        }
        let room = (self.b.len() as i128 - self.o).max(0);
        if count * size <= room {
            self.o += count * size;
            return Ok(());
        }
        self.o += room / size * size;
        self.need(size)
    }
    pub fn pos(&self) -> i128 {
        self.o
    }
    pub fn seek(&mut self, o: i128) {
        self.o = o;
    }
    pub fn len(&self) -> i128 {
        self.b.len() as i128
    }
    pub fn is_empty(&self) -> bool {
        self.b.is_empty()
    }
    pub fn what(&self) -> &str {
        &self.what
    }
    pub fn bytes(&self) -> &'a [u8] {
        self.b
    }
}

// ---- footer --------------------------------------------------------------------------------------------------------------------

/// `read_range(f, off, n, what)`: n bytes of the source at off, after checking that they lie inside it
fn read_range(src: &dyn Source, off: i128, n: i128, what: &str) -> UeRes<Vec<u8>> {
    if off < 0 || n < 0 || off + n > src.len() as i128 {
        return val(format!(
            "{what}: {} bytes at {} do not lie inside the file",
            hx(n),
            hx(off)
        ));
    }
    let b = src
        .read(off as u64, n as usize)
        .map_err(|e| UeErr::Value(format!("OSError: {e}")))?;
    if b.len() as i128 != n {
        return val(format!("{what}: short read"));
    }
    Ok(b)
}

/// `magic_back(f)`: (how far from the end the footer's magic is, the pak version)
pub fn magic_back(src: &dyn Source) -> UeRes<(u64, u32)> {
    let size = src.len();
    for (back, foot) in [
        (0x2cu64, 0x2du64),
        (0xc8, 0xc8 + 21),
        (0xcc, 0xcc + 21),
        (0xcd, 0xcd + 21),
        (SHORT_V8_MAGIC_BACK, SHORT_V8_MAGIC_BACK + 21),
    ] {
        if size < foot {
            continue;
        }
        let b = src
            .read(size - back, 8)
            .map_err(|e| UeErr::Value(format!("OSError: {e}")))?;
        if b.len() < 8 {
            continue;
        }
        let (m, ver) = (
            u32::from_le_bytes(b[..4].try_into().unwrap()),
            u32::from_le_bytes(b[4..8].try_into().unwrap()),
        );
        if m == PAK_MAGIC && (back != SHORT_V8_MAGIC_BACK || ver == 8) {
            return Ok((back, ver));
        }
    }
    val("no pak footer")
}

/// `frozen_index(f, ver)`: version 9's FrozenIndex byte
fn frozen_index(src: &dyn Source, ver: u32) -> UeRes<bool> {
    if ver != 9 {
        return Ok(false);
    }
    let b = src
        .read(src.len().saturating_sub(161), 1)
        .map_err(|e| UeErr::Value(format!("OSError: {e}")))?;
    match b.first() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => val("invalid version 9 FrozenIndex flag"),
    }
}

#[derive(Clone, Debug)]
pub struct Footer {
    pub encrypted: bool,
    pub ioff: u64,
    pub isz: u64,
    pub methods: Vec<String>,
    pub version: u32,
    pub frozen: bool,
}

/// `footer(f)`
pub fn footer(src: &dyn Source) -> UeRes<Footer> {
    let (back, ver) = magic_back(src)?;
    if ([4, 11, 12].contains(&ver) && (ver == 4) != (back == 0x2c))
        || (ver == 8 && back == 0x2c)
        || ((ver == 9) != (back == 0xcd))
    {
        return val(format!(
            "a version {ver} pak footer with its magic {back:#x} bytes from the end, where that version has none"
        ));
    }
    let size = src.len();
    let rd = |off: u64, n: usize| -> UeRes<Vec<u8>> {
        src.read(off, n)
            .map_err(|e| UeErr::Value(format!("OSError: {e}")))
    };
    let (enc, ioff, isz, methods, frozen);
    if ver == 4 {
        let ft = rd(size - 0x2d, 0x2d)?;
        ioff = u64::from_le_bytes(ft[9..17].try_into().unwrap());
        isz = u64::from_le_bytes(ft[17..25].try_into().unwrap());
        enc = ft[0];
        methods = (1..8).map(|i| legacy_method(i as i64)).collect::<Vec<_>>();
        frozen = false;
    } else if ![8, 9, 11, 12].contains(&ver) {
        return val(format!(
            "pak version {ver}, only 4, 8, 9, 11 and 12 are read"
        ));
    } else {
        let n = (back + 21) as usize;
        let ft = rd(size - back - 21, n)?;
        let k = 21usize;
        ioff = u64::from_le_bytes(ft[k + 8..k + 16].try_into().unwrap());
        isz = u64::from_le_bytes(ft[k + 16..k + 24].try_into().unwrap());
        frozen = frozen_index(src, ver)?;
        let names = k + 44 + (ver == 9) as usize;
        let slots = if back == SHORT_V8_MAGIC_BACK { 4 } else { 5 };
        enc = ft[k - 1];
        methods = (0..slots)
            .map(|i| {
                let s = sl(&ft, names + 32 * i, names + 32 * (i + 1));
                let end = s.iter().rposition(|&c| c != 0).map(|p| p + 1).unwrap_or(0);
                s[..end].iter().map(|&c| c as char).collect::<String>()
            })
            .collect();
    }
    if ioff as u128 + isz as u128 > size as u128 {
        return val(format!(
            "the index ({isz:#x} bytes at {ioff:#x}) runs past the end of the pak"
        ));
    }
    Ok(Footer {
        encrypted: enc != 0,
        ioff,
        isz,
        methods,
        version: ver,
        frozen,
    })
}

/// `LEGACY_METHODS.get(i, f'flags {i}')`
fn legacy_method(i: i64) -> String {
    match i {
        1 => "Zlib".into(),
        2 => "Gzip".into(),
        3 => "LZ5".into(),
        4 => "Custom".into(),
        _ => format!("flags {i}"),
    }
}

/// Whether this file is an Unreal pak at all (a footer that parses), and its footer
pub fn probe(src: &dyn Source) -> Option<Footer> {
    footer(src).ok()
}

// ---- the index --------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PakEntry {
    pub off: i128,
    pub size: i128,
    pub raw: i128,
    pub method: String,
    pub encrypted: bool,
    pub block_size: i128,
    /// (start from the entry's offset, stored length)
    pub blocks: Vec<(i128, i128)>,
}

enum Files {
    /// versions 4, 8, 9: the entries as parsed
    Legacy(BTreeMap<String, PakEntry>),
    /// version 11 / 12: name -> offset into the encoded entries
    Encoded(Vec<u8>, BTreeMap<String, i32>),
}

pub struct Pak<'s> {
    /// the path the Python names the pak by (`self.path`)
    pub path: String,
    src: &'s dyn Source,
    pub version: u32,
    pub methods: Vec<String>,
    stored_header: i128,
    pub mount: String,
    files: Files,
    /// the AES key of an encrypted index and its encrypted entries (kept in memory only)
    key: Option<[u8; 32]>,
}

/// `aes_decrypt(key, b)[:n]` with the block-aligned length read before it
fn decrypt_range(
    src: &dyn Source,
    key: &[u8; 32],
    off: i128,
    n: i128,
    what: &str,
) -> UeRes<Vec<u8>> {
    let b = read_range(src, off, (n + 15) & !15, what)?;
    let mut d = crate::aes::decrypt(key, &b).map_err(UeErr::Value)?;
    d.truncate(n.max(0) as usize);
    Ok(d)
}

/// `frozen_pak_index(data, methods, what)`: UE4's 64-bit frozen FPakFileData
fn frozen_pak_index(
    data: &[u8],
    methods: &[String],
    what: &str,
) -> UeRes<(String, BTreeMap<String, PakEntry>)> {
    let reader = |at: i128, size: i128| -> UeRes<R<'_>> {
        let r = R::new(data, at, what);
        r.need(size)?;
        Ok(r)
    };
    let pointer = |at: i128, size: i128| -> UeRes<i128> {
        let packed = reader(at, 8)?.i64()? as i128;
        if packed & 1 == 0 {
            return val(format!("{what}: unfrozen pointer at {}", hx(at)));
        }
        let target = at + (packed >> 1);
        reader(target, size)?;
        Ok(target)
    };
    let array = |at: i128, stride: i128, exact: bool| -> UeRes<(i128, i128)> {
        let mut r = reader(at + 8, 8)?;
        let (n, cap) = (r.i32()? as i128, r.i32()? as i128);
        if n < 0 || cap < n || (exact && cap != n) {
            return val(format!("{what}: invalid frozen array at {}", hx(at)));
        }
        Ok((if n != 0 { pointer(at, n * stride)? } else { 0 }, n))
    };
    let string = |at: i128| -> UeRes<String> {
        let (start, n) = array(at, 2, true)?;
        if n == 0 {
            return Ok(String::new());
        }
        let raw = reader(start, n * 2)?.raw(n * 2)?;
        if raw[raw.len() - 2..] != [0, 0] {
            return val(format!("{what}: unterminated frozen string at {}", hx(at)));
        }
        let u: Vec<u16> = raw[..raw.len() - 2]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&u)
            .map_err(|_| UeErr::Value(format!("{what}: invalid frozen string at {}", hx(at))))
    };
    let slots = |at: i128, stride: i128| -> UeRes<Vec<i128>> {
        reader(at, 52)?;
        let (start, n) = array(at, stride, false)?;
        let mut r = reader(at + 24, 8)?;
        let (bits, capacity) = (r.i32()? as i128, r.i32()? as i128);
        if bits < n || capacity < bits {
            return val(format!(
                "{what}: invalid sparse allocation bits at {}",
                hx(at)
            ));
        }
        let flags = if bits != 0 {
            pointer(at + 16, ((bits + 31) / 32) * 4)?
        } else {
            0
        };
        let mut out = Vec::new();
        for i in 0..n {
            let b = data
                .get((flags + i / 8) as usize)
                .copied()
                .ok_or_else(|| UeErr::Value("IndexError: index out of range".into()))?;
            if b & (1 << (i % 8)) != 0 {
                out.push(start + i * stride);
            }
        }
        Ok(out)
    };
    let mount = string(0)?;
    let (start, count) = array(16, 80, true)?;
    let mut entries: Vec<(PakEntry, bool)> = Vec::new();
    for i in 0..count {
        let at = start + i * 80;
        let mut r = reader(at, 24)?;
        let (off, size, raw) = (r.i64()? as i128, r.i64()? as i128, r.i64()? as i128);
        let (blocks_at, nblocks) = array(at + 48, 16, true)?;
        let mut r = reader(at + 64, 9)?;
        let (bsz, meth, flags) = (r.u32()? as i128, r.i32()? as i128, r.u8()?);
        if off.min(size).min(raw).min(meth) < 0 || flags & !3 != 0 {
            return val(format!("{what}: invalid frozen entry {i}"));
        }
        if meth > methods.len() as i128 || (meth != 0 && methods[meth as usize - 1].is_empty()) {
            return val(format!(
                "{what}: entry {i} names unknown compression method {meth}"
            ));
        }
        let mut blocks = Vec::new();
        for j in 0..nblocks {
            let mut r = reader(blocks_at + j * 16, 16)?;
            let (s, t) = (r.i64()? as i128, r.i64()? as i128);
            if s < 0 || t < s {
                return val(format!("{what}: invalid compression block in entry {i}"));
            }
            blocks.push((s, t - s));
        }
        if meth != 0 && (blocks.is_empty() || bsz == 0) {
            return val(format!(
                "{what}: compressed entry {i} has no blocks or block size"
            ));
        }
        if blocks.is_empty() {
            blocks.push((53, size));
        }
        entries.push((
            PakEntry {
                off,
                size,
                raw,
                method: if meth != 0 {
                    methods[meth as usize - 1].clone()
                } else {
                    String::new()
                },
                encrypted: flags & 1 != 0,
                block_size: bsz,
                blocks,
            },
            flags & 2 != 0,
        ));
    }
    let mut files: BTreeMap<String, PakEntry> = BTreeMap::new();
    for directory in slots(32, 80)? {
        let dn = string(directory)?;
        for item in slots(directory + 16, 32)? {
            let name = format!("{dn}{}", string(item)?)
                .trim_start_matches('/')
                .to_string();
            let index = reader(item + 16, 4)?.i32()? as i128;
            if !(0 <= index && index < count) {
                return val(format!(
                    "{what}: {name} names entry {index} outside the entry array"
                ));
            }
            let (e, deleted) = &entries[index as usize];
            if *deleted && e.size == 0 {
                continue;
            }
            if files.contains_key(&name) {
                return val(format!("{what}: duplicate file {name}"));
            }
            files.insert(name, e.clone());
        }
    }
    Ok((mount, files))
}

impl<'s> Pak<'s> {
    /// `Pak(path, key)`: key = the AES key of an encrypted index and entries ([`Keys`]); None for a pak in the clear
    pub fn open(path: &str, src: &'s dyn Source, key: Option<[u8; 32]>) -> UeRes<Pak<'s>> {
        let ft = footer(src)?;
        let short = ft.version == 8 && ft.methods.len() == 4;
        let stored_header = if short { 50 } else { 53 };
        if ft.encrypted && key.is_none() {
            return val(format!(
                "{path}: the index is encrypted and no key was given"
            ));
        }
        let at = |off: i128, n: i128| -> UeRes<Vec<u8>> {
            match (ft.encrypted, key.as_ref()) {
                (true, Some(k)) => decrypt_range(src, k, off, n, path),
                _ => read_range(src, off, n, path),
            }
        };
        let idx = at(ft.ioff as i128, ft.isz as i128)?;
        let what = format!("{path}: the index");
        let mut pak = Pak {
            path: path.to_string(),
            key,
            src,
            version: ft.version,
            methods: ft.methods.clone(),
            stored_header,
            mount: String::new(),
            files: Files::Legacy(BTreeMap::new()),
        };
        if ft.frozen {
            let (mount, files) = frozen_pak_index(&idx, &ft.methods, &what)?;
            pak.mount = mount;
            pak.files = Files::Legacy(files);
            return Ok(pak);
        }
        let mut r = R::new(&idx, 0, what);
        pak.mount = r.fstr()?;
        let count = r.i32()?;
        if [4, 8, 9].contains(&ft.version) {
            let mut files = BTreeMap::new();
            for _ in 0..count.max(0) {
                let name = r.fstr()?;
                r.need(if short { 25 } else { 28 })?; // `take('<qqqB' / '<qqqi')`: one read
                let (off, size, raw) = (r.i64()? as i128, r.i64()? as i128, r.i64()? as i128);
                let meth = if short {
                    r.u8()? as i64
                } else {
                    r.i32()? as i64
                };
                r.raw(20)?;
                let mut blocks = Vec::new();
                if meth != 0 {
                    let nb = r.i32()?;
                    for _ in 0..nb.max(0) {
                        r.need(16)?;
                        let (s, t) = (r.i64()? as i128, r.i64()? as i128);
                        blocks.push((s, t));
                    }
                }
                r.need(5)?; // `take('<BI')`
                let (e, bsz) = (r.u8()?, r.u32()?);
                let head = stored_header
                    + if meth != 0 {
                        4 + 16 * blocks.len() as i128
                    } else {
                        0
                    };
                let blocks: Vec<(i128, i128)> = if ft.version < 5 {
                    blocks.into_iter().map(|(s, t)| (s - off, t - s)).collect()
                } else {
                    blocks.into_iter().map(|(s, t)| (s, t - s)).collect()
                };
                let method = if meth != 0 {
                    if ft.version < 8 {
                        legacy_method(meth)
                    } else if 0 < meth && meth as usize <= ft.methods.len() {
                        ft.methods[meth as usize - 1].clone()
                    } else {
                        format!("method {meth}")
                    }
                } else {
                    String::new()
                };
                files.insert(
                    name,
                    PakEntry {
                        off,
                        raw,
                        size,
                        method,
                        encrypted: e != 0,
                        block_size: bsz as i128,
                        blocks: if blocks.is_empty() {
                            vec![(head, size)]
                        } else {
                            blocks
                        },
                    },
                );
            }
            pak.files = Files::Legacy(files);
            return Ok(pak);
        }
        r.u64()?;
        if r.i32()? != 0 {
            r.need(16)?;
            r.u64()?;
            r.u64()?;
            r.raw(20)?;
        }
        if r.i32()? == 0 {
            return val(format!("{path}: no full directory index"));
        }
        r.need(16)?;
        let (doff, dsz) = (r.u64()?, r.u64()?);
        r.raw(20)?;
        let n = r.i32()? as i128;
        let enc = r.raw(n)?.to_vec();
        if r.i32()? != 0 {
            return val(format!("{path}: unencoded entries are not read"));
        }
        let dir = at(doff as i128, dsz as i128)?;
        let mut d = R::new(&dir, 0, format!("{path}: the directory index"));
        let mut files = BTreeMap::new();
        let nd = d.i32()?;
        for _ in 0..nd.max(0) {
            let dn = d.fstr()?;
            let nf = d.i32()?;
            for _ in 0..nf.max(0) {
                let f = d.fstr()?;
                let o = d.i32()?;
                files.insert(format!("{dn}{f}").trim_start_matches('/').to_string(), o);
            }
        }
        pak.files = Files::Encoded(enc, files);
        Ok(pak)
    }

    /// the file names, in name order
    pub fn names(&self) -> Vec<String> {
        match &self.files {
            Files::Legacy(m) => m.keys().cloned().collect(),
            Files::Encoded(_, m) => m.keys().cloned().collect(),
        }
    }

    pub fn len(&self) -> usize {
        match &self.files {
            Files::Legacy(m) => m.len(),
            Files::Encoded(_, m) => m.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `entry(path)`
    pub fn entry(&self, name: &str) -> UeRes<PakEntry> {
        let (enc, at) = match &self.files {
            Files::Legacy(m) => {
                return m
                    .get(name)
                    .cloned()
                    .ok_or_else(|| UeErr::Value(format!("KeyError: '{name}'")));
            }
            Files::Encoded(enc, m) => match m.get(name) {
                Some(&o) => (enc, o),
                None => return val(format!("KeyError: '{name}'")),
            },
        };
        let mut r = R::new(
            enc,
            at as i128,
            format!("{}: the entry of {name}", self.path),
        );
        let v = r.u32()?;
        let mut bsz = ((v & 0x3f) as i128) << 11;
        if v & 0x3f == 0x3f {
            bsz = r.u32()? as i128;
        }
        let (nblk, enc_, meth) = ((v >> 6) & 0xffff, (v >> 22) & 1, (v >> 23) & 0x3f);
        let mut field = |bit: u32| -> UeRes<i128> {
            Ok(if (v >> bit) & 1 != 0 {
                r.u32()? as i128
            } else {
                r.u64()? as i128
            })
        };
        let off = field(31)?;
        let raw = field(30)?;
        let size = if meth != 0 { field(29)? } else { raw };
        let mut cur: i128 = 53 + if meth != 0 { 4 + 16 * nblk as i128 } else { 0 };
        let mut blocks = Vec::new();
        if nblk == 1 {
            bsz = raw;
        }
        if nblk == 1 && enc_ == 0 {
            blocks.push((cur, size));
        } else {
            for _ in 0..nblk {
                let s = r.u32()? as i128;
                blocks.push((cur, s));
                cur += if enc_ != 0 { (s + 15) & !15 } else { s };
            }
        }
        if meth as usize > self.methods.len()
            || (meth != 0 && self.methods[meth as usize - 1].is_empty())
        {
            return val(format!(
                "{}: {name} names compression method {meth}, which the footer does not",
                self.path
            ));
        }
        Ok(PakEntry {
            off,
            raw,
            size,
            method: if meth != 0 {
                self.methods[meth as usize - 1].clone()
            } else {
                String::new()
            },
            encrypted: enc_ != 0,
            block_size: bsz,
            blocks,
        })
    }

    fn read_at(&self, off: i128, n: i128, enc: bool) -> UeRes<Vec<u8>> {
        if !enc {
            return read_range(self.src, off, n, &self.path);
        }
        match &self.key {
            Some(k) => decrypt_range(self.src, k, off, n, &self.path),
            None => val(format!(
                "{}: {} encrypted bytes at {} and no key was given",
                self.path,
                hx(n),
                hx(off)
            )),
        }
    }

    /// `read(path, decode=oodle)` (`oodle` = true: a method this port has no decoder for is the Python's red text with no Mac host; false: `decode=None`, its ValueError)
    pub fn read(&self, name: &str, zstd: &dyn Zstd, oodle: bool) -> UeRes<Vec<u8>> {
        let e = self.entry(name)?;
        if e.method.is_empty() {
            return self.read_at(e.off + self.stored_header, e.size, e.encrypted);
        }
        #[derive(PartialEq)]
        enum M {
            Zlib,
            Zstd,
            Lz4,
            Other,
        }
        let m = match e.method.as_str() {
            "Zlib" => M::Zlib,
            "Zstd" => M::Zstd,
            "LZ4" => M::Lz4,
            _ => {
                if !oodle {
                    return val(format!("{name} is {}-compressed in the pak", e.method));
                }
                M::Other
            }
        };
        let raw = e.raw;
        let mut left = raw;
        if left < 0 {
            return val(format!(
                "{}: {name} has negative raw size {}",
                self.path,
                hx(left)
            ));
        }
        let bsz = if e.blocks.len() == 1 {
            left
        } else {
            e.block_size
        };
        if e.blocks.len() > 1 && bsz <= 0 {
            return val(format!(
                "{}: {name} has non-positive block size {}",
                self.path,
                hx(bsz)
            ));
        }
        let mut out: Vec<u8> = Vec::new();
        for (i, (s, n)) in e.blocks.iter().enumerate() {
            let k = left.min(bsz);
            let b = self.read_at(e.off + s, *n, e.encrypted)?;
            let bad = |x: String| -> UeErr {
                UeErr::Value(format!(
                    "{}: {name} block {i} does not decode ({x})",
                    self.path
                ))
            };
            let d = match m {
                M::Zlib => zlib_block(&b, k).map_err(bad)?,
                M::Zstd => zstd_block(zstd, &b, k).map_err(bad)?,
                M::Lz4 => {
                    if k > 255 * b.len() as i128 {
                        return val(format!(
                            "{name}: an LZ4 block of {} bytes cannot hold {}",
                            hx(b.len() as i128),
                            hx(k)
                        ));
                    }
                    lz4_block(&b, k as usize).map_err(bad)?
                }
                M::Other => return Err(UeErr::Red(OODLE_RED.into())),
            };
            if d.len() as i128 != k {
                return val(format!(
                    "{}: {name} block {i} decodes to {} bytes, not {}",
                    self.path,
                    hx(d.len() as i128),
                    hx(k)
                ));
            }
            if out.len() as u128 + d.len() as u128 > inflate::MAX_DECODED as u128 {
                return val(format!(
                    "{}: {name}: {}",
                    self.path,
                    inflate::cap_error(inflate::MAX_DECODED)
                ));
            }
            out.extend_from_slice(&d);
            left -= k;
        }
        if left != 0 {
            return val(format!(
                "{}: {name} decoded {} bytes, not {}",
                self.path,
                hx(raw - left),
                hx(raw)
            ));
        }
        Ok(out)
    }
}

/// `zlib_block(b, k)`: a Zlib block of k raw bytes, inflated no further than k; `Err` = the `zlib.error` text
pub fn zlib_block(b: &[u8], k: i128) -> Result<Vec<u8>, String> {
    let lim = k.max(1) as usize;
    if lim > inflate::MAX_DECODED {
        return Err(inflate::cap_error(inflate::MAX_DECODED));
    }
    // the fast decoder first; anything but a clean end within the limit is decided by the exact zlib model (its error texts, its stop at max_length)
    if let Ok(z) = inflate::run(b, Wrap::Zlib, Some(lim + 1))
        && z.eof
        && z.out.len() <= lim
    {
        return Ok(z.out);
    }
    let z = crate::zlibx::decompress(b, Some(lim))?;
    if !z.eof {
        return Err(format!(
            "the stream does not end within its {} raw bytes",
            hx(k)
        ));
    }
    Ok(z.out)
}

/// `zstd_block(b, k)`: one Zstandard frame bounded by the pak block's raw size, frames without a content size included
pub fn zstd_block(zstd: &dyn Zstd, b: &[u8], k: i128) -> Result<Vec<u8>, String> {
    if k < 0 {
        return Err(format!("negative raw size {}", hx(k)));
    }
    if k as u128 >= inflate::MAX_DECODED as u128 {
        return Err(inflate::cap_error(inflate::MAX_DECODED));
    }
    let out = zstd.decompress(b, None, Some(k as usize + 1))?;
    if out.len() as i128 != k {
        return Err(format!("the frame does not end at its {} raw bytes", hx(k)));
    }
    Ok(out)
}

/// `lz4.block.decompress(b, uncompressed_size=k)`: at most k bytes (fewer when the block holds fewer)
pub fn lz4_block(b: &[u8], k: usize) -> Result<Vec<u8>, String> {
    if k > inflate::MAX_DECODED {
        return Err(inflate::cap_error(inflate::MAX_DECODED));
    }
    let mut out = vec![0u8; k];
    match lz4_flex::block::decompress_into(b, &mut out) {
        Ok(n) => {
            out.truncate(n);
            Ok(out)
        }
        Err(e) => Err(format!(
            "Decompression failed: corrupt input or insufficient space in destination buffer: {e}"
        )),
    }
}

// ---- which route a pak takes ----------------------------------------------------------------------------------------------------------

/// `pak_shader_sources(path, key)`: (scan inline packages, extract shader archives). `beside()` = `inline_in_containers(path)`: the IoStore containers in the pak's folder hold
/// export bundles and no shader library (asked only where the Python asks it). An encrypted pak with no key is sent for extraction, which reports the missing key.
pub fn pak_shader_sources(
    path: &str,
    src: &dyn Source,
    key: Option<[u8; 32]>,
    beside: &dyn Fn() -> bool,
) -> UeRes<(bool, bool)> {
    let ft = footer(src)?;
    if ft.version == 9 || (ft.version == 8 && ft.methods.len() == 4) {
        if ft.encrypted && key.is_none() {
            return Ok((false, true));
        }
        let pak = Pak::open(path, src, key)?;
        let names = pak.names();
        let archives = names.iter().any(|n| n.ends_with(".ushaderbytecode"));
        let members = names.iter().any(|n| {
            n.ends_with(".uexp")
                || n.rsplit('/')
                    .next()
                    .unwrap_or(n)
                    .starts_with("GlobalShaderCache-")
        });
        return Ok((members || beside(), archives));
    }
    if [4, 8, 9].contains(&ft.version) {
        return Ok((true, false));
    }
    if ft.encrypted && key.is_none() {
        return Ok((false, true));
    }
    let at = |off: i128, n: i128| -> UeRes<Vec<u8>> {
        match (ft.encrypted, key.as_ref()) {
            (true, Some(k)) => decrypt_range(src, k, off, n, path),
            _ => read_range(src, off, n, path),
        }
    };
    let idx = at(ft.ioff as i128, ft.isz as i128)?;
    let mut r = R::new(&idx, 0, format!("{path}: the index"));
    r.fstr()?;
    r.need(12)?; // `take('<iQ')`
    let count = r.i32()?;
    r.u64()?;
    if count == 0 {
        return Ok((false, false));
    }
    if r.i32()? != 0 {
        r.need(16)?;
        r.u64()?;
        r.u64()?;
        r.raw(20)?;
    }
    if r.i32()? == 0 {
        return Ok((false, true));
    }
    r.need(16)?;
    let (doff, dsz) = (r.u64()?, r.u64()?);
    let d = at(doff as i128, dsz as i128)?;
    let inline = !contains(&d, b".ushaderbytecode\0")
        && (contains(&d, b".uexp\0") || (contains(&d, b"GlobalShaderCache-") && beside()));
    Ok((inline, !inline))
}

// ---- the AES key in main ------------------------------------------------------------------------------------------------------------------

/// `img[a:b]` with Python's slice rules (negative bounds count from the end)
fn pyslice(img: &[u8], a: i64, b: i64) -> &[u8] {
    let n = img.len() as i64;
    let norm = |x: i64| if x < 0 { (n + x).max(0) } else { x.min(n) };
    let (a, b) = (norm(a), norm(b));
    if a < b {
        &img[a as usize..b as usize]
    } else {
        &[]
    }
}

/// `code_keys(img, lo)`: 32-byte keys that main's code stores from two 16-byte literals (`stp qA, qB, [x0]` after `ldr qA` / `ldr qB`, each from an `adrp` page plus its offset,
/// within 8 instructions before it). A candidate may be shorter than 32 bytes, as the Python's slices are; trying it is the Python's ValueError
fn code_keys(img: &[u8], lo: usize) -> Vec<Vec<u8>> {
    let nw = (lo / 4).min(img.len() / 4);
    let word = |i: usize| u32::from_le_bytes(img[4 * i..4 * i + 4].try_into().unwrap());
    let mut out = Vec::new();
    for i in 0..nw {
        let x = word(i);
        if x & 0xFFFF83E0 != 0xAD000000 {
            continue;
        }
        let mut page: [Option<i64>; 32] = [None; 32];
        let mut half: [Option<i64>; 32] = [None; 32];
        for j in i.saturating_sub(8)..i {
            let y = word(j);
            let rd = (y & 31) as usize;
            if y & 0x9F000000 == 0x90000000 {
                let imm = ((y >> 29) & 3) as i64 | (((y >> 5) & 0x7FFFF) as i64) << 2;
                let imm = if imm & (1 << 20) != 0 {
                    imm - (1 << 21)
                } else {
                    imm
                };
                page[rd] = Some(((j * 4) as i64 & !0xFFF) + (imm << 12));
            } else if y & 0xFFC00000 == 0x3DC00000 && page[((y >> 5) & 31) as usize].is_some() {
                half[rd] =
                    Some(page[((y >> 5) & 31) as usize].unwrap() + ((y >> 10) & 0xFFF) as i64 * 16);
            } else {
                page[rd] = None;
                half[rd] = None;
            }
        }
        let (a, b) = (half[(x & 31) as usize], half[((x >> 10) & 31) as usize]);
        if let (Some(a), Some(b)) = (a, b)
            && a + 16 <= img.len() as i64
            && b + 16 <= img.len() as i64
        {
            let mut k = pyslice(img, a, a + 16).to_vec();
            k.extend_from_slice(pyslice(img, b, b + 16));
            out.push(k);
        }
    }
    out
}

/// `find_key(img, lo, hi, pak, first)`: the AES key that decrypts the index start to a short mount point: one of `first` (a key another pak opened with), one main's code stores from two
/// 16-byte literals ([`code_keys`]), else one of main's 32-byte windows (read-only and data segments, 4-aligned, at least 20 distinct bytes)
fn find_key(
    img: &[u8],
    lo: usize,
    hi: usize,
    path: &str,
    src: &dyn Source,
    first: &[[u8; 32]],
) -> UeRes<Option<[u8; 32]>> {
    let ft = footer(src)?;
    let blk = src
        .read(ft.ioff, 16)
        .map_err(|e| UeErr::Other(format!("OSError: {e}")))?;
    let opens = |k: &[u8]| -> UeRes<bool> {
        let Ok(key) = <[u8; 32]>::try_from(k) else {
            return val("invalid AES key, data or IV length");
        };
        let d = crate::aes::decrypt(&key, &blk).map_err(UeErr::Value)?;
        if d.len() < 16 {
            return Err(UeErr::Other(format!(
                "error: unpack requires a buffer of 16 bytes (the index start has {})",
                d.len()
            )));
        }
        if ft.frozen {
            let ptr = i64::from_le_bytes(d[..8].try_into().unwrap());
            let n = i32::from_le_bytes(d[8..12].try_into().unwrap()) as i64;
            let cap = i32::from_le_bytes(d[12..16].try_into().unwrap()) as i64;
            let at = ptr >> 1;
            if ptr & 1 == 0
                || !(1 < n && n < 512)
                || n != cap
                || !(16 <= at && at <= ft.isz as i64 - n * 2)
            {
                return Ok(false);
            }
            let (begin, end) = (at & !15, (at + n * 2 + 15) & !15);
            let b = read_range(
                src,
                ft.ioff as i128 + begin as i128,
                (end - begin) as i128,
                path,
            )?;
            let x = crate::aes::decrypt(&key, &b).map_err(UeErr::Value)?;
            let raw = pyslice(&x, at - begin, at - begin + n * 2);
            let u: Vec<u16> = raw
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                .collect();
            if !raw.len().is_multiple_of(2) {
                return Ok(false);
            }
            let Ok(s) = String::from_utf16(&u) else {
                return Ok(false);
            };
            let body: Vec<char> = s.chars().collect();
            return Ok((s.starts_with('.') || s.starts_with('/'))
                && s.ends_with('\0')
                && body[..body.len() - 1]
                    .iter()
                    .all(|&c| c >= ' ' && !('\x7f'..='\u{9f}').contains(&c)));
        }
        let n = i32::from_le_bytes(d[..4].try_into().unwrap()) as i64;
        if !(1 < n && n < 512) || !matches!(d[4], b'.' | b'/') {
            return Ok(false);
        }
        let s = &d[4..(4 + n).min(16) as usize];
        let head = &s[..((n - 1) as usize).min(s.len())];
        Ok(head.iter().all(|&c| (0x20..0x7F).contains(&c))
            && ((s.len() as i64) < n || s[(n - 1) as usize] == 0))
    };
    for k in first {
        if opens(k)? {
            return Ok(Some(*k));
        }
    }
    for k in code_keys(img, lo) {
        if opens(&k)? {
            return Ok(Some(k.try_into().unwrap()));
        }
    }
    let mut a = lo;
    while (a as i64) < hi as i64 - 32 {
        let w = sl(img, a, a + 32);
        let mut seen = [0u64; 4];
        let mut distinct = 0;
        for &c in w {
            let (q, bit) = ((c >> 6) as usize, 1u64 << (c & 63));
            if seen[q] & bit == 0 {
                seen[q] |= bit;
                distinct += 1;
            }
        }
        if distinct >= 20 && opens(w)? {
            return Ok(Some(w.try_into().unwrap()));
        }
        a += 4;
    }
    Ok(None)
}

/// `Keys(mains)`: the AES keys of a game's paks, from its mains (the ExeFS main of each version, base game first). Each main is loaded once, at the first encrypted pak; a key found is
/// tried first on the next pak. The keys stay in memory: never printed, logged or written.
pub struct Keys {
    mains: Vec<std::path::PathBuf>,
    imgs: HashMap<usize, (Vec<u8>, usize, usize)>,
    /// the keys found so far, in order
    pub found: Vec<[u8; 32]>,
}

impl Keys {
    pub fn new(mains: Vec<std::path::PathBuf>) -> Keys {
        Keys {
            mains,
            imgs: HashMap::new(),
            found: Vec::new(),
        }
    }

    /// `keys(pak)`: None for a pak whose index is not encrypted, else the key [`find_key`] finds, trying each main in turn; the Python's ValueError when none opens it
    pub fn key(&mut self, path: &str, src: &dyn Source) -> UeRes<Option<[u8; 32]>> {
        if !footer(src)?.encrypted {
            return Ok(None);
        }
        for m in 0..self.mains.len() {
            if let std::collections::hash_map::Entry::Vacant(e) = self.imgs.entry(m) {
                let module =
                    ibuilder_nso::read(&self.mains[m]).map_err(|x| UeErr::Value(x.to_string()))?;
                let lo = module.segments[1].0 as usize;
                let hi = module.image.len();
                e.insert((module.image, lo, hi));
            }
            let (img, lo, hi) = &self.imgs[&m];
            if let Some(k) = find_key(img, *lo, *hi, path, src, &self.found)? {
                if !self.found.contains(&k) {
                    self.found.push(k);
                }
                return Ok(Some(k));
            }
        }
        val(format!(
            "{path}: the index is encrypted and no key in main (a 32-byte window or two 16-byte literals its code stores together) decrypts it"
        ))
    }

    /// shader_split's `key(p)`: any error is no key
    pub fn try_key(&mut self, path: &str, src: &dyn Source) -> Option<[u8; 32]> {
        self.key(path, src).ok().flatten()
    }
}

// ---- normpath ---------------------------------------------------------------------------------------------------------------------------

/// `os.path.normpath` (POSIX)
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let initial = if p.starts_with('/') {
        if p.starts_with("//") && !p.starts_with("///") {
            2
        } else {
            1
        }
    } else {
        0
    };
    let mut comps: Vec<&str> = Vec::new();
    for c in p.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(c);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let s = format!("{}{}", "/".repeat(initial), comps.join("/"));
    if s.is_empty() { ".".into() } else { s }
}

/// `re.sub(r"^(\.\./)+", "", normpath(mount + name))`
pub fn label_of(mount: &str, name: &str) -> String {
    let mut s = normpath(&format!("{mount}{name}"));
    while let Some(t) = s.strip_prefix("../") {
        s = t.to_string();
    }
    s
}

// ---- jobs ---------------------------------------------------------------------------------------------------------------------------

/// The planned jobs, and the `(root, rel)` of the paks and containers they take out of the plain list
pub type Planned = (Vec<UeJob>, HashSet<(usize, String)>);
/// `(root, folder, normpath(mount + name))` -> `(pak, name, raw size)`: one file of the merged view
type ViewItem = ((usize, String, String), (usize, String, i128));
/// One decoded archive by file name: its shaders `(frequency, bytes)`
pub type NamedArchive = (String, Vec<(u8, Vec<u8>)>);
/// A container beside an inline pak: its `.utoc`, path, key and `.ucas` partitions
type IoToc<'a> = (&'a FileRef, String, Option<[u8; 32]>, Vec<(String, Loc)>);
/// `(root, folder, chunk ID)`: one export bundle of the merged view
type ChunkAt = (usize, String, [u8; 12]);
/// One package of a job: its bytes, decoded when asked
type Package<'a> = Box<dyn Fn() -> UeRes<Vec<u8>> + 'a>;

/// What one job of `ue_legacy_jobs` reads
#[derive(Clone)]
pub enum UeWhat {
    /// `UE_LEGACY`: names of one pak's files
    Pak(Vec<String>),
    /// `UE_IO`: export bundles (chunk indices) of one IoStore container, and its `.ucas` partitions by name below the RomFS
    Io(Vec<usize>, Vec<(String, Loc)>),
}

/// One job of `ue_legacy_jobs`
#[derive(Clone)]
pub struct UeJob {
    /// the pak, or the container's `.utoc`
    pub file: FileRef,
    /// the path the Python names the pak or container by (its messages)
    pub path: String,
    pub what: UeWhat,
    /// the AES key of an encrypted pak or container (in memory only; the memo identity says only that there is one)
    pub key: Option<[u8; 32]>,
}

impl UeJob {
    pub fn identity(&self) -> Vec<String> {
        let tag = match self.what {
            UeWhat::Pak(_) => "uepak",
            UeWhat::Io(..) => "uestore",
        };
        let mut v = vec![format!(
            "{tag}:{}:{}:{}{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            if self.key.is_some() { ":aes" } else { "" }
        )];
        match &self.what {
            UeWhat::Pak(names) => v.extend(names.iter().cloned()),
            UeWhat::Io(chunks, _) => v.extend(chunks.iter().map(|c| c.to_string())),
        }
        v
    }
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// A container's `.utoc` parsed without a key (TOC metadata is in the clear), `ucas` finding its partitions
fn open_toc(
    f: &FileRef,
    path: &str,
    key: Option<[u8; 32]>,
    ucas: &dyn Fn(&str) -> Option<Loc>,
) -> UeRes<Toc> {
    let src = f
        .loc
        .open()
        .map_err(|e| UeErr::Other(format!("OSError: {e}")))?;
    let b = src
        .read(0, src.len() as usize)
        .map_err(|e| UeErr::Other(format!("OSError: {e}")))?;
    Toc::parse(path, &f.rel, &b, key, ucas)
}

/// `inline_in_containers(path)`: the containers in the pak's folder hold export bundles and no shader library (containers that do not parse count as absent, as in `io_tocs`)
fn inline_in_containers<'a>(
    pak: &FileRef,
    files: impl Iterator<Item = &'a FileRef>,
    path_of: &dyn Fn(&FileRef) -> String,
) -> bool {
    let dir = dir_of(&pak.rel);
    let (mut bundles, mut libs) = (false, false);
    for f in files {
        if f.root != pak.root || !f.rel.ends_with(".utoc") || dir_of(&f.rel) != dir {
            continue;
        }
        if let Ok(t) = open_toc(f, &path_of(f), None, &|_| None) {
            bundles |= !t.export_bundles().is_empty();
            libs |= !t.of_type(IO_SHADER_LIBRARY).is_empty();
        }
    }
    bundles && !libs
}

/// Splits `items` (`(owner, name, raw size)`, sorted) into jobs of about equal raw size, as `ue_legacy_jobs` does per tag
fn parts<T: Clone>(items: Vec<(usize, T, i128)>) -> Vec<(usize, Vec<T>)> {
    let step = items.iter().map(|x| x.2).sum::<i128>() as f64 / PARTS;
    let mut jobs = Vec::new();
    let mut cur: Vec<(usize, Vec<T>)> = Vec::new();
    let mut size = 0i128;
    for (pi, name, raw) in items {
        match cur.iter_mut().find(|(p, _)| *p == pi) {
            Some((_, v)) => v.push(name),
            None => cur.push((pi, vec![name])),
        }
        size += raw;
        if size as f64 >= step {
            jobs.append(&mut cur);
            size = 0;
        }
    }
    jobs.append(&mut cur);
    jobs
}

/// `ue_legacy_jobs(files, mains=main_v)`: the jobs (paks with inline shaders, then the export bundles of the containers beside them), and the paks and containers they take out of
/// the plain list. `mains`: the ExeFS main of every version, whose AES key opens an encrypted pak ([`Keys`])
pub fn legacy_jobs(
    files: &[FileRef],
    path_of: &dyn Fn(&FileRef) -> String,
    log: &(dyn Fn(&str) + Sync),
    mains: &[std::path::PathBuf],
) -> Result<Planned, SplitError> {
    let mut keys = Keys::new(mains.to_vec());
    let by_rel: HashMap<(usize, &str), &FileRef> = files
        .iter()
        .map(|f| ((f.root, f.rel.as_str()), f))
        .collect();
    let mut paks: Vec<(&FileRef, String, bool)> = Vec::new();
    for f in files {
        if !f.rel.ends_with(".pak") {
            continue;
        }
        let Ok(src) = f.loc.open() else { continue };
        let path = path_of(f);
        let k = keys.try_key(&path, &src);
        let beside = || inline_in_containers(f, files.iter(), path_of);
        match pak_shader_sources(&path, &src, k, &beside) {
            Ok((true, archives)) => paks.push((f, path, archives)),
            Ok(_) => {}
            Err(UeErr::Unsupported(m)) => return Err(SplitError::Unsupported(m)),
            Err(_) => {}
        }
    }
    let mut taken = HashSet::new();
    if paks.is_empty() {
        return Ok((Vec::new(), taken));
    }
    paks.sort_by(|a, b| {
        (a.0.rel.ends_with("_P.pak"), &a.0.rel).cmp(&(b.0.rel.ends_with("_P.pak"), &b.0.rel))
    });
    // (root, folder, normpath(mount + name)) -> (pak index, name, raw size)
    let mut view: HashMap<(usize, String, String), (usize, String, i128)> = HashMap::new();
    let mut pak_keys: HashMap<usize, Option<[u8; 32]>> = HashMap::new();
    for (pi, (f, path, archives)) in paks.iter().enumerate() {
        let src = match f.loc.open() {
            Ok(s) => s,
            Err(e) => {
                log(&format!(
                    "[RED] /{}: the pak cannot be read ([Errno 5] {e}); its shaders are left out",
                    f.rel
                ));
                continue;
            }
        };
        let got = (|| -> UeRes<(Option<[u8; 32]>, Vec<ViewItem>)> {
            let k = keys.key(path, &src)?;
            let pk = Pak::open(path, &src, k)?;
            let mut v = Vec::new();
            for name in pk.names() {
                if *archives && name.ends_with(".ushaderbytecode") {
                    continue;
                }
                let raw = pk.entry(&name)?.raw;
                v.push((
                    (
                        f.root,
                        dir_of(&f.rel).to_string(),
                        normpath(&format!("{}{name}", pk.mount)),
                    ),
                    (pi, name, raw),
                ));
            }
            Ok((k, v))
        })();
        match got {
            Ok((k, v)) => {
                pak_keys.insert(pi, k);
                view.extend(v);
            }
            Err(e) => log(&format!(
                "[RED] /{}: the pak cannot be read ({}); its shaders are left out",
                f.rel,
                e.text()
            )),
        }
    }
    for (f, _, _) in &paks {
        taken.insert((f.root, f.rel.clone()));
    }
    // the export bundles of the containers in the paks' folders, the newest copy of each chunk ID (mount order)
    let folders: HashSet<(usize, &str)> = paks
        .iter()
        .map(|(f, _, _)| (f.root, dir_of(&f.rel)))
        .collect();
    let mut tocs: Vec<(&FileRef, String)> = files
        .iter()
        .filter(|f| f.rel.ends_with(".utoc") && folders.contains(&(f.root, dir_of(&f.rel))))
        .map(|f| (f, path_of(f)))
        .collect();
    tocs.sort_by_cached_key(|(_, p)| mount_order(p));
    let mut io: Vec<IoToc<'_>> = Vec::new();
    let mut chunks: HashMap<ChunkAt, (usize, usize, i128)> = HashMap::new();
    for (f, path) in tocs {
        let t = match open_toc(f, &path, None, &|_| None) {
            Ok(t) => t,
            Err(e) => {
                log(&format!(
                    "[RED] /{}: the container cannot be read ({}); its shaders are left out",
                    f.rel,
                    e.text()
                ));
                continue;
            }
        };
        let bundles = t.export_bundles();
        if bundles.is_empty() {
            continue;
        }
        let mut key = None;
        if t.encrypted {
            let beside = format!("{}.pak", &f.rel[..f.rel.len() - 5]);
            key = by_rel
                .get(&(f.root, beside.as_str()))
                .and_then(|p| {
                    let src = p.loc.open().ok()?;
                    keys.try_key(&path_of(p), &src)
                })
                .or_else(|| keys.found.first().copied());
            if key.is_none() {
                log(&format!(
                    "[RED] /{}: the container is encrypted and no key of the game opens the pak beside it; its shaders are left out",
                    f.rel
                ));
                continue;
            }
        }
        taken.insert((f.root, f.rel.clone()));
        let mut parts_ = Vec::new();
        for rel in t.ucas_rels(&f.rel) {
            if let Some(u) = by_rel.get(&(f.root, rel.as_str())) {
                parts_.push((rel.clone(), u.loc.clone()));
            }
            taken.insert((f.root, rel));
        }
        let ti = io.len();
        io.push((f, path, key, parts_));
        for i in bundles {
            chunks.insert(
                (f.root, dir_of(&f.rel).to_string(), t.ids[i]),
                (ti, i, t.ranges[i].1 as i128),
            );
        }
    }
    let mut items: Vec<(usize, String, i128)> = view.into_values().collect();
    items.sort_by(|a, b| (&paks[a.0].0.rel, &a.1).cmp(&(&paks[b.0].0.rel, &b.1)));
    let mut jobs = Vec::new();
    for (pi, names) in parts(items) {
        jobs.push(UeJob {
            file: paks[pi].0.clone(),
            path: paks[pi].1.clone(),
            what: UeWhat::Pak(names),
            key: pak_keys.get(&pi).copied().flatten(),
        });
    }
    let mut items: Vec<(usize, usize, i128)> = chunks.into_values().collect();
    items.sort_by(|a, b| (&io[a.0].0.rel, a.1).cmp(&(&io[b.0].0.rel, b.1)));
    for (ti, ids) in parts(items) {
        jobs.push(UeJob {
            file: io[ti].0.clone(),
            path: io[ti].1.clone(),
            what: UeWhat::Io(ids, io[ti].3.clone()),
            key: io[ti].2,
        });
    }
    Ok((jobs, taken))
}

/// `UeErr` as the failure of a whole job (`scan_file`'s unreadable result)
fn job_failure(e: UeErr) -> Failure {
    match e {
        UeErr::Unsupported(m) => Failure::Unsupported(m),
        UeErr::Red(m) | UeErr::Other(m) => Failure::Red(m),
        UeErr::Value(m) => Failure::Red(format!("ValueError: {m}")),
    }
}

/// `ue_legacy_shaders` / `ue_io_shaders` + `ue_packages`: every package of the job decoded and scanned on its own; a package that fails costs only itself (at most 8 lines per job).
/// -> the shaders, and `(first reason, packages left out)` when any was
pub fn legacy_shaders(
    cx: &Ctx<'_>,
    job: &UeJob,
    out: &mut Vec<Shader>,
) -> Result<Option<(String, usize)>, Failure> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    // (label, read) of every package, in the job's order
    let pk;
    let toc;
    let mut items: Vec<(String, Package<'_>)> = Vec::new();
    match &job.what {
        UeWhat::Pak(names) => {
            pk = Pak::open(&job.path, &src, job.key).map_err(job_failure)?;
            for name in names {
                let pk = &pk;
                items.push((
                    format!("/{}#{}", job.file.rel, label_of(&pk.mount, name)),
                    Box::new(move || pk.read(name, cx.zstd, true)),
                ));
            }
        }
        UeWhat::Io(ids, ucas) => {
            let lookup = |n: &str| ucas.iter().find(|(r, _)| r == n).map(|(_, l)| l.clone());
            toc = open_toc(&job.file, &job.path, job.key, &lookup).map_err(job_failure)?;
            let names = toc.names().map_err(job_failure)?;
            for &i in ids {
                let toc = &toc;
                let name = names
                    .get(&i)
                    .filter(|n| !n.is_empty())
                    .cloned()
                    .unwrap_or_else(|| crate::uestore::hex8(&toc.ids[i]));
                items.push((
                    format!("/{}#{name}", job.file.rel),
                    Box::new(move || toc.read(i, &|| UeErr::Red(OODLE_RED.into()))),
                ));
            }
        }
    }
    let (mut first, mut lost) = (String::new(), 0usize);
    for (label, read) in items {
        let r = (|| -> UeRes<()> {
            let d = read()?;
            let mut found = Vec::new();
            inline_shaders(&d, &label, &mut found)?;
            out.extend(found);
            Ok(())
        })();
        if let Err(e) = r {
            if let UeErr::Unsupported(m) = e {
                return Err(Failure::Unsupported(m));
            }
            let why = e.why(&label);
            if first.is_empty() {
                first = why.clone();
            }
            lost += 1;
            if lost <= 8 {
                cx.print(&format!("[RED] {why} (its shaders are left out)"));
            }
        }
    }
    Ok((lost > 0).then_some((first, lost)))
}

// ---- records ------------------------------------------------------------------------------------------------------------------------

const CTL_NVN2: [u8; 8] = [0x34, 0x12, 0x76, 0x98, 0xc9, 0, 0, 0];

fn rd32(s: &[u8], o: i128) -> Option<u32> {
    if o < 0 {
        return None;
    }
    let o = o as usize;
    s.get(o..o.checked_add(4)?)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// `_maxwell_sections(s, k, control_size)`: (control section, code area)
fn maxwell_sections(s: &[u8], k: i128, control_size: Option<u32>) -> Option<(&[u8], &[u8])> {
    if k < 8 || k + 0x708 > s.len() as i128 {
        return None;
    }
    let (nctl, ncode) = (rd32(s, k - 4)?, rd32(s, k + 0x704)?);
    if control_size.is_some_and(|c| c != nctl) {
        return None;
    }
    let c = k - 4 - ncode as i128;
    if c < 4 || rd32(s, c - 4)? != ncode || k + nctl as i128 > s.len() as i128 {
        return None;
    }
    let (k, c) = (k as usize, c as usize);
    Some((&s[k..k + nctl as usize], &s[c..c + ncode as usize]))
}

/// `lslg(s)`: (stage tag, control section, code area)
pub fn lslg(s: &[u8]) -> Option<(String, &[u8], &[u8])> {
    if !starts(s, b"LSLG") {
        return None;
    }
    let k = find(s, &CTL_MAGIC, 0).map(|x| x as i128).unwrap_or(-1);
    let (ctl, code) = maxwell_sections(s, k, None)?;
    let tag: String = sl(s, 4, 6).iter().map(|&c| c as char).collect();
    Some((tag, ctl, code))
}

/// `raw_record(s)`: an untagged Switch record with a 0x880-byte control section, when exactly one layout fits
pub fn raw_record(s: &[u8]) -> Option<(&[u8], &[u8])> {
    if starts(s, b"LSLG") {
        return None;
    }
    let mut found = None;
    for k in find_all(s, &CTL_MAGIC) {
        if eq_at(s, k, &CTL_NVN2) {
            continue;
        }
        let Some(sec) = maxwell_sections(s, k as i128, Some(0x880)) else {
            continue;
        };
        if found.is_some() {
            return None;
        }
        found = Some(sec);
    }
    found
}

/// `sage_record(s)`: a Switch 2 record (an NVN2 control section): (control section, code area)
pub fn sage_record(s: &[u8]) -> Option<(&[u8], &[u8])> {
    let k = find(s, &CTL_NVN2, 0)? as i128;
    if k < 12 || k + 0x48 > s.len() as i128 {
        return None;
    }
    let (nctl, ncode) = (rd32(s, k - 4)?, rd32(s, k + 0x44)?);
    let c = k - 4 - ncode as i128;
    if c < 4 || rd32(s, c - 4)? != ncode || k + nctl as i128 > s.len() as i128 {
        return None;
    }
    let (k, c) = (k as usize, c as usize);
    Some((&s[k..k + nctl as usize], &s[c..c + ncode as usize]))
}

fn stname(st: Option<St>) -> &'static str {
    st.map(|s| s.short()).unwrap_or("None")
}

/// `shader_map_entries(d, proved)`: the FShaderMapResourceCode tables whose first entry `proved` accepts
fn shader_map_entries(
    d: &[u8],
    proved: &mut dyn FnMut((usize, usize, u32, u8)) -> UeRes<bool>,
) -> UeRes<Vec<(usize, usize, u32, u8)>> {
    let mut out = Vec::new();
    let mut end = 0usize;
    for i in memchr::memchr_iter(0x8c, d) {
        if !matches!(d.get(i + 1), Some(0x06 | 0x0a | 0x0b | 0x0c)) {
            continue;
        }
        if i < end || i < 12 {
            continue;
        }
        let count = u32::from_le_bytes(d[i - 8..i - 4].try_into().unwrap()) as i128;
        let hashes = i as i128 - 8 - 20 * count;
        if !(1..=1 << 20).contains(&count)
            || hashes < 4
            || rd32(d, hashes - 4) != Some(count as u32)
        {
            continue;
        }
        let mut rows = Vec::new();
        let mut p = i - 4;
        for _ in 0..count {
            if p + 4 > d.len() {
                break;
            }
            let n = u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize;
            let q = p + 4 + n;
            if n < 1 || q + 5 > d.len() {
                break;
            }
            let (raw, f) = (
                u32::from_le_bytes(d[q..q + 4].try_into().unwrap()),
                d[q + 4],
            );
            if !(n as u64 <= raw as u64 && raw <= 1 << 28) || f > 15 {
                break;
            }
            rows.push((p + 4, n, raw, f));
            p = q + 5;
        }
        if rows.len() as i128 == count && proved(rows[0])? {
            out.extend(rows);
            end = p;
        }
    }
    Ok(out)
}

/// `ue_inline_shaders(d, label, out)`: the shader records a cooked package or global shader cache keeps inline: zlib streams with their length in front, stored LSLG records,
/// LZ4 blocks of FShaderMapResourceCode, and tables of the shader map (whose Oodle streams need the game's decoder: with none, the Python's red text)
pub fn inline_shaders(d: &[u8], label: &str, out: &mut Vec<Shader>) -> UeRes<()> {
    let take = |s: &[u8], at: usize, out: &mut Vec<Shader>| -> UeRes<()> {
        let Some((tag, ctl, code)) = lslg(s) else {
            return Err(UeErr::Red(format!(
                "{label}: the shader record at {at:#x} is not an LSLG record of the known layout"
            )));
        };
        let Some(st) = ue_tag(&tag) else {
            return Err(UeErr::Red(format!(
                "{label}: the shader record at {at:#x} has stage tag {tag}"
            )));
        };
        if let Some(w) = check(st, ctl, code)? {
            return Err(UeErr::Red(format!(
                "{label}: shader at {at:#x} ({}): {w}",
                st.short()
            )));
        }
        out.push(Shader::new(format!("{label}#UE{at:x}"), st, ctl, code));
        Ok(())
    };
    let len_before = |i: usize| -> usize {
        if i >= 4 {
            u32::from_le_bytes(d[i - 4..i].try_into().unwrap()) as usize
        } else {
            0
        }
    };
    // zlib streams
    for i in memchr::memchr_iter(0x78, d) {
        if !matches!(d.get(i + 1), Some(0x01 | 0x5e | 0x9c | 0xda)) {
            continue;
        }
        let n = len_before(i);
        if !(2 <= n && n <= d.len() - i) {
            continue;
        }
        let z = &d[i..i + n];
        match inflate::run(z, Wrap::Zlib, Some(4)) {
            Ok(h) if h.out.len() == 4 && h.out == b"LSLG" => {}
            _ => continue,
        }
        let Ok(s) = inflate::run(z, Wrap::Zlib, None) else {
            continue;
        };
        if s.eof && s.used == n && starts(&s.out, b"LSLG") {
            take(&s.out, i, out)?;
        }
    }
    // stored records and LZ4 blocks
    for i in find_all(d, b"LSLG") {
        let n = len_before(i);
        if 2 <= n && n <= d.len() - i && lslg(&d[i..i + n]).is_some() {
            take(&d[i..i + n], i, out)?;
            continue;
        }
        let lo = (i as i64 - 6).max(4);
        let mut j = i as i64 - 1;
        while j >= lo {
            let ju = j as usize;
            j -= 1;
            let (t, ext) = (d[ju] >> 4, &d[ju + 1..i]);
            if (t < 15 && !ext.is_empty())
                || (t == 15
                    && !(!ext.is_empty()
                        && ext[ext.len() - 1] < 0xff
                        && ext[..ext.len() - 1].iter().all(|&x| x == 0xff)))
            {
                continue;
            }
            let n = u32::from_le_bytes(d[ju - 4..ju].try_into().unwrap()) as usize;
            if !(8 <= n && n as i128 <= d.len() as i128 - ju as i128 - 5) {
                continue;
            }
            let raw = u32::from_le_bytes(d[ju + n..ju + n + 4].try_into().unwrap()) as usize;
            if raw > 255 * n {
                continue; // more than the block can hold: lz4 refuses it
            }
            let Ok(s) = lz4_block(&d[ju..ju + n], raw) else {
                continue;
            };
            if s.len() == raw && starts(&s, b"LSLG") {
                take(&s, ju, out)?;
                break;
            }
        }
    }
    // tables of the shader map (Oodle streams: the game's decoder; stored ones are read)
    let mut first: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut proved = |row: (usize, usize, u32, u8)| -> UeRes<bool> {
        let (at, n, raw, _) = row;
        if n as u64 != raw as u64 {
            return Err(UeErr::Red(OODLE_RED.into()));
        }
        let s = &d[at..at + n];
        if lslg(s).is_none() && sage_record(s).is_none() {
            return Ok(false);
        }
        first.insert(at, s.to_vec());
        Ok(true)
    };
    let rows = shader_map_entries(d, &mut proved)?;
    for (at, n, raw, f) in rows {
        let s = match first.remove(&at) {
            Some(s) => s,
            None => {
                if n as u64 != raw as u64 {
                    return Err(UeErr::Red(OODLE_RED.into()));
                }
                d[at..at + n].to_vec()
            }
        };
        if starts(&s, b"LSLG") {
            if n as u64 != raw as u64 {
                take(&s, at, out)?;
            }
            continue;
        }
        let Some((ctl, code)) = sage_record(&s) else {
            return Err(UeErr::Red(format!(
                "{label}: the shader record at {at:#x} (frequency {f}) is not a record of the known layout"
            )));
        };
        let st = nvn2_stage(ctl, code);
        let why = match st {
            Some(st) => check(st, ctl, code)?,
            None => Some("no stage"),
        };
        if let Some(w) = why {
            return Err(UeErr::Red(format!(
                "{label}: shader at {at:#x} ({}): {w}",
                stname(st)
            )));
        }
        out.push(Shader::new(
            format!("{label}#UE{at:x}"),
            st.unwrap(),
            ctl,
            code,
        ));
    }
    Ok(())
}

// ---- shader code archives ---------------------------------------------------------------------------------------------------------

/// `archive(b)`: a version 2 shader code archive's shaders `(offset, size, raw size, frequency)` and where its code starts
pub struct Archive {
    pub shaders: Vec<(u64, u32, u32, u8)>,
    pub code: usize,
}

pub fn archive(b: &[u8], what: &str) -> UeRes<Archive> {
    let mut r = R::new(b, 0, what);
    if r.u32()? != ARCHIVE_VERSION {
        return val("not a version 2 shader code archive");
    }
    let nmap = r.u32()? as i128;
    r.raw(20 * nmap)?;
    let nsh = r.u32()? as i128;
    r.raw(20 * nsh)?;
    let nm = r.u32()? as i128;
    for _ in 0..nm {
        r.raw(16)?; // `take('<IIII')` per shader map
    }
    let n = r.u32()?;
    let mut shaders = Vec::new();
    for _ in 0..n {
        r.need(17)?; // `take('<QIIB')`
        let (o, s, w, f) = (r.u64()?, r.u32()?, r.u32()?, r.u8()?);
        shaders.push((o, s, w, f));
    }
    let np = r.u32()? as i128;
    r.raw(16 * np)?;
    let ni = r.u32()? as i128;
    r.raw(4 * ni)?;
    Ok(Archive {
        shaders,
        code: r.o as usize,
    })
}

/// `archive_v1_decoded(b, what)`, as the shaders it holds: (frequency, decoded bytes)
fn archive_v1(b: &[u8], what: &str) -> UeRes<Vec<(u8, Vec<u8>)>> {
    let mut r = R::new(b, 0, what);
    if r.u32()? != 1 {
        return val(format!("{what}: expected a version 1 shader archive"));
    }
    let count = r.u32()? as i128;
    r.need(37 * count)?;
    let code = r.o + 37 * count;
    let max_decoded = 1032 * (b.len() as i128 - code);
    let mut total = 0i128;
    let mut out = Vec::new();
    for i in 0..count {
        r.raw(20)?;
        let (off, size, raw, fr) = (
            r.u64()? as i128,
            r.u32()? as i128,
            r.u32()? as i128,
            r.u8()?,
        );
        if !(0 < size && size <= raw && raw <= 32 << 20)
            || code + off + size > b.len() as i128
            || freq(fr).is_none()
            || raw > 1032 * size
            || total + raw > max_decoded
        {
            return val(format!("{what}: invalid version 1 shader entry {i}"));
        }
        let a = (code + off) as usize;
        let mut shader = b[a..a + size as usize].to_vec();
        if size != raw {
            let z = inflate::run(&shader, Wrap::Zlib, Some(raw as usize + 1))
                .map_err(|e| UeErr::Value(format!("{what}: shader {i} does not decode ({e})")))?;
            if !z.eof || z.used != shader.len() || z.out.len() as i128 != raw {
                return val(format!(
                    "{what}: shader {i} does not decode to {} bytes",
                    hx(raw)
                ));
            }
            shader = z.out;
        }
        if !starts(&shader, b"LSLG") {
            return val(format!("{what}: shader {i} is missing its LSLG header"));
        }
        total += raw;
        out.push((fr, shader));
    }
    Ok(out)
}

/// The shaders of one archive file, decoded (what `uepak.job` + `finish` leave in `ue_shaders/<name>`): (frequency, bytes)
pub fn decoded_shaders(b: &[u8], what: &str) -> Result<Vec<(u8, Vec<u8>)>, Failure> {
    let ve = |e: UeErr| Failure::Red(format!("ValueError: {}", e.text()));
    if starts(b, &[1, 0, 0, 0]) {
        return archive_v1(b, what).map_err(ve);
    }
    let a = archive(b, what).map_err(ve)?;
    let slice = |off: u64, size: u32| -> &[u8] {
        let s = (a.code as u128 + off as u128).min(b.len() as u128) as usize;
        sl(b, s, s.saturating_add(size as usize))
    };
    let mut out = Vec::new();
    for (i, &(off, size, raw, fr)) in a.shaders.iter().enumerate() {
        let c = slice(off, size);
        let d = if size == raw {
            c.to_vec()
        } else {
            match (raw as u64 <= 255 * c.len() as u64)
                .then(|| lz4_block(c, raw as usize).ok())
                .flatten()
            {
                Some(d) if d.len() == raw as usize && starts(&d, b"LSLG") => d,
                _ => unsupported!(
                    "Unreal Engine shader code archive with Oodle-compressed shaders ({what}: shader {i}). {OODLE_WHY}"
                ),
            }
        };
        if d.len() != raw as usize {
            red!(
                "ValueError: decoded shader of {} bytes, the archive says {raw}",
                d.len()
            );
        }
        out.push((fr, d));
    }
    Ok(out)
}

/// `ue_shaders(d, label)` over decoded shaders
pub fn ue_shaders(shaders: &[(u8, Vec<u8>)], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for (i, (fr, s)) in shaders.iter().enumerate() {
        let fr = *fr;
        let (st, ctl, code): (Option<St>, &[u8], &[u8]);
        if let Some((tag, c, k)) = lslg(s) {
            st = ue_tag(&tag);
            if st.is_none() || freq(fr) != st {
                red!("{label}: shader {i} has tag {tag} but frequency {fr}");
            }
            (ctl, code) = (c, k);
        } else if let Some((c, k)) = sage_record(s) {
            st = nvn2_stage(c, k);
            let ok: &[St] = match fr {
                0 => &[St::Vs],
                1 => &[St::Ms],
                2 => &[St::Ts],
                3 => &[St::Ps],
                4 => &[St::Gs],
                5 => &[St::Cs],
                _ => &[],
            };
            if !st.is_some_and(|s| ok.contains(&s)) {
                red!(
                    "{label}: shader {i} is a {} shader but has frequency {fr}",
                    stname(st)
                );
            }
            (ctl, code) = (c, k);
        } else {
            let Some((c, k)) = raw_record(s) else {
                red!("{label}: shader {i} has no recognized Unreal shader record layout");
            };
            st = freq(fr);
            if st.is_none() {
                red!("{label}: shader {i} has frequency {fr}");
            }
            (ctl, code) = (c, k);
        }
        let st = st.unwrap();
        if let Some(w) = check(st, ctl, code)? {
            red!("{label}: shader {i} ({}): {w}", st.short());
        }
        out.push(Shader::new(format!("{label}#UE{i}"), st, ctl, code));
    }
    Ok(out)
}

/// `ue_shaders` over a decoded archive file (`ue_shaders/<name>` beside the RomFS, as the Mac leaves them): every shader must be stored
pub fn ue_shaders_file(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let a = archive(d, "shader code archive")
        .map_err(|e| Failure::Red(format!("ValueError: {}", e.text())))?;
    let mut v = Vec::new();
    for (i, &(off, size, raw, fr)) in a.shaders.iter().enumerate() {
        if size != raw {
            red!(
                "{label}: shader {i} is still compressed (an archive the NSP script has not decoded)"
            );
        }
        let s = (a.code as u128 + off as u128).min(d.len() as u128) as usize;
        v.push((fr, sl(d, s, s.saturating_add(size as usize)).to_vec()));
    }
    ue_shaders(&v, label)
}

/// Why Oodle data ends the split on the phone: upstream decodes it on the Mac with the game's own `OodleLZ_Decompress` (the host's tool mode, `DM_OODLE_SERVE`), which needs the game's
/// rewritten modules loaded; the phone's split runs before they are built, signed or loaded, and no Oodle decoder under a licence this repository can take exists
pub const OODLE_WHY: &str = "Oodle is decoded only by the game's own decoder (OodleLZ_Decompress in main), which upstream runs on the Mac; on the iPhone the split runs before the game's code is loaded, and there is no Oodle decoder under a licence iBuilder can use. Games with Oodle-compressed Unreal shaders: Octopath Traveler 0, FINAL FANTASY RESONANCE, Yoshi and the Mysterious Book, Mario & Luigi: Brothership";

/// `uepak.job` + `finish` in process: the shader libraries of the version's IoStore containers (Unreal Engine 5: FINAL FANTASY RESONANCE), then the shader code archives of every pak of
/// one version that `pak_shader_sources` routes to extraction, by file name (a later archive of the same name replaces an earlier one, as they are written into one folder), each as
/// its decoded shaders. Sorted by name, as main() lists `ue_shaders/`. `main`: this version's ExeFS main (the AES key of an encrypted pak); `top(f)`: the name of a file at the top
/// of the RomFS (the manifests that name the libraries), None for one below it.
pub fn archives(
    files: &[&FileRef],
    path_of: &dyn Fn(&FileRef) -> String,
    zstd: &dyn Zstd,
    main: &std::path::Path,
    top: &dyn Fn(&FileRef) -> Option<String>,
) -> Result<Vec<NamedArchive>, SplitError> {
    let mut by_name: BTreeMap<String, Vec<(u8, Vec<u8>)>> = BTreeMap::new();
    let ue = |e: UeErr| match e {
        UeErr::Unsupported(m) => SplitError::Unsupported(m),
        e => SplitError::Red(e.text().to_string()),
    };
    // ---- IoStore shader libraries (`io_tocs(romfs)`, in mount order; a library or group the newest container has wins)
    let by_rel: HashMap<&str, &FileRef> = files.iter().map(|f| (f.rel.as_str(), *f)).collect();
    let lookup = |n: &str| by_rel.get(n).map(|f| f.loc.clone());
    let mut found: Vec<(&FileRef, String)> = files
        .iter()
        .filter(|f| f.rel.ends_with(".utoc"))
        .map(|f| (*f, path_of(f)))
        .collect();
    found.sort_by_cached_key(|(_, p)| mount_order(p));
    let tocs: Vec<Toc> = found
        .iter()
        .filter_map(|(f, p)| open_toc(f, p, None, &lookup).ok())
        .collect();
    if tocs
        .iter()
        .any(|t| !t.of_type(IO_SHADER_LIBRARY).is_empty())
    {
        let manifests: Vec<Vec<u8>> = files
            .iter()
            .filter(|f| top(f).is_some_and(|n| n.starts_with("Manifest_") && n.ends_with(".txt")))
            .filter_map(|f| {
                f.loc
                    .open()
                    .ok()
                    .and_then(|s| s.read(0, s.len() as usize).ok())
            })
            .collect();
        let names = crate::uestore::archive_names(&manifests);
        let mut chunk: HashMap<[u8; 12], (usize, usize)> = HashMap::new();
        for (t, toc) in tocs.iter().enumerate().rev() {
            for (k, c) in toc.ids.iter().enumerate() {
                chunk.entry(*c).or_insert((t, k));
            }
        }
        let oodle = |what: &str| {
            UeErr::Unsupported(format!(
                "Unreal Engine IoStore shader library with Oodle-compressed data ({what}). {OODLE_WHY}"
            ))
        };
        let mut seen = HashSet::new();
        for (t, toc) in tocs.iter().enumerate().rev() {
            for i in toc.of_type(IO_SHADER_LIBRARY) {
                if !seen.insert(toc.ids[i]) {
                    continue;
                }
                let name = format!(
                    "{}.ushaderbytecode",
                    crate::uestore::io_library_name(&toc.ids[i], &names)
                );
                let sh = crate::uestore::io_archive(&tocs, t, i, &|c| chunk.get(c).copied(), &oodle)
                    .map_err(|e| match e {
                        UeErr::Value(m) if m.ends_with("the container is encrypted and no key was given") => {
                            SplitError::Unsupported(format!(
                                "Unreal Engine IoStore shader library in an encrypted container ({m}; upstream reads none either)"
                            ))
                        }
                        e => ue(e),
                    })?;
                by_name.insert(name, sh);
            }
        }
    }
    // ---- the paks' shader code archives
    let mut paks: Vec<&FileRef> = files
        .iter()
        .copied()
        .filter(|f| f.rel.ends_with(".pak"))
        .collect();
    paks.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut keys = Keys::new(vec![main.to_path_buf()]);
    for f in paks {
        let Ok(src) = f.loc.open() else { continue };
        let path = path_of(f);
        let k = keys.try_key(&path, &src);
        let beside = || inline_in_containers(f, files.iter().copied(), path_of);
        match pak_shader_sources(&path, &src, k, &beside) {
            Ok((_, true)) => {}
            _ => continue,
        }
        let k = keys.key(&path, &src).map_err(|e| match e {
            UeErr::Value(m) if m.ends_with("decrypts it") => SplitError::Unsupported(format!(
                "Unreal Engine pak with an encrypted index whose AES key is not in main ({m})"
            )),
            e => ue(e),
        })?;
        let pk = Pak::open(&path, &src, k).map_err(ue)?;
        for name in pk
            .names()
            .into_iter()
            .filter(|n| n.ends_with(".ushaderbytecode"))
        {
            let b = pk.read(&name, zstd, false).map_err(ue)?;
            let what = format!("{path}: {name}");
            let sh = decoded_shaders(&b, &what).map_err(|e| match e {
                Failure::Unsupported(m) => SplitError::Unsupported(m),
                e => SplitError::Red(e.to_string()),
            })?;
            let base = name.rsplit('/').next().unwrap_or(&name).to_string();
            by_name.insert(base, sh);
        }
    }
    Ok(by_name.into_iter().collect())
}
