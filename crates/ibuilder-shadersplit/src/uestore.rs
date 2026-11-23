//! Unreal Engine IoStore containers (`.utoc` / `.ucas`): the engine's `uepak.py` `Toc`, `io_tocs`, `mount_order`, `io_archive`, `io_library_name`, `archive_names` and `city64`
//! (upstream seedds/nxnative origin/main).
//!
//! A container is the `.utoc` (chunk IDs, chunk ranges of the container's uncompressed stream, compression blocks, method names, an optional directory index) and the data in the `.ucas`
//! beside it (`<name>_s<n>.ucas` for partitions after the first). Blocks are stored, Zlib, LZ4 or Oodle; encrypted containers (flag 2) are AES-256 ECB with the key of the pak beside them.
//! Oodle needs the game's own decoder, which the Mac host runs for the Python (`OodleHost` / `oodle_ask`); the phone has none (the split runs before the game's code is loaded, and no
//! permissively licensed Oodle decoder exists), so the caller decides what an Oodle block costs: the package (the Python's no-host red line) or the split (`unsupported container`).
//! Two uses, as upstream:
//! * Unreal Engine 5 shader libraries (chunk type 8, `ShaderCodeLibrary`, and their groups, type 9, `ShaderCode`): turned into the shaders of a version 2 archive ([`io_archive`], what
//!   `uepak.job` writes for the NSP script);
//! * export bundles (the cooked packages: type 1 in Unreal Engine 5's numbering, 2 in 4.27's) of the containers beside a pak whose shaders are inline (`ue_legacy_jobs`, `ue_io_shaders`).

use crate::aes;
use crate::image::Loc;
use crate::inflate;
use crate::uepak::{UeErr, label_of};
use std::collections::HashMap;

pub const TOC_MAGIC: &[u8; 16] = b"-==--==--==--==-";
/// EIoChunkType in Unreal Engine 5 (TOC version 4 and later)
pub const IO_SHADER_LIBRARY: u8 = 8;
pub const IO_SHADER_CODE: u8 = 9;
pub const IO_EXPORT_BUNDLE: u8 = 1;
/// Unreal Engine 4.27's export bundle type (TOC version 3 and before)
pub const IO_EXPORT_BUNDLE_427: u8 = 2;

type UeRes<T> = Result<T, UeErr>;

fn val<T>(m: impl Into<String>) -> UeRes<T> {
    Err(UeErr::Value(m.into()))
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |v, &c| (v << 8) | c as u64)
}

fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |v, &c| (v << 8) | c as u64)
}

/// `Toc(path, key)`: an IoStore container's table of contents
pub struct Toc {
    /// the path the Python names the container by
    pub path: String,
    pub version: u8,
    flags: u8,
    block_size: u64,
    pub encrypted: bool,
    parts: u32,
    part_size: u128,
    pub ids: Vec<[u8; 12]>,
    /// (offset, length) in the container's uncompressed stream
    pub ranges: Vec<(u64, u64)>,
    /// (offset in the .ucas, stored size, raw size, method index from 1)
    blocks: Vec<(u64, u32, u32, u8)>,
    methods: Vec<String>,
    dir_index: Vec<u8>,
    key: Option<[u8; 32]>,
    /// the .ucas partitions, by number (None: not in the RomFS)
    ucas: Vec<Option<Loc>>,
}

/// `path[:-5] + ('.ucas' if not k else f'_s{k}.ucas')`
pub fn ucas_name(toc: &str, k: usize) -> String {
    let stem = &toc[..toc.len().saturating_sub(5)];
    if k == 0 {
        format!("{stem}.ucas")
    } else {
        format!("{stem}_s{k}.ucas")
    }
}

