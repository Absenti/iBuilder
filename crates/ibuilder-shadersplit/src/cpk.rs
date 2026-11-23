//! CRI CPK archives (`cpk.py`, `shader_split.cpk_jobs` / `cpk_shaders`: Bayonetta 2 and many Japanese games) and Diablo III's console packages (`d3cpk.py`,
//! `shader_split.d3cpk_jobs` / `d3cpk_shaders`: Diablo III: Eternal Collection's `.cpk` files, another format under the same extension).
//!
//! **CRI CPK.** `'CPK '`, a 0x10-byte chunk header, then an `@UTF` table (stored, or masked: Unicorn Overlord) naming the table of contents (`TocOffset`) and where the data starts
//! (`ContentOffset`); the TOC (`'TOC '`, 0x10 bytes, `@UTF`) has one row per file: `DirName`, `FileName`, `FileSize` (stored), `ExtractSize`, `FileOffset` (from the smaller of
//! `ContentOffset` and `TocOffset` counted as at most 0x800). A file whose stored size is not its size, or that starts with `CRILAYLA`, is CRILAYLA-compressed. The planner reads every
//! `.cpk` that starts with `'CPK '`; one it cannot read is named (`[RED] /<rel>: the CPK cannot be read (...); scanned as one file`) and stays a loose file. Every entry, in ranges of about
//! equal decoded size, is decoded and scanned like a loose file (`scan_blob`); an entry that does not decode is named and left out (`; not scanned`).
//!
//! The values of an `@UTF` table are the Python's values ([`Val`]: int, float, str, bytes, None): a damaged table can put any of them where a number is expected, and what the
//! Python does with it (an error it catches, one that ends the job, one that ends the split) is reproduced, with its message. The CRILAYLA decoder keeps the Python's slice and
//! index semantics on damaged bits as well (a copy that reaches past the end shortens the output, a negative index counts from the end), so a damaged entry fails with the same text.
//! Python's `open` is the builtin one here (a RomFS folder): a negative seek is `OSError` (a RomFS image's reader says `ValueError('invalid seek')`).
//!
//! **Diablo III.** `0xa1b2c3d4` (little-endian) and version 6 or 7, bit-packed tables and the data in 64 KiB sectors. The tables are not read: every zlib stream behind a 6-byte record
//! `{u32 decoded length, u16 stored length}` that inflates to exactly its decoded length and ends where its stored length says is a stream; in it, every control section of a
//! plausible length (0x760..0x880) directly followed by its code area is a single-stage program, the others are counted in one `[RED]` line per range. Jobs are byte ranges of
//! about equal length (at least 16 MiB); a range is read in windows of [`WINDOW`] bytes (the Python reads the whole range, 256 MB a job for the game's 16 GB).
//!
//! Limits (the Python has no cap): an entry, a table or a CRILAYLA output of more than [`MAX_DECODED`] bytes is refused with the cap's message (an entry: left out; a table: the CPK
//! is scanned as one file); an `@UTF` table of more than [`MAX_ROWS`] rows or [`MAX_CELLS`] cells ends the split (the Python would build them all).

use crate::bytes::find_all;
use crate::ctl::{CTL_MAGIC, check, glslc_stage};
use crate::error::{Failure, Res, SplitError};
use crate::inflate::{self, MAX_DECODED, Wrap};
use crate::scan::{FileRef, scan_blob};
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use std::cmp::Ordering;
use std::collections::HashSet;

pub const CPK_MAGIC: &[u8; 4] = b"CPK ";
pub const CRILAYLA: &[u8; 8] = b"CRILAYLA";
/// `d3cpk.MAGIC`: the u32 0xa1b2c3d4, little-endian
pub const D3_MAGIC: [u8; 4] = [0xd4, 0xc3, 0xb2, 0xa1];
/// `d3cpk.HEAD`: version 7's header
pub const D3_HEAD: u64 = 0x48;
/// `d3cpk.SPAN`: what a stream can take up past the start of its record
pub const D3_SPAN: u64 = 6 + 0xffff;
/// how much of a Diablo III range is read at a time
pub const WINDOW: u64 = 4 << 20;
pub const MAX_ROWS: u64 = 1 << 22;
pub const MAX_CELLS: u64 = 1 << 24;
const PARTS: i128 = 64;

// ---- Python values -------------------------------------------------------------------------------------------------------------------

/// A value of an `@UTF` table as the Python holds it
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    None,
    Int(i128),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
}

/// An exception of the Python reader: its type name (`struct.error` is `error`) and `str(e)`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyErr {
    pub kind: &'static str,
    pub msg: String,
}

impl PyErr {
    fn new(kind: &'static str, msg: impl Into<String>) -> PyErr {
        PyErr {
            kind,
            msg: msg.into(),
        }
    }
    /// `f'{type(e).__name__}: {e}'`
    pub fn text(&self) -> String {
        format!("{}: {}", self.kind, self.msg)
    }
    /// what `cpk_jobs` catches: a layout cpk.py does not read (the CPK is scanned as one file)
    pub fn caught_by_planner(&self) -> bool {
        matches!(self.kind, "ValueError" | "error" | "KeyError" | CAP)
    }
    /// what `cpk_shaders` catches: the entry is left out and the job goes on
    pub fn skips_entry(&self) -> bool {
        matches!(self.kind, "ValueError" | "IndexError" | CAP)
    }
}

/// this port's cap ([`MAX_DECODED`]): no Python counterpart
const CAP: &str = "Cap";

type Py<T> = Result<T, PyErr>;

fn value_error(m: impl Into<String>) -> PyErr {
    PyErr::new("ValueError", m)
}
fn type_error(m: impl Into<String>) -> PyErr {
    PyErr::new("TypeError", m)
}
fn cap_error() -> PyErr {
    PyErr::new(CAP, inflate::cap_error(MAX_DECODED))
}
/// `struct.unpack_from(fmt, d, off)` of `size` bytes past the end
fn struct_error(size: u64, off: u64, len: usize) -> PyErr {
    PyErr::new(
        "error",
        format!(
            "unpack_from requires a buffer of at least {} bytes for unpacking {size} bytes at offset {off} (actual buffer size is {len})",
            off + size
        ),
    )
}

