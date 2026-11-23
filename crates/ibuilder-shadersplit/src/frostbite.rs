//! Frostbite's game data on the Switch (EA SPORTS FC 27, Frostbite 2025.1.2): `frostbite.py` and `frostbite_jobs` / `frostbite_shaders` of `shader_split.py` (nxnative 682fdd26).
//!
//! The RomFS holds `Data/` (the base data) and `Patch/` (the update's), each with `NX/<superbundle>.toc` files and `NX/superbundlelayout/<install chunk>/cas_NN.cas` files, and
//! `Data/layout.toc`, whose `installManifest` names the install chunks. Every `.toc` starts with the magic `00 D1 CE 01`; its body is at +0x22c.
//! * `layout.toc`'s body is a DbObject ([`dbobject`]): `installManifest.installChunks` lists {`name` (the folder under `NX/`), `persistentIndex`}, which the cas ids name.
//! * A superbundle `.toc`'s body (big-endian, offsets from the body): +4 bundle table, +8 bundle count. A bundle is {u32 name, u32 0x40000000 | data size, u64 data offset}; its data is
//!   {u32 0, 0, flag offset, entry count, entry offset, ...}, the entries, one flag byte per entry: an entry is {u32 offset, u32 stored size} in a cas file, after an 8-byte cas id when its
//!   flag has bit 7 set (the first one always). A cas id is {u8 0, u8 1 = Patch / 0 = Data, u32 the install chunk's persistentIndex, u16 the cas file's number}. Entry 0 is the bundle's
//!   manifest, the others its items in the manifest's order (ebx, res, chunks).
//! * A manifest ([`manifest`]): u32 big-endian length, then (little-endian) 0x9D798ED6, total count, ebx count, res count, chunk count, string offset, meta offset, meta size, a SHA-1 per
//!   item, the ebx {name, size}, the res {name, size}, the res types ([`name_hash`] of the type's name), 16 bytes of meta and a u64 id per res, the chunks, the names.
//! * An item is a run of blocks {u32 big-endian size (low 24 bits), u8 codec, u8 0x70 | bits 16..19 of the stored size, u16 big-endian stored size bits 0..15} and the stored bytes. Codec 0
//!   is stored as it is; 0x19 is Oodle (Leviathan), which only the game's own decoder reads (the Mac host for the Python). There is none on the phone: such an item ends the split
//!   ([`Failure::Unsupported`]) rather than give the game fewer shaders than it has. Every item of FC 27's cas files is Oodle, so its split still ends there.
//! * A CompiledBytecode resource is a 0x20-byte header and a VNVN record ([`nvn_shader`]): one code area and control section, NVN's own.
//!
//! Memory: the tables (`.toc` files, `layout.toc`) are read whole, as the Python does; a cas file never is (31.8 GB in FC 27): a manifest's tables, its names and each item's blocks are
//! read where they are. The jobs are about equal stored sizes of one cas file's CompiledBytecode resources; the plan reads every manifest (a damaged table ends the whole split, as the
//! Python's planner raises).

use crate::ctl::{check, glslc_stage};
use crate::error::{Failure, Res, SplitError};
use crate::scan::{FileRef, finish};
use crate::scan_types::{Ctx, FileResult, Shader};
use crate::source::Source;
use crate::unity::Trees;
use crate::unsupported;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const TOC_MAGIC: [u8; 4] = [0x00, 0xd1, 0xce, 0x01];
pub const TOC_BODY: usize = 0x22c;
pub const BUNDLE_MAGIC: u32 = 0x9D79_8ED6;
/// `name_hash('CompiledBytecode')`
pub const COMPILED_BYTECODE: u32 = 0x0e2b_02f7;
const PARTS: f64 = 64.0;
/// a DbObject nested deeper than this is refused (the Python's recursion limit is near 1000 frames)
const MAX_DEPTH: usize = 900;

/// `name_hash(name)`: Frostbite's hash of a type name: djb2 with xor over the lower-case name
pub fn name_hash(name: &str) -> u32 {
    let mut h: u32 = 5381;
    for c in name.to_lowercase().bytes() {
        h = h.wrapping_mul(33) ^ c as u32;
    }
    h
}

/// `struct.error` of `unpack_from(fmt, buffer of len bytes, off)` as `scan_file` words it
fn st_err(off: u64, n: u64, len: u64) -> String {
    format!(
        "error: unpack_from requires a buffer of at least {} bytes for unpacking {n} bytes at offset {off} (actual buffer size is {len})",
        off + n
    )
}