impl Toc {
    /// Parses the `.utoc` bytes `b`; `ucas(name)` finds a partition by its name below the RomFS (the TOC's own `rel` with the ending replaced)
    pub fn parse(
        path: &str,
        rel: &str,
        b: &[u8],
        key: Option<[u8; 32]>,
        ucas: &dyn Fn(&str) -> Option<Loc>,
    ) -> UeRes<Toc> {
        if b.len() < 16 || &b[..16] != TOC_MAGIC {
            return val(format!("{path}: not an IoStore TOC"));
        }
        if b.len() < 100 {
            // `struct.unpack_from('<9I', b, 20)` and the header bytes after it (a TOC header is 144 bytes)
            return Err(UeErr::Other(format!(
                "error: unpack_from requires a buffer of at least 100 bytes (actual buffer size is {})",
                b.len()
            )));
        }
        let version = b[16];
        let w: Vec<u32> = (0..9).map(|i| u32at(b, 20 + 4 * i)).collect();
        let (hsz, n, nblk, bsz, nmeth, mlen, block_size, dsz, nparts) =
            (w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], w[8]);
        if bsz != 12 {
            return val(format!("{path}: compression block entries of {bsz} bytes"));
        }
        let flags = b[80];
        let nseeds = if version >= 4 { u32at(b, 84) } else { 0 } as usize;
        let part_size: u128 = if version >= 3 && nparts > 1 {
            u64::from_le_bytes(b[88..96].try_into().unwrap()) as u128
        } else {
            1u128 << 64
        };
        let nover = if version >= 5 { u32at(b, 96) } else { 0 } as usize;
        let (n, nblk) = (n as usize, nblk as usize);
        let short =
            || -> UeErr { UeErr::Value(format!("{path}: the TOC's tables run past its end")) };
        let take = |o: usize, len: usize| -> UeRes<&[u8]> {
            o.checked_add(len)
                .filter(|&e| e <= b.len())
                .map(|e| &b[o..e])
                .ok_or_else(short)
        };
        let mut o = hsz as usize;
        let ids: Vec<[u8; 12]> = take(o, 12 * n)?
            .chunks(12)
            .map(|c| c.try_into().unwrap())
            .collect();
        o += 12 * n;
        let ranges: Vec<(u64, u64)> = take(o, 10 * n)?
            .chunks(10)
            .map(|c| (be(&c[..5]), be(&c[5..])))
            .collect();
        o += 10 * n + 4 * nseeds + 4 * nover;
        let blocks: Vec<(u64, u32, u32, u8)> = take(o, 12 * nblk)?
            .chunks(12)
            .map(|e| {
                (
                    le(&e[..5]),
                    le(&e[5..8]) as u32,
                    le(&e[8..11]) as u32,
                    e[11],
                )
            })
            .collect();
        o += 12 * nblk;
        let (nmeth, mlen) = (nmeth as usize, mlen as usize);
        let methods: Vec<String> = (0..nmeth)
            .map(|i| {
                let s = crate::bytes::sl(b, o + mlen * i, o + mlen * (i + 1));
                let end = s.iter().rposition(|&c| c != 0).map(|p| p + 1).unwrap_or(0);
                s[..end].iter().map(|&c| c as char).collect()
            })
            .collect();
        o = o.saturating_add(mlen * nmeth);
        let encrypted = (flags >> 1) & 1 != 0;
        let d = if flags & 8 != 0 && flags & 4 == 0 {
            crate::bytes::sl(b, o, o.saturating_add(dsz as usize)).to_vec()
        } else {
            Vec::new()
        };
        let dir_index = if !d.is_empty() && encrypted {
            match key {
                Some(k) => {
                    let mut p = d.clone();
                    p.resize(d.len().div_ceil(16) * 16, 0);
                    let mut x = aes::decrypt(&k, &p).map_err(UeErr::Value)?;
                    x.truncate(d.len());
                    x
                }
                None => Vec::new(),
            }
        } else {
            d
        };
        let parts = nparts.max(1) as usize;
        Ok(Toc {
            path: path.to_string(),
            version,
            flags,
            block_size: block_size as u64,
            encrypted,
            parts: nparts,
            part_size,
            ids,
            ranges,
            blocks,
            methods,
            dir_index,
            key,
            ucas: (0..parts).map(|k| ucas(&ucas_name(rel, k))).collect(),
        })
    }

    /// `of_type(t)`: the chunks of one EIoChunkType in Unreal Engine 5's numbering (none before TOC version 4)
    pub fn of_type(&self, t: u8) -> Vec<usize> {
        if self.version < 4 {
            return Vec::new();
        }
        (0..self.ids.len())
            .filter(|&i| self.ids[i][11] == t)
            .collect()
    }

    /// `export_bundles()`: the cooked packages' export chunks (type 1 from TOC version 4, 2 before)
    pub fn export_bundles(&self) -> Vec<usize> {
        let t = if self.version >= 4 {
            IO_EXPORT_BUNDLE
        } else {
            IO_EXPORT_BUNDLE_427
        };
        (0..self.ids.len())
            .filter(|&i| self.ids[i][11] == t)
            .collect()
    }

    /// `ucas_paths()` below the RomFS, from the TOC's `rel`
    pub fn ucas_rels(&self, rel: &str) -> Vec<String> {
        (0..(self.parts.max(1) as usize))
            .map(|k| ucas_name(rel, k))
            .collect()
    }

    /// `names()`: {chunk index: package path} from the directory index, joined to its mount point; empty without one
    pub fn names(&self) -> UeRes<HashMap<usize, String>> {
        let mut out = HashMap::new();
        if self.dir_index.is_empty() {
            return Ok(out);
        }
        let what = format!("{}: the directory index", self.path);
        let mut r = crate::uepak::R::new(&self.dir_index, 0, what.clone());
        let mount = r.fstr()?;
        let nd = r.count()?;
        let mut dirs = Vec::new();
        for _ in 0..nd.max(0) {
            r.need(16)?;
            dirs.push([r.u32()?, r.u32()?, r.u32()?, r.u32()?]);
        }
        let nf = r.count()?;
        let mut files = Vec::new();
        for _ in 0..nf.max(0) {
            r.need(12)?;
            files.push([r.u32()?, r.u32()?, r.u32()?]);
        }
        let ns = r.count()?;
        let mut strs = Vec::new();
        for _ in 0..ns.max(0) {
            strs.push(r.fstr()?);
        }
        const NONE: u32 = 0xffffffff;
        let idx = |v: &[String], i: u32| -> UeRes<String> {
            v.get(i as usize)
                .cloned()
                .ok_or_else(|| UeErr::Other("IndexError: list index out of range".into()))
        };
        let mut raw: Vec<(u32, String)> = Vec::new();
        let mut todo: Vec<(u32, String)> = if dirs.is_empty() {
            Vec::new()
        } else {
            vec![(0, String::new())]
        };
        // a damaged index can make the lists cycle (the Python would not end): bounded by every directory and file once per directory
        let mut budget =
            (dirs.len() as u64 + 1) * (files.len() as u64 + 1) + dirs.len() as u64 + 16;
        while let Some((d, at)) = todo.pop() {
            let [name, child, sib, mut f] = *dirs
                .get(d as usize)
                .ok_or_else(|| UeErr::Other("IndexError: list index out of range".into()))?;
            let here = if name != NONE {
                format!("{at}{}/", idx(&strs, name)?)
            } else {
                at.clone()
            };
            if sib != NONE {
                todo.push((sib, at));
            }
            if child != NONE {
                todo.push((child, here.clone()));
            }
            while f != NONE {
                budget = budget.saturating_sub(1);
                if budget == 0 {
                    return val(format!("{what}: its lists do not end"));
                }
                let [nm, next, chunk] = *files
                    .get(f as usize)
                    .ok_or_else(|| UeErr::Other("IndexError: list index out of range".into()))?;
                raw.push((chunk, format!("{here}{}", idx(&strs, nm)?)));
                f = next;
            }
            budget = budget.saturating_sub(1);
            if budget == 0 {
                return val(format!("{what}: its lists do not end"));
            }
        }
        for (c, v) in raw {
            out.insert(c as usize, label_of(&mount, &v));
        }
        Ok(out)
    }

    /// `read(i, decode)`: chunk i's bytes. `oodle()` is what an Oodle block costs (there is no decoder here)
    pub fn read(&self, i: usize, oodle: &dyn Fn() -> UeErr) -> UeRes<Vec<u8>> {
        if self.encrypted && self.key.is_none() {
            return val(format!(
                "{}: the container is encrypted and no key was given",
                self.path
            ));
        }
        let (off, ln) = self.ranges[i];
        if self.block_size == 0 {
            return Err(UeErr::Other(
                "ZeroDivisionError: integer division or modulo by zero".into(),
            ));
        }
        let first = off / self.block_size;
        let end = if ln != 0 {
            (off + ln - 1) / self.block_size + 1
        } else {
            first
        };
        let mut out: Vec<u8> = Vec::new();
        let mut srcs: HashMap<usize, crate::source::FileSource> = HashMap::new();
        for k in first..end {
            let &(at, size, raw, m) = self
                .blocks
                .get(k as usize)
                .ok_or_else(|| UeErr::Other("IndexError: list index out of range".into()))?;
            let part = (at as u128 / self.part_size) as usize;
            if let std::collections::hash_map::Entry::Vacant(e) = srcs.entry(part) {
                let name = ucas_name(&self.path, part);
                let loc = self.ucas.get(part).cloned().flatten().ok_or_else(|| {
                    UeErr::Other(format!(
                        "FileNotFoundError: [Errno 2] No such file or directory: '{name}'"
                    ))
                })?;
                e.insert(
                    loc.open()
                        .map_err(|x| UeErr::Other(format!("OSError: {x}")))?,
                );
            }
            let src = &srcs[&part];
            let pos = (at as u128 % self.part_size) as u64;
            let n = if self.encrypted {
                (size as usize + 15) & !15
            } else {
                size as usize
            };
            let mut d = crate::source::Source::read(src, pos, n)
                .map_err(|x| UeErr::Other(format!("OSError: {x}")))?;
            if let (true, Some(key)) = (self.encrypted, self.key.as_ref()) {
                d = aes::decrypt(key, &d).map_err(UeErr::Value)?;
                d.truncate(size as usize);
            }
            let meth = if m != 0 {
                self.methods
                    .get(m as usize - 1)
                    .cloned()
                    .ok_or_else(|| UeErr::Other("IndexError: list index out of range".into()))?
            } else {
                String::new()
            };
            let b = match meth.as_str() {
                "" => {
                    d.truncate(raw as usize);
                    d
                }
                "Oodle" => return Err(oodle()),
                "Zlib" => zlib_whole(&d).map_err(|e| UeErr::Other(format!("error: {e}")))?,
                "LZ4" => crate::uepak::lz4_block(&d, raw as usize)
                    .map_err(|e| UeErr::Other(format!("LZ4BlockError: {e}")))?,
                _ => {
                    return val(format!(
                        "{}: compression method {meth} is not read",
                        self.path
                    ));
                }
            };
            if out.len() as u128 + b.len() as u128 > inflate::MAX_DECODED as u128 {
                return val(format!(
                    "{}: chunk {i}: {}",
                    self.path,
                    inflate::cap_error(inflate::MAX_DECODED)
                ));
            }
            out.extend_from_slice(&b);
        }
        let s = (off - first * self.block_size) as usize;
        Ok(crate::bytes::sl(&out, s, s.saturating_add(ln as usize)).to_vec())
    }

    /// The TOC flags (1 compressed, 2 encrypted, 4 signed, 8 indexed)
    pub fn flags(&self) -> u8 {
        self.flags
    }
}

