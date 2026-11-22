//! NSO modules: the build id, the decompressed segments, writes into them, and the NSO written back (stored plain).

use crate::error::{ModsError, Result};
use crate::lz4::decompress_safe;
use crate::patch::{Patch, bid_matches};
use crate::{NSO_HEADER, NSO_ZSTD};
use sha2::{Digest, Sha256};

/// the largest decompressed segment accepted (`ibuilder-profile`'s `SIZE_MAX`)
const SEGMENT_MAX: u64 = 0x7FFF_FFFF;

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn put_u32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// `nso_build_id`: the 0x20-byte build id of an NSO's header, or `None` for a file that is not an NSO.
pub fn nso_build_id(data: &[u8]) -> Option<&[u8]> {
    if data.len() >= NSO_HEADER && &data[..4] == b"NSO0" {
        Some(&data[0x40..0x60])
    } else {
        None
    }
}

/// `lz4_block` (the `lz4` package branch): a stream the format rejects is an `LZ4BlockError` in Python, which `mods.py` does not catch.
fn lz4_block(raw: &[u8], size: usize) -> Result<Vec<u8>> {
    decompress_safe(raw, size).map_err(|_| {
        ModsError::unhandled(
            "LZ4BlockError",
            "Decompression failed: corrupt input or insufficient space in destination buffer",
        )
    })
}

/// `zbic.decompress(bytes(raw))`: the segment through `ibuilder_nso::zbic`. The package raises `zbic.error` ("ZBIC data is invalid") on a stream it rejects, which `mods.py`
/// does not catch (only the `ImportError` of a missing package becomes a `ModError`); the size check against the header is the caller's, as in Python.
fn zbic_segment(raw: &[u8], size: usize) -> Result<Vec<u8>> {
    // `size` is the header's word: the buffer is reserved only for what this input can produce. A block is at most 128 KiB of output and costs at
    // least 4 input bytes (a 3-byte header and one RLE byte), so no valid stream expands by more than 32768; a stream that would need more is
    // refused by the decoder as too long for its buffer (it was a size mismatch against the header either way).
    let cap = size.min(raw.len().saturating_mul(32768).saturating_add(128 << 10));
    let mut out = vec![0u8; cap];
    match ibuilder_nso::zbic::decompress_into(raw, &mut out) {
        Ok(n) => {
            out.truncate(n);
            Ok(out)
        }
        Err(_) => Err(ModsError::unhandled("error", "ZBIC data is invalid")),
    }
}

/// A decompressed segment: `(memory offset, bytes)`.
pub type Segment = (u32, Vec<u8>);

/// `nso_image`: the header and `[(memory offset, bytes)]` of text, ro, data with every segment decompressed.
pub fn nso_image(data: &[u8]) -> Result<(Vec<u8>, Vec<Segment>)> {
    if data.len() < NSO_HEADER || &data[..4] != b"NSO0" {
        return Err(ModsError::modl("not an NSO"));
    }
    let flags = u32_at(data, 0xC);
    let mut segs = Vec::new();
    for i in 0..3usize {
        let fo = u32_at(data, 0x10 + 0x10 * i) as usize;
        let mo = u32_at(data, 0x14 + 0x10 * i);
        let sz = u32_at(data, 0x18 + 0x10 * i) as usize;
        let compressed = flags >> i & 1 != 0;
        let c = if compressed {
            u32_at(data, 0x60 + 4 * i) as usize
        } else {
            sz
        };
        let raw = &data[fo.min(data.len())..(fo + c).min(data.len())];
        if raw.len() != c {
            return Err(ModsError::modl("the NSO is cut short"));
        }
        // the decompressed size is the header's word, up to 0xFFFFFFFF, and becomes the output buffer: bounded first, as ibuilder-profile's reader
        // does (Horizon loads no segment above 2 GiB). The Python decodes first and finds the size wrong afterwards (a ModError too).
        if compressed && sz as u64 > SEGMENT_MAX {
            return Err(ModsError::modl(format!(
                "a segment claims {sz:#x} bytes, more than Horizon loads"
            )));
        }
        let raw: Vec<u8> = if compressed {
            if flags & NSO_ZSTD != 0 {
                zbic_segment(raw, sz)?
            } else {
                lz4_block(raw, sz)?
            }
        } else {
            raw.to_vec()
        };
        if raw.len() != sz {
            return Err(ModsError::modl(format!(
                "a segment is {} bytes, not the {} of the header",
                raw.len(),
                sz
            )));
        }
        segs.push((mo, raw));
    }
    Ok((data[..NSO_HEADER].to_vec(), segs))
}