fn value(msg: String) -> Failure {
    Failure::Red(format!("ValueError: {msg}"))
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- DbObject ----------------------------------------------------------------------------------------------------------------------

/// A DbObject value
#[derive(Clone, Debug, PartialEq)]
pub enum Dbo {
    List(Vec<Dbo>),
    /// a dict: (name, value) in order; a name given twice keeps its first place and its last value, as `dict(items)`
    Obj(Vec<(Option<String>, Dbo)>),
    Bool(bool),
    Str(String),
    Int(i64),
    F32(f32),
    F64(f64),
    Bytes(Vec<u8>),
}

impl Dbo {
    /// `d[key]` of a dict: the last value of that name
    pub fn get(&self, key: &str) -> Option<&Dbo> {
        match self {
            Dbo::Obj(items) => items
                .iter()
                .rev()
                .find(|(k, _)| k.as_deref() == Some(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }
}

fn leb(d: &[u8], mut p: usize) -> Result<(u128, usize), String> {
    let (mut v, mut s) = (0u128, 0u32);
    loop {
        let Some(&b) = d.get(p) else {
            return Err("ValueError: DbObject: a length runs past the end".into());
        };
        p += 1;
        if s < 120 {
            v |= ((b & 0x7f) as u128) << s;
        } else if b & 0x7f != 0 {
            v = u128::MAX >> 1; // more than any file holds
        }
        s += 7;
        if b & 0x80 == 0 {
            return Ok((v, p));
        }
    }
}

/// `p + n` as an index, saturated (the Python's integers do not overflow; a sum past the data only makes a slice shorter)
fn add(p: usize, n: u128) -> usize {
    (p as u128).saturating_add(n).min(usize::MAX as u128) as usize
}

/// `dbobject(d, p)`: one DbObject value at p → ((name or None, value), next offset). A type byte (bit 7 clear: a NUL-terminated name follows; low 5 bits the kind: 1 list, 2 object,
/// 6 bool, 7 string, 8 int32, 9 int64, 0xb float, 0xc double, 0xf GUID, 0x10 SHA-1, 0x13 blob), lists and objects with a LEB128 length and ending in a 0 byte
pub fn dbobject(d: &[u8], p: usize) -> Result<((Option<String>, Dbo), usize), String> {
    dbo_at(d, p, 0)
}

fn dbo_at(d: &[u8], mut p: usize, depth: usize) -> Result<((Option<String>, Dbo), usize), String> {
    if depth > MAX_DEPTH {
        return Err("RecursionError: maximum recursion depth exceeded".into());
    }
    let Some(&t) = d.get(p) else {
        return Err("IndexError: index out of range".into());
    };
    p += 1;
    let mut name = None;
    if t & 0x80 == 0 {
        let Some(e) = d[p.min(d.len())..].iter().position(|&b| b == 0) else {
            return Err("ValueError: subsection not found".into());
        };
        let e = p + e;
        name = Some(String::from_utf8_lossy(&d[p..e]).into_owned());
        p = e + 1;
    }
    let k = t & 0x1f;
    let sl = |a: usize, b: usize| -> Vec<u8> {
        let b = b.min(d.len());
        d[a.min(b)..b].to_vec()
    };
    match k {
        1 | 2 => {
            let (n, q) = leb(d, p)?;
            p = q;
            let end = add(p, n);
            let mut items = Vec::new();
            while p < end {
                let Some(&b) = d.get(p) else {
                    return Err("IndexError: index out of range".into());
                };
                if b == 0 {
                    break;
                }
                let (it, q) = dbo_at(d, p, depth + 1)?;
                items.push(it);
                p = q;
            }
            let v = if k == 2 {
                let mut m: Vec<(Option<String>, Dbo)> = Vec::new();
                for (key, val) in items {
                    match m.iter_mut().find(|(k2, _)| *k2 == key) {
                        Some(e) => e.1 = val,
                        None => m.push((key, val)),
                    }
                }
                Dbo::Obj(m)
            } else {
                Dbo::List(items.into_iter().map(|(_, v)| v).collect())
            };
            Ok(((name, v), end))
        }
        7 => {
            let (n, q) = leb(d, p)?;
            let s = if n == 0 {
                Vec::new()
            } else {
                sl(q, add(q, n - 1))
            };
            Ok((
                (name, Dbo::Str(String::from_utf8_lossy(&s).into_owned())),
                add(q, n),
            ))
        }
        19 => {
            let (n, q) = leb(d, p)?;
            Ok(((name, Dbo::Bytes(sl(q, add(q, n)))), add(q, n)))
        }
        6 | 8 | 9 | 0xb | 0xc => {
            let w = match k {
                6 => 1,
                8 | 0xb => 4,
                _ => 8,
            };
            if p + w > d.len() {
                return Err(format!(
                    "struct.{}",
                    st_err(p as u64, w as u64, d.len() as u64)
                ));
            }
            let b = &d[p..p + w];
            let v = match k {
                6 => Dbo::Bool(b[0] != 0),
                8 => Dbo::Int(i32::from_le_bytes(b.try_into().unwrap()) as i64),
                9 => Dbo::Int(i64::from_le_bytes(b.try_into().unwrap())),
                0xb => Dbo::F32(f32::from_le_bytes(b.try_into().unwrap())),
                _ => Dbo::F64(f64::from_le_bytes(b.try_into().unwrap())),
            };
            Ok(((name, v), p + w))
        }
        0xf | 0x10 => {
            let n = if k == 0xf { 16 } else { 20 };
            Ok(((name, Dbo::Bytes(sl(p, p + n))), p + n))
        }
        _ => Err(format!(
            "ValueError: DbObject: type {t:#x} at {:#x} is not known",
            p - 1
        )),
    }
}

/// `install_chunks(romfs)` from `layout.toc`'s bytes: persistentIndex (u32) → the install chunk's folder under `NX/` (None: a name that is not a string, an error where it is used)
pub fn install_chunks(d: &[u8]) -> Result<HashMap<u32, Option<String>>, String> {
    let ((_, lay), _) = dbobject(d, TOC_BODY)?;
    if !matches!(lay, Dbo::Obj(_)) || lay.get("installManifest").is_none() {
        return Err("ValueError: Data/layout.toc: no installManifest".into());
    }
    let man = lay.get("installManifest").unwrap();
    let chunks = match man {
        Dbo::Obj(_) => man
            .get("installChunks")
            .ok_or("KeyError: 'installChunks'")?,
        _ => return Err("TypeError: the installManifest is not a dict".into()),
    };
    let items: &[Dbo] = match chunks {
        Dbo::List(v) => v,
        Dbo::Obj(v) if v.is_empty() => &[],
        Dbo::Str(s) if s.is_empty() => &[],
        Dbo::Bytes(b) if b.is_empty() => &[],
        _ => return Err("TypeError: installChunks is not a list of dicts".into()),
    };
    let mut out = HashMap::new();
    for c in items {
        if !matches!(c, Dbo::Obj(_)) {
            return Err("TypeError: an install chunk is not a dict".into());
        }
        let pi = match c.get("persistentIndex") {
            Some(Dbo::Int(v)) => (*v as u64 & 0xffff_ffff) as u32,
            Some(Dbo::Bool(b)) => *b as u32,
            Some(_) => return Err("TypeError: persistentIndex is not an integer".into()),
            None => return Err("KeyError: 'persistentIndex'".into()),
        };
        let name = match c.get("name") {
            Some(Dbo::Str(s)) => Some(s.clone()),
            Some(_) => None,
            None => return Err("KeyError: 'name'".into()),
        };
        out.insert(pi, name);
    }
    Ok(out)
}

// ---- superbundle tables, manifests ----------------------------------------------------------------------------------------------------

/// One toc entry: (cas id, offset, stored size)
pub type TocEntry = ([u8; 8], u64, u64);

/// `toc_bundles(d, what)`: the entries of each bundle of a superbundle `.toc` (entry 0 the manifest); [] for a toc without a body
pub fn toc_bundles(d: &[u8], what: &str) -> Result<Vec<Vec<TocEntry>>, String> {
    if d.len() < 4 || d[..4] != TOC_MAGIC {
        return Err(format!(
            "ValueError: {what}: not a Frostbite toc (magic {})",
            hexs(&d[..d.len().min(4)])
        ));
    }
    if d.len() < TOC_BODY + 0x20 {
        return Ok(vec![]);
    }
    let n = d.len() as u64;
    let be32 = |o: u64| -> Result<u64, String> {
        if o + 4 > n {
            return Err(format!("struct.{}", st_err(o, 4, n)));
        }
        let o = o as usize;
        Ok(u32::from_be_bytes(d[o..o + 4].try_into().unwrap()) as u64)
    };
    let b = TOC_BODY as u64;
    let (bo, nb) = (be32(b + 4)?, be32(b + 8)?);
    let mut out = Vec::new();
    for i in 0..nb {
        let r = b + bo + 16 * i;
        if r + 16 > n {
            return Err(format!("struct.{}", st_err(r, 16, n)));
        }
        let sz = be32(r + 4)?;
        let off = u64::from_be_bytes(d[r as usize + 8..r as usize + 16].try_into().unwrap());
        let s = b as u128 + off as u128;
        let end = s + (sz & 0xff_ffff) as u128;
        if end > n as u128 {
            return Err(format!(
                "ValueError: {what}: bundle {i}'s data runs past the end"
            ));
        }
        let s = s as u64;
        let (fo, cnt, p) = (be32(s + 8)?, be32(s + 12)?, be32(s + 16)?);
        let (mut p, fo) = (p + s, fo + s);
        if fo + cnt > end as u64 {
            return Err(format!(
                "ValueError: {what}: bundle {i}'s flags run past its data"
            ));
        }
        let mut cas: Option<[u8; 8]> = None;
        let mut ents = Vec::new();
        for k in 0..cnt {
            if d[(fo + k) as usize] & 0x80 != 0 || cas.is_none() {
                if p + 8 > n {
                    // `bytes(d[p:p + 8])` is shorter, and the entry after it does not unpack
                    return Err(format!("struct.{}", st_err(p + 8, 8, n)));
                }
                cas = Some(d[p as usize..p as usize + 8].try_into().unwrap());
                p += 8;
            }
            if p + 8 > n {
                return Err(format!("struct.{}", st_err(p, 8, n)));
            }
            ents.push((cas.unwrap(), be32(p)?, be32(p + 4)?));
            p += 8;
        }
        if p != fo {
            return Err(format!(
                "ValueError: {what}: bundle {i}'s entries end at {:#x}, its flags start at {:#x}",
                p - s,
                fo - s
            ));
        }
        out.push(ents);
    }
    Ok(out)
}

/// A bundle manifest's items: (its length with the length word, ebx (name, size), res (name, size, type), chunks (GUID, logical offset, logical size))
pub struct Manifest {
    pub len: u64,
    pub ebx: Vec<(String, u32)>,
    pub res: Vec<(String, u32, u32)>,
    pub chunks: Vec<([u8; 16], u32, u32)>,
}

/// `manifest(d, off)`: the bundle manifest at `off` in a cas file, read where it is (the cas file is not read whole)
pub fn manifest(src: &dyn Source, off: u64) -> Result<Manifest, String> {
    let n = src.len();
    let io = |e: std::io::Error| format!("OSError: {e}");
    let need = |o: u64, k: u64| -> Result<(), String> {
        if o.checked_add(k).is_none_or(|e| e > n) {
            Err(format!("struct.{}", st_err(o, k, n)))
        } else {
            Ok(())
        }
    };
    need(off, 4)?;
    let hl = u32::from_be_bytes(src.read(off, 4).map_err(io)?[..4].try_into().unwrap()) as u64;
    need(off + 4, 24)?;
    let w = src.read(off + 4, 24).map_err(io)?;
    let le = |i: usize| u32::from_le_bytes(w[4 * i..4 * i + 4].try_into().unwrap()) as u64;
    let (magic, tot, ne, nr, nc, so) = (le(0), le(1), le(2), le(3), le(4), le(5));
    if magic != BUNDLE_MAGIC as u64 {
        return Err(format!(
            "ValueError: no bundle manifest at {off:#x} (magic {magic:#x})"
        ));
    }
    if ne + nr + nc != tot {
        return Err(format!(
            "ValueError: bundle manifest at {off:#x}: {ne} + {nr} + {nc} items, not the {tot} it counts"
        ));
    }
    let h = off + 4;
    let p0 = h + 0x20 + 20 * tot;
    let read = |o: u64, k: u64| -> Result<Vec<u8>, String> {
        need(o, k)?;
        src.read(o, k as usize).map_err(io)
    };
    let ebx_raw = if ne > 0 { read(p0, 8 * ne)? } else { vec![] };
    let pr = p0 + 8 * ne;
    let res_raw = if nr > 0 { read(pr, 8 * nr)? } else { vec![] };
    let pt = pr + 8 * nr;
    let types_raw = read(pt, 4 * nr)?; // one unpack of nr words: at its offset even when nr is 0
    let pc = pt + 28 * nr;
    let ch_raw = if nc > 0 { read(pc, 24 * nc)? } else { vec![] };
    let u = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    // the names: from h + so, up to the manifest's end (h + hl), read once when that is not much
    let (lo, hi) = (h + so, (h + hl).min(n));
    let names: Option<Vec<u8>> = if lo < hi && hi - lo <= 64 << 20 {
        Some(src.read(lo, (hi - lo) as usize).map_err(io)?)
    } else {
        None
    };
    let name = |o: u32| -> Result<String, String> {
        let a = h + so + o as u64;
        let end = h + hl;
        let bad = || {
            format!(
                "ValueError: bundle manifest at {off:#x}: a name at {:#x} runs past the manifest",
                a - off
            )
        };
        if a >= end.min(n) {
            return Err(bad());
        }
        if let Some(buf) = &names {
            let r = &buf[(a - lo) as usize..];
            return match r.iter().position(|&b| b == 0) {
                Some(e) => Ok(String::from_utf8_lossy(&r[..e]).into_owned()),
                None => Err(bad()),
            };
        }
        // a large name region: searched in pieces
        let (mut at, mut got) = (a, Vec::new());
        let stop = end.min(n);
        while at < stop {
            let k = (stop - at).min(4096);
            let piece = src.read(at, k as usize).map_err(io)?;
            if piece.is_empty() {
                break;
            }
            if let Some(e) = piece.iter().position(|&b| b == 0) {
                got.extend_from_slice(&piece[..e]);
                return Ok(String::from_utf8_lossy(&got).into_owned());
            }
            got.extend_from_slice(&piece);
            at += piece.len() as u64;
        }
        Err(bad())
    };
    let mut ebx = Vec::with_capacity(ne as usize);
    for i in 0..ne as usize {
        ebx.push((u(&ebx_raw, 8 * i), u(&ebx_raw, 8 * i + 4)));
    }
    let mut res = Vec::with_capacity(nr as usize);
    for i in 0..nr as usize {
        res.push((
            u(&res_raw, 8 * i),
            u(&res_raw, 8 * i + 4),
            u(&types_raw, 4 * i),
        ));
    }
    let mut chunks = Vec::with_capacity(nc as usize);
    for i in 0..nc as usize {
        let c = &ch_raw[24 * i..24 * i + 24];
        chunks.push((c[..16].try_into().unwrap(), u(c, 16), u(c, 20)));
    }
    let mut ebx_n = Vec::with_capacity(ebx.len());
    for (a, s) in ebx {
        ebx_n.push((name(a)?, s));
    }
    let mut res_n = Vec::with_capacity(res.len());
    for (a, s, t) in res {
        res_n.push((name(a)?, s, t));
    }
    Ok(Manifest {
        len: hl + 4,
        ebx: ebx_n,
        res: res_n,
        chunks,
    })
}

// ---- the RomFS: install chunks, tables, resources ---------------------------------------------------------------------------------------

/// `os.path.join(romfs, top, 'NX', name, 'cas_NN.cas')` below the RomFS: the path as the Python spells it (not normalised)
fn cas_rel(
    prefix: &str,
    cid: &[u8; 8],
    chunks: &HashMap<u32, Option<String>>,
) -> Result<String, String> {
    let pi = u32::from_be_bytes(cid[2..6].try_into().unwrap());
    let n = u16::from_be_bytes([cid[6], cid[7]]);
    let Some(name) = chunks.get(&pi) else {
        return Err(format!(
            "ValueError: cas id {}: no install chunk has persistentIndex {pi:#x}",
            hexs(cid)
        ));
    };
    let Some(name) = name else {
        return Err("TypeError: an install chunk's name is not a string".into());
    };
    if name.starts_with('/') {
        return Err(format!(
            "FileNotFoundError: an install chunk's folder is outside the RomFS ({name})"
        ));
    }
    let top = if cid[1] != 0 { "Patch" } else { "Data" };
    let mut p = format!("{prefix}{top}/NX/");
    if !name.is_empty() {
        p.push_str(name);
        if !name.ends_with('/') {
            p.push('/');
        }
    }
    p.push_str(&format!("cas_{n:02}.cas"));
    Ok(p)
}

/// a path as the file system resolves it: `.` and empty parts dropped, `..` one folder up (None: above the RomFS)
fn normal(rel: &str) -> Option<String> {
    let mut v: Vec<&str> = Vec::new();
    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                v.pop()?;
            }
            c => v.push(c),
        }
    }
    Some(v.join("/"))
}

/// The order `os.walk(top)` gives the files below it: a folder's files (sorted, as `tocs` sorts them), then its subfolders (sorted here; the Python takes the file system's order)
fn walk_order(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
    for i in 0..x.len().min(y.len()) {
        if x[i] == y[i] {
            continue;
        }
        let (xf, yf) = (i == x.len() - 1, i == y.len() - 1);
        return match (xf, yf) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => x[i].cmp(y[i]),
        };
    }
    x.len().cmp(&y.len())
}

