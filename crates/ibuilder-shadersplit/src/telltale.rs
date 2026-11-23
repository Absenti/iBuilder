//! Telltale's `.ttarch2` archives (Sam & Max: The Devil's Playhouse, nxnative c714bb92): `ttcz_chunks`, `ttcz_inflate`, `ttarch2_shaders`. An 'NCTT' archive holds its entries one after
//! the other, each a 'ZCTT' block; a 'ZCTT' archive is one block. A block is a table of raw deflate chunks (64 KB each decoded). Only an archive whose entry table names a `.t3fxpack`
//! is inflated: its effect pack holds GLSLC shaders as bare records, [code area] [u32 control length] [control section], neither in a BNSH nor aligned.

use crate::bytes::*;
use crate::ctl::{CTL_MAGIC, check, glslc_stage};
use crate::error::{Failure, Res};
use crate::inflate;
use crate::scan_types::Shader;

pub const TTCZ: &[u8] = b"ZCTT";
pub const TTCN: &[u8] = b"NCTT";
const ZPK: &[u8] = b".t3fxpack";

/// `ttcz_chunks(d, at)`: the n + 1 chunk offsets of the 'ZCTT' block at `at` (from the block's start; the first is the header's own length), or None when the bytes there are not one
pub fn ttcz_chunks(d: &[u8], p: usize) -> Option<Vec<u64>> {
    if at(d, p, 4) != TTCZ || at(d, p + 4, 4) != b"\0\0\x01\0" || p + 12 > d.len() {
        return None;
    }
    let n = u32le(d, p + 8).ok()? as u64;
    if !(0 < n && n < 1 << 20) || p as u64 + 12 + 8 * (n + 1) > d.len() as u64 {
        return None;
    }
    let offs: Vec<u64> = (0..=n as usize)
        .map(|i| u64le(d, p + 12 + 8 * i).unwrap_or(0))
        .collect();
    if offs[0] != 12 + 8 * (n + 1)
        || offs.windows(2).any(|w| w[0] >= w[1])
        || p as u64 + offs[offs.len() - 1] > d.len() as u64
    {
        return None;
    }
    Some(offs)
}

/// `zlib.decompress(chunk, -15)`: one whole raw deflate stream (the bytes after it are ignored); a damaged or cut one is the `zlib.error` the Python raises, in zlib's own words
fn inflate_chunk(c: &[u8]) -> Res<Vec<u8>> {
    match inflate::run_as_zlib(c, inflate::Wrap::Raw, None) {
        Ok(r) if r.eof => Ok(r.out),
        Ok(_) => Err(Failure::Red(
            "error: Error -5 while decompressing data: incomplete or truncated stream".into(),
        )),
        Err(e) if e.starts_with("Error ") => Err(Failure::Red(format!("error: {e}"))),
        Err(e) => Err(Failure::Red(e)),
    }
}

/// `ttcz_inflate(d, at, offs, first)`: the block's chunks decoded and joined (only the first `first` of them when given)
pub fn ttcz_inflate(d: &[u8], at: usize, offs: &[u64], first: Option<usize>) -> Res<Vec<u8>> {
    let k = match first {
        None => offs.len() - 1,
        Some(f) => f.min(offs.len() - 1),
    };
    let mut out = Vec::new();
    for i in 0..k {
        out.extend(inflate_chunk(
            &d[at + offs[i] as usize..at + offs[i + 1] as usize],
        )?);
    }
    Ok(out)
}

