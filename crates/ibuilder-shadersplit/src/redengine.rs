//! REDengine 3's shader caches (The Witcher 3: Wild Hunt, nxnative a4c82979): `shadernx.cache` (`rdhs_shaders`: records of one zstd frame each, ended by an index and an `RDHS` footer) and
//! `staticshadernx.cache` (`fzlc_shaders`: an `FZLC` header and a table of chunks whose joined zstd frames hold the GLSLC outputs). Read through the file's source, a record or a chunk at a time.

use crate::bytes::*;
use crate::error::{Failure, Res};
use crate::glslc::glslc_shaders;
use crate::red;
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use crate::zstd::ZSTD_MAGIC;

pub const RDHS: &[u8] = b"RDHS";
pub const FZLC: &[u8] = b"FZLC";

fn io(e: std::io::Error) -> Failure {
    Failure::Red(format!("OSError: {e}"))
}

/// The branch of `scan_file_in` that tests for a material cache: byte 8 is the first record's flag (1), or the file has the engine's name; the footer must be there either way
pub fn is_rdhs(src: &dyn Source, head: &[u8], rel: &str) -> Res<bool> {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    if !(sl(head, 8, 9) == [1] || base == "shadernx.cache") {
        return Ok(false);
    }
    let size = src.len();
    if size < 16 {
        return Ok(false);
    }
    Ok(src.read(size - 8, 4).map_err(io)? == RDHS)
}

/// `rdhs_shaders(path, label)`: from +0, records {u64 hash, u8 1, u32 raw length, u32 stored length, one zstd frame of the stored length}, each frame one whole GLSLC output, up to the
/// index the 16-byte footer {u64 index offset, 'RDHS', u32 version 3} names; the records must end exactly at the index
pub fn rdhs_shaders(cx: &Ctx<'_>, src: &dyn Source, label: &str) -> Res<Vec<Shader>> {
    let size = src.len();
    let ft = src.read(size - 16, 16).map_err(io)?;
    let (idx, ver) = (u64le(&ft, 0)?, u32le(&ft, 12)?);
    let magic = &ft[8..12];
    if magic != RDHS || ver != 3 || idx > size - 16 {
        red!(
            "{label}: footer {} version {ver}, index {idx:#x}: not a version 3 RDHS shader cache",
            crate::casc::py_bytes_repr(magic)
        );
    }
    let mut out = Vec::new();
    let (mut pos, mut n) = (0u64, 0usize);
    while pos < idx {
        let wh = format!("{label}: record {n} at {pos:#x}");
        if pos + 17 > idx {
            red!("{wh}: header runs past the index at {idx:#x}");
        }
        let h = src.read(pos + 8, 9).map_err(io)?;
        let (flag, raw, stored) = (h[0], u32le(&h, 1)? as u64, u32le(&h, 5)? as u64);
        let (start, end) = (pos + 17, pos + 17 + stored);
        let frame = if stored >= 4 && end <= idx {
            src.read(start, stored as usize).map_err(io)?
        } else {
            Vec::new()
        };
        if flag != 1 || stored < 4 || end > idx || !starts(&frame, &ZSTD_MAGIC) {
            red!("{wh}: flag {flag}, stored {stored}, not a zstd frame inside the records");
        }
        let blob = match cx.zstd.decompress(&frame, None, None) {
            Ok(b) => b,
            Err(e) => red!("{wh}: invalid zstd frame ({e})"),
        };
        if blob.len() as u64 != raw {
            red!(
                "{wh} decompresses to {} bytes, its header says {raw}",
                blob.len()
            );
        }
        let got = glslc_shaders(&blob, &format!("{label}@{pos:#x}"), false, &[])?;
        if got.is_empty() {
            red!("{wh} holds no whole GLSLC output");
        }
        out.extend(got);
        pos = end;
        n += 1;
    }
    Ok(out)
}

/// `fzlc_shaders(path, label)`: header {'FZLC', u32 chunk count, u32 first chunk's offset}, then per chunk {u32 stored length, u32 raw length, u32 next chunk's offset}; each chunk is one
/// zstd frame, or kept as it is when its two lengths are equal (the DLC stubs). The GLSLC outputs run across chunk boundaries, so the chunks are joined before they are read
pub fn fzlc_shaders(cx: &Ctx<'_>, src: &dyn Source, label: &str) -> Res<Vec<Shader>> {
    let len = src.len();
    let h = src.read(0, 12).map_err(io)?;
    field_ok(&h, 4, 8, label).map_err(|_| {
        Failure::Red(format!(
            "{label}: 0x8 bytes at 0x4 lie outside the {len:#x} bytes there are"
        ))
    })?;
    let (count, mut at) = (u32le(&h, 4)? as u64, u32le(&h, 8)? as u64);
    if 12 + 12 * count > at || at > len {
        red!("{label}: {count} chunks with data at {at:#x} in {len} bytes");
    }
    let table = src.read(12, (12 * count) as usize).map_err(io)?;
    let mut joined: Vec<u8> = Vec::new();
    for i in 0..count as usize {
        let (stored, raw, nxt) = (
            u32le(&table, 12 * i)? as u64,
            u32le(&table, 12 * i + 4)? as u64,
            u32le(&table, 12 * i + 8)? as u64,
        );
        let wh = format!("{label}: chunk {i} at {at:#x}");
        if stored == 0 {
            if raw != 0 {
                red!("{wh}: {raw} decoded bytes but no stored data");
            }
            continue;
        }
        let end = at + stored;
        if end > len || (i + 1 < count as usize && nxt != end) {
            red!("{wh}: stored {stored}, next {nxt:#x}: inconsistent");
        }
        let mut p = src.read(at, stored as usize).map_err(io)?;
        if stored != raw {
            if !starts(&p, &ZSTD_MAGIC) {
                red!("{wh}: compressed data has no zstd frame header");
            }
            p = match cx.zstd.decompress(&p, None, None) {
                Ok(b) => b,
                Err(e) => red!("{wh}: invalid zstd frame ({e})"),
            };
        }
        if p.len() as u64 != raw {
            red!(
                "{wh} decompresses to {} bytes, its entry says {raw}",
                p.len()
            );
        }
        joined.extend_from_slice(&p);
        at = end;
    }
    glslc_shaders(&joined, label, false, &[])
}