/// `apply_writes`: write `[(module offset, bytes)]` into the segments. Returns `(segments, applied, outside)`: a write that lies in the header (offset < 0),
/// in bss or in a gap between segments is counted in `outside` and not made, as the part of a write that leaves a segment is.
pub fn apply_writes(
    segs: Vec<(u32, Vec<u8>)>,
    writes: &[(i128, Vec<u8>)],
) -> (Vec<(u32, Vec<u8>)>, usize, usize) {
    let mut segs = segs;
    let (mut applied, mut outside) = (0usize, 0usize);
    for (off, data) in writes {
        let mut covered = 0usize;
        for (mo, buf) in segs.iter_mut() {
            let mo = *mo as i128;
            let lo = (*off).max(mo);
            let hi = (*off + data.len() as i128).min(mo + buf.len() as i128);
            if lo < hi {
                let (a, b, n) = ((lo - mo) as usize, (lo - *off) as usize, (hi - lo) as usize);
                buf[a..a + n].copy_from_slice(&data[b..b + n]);
                covered += n;
            }
        }
        if covered > 0 {
            applied += 1;
        }
        if covered < data.len() {
            outside += 1;
        }
    }
    (segs, applied, outside)
}

/// `build_nso`: an NSO of the header and the segments, stored uncompressed: the compression and check-hash flags are cleared (the loader does not check a
/// module whose flags are cleared, as on a Switch), so the patched bytes are what is read.
pub fn build_nso(header: &[u8], segs: &[(u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let total: usize = segs.iter().map(|(_, b)| b.len()).sum();
    // one buffer: the header first (its fields are filled in as the segments are appended), then the segments
    let mut h = Vec::with_capacity(NSO_HEADER + total);
    h.extend_from_slice(header);
    let flags = u32_at(&h, 0xC) & !0xBF;
    put_u32(&mut h, 0xC, flags);
    let mut at = NSO_HEADER;
    for (i, (mo, b)) in segs.iter().enumerate() {
        let fits = |v: usize| {
            u32::try_from(v).map_err(|_| ModsError::unhandled("error", "argument out of range"))
        };
        put_u32(&mut h, 0x10 + 0x10 * i, fits(at)?);
        put_u32(&mut h, 0x14 + 0x10 * i, *mo);
        put_u32(&mut h, 0x18 + 0x10 * i, fits(b.len())?);
        put_u32(&mut h, 0x60 + 4 * i, fits(b.len())?);
        h[0xA0 + 0x20 * i..0xC0 + 0x20 * i].copy_from_slice(&Sha256::digest(b));
        h.extend_from_slice(b);
        at += b.len();
    }
    Ok(h)
}

/// What `patch_nso` returns besides the new bytes.
pub struct Patched<'a> {
    pub data: Vec<u8>,
    /// the patches that name this module's build id and are on
    pub applied: Vec<&'a Patch>,
    /// `(patch, writes that fall outside the segments)`
    pub notes: Vec<(&'a Patch, usize)>,
}

/// `patch_nso`: the NSO with the patches that name its build id applied; `None` when none matched. (With a file that is not an NSO and an enabled patch,
/// the Python calls `.hex()` on `None`: an `AttributeError` it does not catch.)
pub fn patch_nso<'a>(data: &[u8], patches: &'a [Patch]) -> Result<Option<Patched<'a>>> {
    let bid = nso_build_id(data);
    let mut mine = Vec::new();
    for p in patches {
        if !p.on {
            continue;
        }
        let Some(bid) = bid else {
            return Err(ModsError::unhandled(
                "AttributeError",
                "'NoneType' object has no attribute 'hex'",
            ));
        };
        if bid_matches(&p.bid, bid) {
            mine.push(p);
        }
    }
    if mine.is_empty() {
        return Ok(None);
    }
    let (header, mut segs) = nso_image(data)?;
    let mut notes = Vec::new();
    for p in &mine {
        let (s, _n, outside) = apply_writes(segs, &p.writes);
        segs = s;
        if outside > 0 {
            notes.push((*p, outside));
        }
    }
    Ok(Some(Patched {
        data: build_nso(&header, &segs)?,
        applied: mine,
        notes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header that claims a huge segment for a few input bytes must be refused, and the buffer reserved for it must follow the input, not the claim.
    #[test]
    fn zbic_claim_does_not_set_the_allocation() {
        let r = zbic_segment(&[1, 2, 3], 0x7FFF_FFFF);
        assert!(r.is_err());
        // the cap in use: 3 bytes -> 3 * 32768 + 128 KiB, far below the claimed 2 GiB
        assert!(3usize.saturating_mul(32768).saturating_add(128 << 10) < 0x7FFF_FFFF);
    }
}