/// `ttarch2_shaders(d, label)`
pub fn ttarch2_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut blocks: Vec<(usize, Vec<u64>)> = Vec::new();
    let ttcn = starts(d, TTCN);
    if starts(d, TTCZ) {
        if let Some(offs) = ttcz_chunks(d, 0) {
            blocks.push((0, offs));
        }
    } else if ttcn {
        let mut a = 0usize;
        while let Some(p) = find(d, b"ZCTT\0\0\x01\0", a) {
            match ttcz_chunks(d, p) {
                Some(offs) => {
                    a = p + offs[offs.len() - 1] as usize;
                    blocks.push((p, offs));
                }
                None => a = p + 4,
            }
        }
    }
    let mut out = Vec::new();
    let head: Vec<u8> = if ttcn {
        sl(d, 0, 0x400).to_vec()
    } else if let Some((a, o)) = blocks.first() {
        ttcz_inflate(d, *a, o, Some(1))?
    } else {
        Vec::new()
    };
    if !contains(&head, ZPK) {
        let named = match (ttcn, blocks.first()) {
            (true, Some((a, o))) => contains(&ttcz_inflate(d, *a, o, Some(1))?, ZPK),
            _ => false,
        };
        if !named {
            return Ok(out);
        }
    }
    for (a, offs) in &blocks {
        let data = ttcz_inflate(d, *a, offs, None)?;
        if ttcn && !contains(&data, &CTL_MAGIC) {
            continue;
        }
        for c in find_all(&data, &CTL_MAGIC) {
            if c < 8 || c + 0x708 > data.len() {
                continue;
            }
            let (nctl, n) = (
                u32le(&data, c + 0x20)? as usize,
                u32le(&data, c + 0x704)? as usize,
            );
            if u32le(&data, c - 4)? as usize != nctl || c - 4 < n || c + nctl > data.len() {
                continue;
            }
            let (ctl, code) = (&data[c..c + nctl], &data[c - 4 - n..c - 4]);
            if let Some(st) = glslc_stage(ctl, code)?
                && check(st, ctl, code)?.is_none()
            {
                out.push(Shader::new(
                    format!("{label}#TTFX@{a:#x}+{c:#x}"),
                    st,
                    ctl,
                    code,
                ));
            }
        }
    }
    Ok(out)
}

/// `ttcz_chunks` over a source (the gate below reads only the headers and one chunk)
fn ttcz_chunks_src(src: &dyn crate::source::Source, p: u64) -> std::io::Result<Option<Vec<u64>>> {
    let len = src.len();
    let h = src.read(p, 12)?;
    if h.len() < 12 || &h[..4] != TTCZ || &h[4..8] != b"\0\0\x01\0" {
        return Ok(None);
    }
    let n = u32::from_le_bytes(h[8..12].try_into().unwrap()) as u64;
    if !(0 < n && n < 1 << 20) || p + 12 + 8 * (n + 1) > len {
        return Ok(None);
    }
    let t = src.read(p + 12, (8 * (n + 1)) as usize)?;
    let offs: Vec<u64> = (0..=n as usize)
        .map(|i| u64::from_le_bytes(t[8 * i..8 * i + 8].try_into().unwrap()))
        .collect();
    if offs[0] != 12 + 8 * (n + 1)
        || offs.windows(2).any(|w| w[0] >= w[1])
        || p + offs[offs.len() - 1] > len
    {
        return Ok(None);
    }
    Ok(Some(offs))
}

/// Does `ttarch2_shaders` inflate this archive at all (its first 0x400 bytes, or the first chunk of its first block, name a `.t3fxpack`)? Asked of the source first, so an archive of
/// textures or meshes is never read whole. The first chunk's damage is the error the Python gives for it.
pub fn ttarch2_named(src: &dyn crate::source::Source) -> Res<bool> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let h4 = src.read(0, 4).map_err(io)?;
    let ttcn = h4 == TTCN;
    if ttcn && contains(&src.read(0, 0x400).map_err(io)?, ZPK) {
        return Ok(true);
    }
    let first = if h4 == TTCZ {
        ttcz_chunks_src(src, 0).map_err(io)?.map(|o| (0u64, o))
    } else if ttcn {
        let mut a = 0u64;
        let mut got = None;
        while let Some(p) = crate::source::find_from(src, b"ZCTT\0\0\x01\0", a).map_err(io)? {
            match ttcz_chunks_src(src, p).map_err(io)? {
                Some(o) => {
                    got = Some((p, o));
                    break;
                }
                None => a = p + 4,
            }
        }
        got
    } else {
        None
    };
    let Some((p, offs)) = first else {
        return Ok(false);
    };
    let c = src
        .read(p + offs[0], (offs[1] - offs[0]) as usize)
        .map_err(io)?;
    Ok(contains(&inflate_chunk(&c)?, ZPK))
}
