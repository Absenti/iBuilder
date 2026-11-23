//! NXSD shader containers (TETRIS THE GRAND MASTER 4): `nxsd_shaders` of `shader_split.py` (nxnative a63ca651) and its branch at the top of `scan_file_in` (by the magic, before the
//! Next Level Games `.dict` check).
//!
//! Version 0x100, little-endian: `NXSD`, u32 version, u32 file size, u32 code start (32 bits), u32, u32 count, u32 control bytes (count x 0x880), u32 code bytes, then at +0x28 u64 descriptor
//! table, u64 control start, u64 code start. Descriptors (0x18 bytes each, the table runs up to the controls): u32 control offset (in the control region, a multiple of 0x880), u32,
//! u32 code offset (in the code region), three u32. The code areas must cover the code region exactly, without gaps or overlaps; one stage per entry.

use crate::bytes::{field_ok, span, u32le, u64le};
use crate::ctl::{CS_MAGIC, GFX_MAGIC, St, check, sph_stage};
use crate::error::Res;
use crate::red;
use crate::scan_types::Shader;
use std::collections::HashSet;

pub const MAGIC: &[u8; 4] = b"NXSD";
const CTL: u64 = 0x880;

/// `nxsd_shaders(d, label)`
pub fn nxsd_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let wh = format!("{label} NXSD");
    let len = d.len() as u64;
    if d.len() < 0x40 {
        red!("{wh}: truncated header");
    }
    field_ok(d, 4, 28, &wh)?;
    let (version, size, code32, count, ctl_bytes, code_bytes) = (
        u32le(d, 4)? as u64,
        u32le(d, 8)? as u64,
        u32le(d, 12)? as u64,
        u32le(d, 20)? as u64,
        u32le(d, 24)? as u64,
        u32le(d, 28)? as u64,
    );
    field_ok(d, 0x28, 24, &wh)?;
    let (table, ctl_start, code_start) = (u64le(d, 0x28)?, u64le(d, 0x30)?, u64le(d, 0x38)?);
    if version != 0x100 {
        red!("{wh}: unsupported version {version:#x}");
    }
    if size != len || code32 != code_start {
        red!("{wh}: inconsistent file size or code offsets");
    }
    if count == 0 || ctl_bytes != count * CTL {
        red!("{wh}: invalid control count or stride");
    }
    // Python's integers: these sums do not overflow there, so they are taken in 128 bits here
    let w = |x: u64| x as u128;
    if table < 0x40 || w(table) + w(count) * 0x18 != w(ctl_start) {
        red!("{wh}: invalid descriptor table");
    }
    if w(ctl_start) + w(ctl_bytes) != w(code_start) || w(code_start) + w(code_bytes) != w(len) {
        red!("{wh}: invalid control or code region");
    }
    // every offset below lies inside d from here on: the regions add up to its length
    let mut controls: HashSet<u64> = HashSet::new();
    let mut ranges: Vec<(u64, u64, u64)> = Vec::new();
    for i in 0..count {
        let entry = format!("{wh} entry {i}");
        let at = (table + i * 0x18) as usize;
        field_ok(d, at, 24, &entry)?;
        let (ctl_rel, code_rel) = (u32le(d, at)? as u64, u32le(d, at + 8)? as u64);
        if !ctl_rel.is_multiple_of(CTL) || ctl_rel + CTL > ctl_bytes || controls.contains(&ctl_rel)
        {
            red!("{entry}: invalid or duplicate control offset");
        }
        controls.insert(ctl_rel);
        let ctl_at = ctl_start + ctl_rel;
        field_ok(d, (ctl_at + 0x704) as usize, 4, &entry)?;
        let ncode = u32le(d, (ctl_at + 0x704) as usize)? as u64;
        let code_end = code_rel + ncode;
        if code_end > code_bytes {
            red!("{entry}: code section lies outside the code region");
        }
        ranges.push((code_rel, code_end, ctl_at));
    }
    // the coverage is checked before any code is copied: overlapping descriptors could name the same bytes again and again
    let mut sorted = ranges.clone();
    sorted.sort();
    let mut cursor = 0u64;
    for (start, end, _) in &sorted {
        if *start != cursor {
            red!("{wh}: code sections overlap or have gaps");
        }
        cursor = *end;
    }
    if cursor != code_bytes {
        red!("{wh}: code sections do not cover the code region");
    }
    let mut out = Vec::with_capacity(ranges.len());
    for (i, (start, end, ctl_at)) in ranges.iter().enumerate() {
        let entry = format!("{wh} entry {i}");
        let ctl = span(d, *ctl_at as usize, CTL as usize, &entry)?;
        let code = span(
            d,
            (code_start + start) as usize,
            (end - start) as usize,
            &entry,
        )?;
        let st = if code.starts_with(&CS_MAGIC) {
            Some(St::Cs)
        } else if code.starts_with(&GFX_MAGIC) {
            sph_stage(code)
        } else {
            None
        };
        let Some(st) = st else {
            red!("{entry}: unknown shader stage");
        };
        if code.len() < if st == St::Cs { 0x30 } else { 0x80 } {
            red!("{entry}: truncated code header or SPH");
        }
        if let Some(why) = check(st, ctl, code)? {
            red!("{entry}: {why}");
        }
        out.push(Shader::new(format!("{label}#NXSD@{i}"), st, ctl, code));
    }
    Ok(out)
}
