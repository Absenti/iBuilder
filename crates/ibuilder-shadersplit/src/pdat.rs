//! The `.dat` archives of Kirby Air Riders' engine (namespace phx, magic `PDAT`): `pdat_jobs` / `pdat_shaders` of `shader_split.py`.
//!
//! The archive is read in byte ranges of about equal length (at least 16 MiB each); its catalog is not read, a record is found by what it is. A PMAP record (one shader program) is a
//! flatbuffer tagged `PMAP` at +4, mostly one zstd frame: its NVN2 control sections lie 8-aligned in it, each directly followed by its code area. A record stored as it is (too small to
//! shrink) is found by its control section outside every frame. A range is read through a window of a few MiB (the Python reads the whole range, up to 1/64 of a 16.6 GB archive).

use crate::ctl::{CTL_MAGIC, NVN2_KEY_CTL, check, nvn2_stage};
use crate::error::{Failure, Res};
use crate::scan::FileRef;
use crate::scan_types::{Ctx, Shader};
use crate::source::Source;
use std::collections::HashSet;

pub const MAGIC: &[u8; 4] = b"PDAT";
const ZSTD: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
/// `PDAT_CTL`: an NVN2 control section's first 8 bytes
const CTL: [u8; 8] = [
    CTL_MAGIC[0],
    CTL_MAGIC[1],
    CTL_MAGIC[2],
    CTL_MAGIC[3],
    0xc9,
    0,
    0,
    0,
];
/// a PMAP frame never takes up 1 MiB (the largest of Kirby Air Riders' 27,299: 186 KB inflated)
const SPAN: u64 = 1 << 20;
/// a frame's header and whole first block: zstd gives no output before it has the block
const HEAD: u64 = 0x21000;
const PMAP: &[u8; 4] = b"PMAP";
/// the window a range is read through
pub const WINDOW: u64 = 8 << 20;

/// `(path, rel, (PDAT, start, end))`
#[derive(Clone)]
pub struct PdatJob {
    pub file: FileRef,
    pub start: u64,
    pub end: u64,
}

impl PdatJob {
    pub fn identity(&self) -> Vec<String> {
        vec![format!(
            "pdat:{}:{}:{}:{}:{}",
            self.file.root,
            self.file.rel,
            self.file.loc.len(),
            self.start,
            self.end
        )]
    }
}

/// `pdat_jobs(starting(PDAT))`: every PDAT archive in ranges of `max(total // 64, 16 MiB)` bytes; the archives are taken
pub fn plan(files: &[FileRef]) -> (Vec<PdatJob>, HashSet<(usize, String)>) {
    let ds: Vec<&FileRef> = files
        .iter()
        .filter(|f| {
            f.loc
                .open()
                .ok()
                .and_then(|s| s.read(0, 4).ok())
                .is_some_and(|h| h == MAGIC)
        })
        .collect();
    let total: u64 = ds.iter().map(|f| f.loc.len()).sum();
    let step = (total / 64).max(1 << 24);
    let mut jobs = Vec::new();
    for f in &ds {
        let n = f.loc.len();
        let mut a = 0;
        while a < n {
            jobs.push(PdatJob {
                file: (*f).clone(),
                start: a,
                end: (a + step).min(n),
            });
            a += step;
        }
    }
    let taken = ds.iter().map(|f| (f.root, f.rel.clone())).collect();
    (jobs, taken)
}

/// `d = f.read(end - start + SPAN)` read through a window: `d[a:a + n]` (clamped to d)
struct Window<'a> {
    src: &'a dyn Source,
    start: u64,
    len: u64,
    at: u64,
    buf: Vec<u8>,
    window: u64,
}