impl Val {
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::None => "NoneType",
            Val::Int(_) => "int",
            Val::Float(_) => "float",
            Val::Str(_) => "str",
            Val::Bytes(_) => "bytes",
        }
    }
    /// `bool(v)`
    pub fn truthy(&self) -> bool {
        match self {
            Val::None => false,
            Val::Int(i) => *i != 0,
            Val::Float(f) => *f != 0.0,
            Val::Str(s) => !s.is_empty(),
            Val::Bytes(b) => !b.is_empty(),
        }
    }
    /// `str(v)` of a number (the only values a message formats)
    pub fn py_str(&self) -> String {
        match self {
            Val::Int(i) => i.to_string(),
            Val::Float(f) => py_float_repr(*f),
            Val::Str(s) => s.clone(),
            Val::None => "None".into(),
            Val::Bytes(b) => format!("b'{}'", b.escape_ascii()),
        }
    }
}

/// `repr(f)` of a Python float: the shortest digits that round-trip, fixed notation for exponents -4..15, else `d.ddde+XX`
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf" } else { "-inf" }.into();
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    let e = format!("{:e}", f.abs()); // shortest round-trip digits: "1.5e-7"
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sign = if f < 0.0 { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let body = if exp >= 0 {
            let int_len = exp as usize + 1;
            if digits.len() <= int_len {
                format!("{digits}{}.0", "0".repeat(int_len - digits.len()))
            } else {
                format!("{}.{}", &digits[..int_len], &digits[int_len..])
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        };
        format!("{sign}{body}")
    } else {
        let rest = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        let es = if exp < 0 { '-' } else { '+' };
        format!("{sign}{}{rest}e{es}{:02}", &digits[..1], exp.abs())
    }
}

/// an int and a float compared exactly (as Python compares them); None when the float is NaN
fn cmp_int_float(i: i128, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    const TWO127: f64 = 1.7014118346046923e38;
    if f >= TWO127 {
        return Some(Ordering::Less);
    }
    if f < -TWO127 {
        return Some(Ordering::Greater);
    }
    let fl = f.floor();
    let fi = fl as i128;
    Some(match i.cmp(&fi) {
        Ordering::Equal if f > fl => Ordering::Less,
        o => o,
    })
}

fn num_cmp(a: &Val, b: &Val) -> Option<Option<Ordering>> {
    Some(match (a, b) {
        (Val::Int(x), Val::Int(y)) => Some(x.cmp(y)),
        (Val::Float(x), Val::Float(y)) => x.partial_cmp(y),
        (Val::Int(x), Val::Float(y)) => cmp_int_float(*x, *y),
        (Val::Float(x), Val::Int(y)) => cmp_int_float(*y, *x).map(Ordering::reverse),
        _ => return None,
    })
}

/// `a == b`
fn py_eq(a: &Val, b: &Val) -> bool {
    if let Some(o) = num_cmp(a, b) {
        return o == Some(Ordering::Equal);
    }
    match (a, b) {
        (Val::None, Val::None) => true,
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::Bytes(x), Val::Bytes(y)) => x == y,
        _ => false,
    }
}

/// `a < b`
fn py_lt(a: &Val, b: &Val) -> Py<bool> {
    if let Some(o) = num_cmp(a, b) {
        return Ok(o == Some(Ordering::Less));
    }
    match (a, b) {
        (Val::Str(x), Val::Str(y)) => Ok(x.chars().lt(y.chars())),
        (Val::Bytes(x), Val::Bytes(y)) => Ok(x < y),
        _ => Err(type_error(format!(
            "'<' not supported between instances of '{}' and '{}'",
            a.type_name(),
            b.type_name()
        ))),
    }
}

/// `min(a, b)`: `b` when `b < a`, else `a`
fn py_min(a: Val, b: Val) -> Py<Val> {
    Ok(if py_lt(&b, &a)? { b } else { a })
}

/// `a + b`
fn py_add(a: &Val, b: &Val) -> Py<Val> {
    Ok(match (a, b) {
        (Val::Int(x), Val::Int(y)) => Val::Int(x + y),
        (Val::Int(x), Val::Float(y)) => Val::Float(*x as f64 + y),
        (Val::Float(x), Val::Int(y)) => Val::Float(x + *y as f64),
        (Val::Float(x), Val::Float(y)) => Val::Float(x + y),
        (Val::Str(x), Val::Str(y)) => Val::Str(format!("{x}{y}")),
        (Val::Bytes(x), Val::Bytes(y)) => Val::Bytes([x.as_slice(), y].concat()),
        (Val::Str(_), _) => {
            return Err(type_error(format!(
                "can only concatenate str (not \"{}\") to str",
                b.type_name()
            )));
        }
        (Val::Bytes(_), _) => {
            return Err(type_error(format!(
                "can't concat {} to bytes",
                b.type_name()
            )));
        }
        _ => {
            return Err(type_error(format!(
                "unsupported operand type(s) for +: '{}' and '{}'",
                a.type_name(),
                b.type_name()
            )));
        }
    })
}

// ---- @UTF tables ------------------------------------------------------------------------------------------------------------------------

/// `cpk.unmask(b)`: each byte XOR the low byte of a 16-bit value that starts at 0x655f and is multiplied by 0x4115 after every byte
pub fn unmask(b: &[u8]) -> Vec<u8> {
    let mut m: u16 = 0x655f;
    b.iter()
        .map(|x| {
            let o = x ^ (m as u8);
            m = m.wrapping_mul(0x4115);
            o
        })
        .collect()
}

