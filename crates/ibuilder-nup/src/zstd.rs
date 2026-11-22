//! zstd behind one small trait, as the shader split's `Zstd`, but streaming: a NUP file can be tens of GB, so a frame is decoded
//! chunk by chunk and never held whole (the Python's `gameapp/nup_zstd.py` exists for the same reason: the one-shot binding would).
//!
//! Three implementations:
//! * [`NativeZstd`] (feature `native-zstd`, the default): the C libzstd through the `zstd` crate, built by `cc`; builds for
//!   `aarch64-apple-ios` and is several times faster than the pure-Rust decoder (numbers in `docs/crates/SWITCH2-RUST.md`).
//! * [`Ruzstd`]: the pure-Rust `ruzstd` crate (already in the lock through `ibuilder-shadersplit`), no C at all.
//! * [`CallbackZstd`]: whatever the host app supplies (the app already has a streaming zstd binding, `zstd_stream_feed`).
//!
//! The contract: decode exactly ONE zstd frame from `src` (a file's extent) and give the output to `sink` in chunks. A frame that
//! ends before `src` does (trailing bytes), or `src` ending inside a frame, is an `Err`.

use std::io::{self, Read};
use std::sync::Arc;

pub trait ZstdBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn decode_frame(
        &self,
        src: &mut dyn Read,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), String>;
}

/// The best backend this build has: the C libzstd when the `native-zstd` feature is on, else the pure-Rust decoder.
pub fn default_backend() -> Arc<dyn ZstdBackend> {
    #[cfg(feature = "native-zstd")]
    {
        Arc::new(NativeZstd)
    }
    #[cfg(not(feature = "native-zstd"))]
    {
        Arc::new(Ruzstd)
    }
}

const IN_CHUNK: usize = 1 << 20;
const OUT_CHUNK: usize = 1 << 20;

// ---- the C libzstd ------------------------------------------------------------------------------------------------------------

#[cfg(feature = "native-zstd")]
#[derive(Default, Clone, Copy)]
pub struct NativeZstd;

#[cfg(feature = "native-zstd")]
impl ZstdBackend for NativeZstd {
    fn name(&self) -> &'static str {
        "libzstd (C)"
    }
    fn decode_frame(
        &self,
        src: &mut dyn Read,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), String> {
        use zstd::stream::raw::{Decoder, InBuffer, Operation, OutBuffer};
        let mut dec = Decoder::new().map_err(|e| format!("zstd: {e}"))?;
        let mut inbuf = vec![0u8; IN_CHUNK];
        let mut out = vec![0u8; OUT_CHUNK];
        let mut finished = false;
        loop {
            let n = read_some(src, &mut inbuf)?;
            if n == 0 {
                break;
            }
            if finished {
                return Err("zstd: data after the end of the frame".into());
            }
            let mut input = InBuffer::around(&inbuf[..n]);
            loop {
                let mut o = OutBuffer::around(&mut out[..]);
                let hint = dec
                    .run(&mut input, &mut o)
                    .map_err(|e| format!("zstd: {e}"))?;
                let produced = o.pos();
                if produced > 0 {
                    sink(&out[..produced]).map_err(|e| e.to_string())?;
                }
                if hint == 0 {
                    finished = true;
                    if input.pos() < n {
                        return Err("zstd: data after the end of the frame".into());
                    }
                    break;
                }
                // the output buffer was full: more output may be pending though the input is used up
                if input.pos() >= n && produced < OUT_CHUNK {
                    break;
                }
            }
        }
        if !finished {
            return Err("zstd: the frame is cut short".into());
        }
        Ok(())
    }
}

// ---- the pure-Rust decoder ------------------------------------------------------------------------------------------------------

#[derive(Default, Clone, Copy)]
pub struct Ruzstd;

impl ZstdBackend for Ruzstd {
    fn name(&self) -> &'static str {
        "ruzstd (pure Rust)"
    }
    fn decode_frame(
        &self,
        src: &mut dyn Read,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), String> {
        use ruzstd::decoding::StreamingDecoder;
        let mut counted = CountingReader { src, n: 0 };
        let mut dec = StreamingDecoder::new(&mut counted).map_err(|e| format!("zstd: {e}"))?;
        let mut out = vec![0u8; OUT_CHUNK];
        loop {
            let n = dec.read(&mut out).map_err(|e| format!("zstd: {e}"))?;
            if n == 0 {
                break;
            }
            sink(&out[..n]).map_err(|e| e.to_string())?;
        }
        let inner = dec.into_inner();
        // trailing bytes after the frame: the Python's decompressor raises EOFError on them
        let mut probe = [0u8; 1];
        match inner.read(&mut probe) {
            Ok(0) => Ok(()),
            Ok(_) => Err("zstd: data after the end of the frame".into()),
            Err(e) => Err(format!("zstd: {e}")),
        }
    }
}

struct CountingReader<'a> {
    src: &'a mut dyn Read,
    #[allow(dead_code)]
    n: u64,
}

impl Read for CountingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let k = self.src.read(buf)?;
        self.n += k as u64;
        Ok(k)
    }
}

// ---- the host's own ------------------------------------------------------------------------------------------------------------------

/// `decode(src, sink)` supplied by the host (on the phone: the app's streaming zstd binding)
pub struct CallbackZstd<F>(pub F);

impl<F> ZstdBackend for CallbackZstd<F>
where
    F: Fn(&mut dyn Read, &mut dyn FnMut(&[u8]) -> io::Result<()>) -> Result<(), String>
        + Send
        + Sync,
{
    fn name(&self) -> &'static str {
        "host callback"
    }
    fn decode_frame(
        &self,
        src: &mut dyn Read,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), String> {
        (self.0)(src, sink)
    }
}

/// `read` that retries on `Interrupted`
fn read_some(src: &mut dyn Read, buf: &mut [u8]) -> Result<usize, String> {
    loop {
        match src.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("reading the container: {e}")),
        }
    }
}
