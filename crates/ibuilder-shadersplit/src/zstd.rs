//! zstd, behind one small trait.
//!
//! The phone gives the Python split a callable `zstd(data, dict_bytes_or_None, max_length) -> bytes` (the app's own zstd); the Rust split takes the same shape as a trait, so
//! the app can pass a callback and the Mac CLI the pure-Rust default ([`Ruzstd`], the `ruzstd` crate: dictionaries included, no C, builds for iOS).
//! No file of Grizzy is zstd; the games that are (Tears of the Kingdom's `.zs` with their dictionaries, Super Mario Bros. Wonder, Kirby, ...) are what it is for.

use std::io::Read;

pub trait Zstd: Send + Sync {
    /// All the frames of `data` (concatenated frames give their concatenated output), decoded with `dict` (a zstd dictionary file) when given.
    /// `max_length`: stop after this many output bytes (the Python's `ZstdDecompressor.decompress(data, max_length)`). `Err` carries the reason (`ZstdError`).
    /// With no `max_length` the pure-Rust decoder refuses output past [`crate::inflate::MAX_DECODED`] (review r2, 4.1); another implementation should do the same.
    fn decompress(
        &self,
        data: &[u8],
        dict: Option<&[u8]>,
        max_length: Option<usize>,
    ) -> Result<Vec<u8>, String>;
}

/// The pure-Rust decoder
#[derive(Default, Clone, Copy)]
pub struct Ruzstd;

impl Zstd for Ruzstd {
    fn decompress(
        &self,
        data: &[u8],
        dict: Option<&[u8]>,
        max_length: Option<usize>,
    ) -> Result<Vec<u8>, String> {
        self.decompress_capped(data, dict, max_length, crate::inflate::MAX_DECODED)
    }
}

impl Ruzstd {
    /// [`Zstd::decompress`] with the cap on an unlimited decode as a parameter (the tests use a small one). A frame of RLE blocks is 32,768:1: 41 KB of input decoded to 1.25 GiB (2.2 GB RSS) before the cap.
    pub fn decompress_capped(
        &self,
        data: &[u8],
        dict: Option<&[u8]>,
        max_length: Option<usize>,
        cap: usize,
    ) -> Result<Vec<u8>, String> {
        use ruzstd::decoding::{Dictionary, FrameDecoder, StreamingDecoder};
        let decoder = || -> Result<FrameDecoder, String> {
            let mut fd = FrameDecoder::new();
            if let Some(raw) = dict {
                if raw.len() < 8 {
                    return Err("zstd dictionary is shorter than its header".into());
                }
                let d =
                    Dictionary::decode_dict(raw).map_err(|e| format!("zstd dictionary: {e}"))?;
                fd.add_dict(d)
                    .map_err(|e| format!("zstd dictionary: {e}"))?;
            }
            Ok(fd)
        };
        let mut out = Vec::new();
        if let Some(n) = max_length {
            // `ZstdDecompressor().decompress(data, max_length)`: one frame
            let mut sd = StreamingDecoder::new_with_decoder(data, decoder()?)
                .map_err(|e| format!("zstd: {e}"))?;
            (&mut sd)
                .take(n as u64)
                .read_to_end(&mut out)
                .map_err(|e| format!("zstd: {e}"))?;
            return Ok(out);
        }
        // `zstd.decompress(data)`: every frame, one after the other (6aaf73ba: the files the zstd command decoded are decoded in process, concatenated frames included); a skippable frame
        // gives nothing, anything else after a frame is an error
        let mut rest: &[u8] = data;
        loop {
            if rest.len() >= 8 && rest[0] & 0xf0 == 0x50 && rest[1..4] == [0x2a, 0x4d, 0x18] {
                let n = u32::from_le_bytes(rest[4..8].try_into().unwrap()) as usize;
                if rest.len() - 8 < n {
                    return Err("zstd: a skippable frame runs past the data".into());
                }
                rest = &rest[8 + n..];
            } else {
                let mut sd = StreamingDecoder::new_with_decoder(&mut rest, decoder()?)
                    .map_err(|e| format!("zstd: {e}"))?;
                // one byte more than the cap is read, to tell "fits exactly" from "passes it"
                let room = (cap as u64 + 1).saturating_sub(out.len() as u64);
                (&mut sd)
                    .take(room)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("zstd: {e}"))?;
                if out.len() > cap {
                    return Err(crate::inflate::cap_error(cap));
                }
            }
            if rest.is_empty() {
                break;
            }
        }
        Ok(out)
    }
}

/// `zs_dict_id(d)`: the zstd dictionary a frame names (0: none)
pub fn dict_id(d: &[u8]) -> u32 {
    let fhd = if d.len() > 5 && d[..4] == ZSTD_MAGIC {
        d[4]
    } else {
        0
    };
    let n = [0usize, 1, 2, 4][(fhd & 3) as usize];
    let o = 6 - ((fhd >> 5) & 1) as usize;
    let mut v = 0u32;
    for i in 0..n {
        v |= (*d.get(o + i).unwrap_or(&0) as u32) << (8 * i);
    }
    v
}

pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
pub const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4d, 0x18];

/// `frame_size(f)`: the uncompressed size a zstd or LZ4 frame header records, None when it records none
pub fn frame_size(f: &[u8]) -> Option<u64> {
    if f.len() >= 4 && f[..4] == LZ4_FRAME_MAGIC {
        // FLG, BD, then the 8-byte content size when FLG bit 3 is set
        return if f.len() > 14 && f[4] & 8 != 0 {
            Some(u64::from_le_bytes(f[6..14].try_into().unwrap()))
        } else {
            None
        };
    }
    let fhd = if f.len() > 5 { f[4] } else { 0 };
    let single = ((fhd >> 5) & 1) as usize;
    let n = [single, 2, 4, 8][(fhd >> 6) as usize];
    if n == 0 {
        return None;
    }
    let o = 6 - single + [0usize, 1, 2, 4][(fhd & 3) as usize];
    let mut v = 0u64;
    for i in 0..n {
        v |= (*f.get(o + i).unwrap_or(&0) as u64) << (8 * i);
    }
    Some(v + if n == 2 { 256 } else { 0 })
}

/// `zstd_frame_len(d, p)`: stored length of the zstd frame at p, from its header and block headers (RFC 8878). None when the headers run past the data.
pub fn frame_len(d: &[u8], p: usize) -> Option<usize> {
    let fhd = *d.get(p + 4)?;
    let single = ((fhd >> 5) & 1) as usize;
    let mut q = p
        + 5
        + (1 - single)
        + [0usize, 1, 2, 4][(fhd & 3) as usize]
        + [single, 2, 4, 8][(fhd >> 6) as usize];
    loop {
        let b = d.get(q..q + 3)?;
        let h = u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize;
        q += 3 + if (h >> 1) & 3 == 1 { 1 } else { h >> 3 };
        if h & 1 != 0 {
            return Some(q + 4 * ((fhd >> 2) & 1) as usize - p);
        }
    }
}

/// The first `n` decoded bytes of the zstd frame at the start of `d`, when `d` is cut short and libzstd's streaming decoder would still have handed them out: raw and RLE blocks are
/// delivered as they arrive (a compressed block only whole: `Ok(None)`, and the caller asks the decoder). `Err`: libzstd refuses a block larger than the frame's window (`ZstdError`).
/// The Python peeks a Snowdrop chunk's head from such a frame and then fails with `truncated frame`.
#[allow(clippy::result_unit_err)] // the Err carries no data: it is the ported ZstdError refusal (see the doc above)
pub fn peek_cut(d: &[u8], n: usize) -> Result<Option<Vec<u8>>, ()> {
    if d.len() < 6 || d[..4] != ZSTD_MAGIC {
        return Ok(None);
    }
    let fhd = d[4];
    let single = ((fhd >> 5) & 1) as usize;
    let fcs_n = [single, 2, 4, 8][(fhd >> 6) as usize];
    let mut q = 5 + (1 - single) + [0usize, 1, 2, 4][(fhd & 3) as usize] + fcs_n;
    // the block size limit: min(window, 128 KB); a single-segment frame's window is its content size
    let window = if single == 1 {
        let o = 5 + [0usize, 1, 2, 4][(fhd & 3) as usize];
        let mut v = 0u64;
        for i in 0..fcs_n {
            v |= (*d.get(o + i).unwrap_or(&0) as u64) << (8 * i);
        }
        v + if fcs_n == 2 { 256 } else { 0 }
    } else {
        let wd = d[5] as u64;
        let base = 1u64 << (10 + (wd >> 3));
        base + base / 8 * (wd & 7)
    };
    let max_block = window.min(1 << 17) as usize;
    let mut out = Vec::new();
    loop {
        let Some(b) = d.get(q..q + 3) else {
            return Ok(None);
        };
        let h = u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize;
        let size = h >> 3;
        q += 3;
        let kind = (h >> 1) & 3;
        if kind == 3 || (kind != 2 && size > max_block) {
            return Err(());
        }
        match kind {
            0 => {
                let have = d.len().saturating_sub(q).min(size);
                out.extend_from_slice(&d[q..q + have]);
                if have < size {
                    break; // the data ends inside this block
                }
                q += size;
            }
            1 => {
                let Some(&v) = d.get(q) else { return Ok(None) };
                out.resize(out.len() + size, v);
                q += 1;
            }
            _ => return Ok(None), // a compressed block is decoded only when it is whole
        }
        if out.len() >= n || h & 1 != 0 {
            break;
        }
    }
    Ok((out.len() >= n).then(|| out[..n].to_vec()))
}