/// One Frostbite RomFS (or version, or add-on) inside the split's file list: its root and the path prefix of its `Data/layout.toc`
pub struct FbRoot<'a> {
    pub files: &'a [FileRef],
    pub root: usize,
    pub prefix: String,
    index: HashMap<&'a str, &'a FileRef>,
}

/// The CompiledBytecode (or any `rtype`) resources: (cas file's path, offset, stored size) → (size, name), each stored copy once (a later bundle naming the same copy gives its name)
pub type Resources = BTreeMap<(String, u64, u64), (u64, String)>;

impl<'a> FbRoot<'a> {
    pub fn new(files: &'a [FileRef], root: usize, prefix: String) -> FbRoot<'a> {
        let index = files
            .iter()
            .filter(|f| f.root == root)
            .map(|f| (f.rel.as_str(), f))
            .collect();
        FbRoot {
            files,
            root,
            prefix,
            index,
        }
    }

    fn find(&self, rel: &str) -> Option<&'a FileRef> {
        self.index.get(rel).copied()
    }

    /// `tocs(romfs)`: the superbundle tables of `Data/NX` and `Patch/NX`, in `os.walk`'s order
    pub fn tocs(&self) -> Vec<&FileRef> {
        let mut out = Vec::new();
        for top in ["Data/NX/", "Patch/NX/"] {
            let base = format!("{}{top}", self.prefix);
            let mut v: Vec<&FileRef> = self
                .files
                .iter()
                .filter(|f| {
                    f.root == self.root && f.rel.starts_with(&base) && f.rel.ends_with(".toc")
                })
                .collect();
            v.sort_by(|a, b| walk_order(&a.rel[base.len()..], &b.rel[base.len()..]));
            out.extend(v);
        }
        out
    }