/// `mount_order(path)`: the base container first, then the numbered patch chunks (`<name>_<n>_P.utoc`, unnumbered `_P` as 0), then by path
pub fn mount_order(path: &str) -> (u8, u128, String) {
    let base = path.rsplit('/').next().unwrap_or(path);
    let lower = base.to_ascii_lowercase();
    if let Some(stem) = lower.strip_suffix("_p.utoc") {
        let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let n = if digits > 0 && stem[..stem.len() - digits].ends_with('_') {
            stem[stem.len() - digits..]
                .parse::<u128>()
                .unwrap_or(u128::MAX)
        } else {
            0
        };
        return (1, n, path.to_string());
    }
    (0, 0, path.to_string())
}

// ---- CityHash64 ------------------------------------------------------------------------------------------------------------------------

const K0: u64 = 0xc3a5c85c97cb3127;
const K1: u64 = 0xb492b66fbe98f273;
const K2: u64 = 0x9ae16a3b2f90404f;

fn f64_(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().unwrap())
}
fn f32_(s: &[u8], i: usize) -> u64 {
    u32::from_le_bytes(s[i..i + 4].try_into().unwrap()) as u64
}
fn rot(v: u64, s: u32) -> u64 {
    if s == 0 { v } else { v.rotate_right(s) }
}
fn h16(u: u64, v: u64, mul: u64) -> u64 {
    let mut a = (u ^ v).wrapping_mul(mul);
    a ^= a >> 47;
    let mut b = (v ^ a).wrapping_mul(mul);
    b ^= b >> 47;
    b.wrapping_mul(mul)
}
const KMUL: u64 = 0x9ddfea08eb382d69;
fn weak(s: &[u8], i: usize, a: u64, b: u64) -> (u64, u64) {
    let (w, x, y, z) = (f64_(s, i), f64_(s, i + 8), f64_(s, i + 16), f64_(s, i + 24));
    let mut a = a.wrapping_add(w);
    let mut b = rot(b.wrapping_add(a).wrapping_add(z), 21);
    let c = a;
    a = a.wrapping_add(x).wrapping_add(y);
    b = b.wrapping_add(rot(a, 44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

/// `city64(s)`: CityHash64 (Google's CityHash 1.1, the one Unreal's Core links)
pub fn city64(s: &[u8]) -> u64 {
    let n = s.len();
    let nn = n as u64;
    if n <= 16 {
        if n >= 8 {
            let mul = K2.wrapping_add(nn * 2);
            let a = f64_(s, 0).wrapping_add(K2);
            let b = f64_(s, n - 8);
            return h16(
                rot(b, 37).wrapping_mul(mul).wrapping_add(a),
                rot(a, 25).wrapping_add(b).wrapping_mul(mul),
                mul,
            );
        }
        if n >= 4 {
            let mul = K2.wrapping_add(nn * 2);
            return h16(nn.wrapping_add(f32_(s, 0) << 3), f32_(s, n - 4), mul);
        }
        if n > 0 {
            let y = s[0] as u64 + ((s[n >> 1] as u64) << 8);
            let z = nn + ((s[n - 1] as u64) << 2);
            let v = y.wrapping_mul(K2) ^ z.wrapping_mul(K0);
            return (v ^ (v >> 47)).wrapping_mul(K2);
        }
        return K2;
    }
    let mul = K2.wrapping_add(nn * 2);
    if n <= 32 {
        let a = f64_(s, 0).wrapping_mul(K1);
        let b = f64_(s, 8);
        let c = f64_(s, n - 8).wrapping_mul(mul);
        let d = f64_(s, n - 16).wrapping_mul(K2);
        return h16(
            rot(a.wrapping_add(b), 43)
                .wrapping_add(rot(c, 30))
                .wrapping_add(d),
            a.wrapping_add(rot(b.wrapping_add(K2), 18)).wrapping_add(c),
            mul,
        );
    }
    if n <= 64 {
        let a = f64_(s, 0).wrapping_mul(K2);
        let b = f64_(s, 8);
        let c = f64_(s, n - 24);
        let d = f64_(s, n - 32);
        let e = f64_(s, 16).wrapping_mul(K2);
        let f = f64_(s, 24).wrapping_mul(9);
        let g = f64_(s, n - 8);
        let h = f64_(s, n - 16).wrapping_mul(mul);
        let u = rot(a.wrapping_add(g), 43).wrapping_add(rot(b, 30).wrapping_add(c).wrapping_mul(9));
        let v = (a.wrapping_add(g) ^ d).wrapping_add(f).wrapping_add(1);
        let w = u
            .wrapping_add(v)
            .wrapping_mul(mul)
            .swap_bytes()
            .wrapping_add(h);
        let x = rot(e.wrapping_add(f), 42).wrapping_add(c);
        let y = v
            .wrapping_add(w)
            .wrapping_mul(mul)
            .swap_bytes()
            .wrapping_add(g)
            .wrapping_mul(mul);
        let z = e.wrapping_add(f).wrapping_add(c);
        let a = x
            .wrapping_add(z)
            .wrapping_mul(mul)
            .wrapping_add(y)
            .swap_bytes()
            .wrapping_add(b);
        let mut b = z
            .wrapping_add(a)
            .wrapping_mul(mul)
            .wrapping_add(d)
            .wrapping_add(h);
        b = (b ^ (b >> 47)).wrapping_mul(mul);
        return b.wrapping_add(x);
    }
    let mut x = f64_(s, n - 40);
    let mut y = f64_(s, n - 16).wrapping_add(f64_(s, n - 56));
    let mut z = h16(f64_(s, n - 48).wrapping_add(nn), f64_(s, n - 24), KMUL);
    let mut v = weak(s, n - 64, nn, z);
    let mut w = weak(s, n - 32, y.wrapping_add(K1), x);
    x = x.wrapping_mul(K1).wrapping_add(f64_(s, 0));
    let mut left = (n - 1) & !63;
    let mut i = 0;
    while left != 0 {
        x = rot(
            x.wrapping_add(y)
                .wrapping_add(v.0)
                .wrapping_add(f64_(s, i + 8)),
            37,
        )
        .wrapping_mul(K1);
        y = rot(y.wrapping_add(v.1).wrapping_add(f64_(s, i + 48)), 42).wrapping_mul(K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(f64_(s, i + 40));
        z = rot(z.wrapping_add(w.0), 33).wrapping_mul(K1);
        v = weak(s, i, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak(
            s,
            i + 32,
            z.wrapping_add(w.1),
            y.wrapping_add(f64_(s, i + 16)),
        );
        std::mem::swap(&mut z, &mut x);
        i += 64;
        left -= 64;
    }
    h16(
        h16(v.0, w.0, KMUL)
            .wrapping_add((y ^ (y >> 47)).wrapping_mul(K1))
            .wrapping_add(z),
        h16(v.1, w.1, KMUL).wrapping_add(x),
        KMUL,
    )
}

/// `archive_names(romfs)`: the `ShaderArchive-<library>-<format>` names the RomFS's manifests (`Manifest_*.txt` at its top) list, sorted
pub fn archive_names(manifests: &[Vec<u8>]) -> Vec<String> {
    const PRE: &str = "ShaderArchive-";
    const SUF: &str = ".ushaderbytecode";
    let mut names = std::collections::BTreeSet::new();
    for m in manifests {
        let t = String::from_utf8_lossy(m);
        let mut from = 0;
        while let Some(p) = t[from..].find(PRE) {
            let start = from + p;
            let body = &t[start + PRE.len()..];
            let run: usize = body
                .chars()
                .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '-')
                .map(|c| c.len_utf8())
                .sum();
            if run > 0 && body[run..].starts_with(SUF) {
                names.insert(t[start..start + PRE.len() + run].to_string());
                from = start + PRE.len() + run + SUF.len();
            } else {
                from = start + 1;
            }
        }
    }
    names.into_iter().collect()
}

/// `io_library_name(toc, i, names)`: the archive name whose CityHash64 of `<library>-<format>` (lowercased, UTF-16) is the chunk ID's first 8 bytes, else one named by the ID
pub fn io_library_name(id: &[u8; 12], names: &[String]) -> String {
    for n in names {
        let low = n["ShaderArchive-".len()..].to_lowercase();
        let u: Vec<u8> = low.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        if city64(&u).to_le_bytes() == id[..8] {
            return n.clone();
        }
    }
    format!("ShaderArchive-IoStore-{}", hex8(&id[..8]))
}

pub fn hex8(b: &[u8]) -> String {
    b.iter().map(|c| format!("{c:02x}")).collect()
}

/// `io_archive(toc, i, decode, chunk)`: a shader library as the shaders of a version 2 archive, every one decoded: (frequency, bytes).
/// `chunk(id)` finds a group in every container of the RomFS (newest first); `oodle(what)` is the error of a block or group that needs Oodle.
pub fn io_archive(
    tocs: &[Toc],
    t: usize,
    i: usize,
    chunk: &GroupOwner<'_>,
    oodle: &dyn Fn(&str) -> UeErr,
) -> UeRes<Vec<(u8, Vec<u8>)>> {
    let toc = &tocs[t];
    let lib = toc.read(i, &|| {
        oodle(&format!("{}: shader library chunk {i}", toc.path))
    })?;
    let what = format!("{}: shader library chunk {i}", toc.path);
    let mut r = crate::uepak::R::new(&lib, 0, what.clone());
    if r.u32()? != 1 {
        return val(format!(
            "{}: shader library chunk {i} is not version 1",
            toc.path
        ));
    }
    let tables = match library_tables(&mut r, 20) {
        Ok(t) => t,
        Err(_) => {
            r.seek(4);
            library_tables(&mut r, 8)?
        }
    };
    let (gids, ents, groups, idx) = tables;
    let n = ents.len();
    let mut code: Vec<Option<Vec<u8>>> = vec![None; n];
    for (g, (gid, &(first, cnt, raw, stored))) in gids.iter().zip(groups.iter()).enumerate() {
        let Some((ot, oi)) = chunk(gid) else {
            return val(format!(
                "{}: shader group {g} ({}) is in no container under the RomFS",
                toc.path,
                hex8(gid)
            ));
        };
        let owner = &tocs[ot];
        let d = owner.read(oi, &|| {
            oodle(&format!("{}: chunk {oi} (shader group {g})", owner.path))
        })?;
        if d.len() as u64 != stored as u64 {
            return val(format!(
                "{}: shader group {g} is {} bytes, its entry says {stored}",
                toc.path,
                d.len()
            ));
        }
        if stored < raw {
            return Err(oodle(&format!(
                "{}: shader group {g} ({stored} bytes Oodle-compressed to {raw})",
                toc.path
            )));
        }
        let lo = (first as usize).min(idx.len());
        let hi = (first as usize).saturating_add(cnt as usize).min(idx.len());
        let mut mine: Vec<(u64, usize)> = Vec::new();
        for &s in &idx[lo..hi] {
            let e = *ents
                .get(s as usize)
                .ok_or_else(|| UeErr::Other("IndexError: tuple index out of range".into()))?;
            mine.push((e >> 34, s as usize));
        }
        mine.sort();
        for k in 0..mine.len() {
            let (at, s) = mine[k];
            let nxt = if k + 1 < mine.len() {
                mine[k + 1].0
            } else {
                raw as u64
            };
            if (ents[s] >> 4) & 0x3FFFFFFF != g as u64 {
                return val(format!(
                    "{}: shader {s} is listed in group {g} but names another",
                    toc.path
                ));
            }
            let a = at.min(d.len() as u64) as usize;
            let z = nxt.min(d.len() as u64) as usize;
            code[s] = Some(if a < z { d[a..z].to_vec() } else { Vec::new() });
        }
    }
    let missing = code.iter().filter(|c| c.is_none()).count();
    if missing > 0 {
        return val(format!("{}: {missing} shaders in no group", toc.path));
    }
    Ok(ents
        .iter()
        .zip(code)
        .map(|(e, c)| ((e & 0xF) as u8, c.unwrap()))
        .collect())
}

/// `chunk(id)` of `io_archive`: (container, chunk index) of a chunk ID
pub type GroupOwner<'a> = dyn Fn(&[u8; 12]) -> Option<(usize, usize)> + 'a;

type LibTables = (Vec<[u8; 12]>, Vec<u64>, Vec<(u32, u32, u32, u32)>, Vec<u32>);

/// `_io_library_tables(r, hash_size)`: the group IDs, shader entries, groups and indices (the hashes and shader maps are not needed for the shaders)
fn library_tables(r: &mut crate::uepak::R<'_>, hash_size: i128) -> UeRes<LibTables> {
    let nm = r.u32()? as i128;
    r.raw(hash_size * nm)?;
    let nsh = r.u32()? as i128;
    r.raw(hash_size * nsh)?;
    let ng = r.u32()? as i128;
    r.items(ng, 12)?;
    let gids_at = r.pos() - 12 * ng;
    let nmaps = r.u32()? as i128;
    r.items(nmaps, 8)?;
    let n = r.u32()? as i128;
    let ents_raw = r.raw(8 * n)?;
    let ents: Vec<u64> = ents_raw
        .chunks(8)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let ngr = r.u32()? as i128;
    r.items(ngr, 16)?;
    let groups_at = r.pos() - 16 * ngr;
    let ni = r.u32()? as i128;
    let idx: Vec<u32> = r
        .raw(4 * ni)?
        .chunks(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    if r.pos() != r.len() || n != nsh || ng != ngr {
        return val(format!(
            "{} does not parse with {hash_size}-byte hashes",
            r.what()
        ));
    }
    let b = r.bytes();
    let gids = (0..ng as usize)
        .map(|k| {
            let a = gids_at as usize + 12 * k;
            b[a..a + 12].try_into().unwrap()
        })
        .collect();
    let groups = (0..ngr as usize)
        .map(|k| {
            let a = groups_at as usize + 16 * k;
            (
                u32at(b, a),
                u32at(b, a + 4),
                u32at(b, a + 8),
                u32at(b, a + 12),
            )
        })
        .collect();
    Ok((gids, ents, groups, idx))
}

/// `zlib.decompress(d)`: the fast decoder, and zlib's own texts for a stream it refuses (the exact model, `zlibx`)
fn zlib_whole(d: &[u8]) -> Result<Vec<u8>, String> {
    if let Ok(v) = inflate::zlib_decompress(d) {
        return Ok(v);
    }
    let z = crate::zlibx::decompress(d, None)?;
    if !z.eof {
        return Err("Error -5 while decompressing data: incomplete or truncated stream".into());
    }
    Ok(z.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn city64_known() {
        // the example of uepak.io_library_name: FINAL FANTASY RESONANCE's global library
        let u: Vec<u8> = "global-sf_glsl_switch-sp_glsl_switch"
            .encode_utf16()
            .flat_map(|c| c.to_le_bytes())
            .collect();
        assert_eq!(hex8(&city64(&u).to_le_bytes()), "be76feab0415c156");
    }

    #[test]
    fn mount_orders() {
        assert_eq!(mount_order("a/Game.utoc").0, 0);
        assert_eq!(mount_order("a/Game_10_P.utoc").1, 10);
        assert_eq!(mount_order("a/A_p.utoc"), (1, 0, "a/A_p.utoc".into()));
        assert_eq!(mount_order("a/A2_P.utoc").1, 0);
        assert_eq!(mount_order("a/A_1_2_P.utoc").1, 2);
    }

    #[test]
    fn manifest_names() {
        let m = b"x ShaderArchive-Global-SF_GLSL_SWITCH.ushaderbytecode\tShaderArchive-.ushaderbytecode ShaderArchive-A-B.ushaderbytecodeZ".to_vec();
        assert_eq!(
            archive_names(&[m]),
            ["ShaderArchive-A-B", "ShaderArchive-Global-SF_GLSL_SWITCH"]
        );
    }
}