/// `cpk.utf_at(d, o, n)`: the table of n bytes at d[o:], unmasked when it is stored masked
pub fn utf_at(d: &[u8], o: usize, n: usize) -> Vec<u8> {
    let t = crate::bytes::sl(d, o, o.saturating_add(n));
    if t.starts_with(b"@UTF") {
        t.to_vec()
    } else {
        unmask(t)
    }
}

/// A parsed table: the column names and one value per column for each row
pub struct Table {
    pub name: String,
    pub cols: Vec<String>,
    pub rows: Vec<Vec<Val>>,
}

impl Table {
    /// `row.get(name)`: the value of the last column of that name (a dict keeps the last assignment)
    pub fn get<'a>(&self, row: &'a [Val], name: &str) -> Option<&'a Val> {
        self.cols.iter().rposition(|c| c == name).map(|i| &row[i])
    }
}

enum Col {
    Const(Val),
    Row(u8),
    Zero,
}

/// `cpk.utf(d)`: a CRI `@UTF` table at d[0:]
pub fn utf(d: &[u8]) -> Py<Table> {
    if !d.starts_with(b"@UTF") {
        return Err(value_error("no @UTF table at 0x0"));
    }
    if d.len() < 32 {
        return Err(struct_error(24, 8, d.len()));
    }
    let be16 = |o: usize| u16::from_be_bytes(d[o..o + 2].try_into().unwrap()) as u64;
    let be32 = |o: usize| u32::from_be_bytes(d[o..o + 4].try_into().unwrap()) as u64;
    let (ro, so, dof, tn, nc, rw, nr) = (
        be16(10),
        be32(12),
        be32(16),
        be32(20),
        be16(24),
        be16(26),
        be32(28),
    );
    let b: u64 = 8;
    let len = d.len();
    let sz = |off: u64| -> Py<String> {
        let start = b + so + off;
        if start >= len as u64 {
            return Err(value_error("subsection not found"));
        }
        let s = start as usize;
        match memchr::memchr(0, &d[s..]) {
            Some(e) => Ok(String::from_utf8_lossy(&d[s..s + e]).into_owned()),
            None => Err(value_error("subsection not found")),
        }
    };
    let fixed = |p: u64, n: u64| -> Py<&[u8]> {
        if p + n > len as u64 {
            return Err(struct_error(n, p, len));
        }
        Ok(&d[p as usize..(p + n) as usize])
    };
    let val = |t: u8, p: u64| -> Py<(Val, u64)> {
        Ok(match t {
            0 => (Val::Int(fixed(p, 1)?[0] as i128), 1),
            1 => (Val::Int(fixed(p, 1)?[0] as i8 as i128), 1),
            2 => (
                Val::Int(u16::from_be_bytes(fixed(p, 2)?.try_into().unwrap()) as i128),
                2,
            ),
            3 => (
                Val::Int(i16::from_be_bytes(fixed(p, 2)?.try_into().unwrap()) as i128),
                2,
            ),
            4 => (
                Val::Int(u32::from_be_bytes(fixed(p, 4)?.try_into().unwrap()) as i128),
                4,
            ),
            5 => (
                Val::Int(i32::from_be_bytes(fixed(p, 4)?.try_into().unwrap()) as i128),
                4,
            ),
            6 => (
                Val::Int(u64::from_be_bytes(fixed(p, 8)?.try_into().unwrap()) as i128),
                8,
            ),
            7 => (
                Val::Int(i64::from_be_bytes(fixed(p, 8)?.try_into().unwrap()) as i128),
                8,
            ),
            8 => (
                Val::Float(f32::from_be_bytes(fixed(p, 4)?.try_into().unwrap()) as f64),
                4,
            ),
            0xa => {
                let o = u32::from_be_bytes(fixed(p, 4)?.try_into().unwrap()) as u64;
                (Val::Str(sz(o)?), 4)
            }
            0xb => {
                let w = fixed(p, 8)?;
                let a = u32::from_be_bytes(w[..4].try_into().unwrap()) as u64;
                let n = u32::from_be_bytes(w[4..].try_into().unwrap()) as u64;
                let s = (b + dof + a).min(len as u64) as usize;
                let e = (b + dof + a + n).min(len as u64) as usize;
                (Val::Bytes(d[s..e.max(s)].to_vec()), 8)
            }
            _ => return Err(value_error(format!("@UTF column type {t:#x}"))),
        })
    };
    let mut cols: Vec<(String, Col)> = Vec::new();
    let mut p: u64 = 0x20;
    for _ in 0..nc {
        let Some(&fl) = d.get(p as usize) else {
            return Err(PyErr::new("IndexError", "index out of range"));
        };
        let nmo = u32::from_be_bytes(fixed(p + 1, 4)?.try_into().unwrap()) as u64;
        let name = sz(nmo)?;
        p += 5;
        let (st, t) = (fl & 0xf0, fl & 0x0f);
        if st == 0x30 {
            let (v, n) = val(t, p)?;
            p += n;
            cols.push((name, Col::Const(v)));
        } else if st == 0x50 {
            cols.push((name, Col::Row(t)));
        } else {
            cols.push((name, Col::Zero));
        }
    }
    let too_many = || {
        PyErr::new(
            "MemoryError",
            format!(
                "an @UTF table of {nr} rows of {} columns (more than this port builds)",
                cols.len()
            ),
        )
    };
    // rows that read nothing of their own (no per-row column, or a row width of 0) are all the first one: a damaged count of 4 billion is not read 4 billion times
    let same = rw == 0 || !cols.iter().any(|(_, c)| matches!(c, Col::Row(_)));
    let mut rows: Vec<Vec<Val>> = Vec::with_capacity(nr.min(1 << 16) as usize);
    for i in 0..nr {
        if same && i > 0 {
            if nr > MAX_ROWS || nr.saturating_mul(cols.len().max(1) as u64) > MAX_CELLS {
                return Err(too_many());
            }
            let r = rows[0].clone();
            rows.resize(nr as usize, r);
            break;
        }
        if i >= MAX_ROWS || (i + 1).saturating_mul(cols.len().max(1) as u64) > MAX_CELLS {
            return Err(too_many());
        }
        let mut p = b + ro + i * rw;
        let mut r = Vec::with_capacity(cols.len());
        for (_, c) in &cols {
            r.push(match c {
                Col::Row(t) => {
                    let (v, n) = val(*t, p)?;
                    p += n;
                    v
                }
                Col::Const(v) => v.clone(),
                Col::Zero => Val::None,
            });
        }
        rows.push(r);
    }
    Ok(Table {
        name: sz(tn)?,
        cols: cols.into_iter().map(|(n, _)| n).collect(),
        rows,
    })
}

