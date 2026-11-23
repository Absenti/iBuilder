//! RAGE's WAD v1 with split GLSLC arrays (L.A. Noire's `out.wad.nx`, nxnative 19c5379d): `noire_wad_shaders` and `noire_vfp_shaders`. The archive is read through its source: the directory,
//! the names, and the bytes of the `.vfp.nx` members only.

use crate::bytes::*;
use crate::ctl::{GLSLC_MAGIC, check, glslc_stage};
use crate::error::{Failure, Res};
use crate::red;
use crate::scan_types::Shader;
use crate::source::Source;

pub const MAGIC: &[u8] = b"WAD\x01";
/// the only file name the branch of `scan_file_in` takes
pub const NAME: &str = "out.wad.nx";

fn io(e: std::io::Error) -> Failure {
    Failure::Red(format!("OSError: {e}"))
}

/// `field(d, fmt, at, where)` / `span(d, at, n, where)` over a source: the bytes, or the red line naming where
fn span_src(src: &dyn Source, a: u64, n: u64, wh: &str) -> Res<Vec<u8>> {
    let len = src.len();
    if a <= len && n <= len - a {
        src.read(a, n as usize).map_err(io)
    } else {
        red!("{wh}: {n:#x} bytes at {a:#x} lie outside the {len:#x} bytes there are")
    }
}

/// `noire_wad_shaders(d, label)`: u32 member count at +4, then 12 bytes per member (u32, offset, size), then after the last member's data a length-prefixed name per member (u16 length,
/// the bytes). Only the `.vfp.nx` members are read, in directory order, which must be ascending and inside the data
pub fn noire_wad_shaders(src: &dyn Source, label: &str) -> Res<Vec<Shader>> {
    let len = src.len();
    let count = u32le(&span_src(src, 4, 4, label)?, 0)? as u64;
    if count == 0 {
        red!("{label}: empty WAD table");
    }
    let directory = span_src(src, 8, 12 * count, &format!("{label} directory"))?;
    let k = directory.len() - 12;
    let (last_offset, last_size) = (
        u32le(&directory, k + 4)? as u64,
        u32le(&directory, k + 8)? as u64,
    );
    let data_start = 8 + directory.len() as u64;
    let data_end = last_offset + last_size;
    let mut names = data_end;
    if !(data_start <= last_offset && last_offset <= data_end && data_end <= len) {
        red!("{label}: WAD last member or names outside archive data");
    }
    let mut out = Vec::new();
    let mut shader_end = data_start;
    for e in directory.as_chunks::<12>().0 {
        let (offset, size) = (u32le(e, 4)? as u64, u32le(e, 8)? as u64);
        let n = u16le(
            &span_src(src, names, 2, &format!("{label} member name length"))?,
            0,
        )? as u64;
        let name = span_src(src, names + 2, n, &format!("{label} member name"))?;
        names += 2 + n;
        if !name.ends_with(b".vfp.nx") {
            continue;
        }
        let Ok(member) = String::from_utf8(name.clone()) else {
            red!(
                "{label}: WAD shader member name is not UTF-8: {}",
                crate::casc::py_bytes_repr(&name)
            )
        };
        if member.contains(['\t', '\r', '\n']) {
            red!(
                "{label}: WAD shader member name would break the shader map: {}",
                crate::casc::py_bytes_repr(&name)
            );
        }
        let wh = format!("{label}/{member}");
        if offset < data_start || offset + size > data_end {
            red!("{wh}: WAD member outside data region");
        }
        if offset < shader_end {
            red!("{wh}: overlapping or unordered WAD shader members");
        }
        shader_end = offset + size;
        let d = src.read(offset, size as usize).map_err(io)?;
        out.extend(noire_vfp_shaders(&d, &wh)?);
    }
    if out.is_empty() {
        red!("{label}: no VFP shader records");
    }
    Ok(out)
}

/// `noire_vfp_shaders(d, label)`: {u64, u64 header array length, u64 control array length, u64 code array length}, the three arrays at the end of the file: 0x120-byte GLSLC headers (+0xc0
/// control offset, code offset, code length, control length, each continuing where the one before ended), the control sections, the code areas
pub fn noire_vfp_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    field_ok(d, 0, 0x20, label)?;
    let (headers_len, controls_len, code_len) = (
        u64le(d, 8)? as i128,
        u64le(d, 16)? as i128,
        u64le(d, 24)? as i128,
    );
    let header_size: i128 = 0x120;
    let headers = d.len() as i128 - headers_len - controls_len - code_len;
    if headers_len == 0 || headers_len % header_size != 0 || headers < 32 {
        red!("{label}: invalid VFP array lengths");
    }
    let controls = headers + headers_len;
    let code = controls + controls_len;
    let mut out = Vec::new();
    let (mut ctl_end, mut code_end): (i128, i128) = (0, 0);
    let mut h = headers;
    while h < controls {
        let hu = h as usize;
        let wh = format!("{label}#VFP@{hu:#x}");
        if !eq_at(d, hu, &GLSLC_MAGIC) {
            red!("{wh}: missing GLSLC header");
        }
        let (ctl_offset, code_offset, code_size, ctl_size) = (
            u32le(d, hu + 0xc0)? as i128,
            u32le(d, hu + 0xc4)? as i128,
            u32le(d, hu + 0xc8)? as i128,
            u32le(d, hu + 0xcc)? as i128,
        );
        if ctl_offset != ctl_end
            || code_offset != code_end
            || ctl_offset + ctl_size > controls_len
            || !(0 < code_size && code_size <= code_len - code_offset)
        {
            red!("{wh}: invalid VFP record spans");
        }
        ctl_end = ctl_offset + ctl_size;
        code_end = code_offset + code_size;
        let ctl = sl(
            d,
            (controls + ctl_offset) as usize,
            (controls + ctl_end) as usize,
        );
        let ins = sl(d, (code + code_offset) as usize, (code + code_end) as usize);
        let stage = if ctl_size >= 0x708 {
            glslc_stage(ctl, ins)?
        } else {
            None
        };
        let why = match stage {
            Some(st) => check(st, ctl, ins)?,
            None => Some("invalid VFP shader header"),
        };
        if let Some(w) = why {
            red!("{wh}: {w}");
        }
        out.push(Shader::new(wh, stage.unwrap(), ctl, ins));
        h += header_size;
    }
    if ctl_end != controls_len || code_end != code_len {
        red!("{label}: unclaimed bytes in VFP arrays");
    }
    Ok(out)
}
