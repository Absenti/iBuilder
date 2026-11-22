//! `dm/tools/nso.py` `load()`: an NSO (ExeFS code module) -> its memory image of text + ro + data, bss not included.
//!
//! It accepts what Horizon accepts and checks it the same way, before anything the header claims is allocated:
//! memory offsets on pages, text at 0, then ro, then data, none overlapping; sizes below 2 GiB; a compressed
//! segment no larger stored than loaded, at most what LZ4 can produce from its stored bytes, and decompressing to
//! exactly its size; a segment whose check-hash flag is set must match its SHA-256 in the header; and the image no
//! larger than what the whole file can fill. A damaged or truncated file is an [`Error::Format`].
//!
//! ZBIC segments (NSO flag bit 7, system 22.0.0+: Nintendo's zstd variant) go through `ibuilder_nso::zbic`, the one
//! decoder of the toolchain. Not ported: NRO modules (a clear error instead of a wrong image). Segments are read with
//! positional reads one at a time, so the compressed file is never held whole in memory.
use crate::error::{Error, Result, fmt_err};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

const PAGE: u64 = 0x1000;
const LZ4_MAX_RATIO: u64 = 255;
const SIZE_MAX: u64 = 0x7FFF_FFFF;
const NSO_ZSTD: u32 = 1 << 7;
const NAMES: [&str; 3] = ["text", "ro", "data"];

#[derive(Debug)]
pub struct Module {
    /// text + ro + data at their memory offsets.
    pub image: Vec<u8>,
    /// `(memory offset, size)` of text, ro and data.
    pub segments: [(u64, u64); 3],
    pub bss: u32,
    /// 0x20 bytes; the build ID tools key on is its first 0x10.
    pub build_id: [u8; 0x20],
}

impl Module {
    /// `m.build_id[:0x10].hex()`
    pub fn bid_hex(&self) -> String {
        self.build_id[..0x10]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Positional reads from a file or a byte slice.
pub trait ReadAt: Sync {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<()>;
}

impl ReadAt for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let end = off
            .checked_add(buf.len() as u64)
            .filter(|&e| e <= <[u8]>::len(self) as u64);
        match end {
            Some(e) => {
                buf.copy_from_slice(&self[off as usize..e as usize]);
                Ok(())
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "short read",
            )),
        }
    }
}

struct FileAt {
    f: File,
    len: u64,
}

impl ReadAt for FileAt {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> std::io::Result<()> {
        self.f.read_exact_at(buf, off)
    }
}

fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

pub fn load_file(path: &Path) -> Result<Module> {
    let f = File::open(path)?;
    let len = f.metadata()?.len();
    load(&FileAt { f, len }, &path.display().to_string())
}

pub fn load_bytes(d: &[u8], name: &str) -> Result<Module> {
    load(d, name)
}

