//! LZ4 block decompression with the semantics of `LZ4_decompress_safe` (what `lz4.block.decompress(raw, uncompressed_size=n)` calls, and what
//! `mods.py` uses wherever the interpreter has the `lz4` package, as the app's does): `capacity` is the size of the output buffer; the output may be
//! shorter (the caller checks the size), a stream that overruns the buffer or breaks the format's end rules is an error.

const MFLIMIT: i64 = 12;
const LASTLITERALS: i64 = 5;
const MINMATCH: usize = 4;

/// `read_variable_length`: the run length bytes of a literal or match length; `ilimit` is an index (it may be negative).
fn read_var(src: &[u8], ip: &mut usize, ilimit: i64) -> Option<usize> {
    if *ip as i64 >= ilimit {
        return None;
    }
    let mut length = 0usize;
    loop {
        let s = *src.get(*ip)?;
        *ip += 1;
        length += s as usize;
        if *ip as i64 > ilimit {
            return None;
        }
        if s != 255 {
            return Some(length);
        }
    }
}

/// `Err(())` where LZ4 reports an error (python-lz4 raises `LZ4BlockError`).
#[allow(clippy::result_unit_err)] // the error is the format's single "invalid" verdict, as in python-lz4; callers map it themselves
pub fn decompress_safe(src: &[u8], capacity: usize) -> Result<Vec<u8>, ()> {
    let iend = src.len() as i64;
    if capacity == 0 {
        return if src == [0] { Ok(Vec::new()) } else { Err(()) };
    }
    if src.is_empty() {
        return Err(());
    }
    let oend = capacity as i64;
    // `capacity` is the caller's word (an NSO header's segment size, up to 4 GiB): it bounds the output, but the buffer is reserved only for what
    // this input can produce (each input byte yields at most 255 output bytes, plus a match's MINMATCH), and grows past that if it must.
    let mut out: Vec<u8> = Vec::with_capacity(capacity.min(src.len().saturating_mul(256)));
    let mut ip = 0usize;
    loop {
        let token = *src.get(ip).ok_or(())?;
        ip += 1;
        let mut length = (token >> 4) as usize;
        // The reference decoder's "two-stage shortcut" takes a short literal run well inside both buffers without the end-of-block checks below
        // (they cannot fail there for a valid stream, but a stream that is a little unusual is accepted by it: the fuzz against python-lz4 found one).
        let shortcut = length != 15 && (ip as i64) < iend - 16 && (out.len() as i64) <= oend - 32;
        if !shortcut {
            if length == 15 {
                length += read_var(src, &mut ip, iend - 15).ok_or(())?;
            }
            let cpy = out.len() as i64 + length as i64;
            if cpy > oend - MFLIMIT || ip as i64 + length as i64 > iend - (2 + 1 + LASTLITERALS) {
                // the last sequence (or an invalid one): it must consume the input exactly and not overrun the buffer
                if ip as i64 + length as i64 != iend || cpy > oend {
                    return Err(());
                }
                out.extend_from_slice(&src[ip..ip + length]);
                return Ok(out);
            }
        }
        out.extend_from_slice(&src[ip..ip + length]);
        ip += length;
        let offset = u16::from_le_bytes([src[ip], src[ip + 1]]) as usize;
        ip += 2;
        if offset == 0 || offset > out.len() {
            return Err(()); // offset 0, or outside the output written so far
        }
        let mut mlen = (token & 15) as usize;
        if mlen == 15 {
            mlen += read_var(src, &mut ip, iend - LASTLITERALS + 1).ok_or(())?;
        }
        mlen += MINMATCH;
        if out.len() as i64 + mlen as i64 > oend - LASTLITERALS {
            return Err(()); // the last LASTLITERALS bytes must be literals
        }
        let from = out.len() - offset;
        if offset >= mlen {
            out.extend_from_within(from..from + mlen); // no overlap
        } else {
            // the match overlaps what it writes (a run): byte by byte
            for i in 0..mlen {
                let b = out[from + i];
                out.push(b);
            }
        }
    }
}