// ---- the archive's files -------------------------------------------------------------------------------------------------------------

/// One file of a CPK: `(path in the archive, offset, stored length, length)`, each as the Python holds it
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub off: Val,
    pub csz: Val,
    pub usz: Val,
}

fn read_src(src: &dyn Source, off: u64, n: u64) -> Py<Vec<u8>> {
    let avail = src.len().saturating_sub(off).min(n);
    if avail > MAX_DECODED as u64 {
        return Err(cap_error());
    }
    src.read(off, avail as usize)
        .map_err(|e| PyErr::new("OSError", e.to_string()))
}

/// `f.seek(v)` of the builtin file: the offset, or the exception
fn seek(v: &Val) -> Py<u64> {
    match v {
        Val::Int(i) => {
            if *i < -(1i128 << 63) || *i >= 1i128 << 63 {
                Err(value_error("cannot fit 'int' into an offset-sized integer"))
            } else if *i < 0 {
                Err(PyErr::new("OSError", "[Errno 22] Invalid argument"))
            } else {
                Ok(*i as u64)
            }
        }
        other => Err(type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            other.type_name()
        ))),
    }
}

/// `cpk.files(path)`: the files of a CPK by its TOC. `path` names the archive in the messages
pub fn files(path: &str, src: &dyn Source) -> Py<Vec<Entry>> {
    let mut h = read_src(src, 0, 0x800)?;
    if !h.starts_with(CPK_MAGIC) {
        return Err(value_error(format!("{path}: not a CPK")));
    }
    if h.len() < 12 {
        return Err(struct_error(4, 8, h.len()));
    }
    let n = u32::from_le_bytes(h[8..12].try_into().unwrap()) as u64;
    if 0x10 + n > h.len() as u64 {
        h = read_src(src, 0, 0x10 + n)?;
    }
    let head = utf(&utf_at(&h, 0x10, n as usize))?;
    if head.rows.len() != 1 {
        return Err(value_error(if head.rows.is_empty() {
            "not enough values to unpack (expected 1, got 0)".to_string()
        } else {
            format!(
                "too many values to unpack (expected 1, got {})",
                head.rows.len()
            )
        }));
    }
    let hd = &head.rows[0];
    let toc = head.get(hd, "TocOffset").cloned().unwrap_or(Val::None);
    let ctt = head.get(hd, "ContentOffset").cloned().unwrap_or(Val::None);
    if !toc.truthy() {
        return Err(value_error(format!(
            "{path}: no TOC (ITOC only is not read)"
        )));
    }
    let at = seek(&toc)?;
    let Val::Int(toc_i) = toc else {
        unreachable!("seek takes an int only")
    };
    let t = read_src(src, at, 0x10)?;
    if !t.starts_with(b"TOC ") {
        return Err(value_error(format!("{path}: no TOC at {toc_i:#x}")));
    }
    if t.len() < 12 {
        return Err(struct_error(4, 8, t.len()));
    }
    let n = u32::from_le_bytes(t[8..12].try_into().unwrap()) as u64;
    let at = seek(&Val::Int(toc_i + 0x10))?;
    let t = utf_at(&read_src(src, at, n)?, 0, n as usize);
    let toc_base = Val::Int(toc_i.min(0x800));
    let base = if ctt.truthy() {
        py_min(ctt, toc_base)?
    } else {
        toc_base
    };
    let toc = utf(&t)?;
    let mut out = Vec::with_capacity(toc.rows.len());
    let key = |r: &[Val], k: &str| -> Py<Val> {
        toc.get(r, k)
            .cloned()
            .ok_or_else(|| PyErr::new("KeyError", format!("'{k}'")))
    };
    for r in &toc.rows {
        let dir = toc.get(r, "DirName").cloned().unwrap_or(Val::None);
        let prefix = if dir.truthy() {
            py_add(&dir, &Val::Str("/".into()))?
        } else {
            Val::Str(String::new())
        };
        let Val::Str(name) = py_add(&prefix, &key(r, "FileName")?)? else {
            unreachable!("str + str is a str")
        };
        let off = py_add(&base, &key(r, "FileOffset")?)?;
        let csz = key(r, "FileSize")?;
        let usz = match toc.get(r, "ExtractSize") {
            Some(v) if *v != Val::None => v.clone(),
            _ => csz.clone(),
        };
        out.push(Entry {
            name,
            off,
            csz,
            usz,
        });
    }
    Ok(out)
}

// ---- CRILAYLA ------------------------------------------------------------------------------------------------------------------------

/// The decoder's output with the Python's `bytearray` semantics. While every write goes to the next byte below the ones written (a healthy stream) it is kept as `rev`, the bytes
/// written from the end backwards, and nothing below them is allocated: a damaged size of 4 GiB costs what the stream decodes. Anything else (a copy that reaches past the end, an
/// index below 0x100) turns it into the whole `bytearray` (`full`).
struct Out {
    len: usize,
    prefix: Vec<u8>,
    rev: Vec<u8>,
    full: Option<Vec<u8>>,
}

fn index_error() -> PyErr {
    PyErr::new("IndexError", "bytearray index out of range")
}