    /// `resources(romfs, rtype)`; `path_of(f)` names a table in the messages
    pub fn resources(
        &self,
        rtype: u32,
        path_of: &dyn Fn(&FileRef) -> String,
    ) -> Result<Resources, String> {
        let io = |e: std::io::Error| format!("OSError: {e}");
        let layout = self
            .find(&format!("{}Data/layout.toc", self.prefix))
            .ok_or("FileNotFoundError: Data/layout.toc")?;
        let d = layout.loc.open().and_then(|s| s.read_all()).map_err(io)?;
        let chunks = install_chunks(&d)?;
        let mut out = Resources::new();
        let mut opened: HashMap<String, crate::source::FileSource> = HashMap::new();
        for t in self.tocs() {
            let what = path_of(t);
            let d = t.loc.open().and_then(|s| s.read_all()).map_err(io)?;
            for ents in toc_bundles(&d, &what)? {
                let Some(&(cid, off, size)) = ents.first() else {
                    return Err("IndexError: list index out of range".into());
                };
                let cas = cas_rel(&self.prefix, &cid, &chunks)?;
                let norm = normal(&cas).ok_or_else(|| format!("FileNotFoundError: {cas}"))?;
                if !opened.contains_key(&norm) {
                    let f = self
                        .find(&norm)
                        .ok_or_else(|| format!("FileNotFoundError: {cas}"))?;
                    if f.loc.is_empty() {
                        return Err("ValueError: cannot mmap an empty file".into());
                    }
                    opened.insert(norm.clone(), f.loc.open().map_err(io)?);
                }
                let m = manifest(&opened[&norm], off)?;
                if m.len != size {
                    return Err(format!(
                        "ValueError: {what}: a bundle manifest of {:#x} bytes, the toc says {size:#x}",
                        m.len
                    ));
                }
                let items = m.ebx.len() + m.res.len() + m.chunks.len();
                if items != ents.len() - 1 {
                    return Err(format!(
                        "ValueError: {what}: a bundle of {items} items with {} entries",
                        ents.len() - 1
                    ));
                }
                for ((nm, sz, ty), (c, o, s)) in m.res.iter().zip(&ents[1 + m.ebx.len()..]) {
                    if *ty == rtype {
                        out.insert(
                            (cas_rel(&self.prefix, c, &chunks)?, *o, *s),
                            (*sz as u64, nm.clone()),
                        );
                    }
                }
            }
        }
        Ok(out)
    }
}

