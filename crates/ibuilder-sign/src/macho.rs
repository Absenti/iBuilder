//! The little Mach-O parsing adhoc_sign.py does: load commands, the segment table, the dylib identifier, the
//! `LC_CODE_SIGNATURE` slot and the header slack. All offsets are into the header buffer the caller read.
use crate::error::{Error, fmt_err};
use std::collections::HashMap;

pub const LC_SEGMENT_64: u32 = 0x19;
pub const LC_ID_DYLIB: u32 = 0xD;
pub const LC_CODE_SIGNATURE: u32 = 0x1D;
pub const MH_MAGIC_64: u32 = 0xFEED_FACF;
pub const SEG_LINKEDIT: &str = "__LINKEDIT";
pub const OFF_NCMDS: usize = 16;
pub const OFF_SIZEOFCMDS: usize = 20;

pub fn rd_u32(d: &[u8], off: usize) -> Result<u32, Error> {
    off.checked_add(4)
        .and_then(|e| d.get(off..e))
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| {
            Error::Format(format!(
                "unpack_from requires a buffer of at least {} bytes (header has {})",
                off + 4,
                d.len()
            ))
        })
}

pub fn rd_u64(d: &[u8], off: usize) -> Result<u64, Error> {
    off.checked_add(8)
        .and_then(|e| d.get(off..e))
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or_else(|| {
            Error::Format(format!(
                "unpack_from requires a buffer of at least {} bytes (header has {})",
                off + 8,
                d.len()
            ))
        })
}

pub fn wr_u32(d: &mut [u8], off: usize, v: u32) -> Result<(), Error> {
    match d.get_mut(off..off + 4) {
        Some(b) => {
            b.copy_from_slice(&v.to_le_bytes());
            Ok(())
        }
        None => fmt_err("pack_into requires a buffer of at least 4 more bytes"),
    }
}

pub fn wr_u64(d: &mut [u8], off: usize, v: u64) -> Result<(), Error> {
    match d.get_mut(off..off + 8) {
        Some(b) => {
            b.copy_from_slice(&v.to_le_bytes());
            Ok(())
        }
        None => fmt_err("pack_into requires a buffer of at least 8 more bytes"),
    }
}

/// `(offset, cmd, cmdsize)` of every load command (`_load_commands`).
pub fn load_commands(d: &[u8]) -> Result<Vec<(usize, u32, u32)>, Error> {
    let magic = rd_u32(d, 0)?;
    if magic != MH_MAGIC_64 {
        return fmt_err(format!(
            "not a 64-bit little-endian Mach-O (magic {magic:#x})"
        ));
    }
    let ncmds = rd_u32(d, OFF_NCMDS)?;
    let sizeofcmds = rd_u32(d, OFF_SIZEOFCMDS)?;
    // Both counts are the file's word. A load command is at least its own 8-byte header, so `ncmds` of them cannot be more than `sizeofcmds / 8`; and one whose `cmdsize` is smaller than that
    // header does not move `off`, so `ncmds` (up to 2^32) of them would be pushed one by one (16 bytes each: tens of GB) before anything looked at the end. (review: ibuilder-macho `parse` had it, this walk did not)
    if u64::from(ncmds) * 8 > u64::from(sizeofcmds) {
        return fmt_err(format!(
            "{ncmds} load commands cannot fit in sizeofcmds {sizeofcmds}"
        ));
    }
    let mut off = 32usize;
    let mut out = Vec::new();
    for i in 0..ncmds {
        let cmd = rd_u32(d, off)?;
        let cmdsize = rd_u32(d, off + 4)?;
        if cmdsize < 8 {
            return fmt_err(format!(
                "load command {i} (cmd {cmd:#x}) has cmdsize {cmdsize}, smaller than its own 8-byte header"
            ));
        }
        out.push((off, cmd, cmdsize));
        off = off
            .checked_add(cmdsize as usize)
            .ok_or_else(|| Error::Format("load command size overflows".into()))?;
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug)]
pub struct Segment {
    /// Offset of the `LC_SEGMENT_64` command (`lo`).
    pub lo: usize,
    #[allow(dead_code)]
    pub vmaddr: u64,
    pub vmsize: u64,
    pub fileoff: u64,
    pub filesize: u64,
}

#[derive(Debug)]
pub struct Info {
    pub segments: HashMap<String, Segment>,
    pub identifier: Option<String>,
    pub sig_lo: Option<usize>,
}

/// `_inspect`: segment table, the dylib identifier, the signature command slot.
pub fn inspect(d: &[u8]) -> Result<Info, Error> {
    let mut info = Info {
        segments: HashMap::new(),
        identifier: None,
        sig_lo: None,
    };
    for (off, cmd, _size) in load_commands(d)? {
        match cmd {
            LC_SEGMENT_64 => {
                let raw = d
                    .get(off + 8..off + 24)
                    .ok_or_else(|| Error::Format("truncated LC_SEGMENT_64".into()))?;
                let name = raw.split(|&b| b == 0).next().unwrap_or(&[]);
                let segname = std::str::from_utf8(name)
                    .map_err(|e| Error::Format(format!("segment name is not UTF-8: {e}")))?
                    .to_string();
                info.segments.insert(
                    segname,
                    Segment {
                        lo: off,
                        vmaddr: rd_u64(d, off + 24)?,
                        vmsize: rd_u64(d, off + 32)?,
                        fileoff: rd_u64(d, off + 40)?,
                        filesize: rd_u64(d, off + 48)?,
                    },
                );
            }
            LC_ID_DYLIB => {
                let nameoff = rd_u32(d, off + 8)? as usize;
                let start = off + nameoff;
                let tail = d
                    .get(start..)
                    .ok_or_else(|| Error::Format("subsection not found".into()))?;
                let end = tail
                    .iter()
                    .position(|&b| b == 0)
                    .ok_or_else(|| Error::Format("subsection not found".into()))?;
                let s = std::str::from_utf8(&tail[..end])
                    .map_err(|e| Error::Format(format!("identifier is not UTF-8: {e}")))?;
                info.identifier = Some(s.to_string());
            }
            LC_CODE_SIGNATURE => info.sig_lo = Some(off),
            _ => {}
        }
    }
    Ok(info)
}

/// `_slack`: bytes available between the end of the load commands and the first file-backed section (negative
/// when a section starts before the end of the commands).
pub fn slack(d: &[u8]) -> Result<i64, Error> {
    let sizeofcmds = rd_u32(d, OFF_SIZEOFCMDS)? as i64;
    let end = 32 + sizeofcmds;
    let mut first: Option<i64> = None;
    for (off, cmd, _size) in load_commands(d)? {
        if cmd != LC_SEGMENT_64 {
            continue;
        }
        let nsects = rd_u32(d, off + 64)? as usize;
        let mut p = off + 72;
        for _ in 0..nsects {
            let sect_off = rd_u32(d, p + 48)? as i64;
            if sect_off != 0 && first.is_none_or(|f| sect_off < f) {
                first = Some(sect_off);
            }
            p += 80;
        }
    }
    Ok(first.unwrap_or(end) - end)
}