impl Out {
    fn materialize(&mut self) -> Py<&mut Vec<u8>> {
        if self.full.is_none() {
            if self.len > MAX_DECODED + 0x100 {
                return Err(cap_error());
            }
            let mut v = self.prefix.clone();
            v.resize(self.len - self.rev.len(), 0);
            v.extend(self.rev.iter().rev());
            self.rev = Vec::new();
            self.full = Some(v);
        }
        Ok(self.full.as_mut().unwrap())
    }
    fn push(&mut self, v: u8) -> Py<()> {
        if self.rev.len() >= MAX_DECODED {
            return Err(cap_error());
        }
        self.rev.push(v);
        Ok(())
    }
}

/// Python's index `o[i]` of a list of n: the position, or IndexError
fn py_index(i: i128, n: usize) -> Py<usize> {
    let j = if i < 0 { i + n as i128 } else { i };
    if j < 0 || j >= n as i128 {
        return Err(index_error());
    }
    Ok(j as usize)
}

/// Python's slice bound `o[i:]` of a list of n
fn py_bound(i: i128, n: usize) -> usize {
    let n = n as i128;
    (if i < 0 { (i + n).max(0) } else { i.min(n) }) as usize
}

/// `cpk.crilayla(c)`: `'CRILAYLA'`, u32 size, u32 compressed length, the compressed bits, then 0x100 bytes stored as they are (the output's first 0x100). The bits are read from
/// the end of the compressed bytes backwards, highest bit first, and the output is written from its end backwards: a 1 bit, 13 offset bits (+3) and a length in growing fields
/// (2, 3, 5 bits, then 8 bits while all ones; +3), or a 0 bit and a literal byte
pub fn crilayla(c: &[u8]) -> Py<Vec<u8>> {
    if !c.starts_with(CRILAYLA) {
        return Err(value_error("not CRILAYLA"));
    }
    if c.len() < 16 {
        return Err(struct_error(8, 8, c.len()));
    }
    let usz = u32::from_le_bytes(c[8..12].try_into().unwrap()) as usize;
    let csz = u32::from_le_bytes(c[12..16].try_into().unwrap()) as usize;
    let ps = 0x10 + csz;
    let prefix = crate::bytes::sl(c, ps, ps + 0x100).to_vec();
    // `out[:0x100] = <the stored bytes>`: a shorter slice shortens the bytearray
    let len = usz + prefix.len();
    // the bit string: c[0x10 + csz - 1:0xf:-1], highest bit of each byte first
    let hi = (0x10 + csz - 1).min(c.len() - 1);
    let nb = if hi >= 0x10 { hi - 0x10 + 1 } else { 0 };
    let nbits = 8 * nb as u64;
    let bit = |q: u64| (c[hi - (q / 8) as usize] >> (7 - (q % 8))) & 1;
    // int(s[a:b], 2)
    let int_of = |a: u64, b: u64| -> Py<u64> {
        let e = b.min(nbits);
        if a >= e {
            return Err(value_error("invalid literal for int() with base 2: ''"));
        }
        let mut v = 0u64;
        for q in a..e {
            v = v << 1 | bit(q) as u64;
        }
        Ok(v)
    };
    let mut o = Out {
        len,
        prefix,
        rev: Vec::new(),
        full: None,
    };
    let (mut p, mut w): (u64, i128) = (0, len as i128 - 1);
    while w >= 0x100 {
        if p >= nbits {
            return Err(PyErr::new("IndexError", "string index out of range"));
        }
        if bit(p) == 1 {
            let back = int_of(p + 1, p + 14)? as i128 + 3;
            let mut v = int_of(p + 14, p + 16)?;
            p += 16;
            let mut ln = 3 + v as i128;
            if v == 3 {
                v = int_of(p, p + 3)?;
                p += 3;
                ln += v as i128;
                if v == 7 {
                    v = int_of(p, p + 5)?;
                    p += 5;
                    ln += v as i128;
                    if v == 31 {
                        loop {
                            v = int_of(p, p + 8)?;
                            p += 8;
                            ln += v as i128;
                            if v != 255 {
                                break;
                            }
                        }
                    }
                }
            }
            if back >= ln {
                // out[w - ln + 1:w + 1] = out[w + back - ln + 1:w + back + 1]
                if o.full.is_none() && back as usize <= o.rev.len() && w - ln + 1 >= 0x100 {
                    for _ in 0..ln {
                        let x = o.rev[o.rev.len() - back as usize];
                        o.push(x)?;
                    }
                } else {
                    let f = o.materialize()?;
                    let n = f.len();
                    let (a, z) = (py_bound(w - ln + 1, n), py_bound(w + 1, n));
                    let z = z.max(a);
                    let (sa, sz) = (py_bound(w + back - ln + 1, n), py_bound(w + back + 1, n));
                    let src: Vec<u8> = if sz > sa {
                        f[sa..sz].to_vec()
                    } else {
                        Vec::new()
                    };
                    f.splice(a..z, src);
                    if f.len() > MAX_DECODED + 0x100 {
                        return Err(cap_error());
                    }
                }
                w -= ln;
            } else {
                // for _ in range(ln): out[w] = out[w + back]; w -= 1
                let mut k: i128 = 0;
                while k < ln {
                    if o.full.is_none() && w >= 0x100 {
                        if back as usize > o.rev.len() {
                            return Err(index_error()); // out[w + back] past the end
                        }
                        let x = o.rev[o.rev.len() - back as usize];
                        o.push(x)?;
                    } else {
                        let f = o.materialize()?;
                        let x = f[py_index(w + back, f.len())?];
                        let i = py_index(w, f.len())?;
                        f[i] = x;
                    }
                    w -= 1;
                    k += 1;
                }
            }
        } else {
            let v = int_of(p + 1, p + 9)? as u8;
            p += 9;
            if let Some(f) = o.full.as_mut() {
                let i = py_index(w, f.len())?;
                f[i] = v;
            } else {
                o.push(v)?;
            }
            w -= 1;
        }
    }
    if let Some(f) = o.full {
        return Ok(f);
    }
    let mut v = o.prefix;
    v.resize(o.len - o.rev.len(), 0);
    v.extend(o.rev.iter().rev());
    Ok(v)
}