// ---- items, VNVN records -------------------------------------------------------------------------------------------------------------

/// `read(d, off, stored, size, decode)`: an item's blocks at `off` decoded (stored ones only: an Oodle block is unsupported here)
pub fn read(src: &dyn Source, off: u64, stored: u64, size: u64) -> Res<Vec<u8>> {
    let n = src.len();
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let (mut o, end) = (off, off + stored);
    let mut out: Vec<u8> = Vec::new();
    while o < end {
        if o + 8 > end {
            return Err(value(format!(
                "a block header at {o:#x} runs past the item"
            )));
        }
        if o + 6 > n {
            return Err(Failure::Red(st_err(o, 6, n)));
        }
        if o + 8 > n {
            return Err(Failure::Red(st_err(o + 6, 2, n)));
        }
        let h = src.read(o, 8).map_err(io)?;
        let (codec, cs) = (h[4], h[5]);
        let c = ((cs & 0xf) as u64) << 16 | u16::from_be_bytes([h[6], h[7]]) as u64;
        if cs & 0xf0 != 0x70 || o + 8 + c > end {
            return Err(value(format!("a damaged block at {o:#x} ({})", hexs(&h))));
        }
        match codec {
            0 => out.extend_from_slice(&src.read(o + 8, c as usize).map_err(io)?),
            0x19 => unsupported!(
                "Oodle Leviathan block in a Frostbite cas file (item at {off:#x}, block at {o:#x}: the game's own decoder in the Mac host)"
            ),
            _ => {
                return Err(value(format!(
                    "block at {o:#x}: codec {codec:#x} is not known"
                )));
            }
        }
        o += 8 + c;
    }
    if out.len() as u64 != size {
        return Err(value(format!(
            "item at {off:#x}: {:#x} bytes decoded, not {size:#x}",
            out.len()
        )));
    }
    Ok(out)
}

