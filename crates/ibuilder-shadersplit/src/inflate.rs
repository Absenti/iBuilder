//! zlib / raw deflate / gzip, with Python's `zlib` semantics where the readers depend on them (`decompressobj().decompress(data, max_length)`, `.eof`, `.unused_data`).

use flate2::{Decompress, FlushDecompress, Status};
use std::io::Read;

#[derive(Debug, Clone, Default)]
pub struct Inflated {
    pub out: Vec<u8>,
    /// the stream's end was reached (`decompressobj.eof`)
    pub eof: bool,
    /// input bytes the stream took; the rest is `unused_data` once `eof`
    pub used: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    Zlib,
    Raw,
}

/// The most ONE decode of an archive member may produce when its caller asks for no limit (review r2, 4.1). The size a container announces is the file's word (a `u32`: up to 4 GiB) and a
/// truthful decompression bomb costs about TWICE its size in memory (measured: 1.25 GiB out of a 1.3 MB zlib stream or a 41 KB zstd frame -> 2.1-2.2 GB RSS; the phone is killed at ~3.3 GiB).
/// No real shader cache, archive member or library comes near 1 GiB. Past it a decode is an error that names this number, not a process killed for memory. Under it nothing changes:
/// `max_out == None` and `max_out == Some(MAX_DECODED + 1)` give the same bytes. (The engine's Python has no cap; only a file that would need more than 1 GiB behaves differently.)
pub const MAX_DECODED: usize = 1 << 30;

/// The error of a decode that passed [`MAX_DECODED`] (or the `cap` a test passes)
pub fn cap_error(cap: usize) -> String {
    format!(
        "the decoded data passes {cap} bytes ({} MiB), the most one member may take",
        cap >> 20
    )
}

/// `zlib.decompressobj(wbits).decompress(data, max_out)` (a data error is `Err`; a truncated stream is `Ok` with `eof == false`, as in Python).
/// With no `max_out` the output may not pass [`MAX_DECODED`]: that is an `Err` naming the cap.
pub fn run(data: &[u8], wrap: Wrap, max_out: Option<usize>) -> Result<Inflated, String> {
    run_capped(data, wrap, max_out, MAX_DECODED)
}

/// [`run`] with the cap as a parameter (the tests use a small one)
pub fn run_capped(
    data: &[u8],
    wrap: Wrap,
    max_out: Option<usize>,
    cap: usize,
) -> Result<Inflated, String> {
    let unlimited = max_out.is_none();
    // one byte more than the cap is asked for, to tell "fits exactly" from "passes it"
    let max_out = if unlimited {
        Some(cap.saturating_add(1))
    } else {
        max_out
    };
    let r = run_limited(data, wrap, max_out)?;
    if unlimited && r.out.len() > cap {
        return Err(cap_error(cap));
    }
    Ok(r)
}

fn run_limited(data: &[u8], wrap: Wrap, max_out: Option<usize>) -> Result<Inflated, String> {
    let mut d = Decompress::new(wrap == Wrap::Zlib);
    let mut out: Vec<u8> = Vec::new();
    let mut eof = false;
    loop {
        let before_in = d.total_in();
        let before_out = d.total_out();
        let room = match max_out {
            Some(m) => m.saturating_sub(out.len()),
            None => usize::MAX,
        };
        if max_out.is_some() && room == 0 {
            break;
        }
        // grow the spare capacity in steps (a decoder writes only into spare capacity)
        let want = room.min((out.capacity() - out.len()).max(1 << 16));
        out.reserve(want);
        let spare = out.capacity() - out.len();
        let take = spare.min(room);
        // limit the writable window to `take` bytes by temporarily capping capacity use
        let start = out.len();
        out.resize(start + take, 0);
        let inp = &data[(before_in as usize).min(data.len())..];
        let res = d.decompress(inp, &mut out[start..], FlushDecompress::None);
        let produced = (d.total_out() - before_out) as usize;
        out.truncate(start + produced);
        match res {
            Ok(Status::StreamEnd) => {
                eof = true;
                break;
            }
            Ok(_) => {
                let consumed = (d.total_in() - before_in) as usize;
                if produced == 0 && consumed == 0 {
                    break; // needs more input: a truncated stream
                }
            }
            Err(e) => return Err(format!("Error -3 while decompressing data: {e}")),
        }
    }
    Ok(Inflated {
        out,
        eof,
        used: d.total_in() as usize,
    })
}

/// `zlib.decompress(data)`: the whole stream or an error (bytes after the stream are ignored)
pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let r = run_as_zlib(data, Wrap::Zlib, None)?;
    if !r.eof {
        return Err("Error -5 while decompressing data: incomplete or truncated stream".into());
    }
    Ok(r.out)
}

/// `gzip.GzipFile(...).read(limit + 1)`: every member, at most `limit + 1` bytes
pub fn gunzip(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut r = flate2::read::MultiGzDecoder::new(data).take(limit as u64 + 1);
    r.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// `zlib.decompressobj(wbits).decompress(data, max_out)` with zlib's own result whenever the fast decoder refuses the stream: `miniz_oxide` reads ahead of its output and words its
/// errors its own way, so a refusal is decoded again by the model of zlib (`zlibx`), whose answer (the bytes up to `max_out`, or zlib's error text) is the Python's
pub fn run_as_zlib(data: &[u8], wrap: Wrap, max_out: Option<usize>) -> Result<Inflated, String> {
    match run(data, wrap, max_out) {
        Ok(r) if r.eof => Ok(r),
        // a stream that did not end (cut, or stopped at `max_out`): zlib may have found an error the fast decoder did not reach
        Ok(_) => zlib_model(data, wrap, max_out),
        Err(e) if e.starts_with("Error -3") => zlib_model(data, wrap, max_out),
        Err(e) => Err(e),
    }
}

fn zlib_model(data: &[u8], wrap: Wrap, max_out: Option<usize>) -> Result<Inflated, String> {
    let limit = max_out.unwrap_or(MAX_DECODED + 1);
    let z = if wrap == Wrap::Zlib {
        crate::zlibx::decompress(data, Some(limit))
    } else {
        crate::zlibx::decompress_raw(data, Some(limit))
    }?;
    if max_out.is_none() && z.out.len() > MAX_DECODED {
        return Err(cap_error(MAX_DECODED));
    }
    Ok(Inflated {
        out: z.out,
        eof: z.eof,
        used: z.used,
    })
}