/// `cpk.read(path, entry)`: one entry's bytes, decoded when CRILAYLA
pub fn read(path: &str, src: &dyn Source, e: &Entry) -> Py<Vec<u8>> {
    let off = seek(&e.off)?;
    let n: u64 = match &e.csz {
        Val::None => u64::MAX,
        Val::Int(i) if *i >= 1i128 << 63 || *i < -(1i128 << 63) => {
            return Err(PyErr::new(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer",
            ));
        }
        Val::Int(-1) => u64::MAX,
        Val::Int(i) if *i < 0 => {
            return Err(value_error("read length must be non-negative or -1"));
        }
        Val::Int(i) => *i as u64,
        other => {
            return Err(type_error(format!(
                "argument should be integer or None, not '{}'",
                other.type_name()
            )));
        }
    };
    let mut c = read_src(src, off, n)?;
    if !py_eq(&e.csz, &e.usz) || c.starts_with(CRILAYLA) {
        c = crilayla(&c)?;
    }
    if !py_eq(&Val::Int(c.len() as i128), &e.usz) {
        return Err(value_error(format!(
            "{path}#{}: {} bytes, the TOC says {}",
            e.name,
            c.len(),
            e.usz.py_str()
        )));
    }
    Ok(c)
}

// ---- CPK jobs --------------------------------------------------------------------------------------------------------------------------

/// One job: a range of one CPK's entries (`(path, rel, entries)`)
#[derive(Clone)]
pub struct CpkJob {
    pub file: FileRef,
    /// the path the Python's messages name: the RomFS argument, then the file's path below it
    pub path: String,
    pub entries: Vec<Entry>,
}

impl CpkJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "cpk:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(self.entries.iter().map(|e| format!("{e:?}")))
        .collect()
    }
}

/// `cpk_jobs`' chunking: every archive's entries in ranges of about equal length (`step = sum(lengths) / 64`, a range closes once its length reaches it), each range within one
/// archive. → (the archive's index, its entries) per job; `Err` is the exception that ends the split (a length that is no number)
pub fn chunk_entries(sets: &[Vec<Entry>]) -> Py<Vec<(usize, Vec<Entry>)>> {
    let mut total = Val::Int(0);
    for e in sets.iter().flatten() {
        total = py_add(&total, &e.usz)?;
    }
    let step = match total {
        Val::Int(t) => t as f64 / PARTS as f64,
        Val::Float(t) => t / PARTS as f64,
        _ => unreachable!("a sum of numbers is a number"),
    };
    let step = Val::Float(step);
    let mut jobs = Vec::new();
    for (i, es) in sets.iter().enumerate() {
        let (mut cur, mut size) = (Vec::new(), Val::Int(0));
        for e in es {
            cur.push(e.clone());
            size = py_add(&size, &e.usz)?;
            if !py_lt(&size, &step)? && num_cmp(&size, &step) != Some(None) {
                jobs.push((i, std::mem::take(&mut cur)));
                size = Val::Int(0);
            }
        }
        if !cur.is_empty() {
            jobs.push((i, cur));
        }
    }
    Ok(jobs)
}

/// The CPK plan: (the jobs, the files they read as (root, rel))
pub type CpkPlan = (Vec<CpkJob>, HashSet<(usize, String)>);

/// `cpk_jobs(files)`: every `.cpk` that starts with `'CPK '`, read by its TOC. One whose layout cpk.py does not read is logged and scanned as one file; an exception cpk_jobs does not
/// catch ends the split, as it does the Python's
pub fn cpk_jobs(
    files: &[FileRef],
    path_of: &dyn Fn(&FileRef) -> String,
    log: &(dyn Fn(&str) + Sync),
) -> Result<CpkPlan, SplitError> {
    let mut with_head: Vec<(&FileRef, String, Vec<Entry>)> = Vec::new();
    for f in files {
        if !f.rel.to_lowercase().ends_with(".cpk") {
            continue;
        }
        let src = f.loc.open().map_err(|e| SplitError::Io(e.to_string()))?;
        if src.read(0, 4).map_err(|e| SplitError::Io(e.to_string()))? != CPK_MAGIC {
            continue;
        }
        let path = path_of(f);
        match self::files(&path, &src) {
            Ok(es) => with_head.push((f, path, es)),
            Err(e) if e.caught_by_planner() => log(&format!(
                "[RED] /{}: the CPK cannot be read ({}); scanned as one file",
                f.rel, e.msg
            )),
            Err(e) => {
                return Err(SplitError::Red(format!(
                    "/{}: the CPK reader failed ({})",
                    f.rel,
                    e.text()
                )));
            }
        }
    }
    let sets: Vec<Vec<Entry>> = with_head.iter().map(|x| x.2.clone()).collect();
    let chunks = chunk_entries(&sets)
        .map_err(|e| SplitError::Red(format!("the CPK jobs cannot be planned ({})", e.text())))?;
    let jobs = chunks
        .into_iter()
        .map(|(i, entries)| CpkJob {
            file: with_head[i].0.clone(),
            path: with_head[i].1.clone(),
            entries,
        })
        .collect();
    let taken = with_head
        .iter()
        .map(|(f, _, _)| (f.root, f.rel.clone()))
        .collect();
    Ok((jobs, taken))
}

/// `cpk_shaders(path, rel, entries, found)`: each entry decoded (CRILAYLA or stored) and scanned like a loose file; one that does not decode is named and left out
pub fn cpk_shaders(cx: &Ctx<'_>, job: &CpkJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    for e in &job.entries {
        let d = match read(&job.path, &src, e) {
            Ok(d) => d,
            Err(x) if x.skips_entry() => {
                cx.print(&format!(
                    "[RED] /{}/{}: {}; not scanned",
                    job.file.rel, e.name, x.msg
                ));
                continue;
            }
            Err(x) => return Err(Failure::Red(x.text())),
        };
        scan_blob(
            cx,
            &d,
            &format!("/{}/{}", job.file.rel, e.name),
            &mut out,
            0,
            None,
        )?;
    }
    Ok(out)
}