/// `load(d, name)` for an NSO0 module.
pub fn load<R: ReadAt + ?Sized>(src: &R, name: &str) -> Result<Module> {
    let len = src.len();
    let mut head = vec![0u8; len.min(0x100) as usize];
    src.read_at(0, &mut head)?;
    if head.len() < 4 || &head[..4] != b"NSO0" {
        if head.len() >= 0x14 && &head[0x10..0x14] == b"NRO0" {
            return fmt_err(format!(
                "{name}: NRO modules are not supported by this loader (profile derivation reads ExeFS NSOs only)"
            ));
        }
        return fmt_err(format!(
            "{name}: not an NSO or NRO (magic {:?})",
            &head[..head.len().min(4)]
        ));
    }
    if len < 0x100 {
        return fmt_err(format!("{name}: the NSO header is cut short ({len} bytes)"));
    }
    let d = &head;
    let flags = u32le(d, 0xc);
    let mut stored = [(0u64, 0u64, 0u64); 3]; // (file offset, memory offset, size)
    let mut csz = [0u64; 3];
    for i in 0..3 {
        stored[i] = (
            u32le(d, 0x10 + i * 0x10) as u64,
            u32le(d, 0x14 + i * 0x10) as u64,
            u32le(d, 0x18 + i * 0x10) as u64,
        );
        let c = u32le(d, 0x60 + i * 4) as u64;
        csz[i] = if flags >> i & 1 != 0 { c } else { stored[i].2 };
    }
    let bss = u32le(d, 0x3c);
    let mut build_id = [0u8; 0x20];
    build_id.copy_from_slice(&d[0x40..0x60]);
    for i in 0..3 {
        let (fo, mo, sz) = stored[i];
        let c = csz[i];
        let what = format!("{name}: {} segment", NAMES[i]);
        if mo % PAGE != 0 {
            return fmt_err(format!("{what} is at {mo:#x}, not on a page"));
        }
        if sz > SIZE_MAX {
            return fmt_err(format!(
                "{what} claims {sz:#x} bytes, more than Horizon loads"
            ));
        }
        if fo + c > len {
            return fmt_err(format!(
                "{what} is stored at {fo:#x}..{:#x}, past the end of the file ({len:#x} bytes): the file is cut short or damaged",
                fo + c
            ));
        }
        if flags >> i & 1 != 0 && c > sz {
            return fmt_err(format!(
                "{what} is stored compressed in {c:#x} bytes, more than its {sz:#x}"
            ));
        }
        if flags >> i & 1 != 0 && flags & NSO_ZSTD == 0 && sz > c * LZ4_MAX_RATIO {
            return fmt_err(format!(
                "{what} claims {sz:#x} bytes from {c:#x} stored, more than LZ4 can produce"
            ));
        }
    }
    if stored[0].1 != 0 {
        return fmt_err(format!(
            "{name}: text segment is at {:#x}, not at 0",
            stored[0].1
        ));
    }
    for i in 1..3 {
        if stored[i].1 < stored[i - 1].1 + stored[i - 1].2 {
            return fmt_err(format!(
                "{name}: {} segment at {:#x} overlaps the {} segment before it",
                NAMES[i],
                stored[i].1,
                NAMES[i - 1]
            ));
        }
    }
    let end = stored[2].1 + stored[2].2;
    if end > len * LZ4_MAX_RATIO {
        return fmt_err(format!(
            "{name}: the segments span {end:#x} bytes, more than a file of {len:#x} bytes can fill (a gap of zeros between them)"
        ));
    }
    let mut img = vec![0u8; end as usize];
    for i in 0..3 {
        let (fo, mo, sz) = stored[i];
        let c = csz[i];
        let what = format!("{name}: {} segment", NAMES[i]);
        let out = &mut img[mo as usize..(mo + sz) as usize];
        if flags >> i & 1 != 0 {
            let mut raw = vec![0u8; c as usize];
            src.read_at(fo, &mut raw)?;
            if flags & NSO_ZSTD != 0 {
                let n = ibuilder_nso::zbic::decompress_into(&raw, out).map_err(|e| {
                    Error::Format(format!("{what}: ZBIC does not decompress ({e})"))
                })?;
                if n as u64 != sz {
                    return fmt_err(format!(
                        "{what}: ZBIC decompresses to {n:#x} bytes, not the {sz:#x} of the header: the file is damaged"
                    ));
                }
            } else {
                let n = lz4_flex::block::decompress_into(&raw, out)
                    .map_err(|e| Error::Format(format!("{what} does not decompress ({e})")))?;
                if n as u64 != sz {
                    return fmt_err(format!(
                        "{what} decompresses to {n:#x} bytes, not the {sz:#x} of the header: the file is damaged"
                    ));
                }
            }
        } else {
            src.read_at(fo, out)?;
        }
        if flags >> (3 + i) & 1 != 0 {
            let want = &d[0xa0 + 0x20 * i..0xc0 + 0x20 * i];
            if Sha256::digest(&*out).as_slice() != want {
                return fmt_err(format!(
                    "{what} does not match its SHA-256 in the header: the file is damaged"
                ));
            }
        }
    }
    Ok(Module {
        image: img,
        segments: [
            (stored[0].1, stored[0].2),
            (stored[1].1, stored[1].2),
            (stored[2].1, stored[2].2),
        ],
        bss,
        build_id,
    })
}