/// `nvn_shader(b)`: a CompiledBytecode resource → (code area, control section), or None when it holds no VNVN record. The record ('VNVN', u32 size, u16 version, ..., a byte from version 3
/// on, four lists of register mappings {u16 count, 4 bytes each}, one list {u16 count, 8 bytes each}, then {u32 code offset, u32 code length, u32 control offset, u32 control length}) is at +0x20
pub fn nvn_shader(b: &[u8]) -> Res<Option<(&[u8], &[u8])>> {
    if b.len() < 0x24 || b[0x20..0x24] != *b"VNVN" {
        return Ok(None);
    }
    let n = b.len() as u64;
    let u16at = |p: usize| -> Res<usize> {
        if p + 2 > b.len() {
            return Err(Failure::Red(st_err(p as u64, 2, n)));
        }
        Ok(u16::from_le_bytes([b[p], b[p + 1]]) as usize)
    };
    let r = 0x20usize;
    let v = u16at(r + 8)?;
    let mut p = r + if v >= 3 { 0x11 } else { 0x10 };
    for _ in 0..4 {
        p += 2 + 4 * u16at(p)?;
    }
    p += 2 + 8 * u16at(p)?;
    if p + 16 > b.len() {
        return Err(Failure::Red(st_err(p as u64, 16, n)));
    }
    let w = |i: usize| u32::from_le_bytes(b[p + 4 * i..p + 4 * i + 4].try_into().unwrap()) as usize;
    let (co, cl, ko, kl) = (w(0), w(1), w(2), w(3));
    if r + co + cl > b.len() || r + ko + kl > b.len() {
        return Err(value(format!(
            "VNVN record: code {co:#x}+{cl:#x} or control {ko:#x}+{kl:#x} outside its {:#x} bytes",
            b.len() - r
        )));
    }
    Ok(Some((&b[r + co..r + co + cl], &b[r + ko..r + ko + kl])))
}

