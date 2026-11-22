//! Vector Unit's APF asset packages (Beach Buggy Racing 2: Island Adventure, `0100C43012344000`): `apf_entries`, `apf_decompress` and `apf_shaders` of `shader_split.py`, and its
//! `FPUV` branch of `scan_file_in`.
//!
//! One `Assets.apf` (343 MB, 7,749 entries). Little-endian header of 0x3c bytes: `FPUV`, u32 version (7), u32 directory offset, u32 entry count, u32 directory length, u32 FNV-1a of the
//! directory, the platform's name (`Switch`, 32 bytes NUL-padded), u32 FNV-1a of the 0x38 bytes before it. The entries' data follows, the directory comes last; per entry the asset's
//! name (`<asset type>/<asset path>`, NUL-terminated), u32 offset, u32 length, u32 stored length, u32 FNV-1a of the decoded data, u16 the asset type's version, u16 compression
//! (0 stored, 1 zlib, 5 one LZ4 block). `Assets/AssetData` (zlib) lists every asset by type, without offsets.
//! The game's shaders are its 233 `VuShaderAsset` entries (LZ4): a small header, then whole GLSLC outputs of a vertex and a pixel program each. No asset type is relied on: every entry
//! is decoded and scanned like a file (`scan_blob`, then whole GLSLC outputs), one at a time, and the package is never read whole.

use crate::bytes::{contains, u16le, u32le};
use crate::ctl::GLSLC_MAGIC;
use crate::error::{Failure, Res};
use crate::glslc::glslc_shaders;
use crate::inflate;
use crate::red;
use crate::scan::scan_blob;
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use std::collections::BTreeMap;

pub const MAGIC: &[u8; 4] = b"FPUV";
const HEADER: usize = 0x3c;
/// a NUL and the five fields after an entry's name
const FIELDS: usize = 21;

/// FNV-1a, 32 bits: what an APF package checks its header, directory and entries with
pub fn fnv1a32(d: &[u8]) -> u32 {
    d.iter().fold(0x811c_9dc5u32, |h, &c| {
        (h ^ c as u32).wrapping_mul(0x0100_0193)
    })
}

/// One directory entry
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub offset: u64,
    /// decoded length
    pub size: usize,
    /// bytes it takes in the package
    pub stored: usize,
    pub compression: u16,
}

/// `apf_decompress(raw, usz, comp, where)`: compression 0 = stored, 1 = zlib, 5 = one LZ4 block; `None` for another compression (counted in one `[WARN]` per package)
pub fn apf_decompress(raw: Vec<u8>, usz: usize, comp: u16, wh: &str) -> Res<Option<Vec<u8>>> {
    let m = match comp {
        0 => {
            if raw.len() != usz {
                red!(
                    "{wh}: {:#x} stored bytes, the entry says {usz:#x}",
                    raw.len()
                );
            }
            return Ok(Some(raw));
        }
        1 => inflate::zlib_decompress(&raw)
            .map_err(|e| Failure::Red(format!("{wh}: invalid zlib stream ({e})")))?,
        5 => {
            if usz as u128 > 255 * raw.len() as u128 {
                red!(
                    "{wh}: {usz:#x} bytes from {:#x} stored is more than an LZ4 block can hold (255 to 1)",
                    raw.len()
                );
            }
            if usz > inflate::MAX_DECODED {
                red!("{wh}: {}", inflate::cap_error(inflate::MAX_DECODED));
            }
            let mut out = vec![0u8; usz];
            match lz4_flex::block::decompress_into(&raw, &mut out) {
                Ok(k) => out.truncate(k),
                Err(e) => red!(
                    "{wh}: its {:#x} stored bytes are not an LZ4 block (Decompression failed: corrupt input or insufficient space in destination buffer ({e}))",
                    raw.len()
                ),
            }
            out
        }
        _ => return Ok(None),
    };
    if m.len() != usz {
        red!(
            "{wh}: {:#x} bytes decoded, the entry says {usz:#x}",
            m.len()
        );
    }
    Ok(Some(m))
}