// ---- Diablo III packages -------------------------------------------------------------------------------------------------------------

/// `d3cpk.is_d3cpk(head)`: the magic and version 6 or 7
pub fn is_d3cpk(h: &[u8]) -> bool {
    h.len() >= 8
        && h[..4] == D3_MAGIC
        && matches!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 6 | 7)
}

/// One job: a byte range of one package (`(path, rel, (D3CPK, start, end))`)
#[derive(Clone)]
pub struct D3Job {
    pub file: FileRef,
    pub start: u64,
    pub end: u64,
}

impl D3Job {
    pub fn identity(&self) -> Vec<String> {
        vec![format!(
            "d3cpk:{}:{}:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.start,
            self.end
        )]
    }
}

/// `d3cpk_jobs`' ranges of packages of these sizes: `step = max(total // 64, 16 MiB)` → (package, start, end)
pub fn d3_ranges(sizes: &[u64]) -> Vec<(usize, u64, u64)> {
    let total: u64 = sizes.iter().sum();
    let step = (total / PARTS as u64).max(1 << 24);
    let mut out = Vec::new();
    for (i, &n) in sizes.iter().enumerate() {
        let mut a = 0;
        while a < n {
            out.push((i, a, (a + step).min(n)));
            a += step;
        }
    }
    out
}

/// The Diablo III plan: (the jobs, the files they read as (root, rel))
pub type D3Plan = (Vec<D3Job>, HashSet<(usize, String)>);

/// `d3cpk_jobs(files)`: every `.cpk` that starts like a Diablo III package, in byte ranges
pub fn d3cpk_jobs(files: &[FileRef]) -> Result<D3Plan, SplitError> {
    let mut ds: Vec<&FileRef> = Vec::new();
    for f in files {
        if !f.rel.to_lowercase().ends_with(".cpk") {
            continue;
        }
        let src = f.loc.open().map_err(|e| SplitError::Io(e.to_string()))?;
        if is_d3cpk(&src.read(0, 8).map_err(|e| SplitError::Io(e.to_string()))?) {
            ds.push(f);
        }
    }
    let sizes: Vec<u64> = ds.iter().map(|f| f.loc.len()).collect();
    let jobs = d3_ranges(&sizes)
        .into_iter()
        .map(|(i, start, end)| D3Job {
            file: ds[i].clone(),
            start,
            end,
        })
        .collect();
    Ok((jobs, ds.iter().map(|f| (f.root, f.rel.clone())).collect()))
}

/// `zlib.decompressobj().decompress(data, n + 1)` that ends exactly at the data's end with n bytes: the stream, or None
fn d3_stream(data: &[u8], n: u64) -> Option<Vec<u8>> {
    let lim = n as usize + 1;
    match inflate::run(data, Wrap::Zlib, Some(lim)) {
        Ok(z) => (z.eof && z.used == data.len() && z.out.len() as u64 == n).then_some(z.out),
        // the fast decoder refused it: zlib's own model decides (a stream it takes and miniz_oxide does not)
        Err(_) => match crate::zlibx::decompress(data, Some(lim)) {
            Ok(z) if z.eof && z.used == data.len() && z.out.len() as u64 == n => Some(z.out),
            _ => None,
        },
    }
}

/// `d3cpk.streams` over the range plus `d3cpk_shaders`' search of each stream: the zlib streams whose record starts in [start, end) (the range is read with [`D3_SPAN`] bytes more,
/// so its last stream is whole), each searched for control sections directly followed by their code area. → (the shaders, the `[RED]` line of the control sections left out)
pub fn d3cpk_range(
    src: &dyn Source,
    rel: &str,
    start: u64,
    end: u64,
) -> Res<(Vec<Shader>, Option<String>)> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let flen = src.len();
    // d = f.read(end - start + SPAN) from start: its length, and the range's end in it
    let span = end.saturating_sub(start);
    let len_d = (start.saturating_add(span).saturating_add(D3_SPAN)).min(flen);
    let len_d = len_d.saturating_sub(start);
    // ZLIB.finditer(d, max(start + 6, HEAD - base + 6), min(end + 6, len(d))) with start 0, base = the range's start: a match m with pos <= m and m + 2 <= endpos
    let pos = 6u64.max((D3_HEAD + 6).saturating_sub(start));
    let endpos = (span + 6).min(len_d);
    let mut found = Vec::new();
    let mut lost: Vec<String> = Vec::new();
    let mut last: u64 = 0; // records before it are inside the stream last taken (the Python's -1: none yet)
    let mut ws = pos;
    while ws + 2 <= endpos {
        let we = (ws + WINDOW).min(endpos - 1); // matches m in [ws, we)
        // the bytes this window needs: from its first record's header (m - 6) to its last stream's end
        let b0 = ws - 6;
        let b1 = (we + D3_SPAN).min(len_d);
        let buf = src.read(start + b0, (b1 - b0) as usize).map_err(io)?;
        let at = |rel_pos: u64| (rel_pos - b0) as usize;
        for m in crate::glslc::zlib_heads(crate::bytes::sl(&buf, at(ws), at(we) + 1)) {
            let m = ws + m as u64;
            if m >= we {
                continue; // its second byte is the next window's
            }
            let p = m - 6;
            if p < last {
                continue;
            }
            let h = &buf[at(p)..at(p) + 6];
            let n = u32::from_le_bytes(h[..4].try_into().unwrap()) as u64;
            let stored = u16::from_le_bytes(h[4..6].try_into().unwrap()) as u64;
            if n == 0 || stored < 3 || p + 6 + stored > len_d {
                continue;
            }
            let Some(x) = d3_stream(&buf[at(p) + 6..at(p) + 6 + stored as usize], n) else {
                continue;
            };
            last = p + 6 + stored;
            let label = format!("/{rel}@{:#x}", start + p);
            for c in find_all(&x, &CTL_MAGIC) {
                if c + 0x708 > x.len() {
                    continue;
                }
                let nctl = u32::from_le_bytes(x[c + 0x20..c + 0x24].try_into().unwrap()) as usize;
                let k = u32::from_le_bytes(x[c + 0x704..c + 0x708].try_into().unwrap()) as usize;
                if !(0x760..=0x880).contains(&nctl) {
                    continue; // the magic in other data
                }
                let ctl = crate::bytes::sl(&x, c, c + nctl);
                let code = crate::bytes::sl(&x, c + nctl, c + nctl + k);
                let st = if code.len() == k {
                    glslc_stage(ctl, code)?
                } else {
                    None
                };
                match st {
                    Some(st) if check(st, ctl, code)?.is_none() => {
                        found.push(Shader::new(format!("{label}#D3@{c:#x}"), st, ctl, code))
                    }
                    _ => lost.push(format!("{label}+{c:#x}")),
                }
            }
        }
        ws = we;
    }
    let line = (!lost.is_empty()).then(|| {
        format!(
            "[RED] /{rel} {start:#x}..{end:#x}: {} control section(s) not followed by their code area in the same stream, left out (first: {})",
            lost.len(),
            lost[0]
        )
    });
    Ok((found, line))
}