// ---- jobs ------------------------------------------------------------------------------------------------------------------------------

/// One job of `frostbite_jobs`: a range of one cas file's CompiledBytecode resources, (offset, stored size, size, name) each
#[derive(Clone)]
pub struct FbJob {
    pub file: FileRef,
    pub entries: Vec<(u64, u64, u64, String)>,
}

impl FbJob {
    pub fn identity(&self) -> Vec<String> {
        std::iter::once(format!(
            "frostbite:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len()
        ))
        .chain(self.entries.iter().map(|e| format!("{e:?}")))
        .collect()
    }
}

/// The Frostbite plan: (the jobs, the files they read as (root, rel)).
pub type FbPlan = (Vec<FbJob>, HashSet<(usize, String)>);

/// `frostbite_jobs(files)`: every `.../Data/layout.toc` with the toc magic is a Frostbite root; its CompiledBytecode resources by cas file, in ranges of about equal stored size, and every
/// `.toc` and `.cas` file below it is taken (they hold nothing a loose scan reads). A table that cannot be read ends the split, as the Python's planner raises
pub fn plan(files: &[FileRef], path_of: &dyn Fn(&FileRef) -> String) -> Result<FbPlan, SplitError> {
    const LAYOUT: &str = "Data/layout.toc";
    let mut roots: Vec<(String, usize, String)> = Vec::new();
    for f in files {
        if f.rel == LAYOUT || f.rel.ends_with(&format!("/{LAYOUT}")) {
            let full = path_of(f);
            roots.push((
                full[..full.len() - LAYOUT.len()].to_string(),
                f.root,
                f.rel[..f.rel.len() - LAYOUT.len()].to_string(),
            ));
        }
    }
    roots.sort();
    roots.dedup();
    let (mut jobs, mut taken) = (Vec::new(), HashSet::new());
    for (_, root, prefix) in roots {
        let r = FbRoot::new(files, root, prefix);
        let Some(layout) = r.find(&format!("{}{LAYOUT}", r.prefix)) else {
            continue;
        };
        let head = layout
            .loc
            .open()
            .and_then(|s| s.read(0, 4))
            .map_err(|e| SplitError::Io(e.to_string()))?;
        if head != TOC_MAGIC {
            continue; // is_frostbite
        }
        let res = r
            .resources(COMPILED_BYTECODE, path_of)
            .map_err(|e| SplitError::Red(format!("{}: {e}", path_of(layout))))?;
        for f in files {
            if f.root == root
                && f.rel.starts_with(&r.prefix)
                && (f.rel.ends_with(".toc") || f.rel.ends_with(".cas"))
            {
                taken.insert((f.root, f.rel.clone()));
            }
        }
        let mut by_cas: BTreeMap<&str, Vec<(u64, u64, u64, String)>> = BTreeMap::new();
        for ((cas, off, stored), (size, name)) in &res {
            by_cas
                .entry(cas.as_str())
                .or_default()
                .push((*off, *stored, *size, name.clone()));
        }
        let step = res.keys().map(|k| k.2).sum::<u64>() as f64 / PARTS;
        for (cas, mut es) in by_cas {
            let Some(file) = r.find(cas) else {
                return Err(SplitError::Red(format!(
                    "{}: KeyError: the cas file {cas} is not in the RomFS",
                    path_of(layout)
                )));
            };
            es.sort();
            let (mut cur, mut size) = (Vec::new(), 0u64);
            for e in es {
                size += e.1;
                cur.push(e);
                if size as f64 >= step {
                    jobs.push(FbJob {
                        file: file.clone(),
                        entries: std::mem::take(&mut cur),
                    });
                    size = 0;
                }
            }
            if !cur.is_empty() {
                jobs.push(FbJob {
                    file: file.clone(),
                    entries: cur,
                });
            }
        }
    }
    Ok((jobs, taken))
}