/// `apf_entries(f, size, label)`: the directory, every entry checked to lie in the data before it
pub fn apf_entries(src: &dyn Source, label: &str) -> Res<Vec<Entry>> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let size = src.len();
    let h = src.read(0, HEADER).map_err(io)?;
    if h.len() < HEADER {
        red!(
            "{label}: {} bytes, shorter than a Vector Unit APF header ({HEADER:#x})",
            h.len()
        );
    }
    let (ver, at, n, dlen, dsum) = (
        u32le(&h, 4)?,
        u32le(&h, 8)? as u64,
        u32le(&h, 0xc)? as usize,
        u32le(&h, 0x10)? as u64,
        u32le(&h, 0x14)?,
    );
    if ver != 7 {
        red!("{label}: Vector Unit APF version {ver} (only version 7 is read)");
    }
    if fnv1a32(&h[..0x38]) != u32le(&h, 0x38)? {
        red!("{label}: the APF header does not match its FNV-1a sum");
    }
    if at < HEADER as u64 || at > size || dlen > size - at {
        red!(
            "{label}: the APF directory ({dlen:#x} bytes at {at:#x}) lies outside the file ({size:#x} bytes)"
        );
    }
    if n as u64 > dlen / FIELDS as u64 {
        red!("{label}: the APF directory lists {n} entries, more than fit in its {dlen:#x} bytes");
    }
    let t = src.read(at, dlen as usize).map_err(io)?;
    if t.len() as u64 != dlen || fnv1a32(&t) != dsum {
        red!("{label}: the APF directory does not match its FNV-1a sum");
    }
    let mut p = 0usize;
    let mut es = Vec::with_capacity(n);
    for i in 0..n {
        let e = match memchr::memchr(0, &t[p..]) {
            Some(k) if p + k + FIELDS <= t.len() => p + k,
            _ => red!("{label}: entry {i} of {n} runs past the APF directory ({dlen:#x} bytes)"),
        };
        let name = String::from_utf8_lossy(&t[p..e]).into_owned();
        let (off, usz, csz, comp) = (
            u32le(&t, e + 1)? as u64,
            u32le(&t, e + 5)? as usize,
            u32le(&t, e + 9)? as u64,
            u16le(&t, e + 19)?,
        );
        if off < HEADER as u64 || off > at || csz > at - off {
            red!(
                "{label}/{name}: its {csz:#x} stored bytes at {off:#x} lie outside the APF data ({HEADER:#x}..{at:#x})"
            );
        }
        es.push(Entry {
            name,
            offset: off,
            size: usz,
            stored: csz as usize,
            compression: comp,
        });
        p = e + FIELDS;
    }
    if p != t.len() {
        red!(
            "{label}: {} bytes after the {n} entries of the APF directory",
            t.len() - p
        );
    }
    Ok(es)
}

/// `apf_shaders(path, rel, out)`: every entry of the package decoded and scanned like a file, in the directory's order
pub fn apf_shaders(cx: &Ctx<'_>, src: &dyn Source, rel: &str, out: &mut Vec<Shader>) -> Res<()> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let label = format!("/{rel}");
    let mut other: BTreeMap<u16, usize> = BTreeMap::new();
    for e in apf_entries(src, &label)? {
        if e.size == 0 {
            continue;
        }
        let wh = format!("{label}/{}", e.name);
        let raw = src.read(e.offset, e.stored).map_err(io)?;
        if raw.len() != e.stored {
            red!(
                "{wh}: short read ({:#x} of {:#x} bytes)",
                raw.len(),
                e.stored
            );
        }
        let m = match apf_decompress(raw, e.size, e.compression, &wh)? {
            Some(m) => m,
            None => {
                *other.entry(e.compression).or_default() += 1;
                continue;
            }
        };
        let mut found = Vec::new();
        scan_blob(cx, &m, &wh, &mut found, 0, None)?;
        if found.is_empty() && contains(&m, &GLSLC_MAGIC) {
            found = glslc_shaders(&m, &wh, false, &[])?;
        }
        out.extend(found);
    }
    if !other.is_empty() {
        let list: Vec<String> = other.iter().map(|(c, k)| format!("{c} × {k}")).collect();
        cx.print(&format!(
            "[WARN] {label}: {} entries not read, their compression ({}) is not one this reader knows (0 = stored, 1 = zlib, 5 = LZ4)",
            other.values().sum::<usize>(),
            list.join(", ")
        ));
    }
    Ok(())
}