/// `d3cpk_shaders(path, rel, start, end, found)`
pub fn d3cpk_shaders(cx: &Ctx<'_>, job: &D3Job) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let (found, line) = d3cpk_range(&src, &job.file.rel, job.start, job.end)?;
    if let Some(l) = line {
        cx.print(&l);
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_float_repr() {
        for (f, s) in [
            (100.0, "100.0"),
            (1e16, "1e+16"),
            (1.5e-7, "1.5e-07"),
            (0.0001, "0.0001"),
            (123456789012345.6, "123456789012345.6"),
            (-2.5, "-2.5"),
            (3.4028234663852886e38, "3.4028234663852886e+38"),
            (f64::NAN, "nan"),
            (-0.0, "-0.0"),
        ] {
            assert_eq!(py_float_repr(f), s);
        }
    }

    #[test]
    fn numbers_compare_and_add_as_in_python() {
        assert!(py_eq(&Val::Int(100), &Val::Float(100.0)));
        assert!(!py_eq(&Val::Int(100), &Val::Str("100".into())));
        assert!(py_lt(&Val::Int(5), &Val::Float(5.5)).unwrap());
        assert!(!py_lt(&Val::Int(6), &Val::Float(5.5)).unwrap());
        assert_eq!(
            py_lt(&Val::Int(5), &Val::Str("a".into())).unwrap_err().msg,
            "'<' not supported between instances of 'int' and 'str'"
        );
        assert_eq!(
            py_add(&Val::Str(String::new()), &Val::Int(1))
                .unwrap_err()
                .msg,
            "can only concatenate str (not \"int\") to str"
        );
        assert_eq!(
            py_add(&Val::Bytes(vec![1]), &Val::Str("/".into()))
                .unwrap_err()
                .msg,
            "can't concat str to bytes"
        );
    }

    #[test]
    fn unmask_is_its_own_inverse_and_a_stored_table_is_kept() {
        let t = b"@UTF\0\0\0\x10hello world!".to_vec();
        assert_eq!(unmask(&unmask(&t)), t);
        assert_eq!(utf_at(&t, 0, t.len()), t);
        assert_eq!(utf_at(&unmask(&t), 0, t.len()), t);
    }

    #[test]
    fn crilayla_literals_and_an_overlapping_copy() {
        // 0x100 stored bytes, then 6 coded from the end backwards: the literals 'c', 'b', 'a', then a copy of 3 from 3 above
        let mut bits = String::new();
        for b in *b"cba" {
            bits += &format!("0{b:08b}");
        }
        bits += &format!("1{:013b}{:02b}", 0, 0); // back 3, length 3
        while !bits.len().is_multiple_of(8) {
            bits.push('0');
        }
        let packed: Vec<u8> = (0..bits.len() / 8)
            .map(|i| u8::from_str_radix(&bits[8 * i..8 * i + 8], 2).unwrap())
            .collect();
        let mut c = CRILAYLA.to_vec();
        c.extend_from_slice(&6u32.to_le_bytes());
        c.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        c.extend(packed.iter().rev());
        c.extend(std::iter::repeat_n(7u8, 0x100));
        let d = crilayla(&c).unwrap();
        assert_eq!(d.len(), 0x106);
        assert_eq!(&d[0x100..], b"abcabc");
        // the bits run out
        let mut cut = c.clone();
        cut[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(crilayla(&cut).unwrap_err().msg, "string index out of range");
        assert_eq!(crilayla(b"CRILAYLA\0").unwrap_err().kind, "error");
        assert_eq!(crilayla(b"CRILAYL").unwrap_err().msg, "not CRILAYLA");
    }

    #[test]
    fn a_huge_size_costs_only_what_the_stream_decodes() {
        let mut c = CRILAYLA.to_vec();
        c.extend_from_slice(&0xffff_fff0u32.to_le_bytes());
        c.extend_from_slice(&2u32.to_le_bytes());
        c.extend_from_slice(&[0x00, 0x00]); // 16 bits of literals' worth: runs out
        c.extend(std::iter::repeat_n(0u8, 0x100));
        let e = crilayla(&c).unwrap_err();
        assert!(e.skips_entry(), "{e:?}");
    }

    #[test]
    fn d3_ranges_are_at_least_16_mib() {
        assert_eq!(d3_ranges(&[100, 50]), vec![(0, 0, 100), (1, 0, 50)]);
        let big = 64 << 24;
        let r = d3_ranges(&[big, 1 << 20]);
        assert_eq!(r.len(), 64 + 1);
        assert_eq!(
            r[1],
            (0, (big + (1 << 20)) / 64, 2 * ((big + (1 << 20)) / 64))
        );
    }
}