fn run(cx: &Ctx<'_>, job: &FbJob) -> Res<FileResult> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    if src.is_empty() {
        return Err(value("cannot mmap an empty file".into()));
    }
    let mut found: Vec<Shader> = Vec::new();
    for (off, stored, size, name) in &job.entries {
        let label = format!("/{}#{name}", job.file.rel);
        let b = read(&src, *off, *stored, *size)?;
        let Some((code, ctl)) = nvn_shader(&b)? else {
            cx.print(&format!(
                "[RED] {label}: a CompiledBytecode resource without a VNVN record; left out"
            ));
            continue;
        };
        let st = glslc_stage(ctl, code)?;
        let why = match st {
            Some(st) => check(st, ctl, code)?,
            None => Some("no stage in the code area header"),
        };
        if let Some(w) = why {
            cx.print(&format!("[RED] {label}: {w}; left out"));
            continue;
        }
        found.push(Shader::new(label, st.unwrap(), ctl, code));
    }
    finish(cx, &job.file.rel, found, std::iter::empty(), Trees::new())
}

/// `scan_file((cas, rel, (FB_CAS, entries)))`: an NXSD magic at the file's start is read as NXSD first, as `scan_file_in` does; an item that cannot be read leaves the job's shaders out
pub fn scan(cx: &Ctx<'_>, job: &FbJob) -> Result<FileResult, String> {
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
    fn hashes_and_walk_order() {
        assert_eq!(name_hash("CompiledBytecode"), COMPILED_BYTECODE);
        assert_eq!(name_hash("Texture"), 0x6bde20ba);
        assert_eq!(name_hash("MeshSet"), 0x49b156d4);
        assert_eq!(name_hash("ShaderBlockDepotResource"), 0x73312045);
        let mut v = vec!["b/x.toc", "a.toc", "a/z.toc", "c.toc", "a/b/y.toc"];
        v.sort_by(|a, b| walk_order(a, b));
        assert_eq!(v, ["a.toc", "c.toc", "a/z.toc", "a/b/y.toc", "b/x.toc"]);
    }
}
