//! GIANTS Engine SHARC v11 shader archives (Farming Simulator 26's `shared/shader_switch.sharc`, nxnative f3950c50): `sharc_shaders`. A 0x2c-byte header sizes three entry tables (24, 32 and
//! 20 bytes per entry); the data after them: metadata blocks, then each shader member (five LZMA1 property bytes and a raw LZMA1 stream without its end marker, one GLSLC output), its
//! property records, side records and length-prefixed names.

use crate::bytes::*;
use crate::ctl::GLSLC_MAGIC;
use crate::error::Res;
use crate::glslc::{glslc_layout, glslc_shaders_at};
use crate::lzma1;
use crate::red;
use crate::scan_types::Shader;

pub const MAGIC: &[u8] = b"RAHS";
const OUTPUT_LIMIT: usize = 10_000_000;

/// `sharc_shaders(d, label)`
pub fn sharc_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let len = d.len() as u64;
    if d.len() < 0x2c {
        red!("{label}: truncated SHARC header");
    }
    let w = |o: usize| -> Res<u64> { Ok(u32le(d, o)? as u64) };
    let (version, header_size, metadata_count, shader_count, hierarchy_count) =
        (w(4)?, w(0x10)?, w(0x14)?, w(0x18)?, w(0x1c)?);
    if version != 11 || header_size < 0x2c {
        red!(
            "{label}: SHARC version {version}, header size {header_size:#x}: layout this reader does not know"
        );
    }
    let shader_table = header_size + 24 * metadata_count;
    let mut pos = shader_table + 32 * shader_count + 20 * hierarchy_count;
    if pos > len {
        red!("{label}: SHARC entry tables end at {pos:#x}, past the file size {len:#x}");
    }
    for i in 0..metadata_count {
        let e = (header_size + 24 * i) as usize;
        let (size, properties) = (u32le(d, e)? as u64, u16le(d, e + 4)? as u64);
        pos += size + 5 * properties;
        if pos > len {
            red!("{label}: SHARC metadata {i} runs past the file");
        }
    }
    let mut found = Vec::new();
    for i in 0..shader_count {
        let e = (shader_table + 32 * i) as usize;
        let (size, properties, side_records, names) = (
            u32le(d, e)? as u64,
            u32le(d, e + 4)? as u64,
            u32le(d, e + 8)? as u64,
            u32le(d, e + 28)? as u64,
        );
        let wh = format!("{label}#{i}");
        let chunk = span(
            d,
            pos as usize,
            size as usize,
            &format!("{wh}: SHARC compressed member"),
        )?;
        if chunk.len() <= 5 {
            red!("{wh}: short SHARC LZMA member");
        }
        pos += size + 5 * properties + 16 * side_records;
        if pos > len {
            red!("{wh}: SHARC property or side records run past the file");
        }
        for _ in 0..names {
            field_ok(d, pos as usize, 2, &format!("{wh}: SHARC name header"))?;
            pos += 2 + d[pos as usize + 1] as u64;
            if pos > len {
                red!("{wh}: SHARC name runs past the file");
            }
        }
        let props = chunk[0] as u32;
        let dict_size = u32le(chunk, 1)?;
        let raw = match lzma1::decompress_raw(
            &chunk[5..],
            props % 9,
            props / 9 % 5,
            props / 45,
            dict_size.min(OUTPUT_LIMIT as u32),
            OUTPUT_LIMIT + 1,
        ) {
            Ok(r) => r.out,
            Err(e) => red!("{wh}: damaged SHARC LZMA stream ({e})"),
        };
        if raw.len() > OUTPUT_LIMIT {
            red!("{wh}: SHARC member exceeds the 10,000,000-byte decode limit");
        }
        let (expected, _, _, _) = glslc_layout(&raw, 0)?;
        if !starts(&raw, &GLSLC_MAGIC) || !(0 < expected && expected <= raw.len()) {
            red!(
                "{wh}: incomplete SHARC GLSLC output ({} bytes decoded, {expected} declared)",
                raw.len()
            );
        }
        found.extend(glslc_shaders_at(&raw[..expected], &wh, true, &[0])?);
    }
    Ok(found)
}