impl<'a> Window<'a> {
    fn new(src: &'a dyn Source, start: u64, end: u64, window: u64) -> Window<'a> {
        let len = (end + SPAN).min(src.len()).saturating_sub(start);
        Window {
            src,
            start,
            len,
            at: 0,
            buf: Vec::new(),
            window: window.max(16),
        }
    }

    fn get(&mut self, a: u64, n: u64) -> Res<&[u8]> {
        let a = a.min(self.len);
        let b = a.saturating_add(n).min(self.len);
        if a < self.at || b > self.at + self.buf.len() as u64 {
            let want = (b - a).max(self.window).min(self.len - a);
            self.buf = self
                .src
                .read(self.start + a, want as usize)
                .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
            self.at = a;
        }
        let o = (a - self.at) as usize;
        Ok(&self.buf[o..o + (b - a) as usize])
    }

    /// the offsets of `needle` in `d[:end]`, in order
    fn find_all(&mut self, needle: &[u8], end: u64) -> Res<Vec<u64>> {
        let end = end.min(self.len);
        let (mut out, mut a) = (Vec::new(), 0u64);
        let k = needle.len() as u64;
        while a < end {
            let n = self.window.min(end - a);
            // the chunk and the bytes a match starting in it needs
            let w = self.get(a, n + k - 1)?;
            let lim = (end - a).min(w.len() as u64) as usize;
            for p in memchr::memmem::find_iter(&w[..lim], needle) {
                if (p as u64) < n {
                    out.push(a + p as u64);
                }
            }
            a += n;
        }
        Ok(out)
    }
}

/// The zstd frame at the start of `d` when all of it is there (None: it runs past `d`): its bytes and stored length, or libzstd's refusal of a frame whose content size is not
/// what its blocks give
fn one_frame(zstd: &dyn crate::zstd::Zstd, d: &[u8]) -> Option<Result<(Vec<u8>, usize), String>> {
    let n = crate::zstd::frame_len(d, 0).filter(|&n| n <= d.len())?;
    Some(zstd.decompress(&d[..n], None, None).and_then(|x| {
        match crate::zstd::frame_size(&d[..n]) {
            Some(k) if k != x.len() as u64 => Err("Data corruption detected".to_string()),
            _ => Ok((x, n)),
        }
    }))
}

fn u32at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// `program(x, at, label)`: the shaders of one PMAP record's bytes
fn program(x: &[u8], at: u64, label: &str, out: &mut Vec<Shader>) -> Res<()> {
    for c in memchr::memmem::find_iter(x, &CTL) {
        if c & 7 != 0 || c + NVN2_KEY_CTL > x.len() {
            continue;
        }
        let (nctl, ncode) = (u32at(x, c + 0x30) as usize, u32at(x, c + 0x44) as usize);
        if nctl < NVN2_KEY_CTL || c as u64 + nctl as u64 + ncode as u64 > x.len() as u64 {
            crate::red!("{label}: the control section at {c:#x} runs past its record");
        }
        let (ctl, code) = (&x[c..c + nctl], &x[c + nctl..c + nctl + ncode]);
        let Some(st) = nvn2_stage(ctl, code) else {
            crate::red!("{label}: the shader at {c:#x} is not one: no stage");
        };
        if let Some(why) = check(st, ctl, code)? {
            crate::red!("{label}: the shader at {c:#x} is not one: {why}");
        }
        out.push(Shader::new(format!("{label}@{at:#x}#var0"), st, ctl, code));
    }
    Ok(())
}

/// `pdat_shaders(path, rel, start, end, out)`
pub fn shaders(cx: &Ctx<'_>, job: &PdatJob) -> Res<Vec<Shader>> {
    let src = job
        .file
        .loc
        .open()
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    range_shaders(cx.zstd, &src, &job.file.rel, job.start, job.end, WINDOW)
}

/// [`shaders`] of the byte range `start..end` of an archive, read through a window of `window` bytes (any window gives the same answer)
pub fn range_shaders(
    zstd: &dyn crate::zstd::Zstd,
    src: &dyn Source,
    rel: &str,
    start: u64,
    end: u64,
    window: u64,
) -> Res<Vec<Shader>> {
    let label = format!("/{rel}#PMAP");
    let mut w = Window::new(src, start, end, window);
    let e = end - start;
    let mut out = Vec::new();
    let mut frames: Vec<(u64, u64)> = Vec::new();
    for p in w.find_all(&ZSTD, e)? {
        let head = {
            let h = w.get(p, HEAD)?;
            // `ZstdDecompressor().decompress(h, max_length=8)`, a ZstdError giving b'': libzstd hands out raw and RLE blocks as they arrive (a frame cut inside one still gives its
            // head) and refuses a block larger than the frame's window
            // (a frame of raw blocks that is all there: libzstd checks its content size before it hands out a byte)
            match crate::zstd::peek_cut(h, 8) {
                Ok(Some(v)) => match one_frame(zstd, h) {
                    Some(Err(_)) => Vec::new(),
                    _ => v,
                },
                Err(()) => Vec::new(),
                Ok(None) => zstd.decompress(h, None, Some(8)).unwrap_or_default(),
            }
        };
        if head.get(4..8) != Some(&PMAP[..]) {
            continue;
        }
        let d = w.get(p, SPAN)?;
        let l = d.len() as u64;
        // `ZstdDecompressor().decompress(d[p:p + SPAN])`: one frame; a frame that does not end in the span is not `eof`
        let (x, flen) = match one_frame(zstd, d) {
            Some(r) => r.map_err(|e| Failure::Red(format!("ZstdError: {e}")))?,
            None => crate::red!(
                "/{rel}: the PMAP frame at {:#x} is longer than {SPAN:#x} bytes",
                start + p
            ),
        };
        frames.push((p, p + SPAN - (l - flen as u64)));
        program(&x, start + p, &label, &mut out)?;
    }
    let len = w.len;
    for c in w.find_all(&CTL, e)? {
        if frames.iter().any(|&(a, b)| a <= c && c < b) {
            continue;
        }
        let word = |w: &mut Window<'_>, o: u64| -> Res<u64> {
            if o + 4 > len {
                crate::red!(
                    "error: unpack_from requires a buffer of at least {} bytes for unpacking 4 bytes at offset {o} (actual buffer size is {len})",
                    o + 4
                );
            }
            Ok(u32at(w.get(o, 4)?, 0) as u64)
        };
        let n = word(&mut w, c + 0x30)? + word(&mut w, c + 0x44)?;
        let x = w.get(c, n)?.to_vec();
        program(&x, start + c, &label, &mut out)?;
    }
    Ok(out)
}
