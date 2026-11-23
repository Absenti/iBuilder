//! Virtuos SRYK archives (FFX/X-2 and FFXII TZA for the Switch): the engine's `vbf.py` (`entries`, `read`, `block`) and the branch of `scan_file_in` that reads their effects and loose
//! GPU programs with `phyre_shaders`. Two MD5 tables, 32-byte file records, the names, then 64 KB raw or compressed blocks: one whole zlib stream (FFX/X-2), else a raw LZ4 block
//! (FFXII TZA). A member at offset `u64::MAX` is missing or shared.

use crate::source::Source;

/// (name, size, offset, the stored length of each block; 0: 64 KB)
pub type Entry = (String, u64, u64, Vec<u16>);

fn ve(m: impl Into<String>) -> String {
    format!("ValueError: {}", m.into())
}

/// `vbf.entries(path)`: `Err` is the exception's text (`ValueError: ...`, `UnicodeDecodeError: ...`)
pub fn entries(src: &dyn Source) -> Result<Vec<Entry>, String> {
    let io = |e: std::io::Error| format!("OSError: {e}");
    let head = src.read(0, 16).map_err(io)?;
    if head.len() != 16 || &head[..4] != b"SRYK" {
        return Err(ve("not a VBF archive"));
    }
    let hlen = u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
    let count = u64::from_le_bytes(head[8..16].try_into().unwrap()) as u128;
    if !(16 + count * 64 + 4 <= hlen as u128 && hlen <= 256 << 20) {
        return Err(ve("invalid VBF header length"));
    }
    let head = src.read(0, hlen as usize).map_err(io)?;
    if head.len() as u64 != hlen {
        return Err(ve("truncated VBF header"));
    }
    let count = count as usize;
    let u32at = |o: usize| u32::from_le_bytes(head[o..o + 4].try_into().unwrap()) as usize;
    let u64at = |o: usize| u64::from_le_bytes(head[o..o + 8].try_into().unwrap());
    let strings = 16 + count * 64;
    let slen = u32at(strings);
    let blocks = strings + slen;
    if slen < 4 || blocks > hlen as usize {
        return Err(ve("invalid VBF string table"));
    }
    let mut out = Vec::new();
    for i in 0..count {
        let r = 16 + count * 32 + i * 32;
        let (bi, size, offset, name) = (u32at(r), u64at(r + 8), u64at(r + 16), u64at(r + 24));
        if offset == u64::MAX {
            continue;
        }
        let start = (strings + 4) as u128 + name as u128;
        if !((strings + 4) as u128 <= start && start < blocks as u128) {
            return Err(ve("invalid VBF name offset"));
        }
        let start = start as usize;
        let Some(end) = crate::bytes::find_in(&head, &[0], start, blocks) else {
            return Err(ve("unterminated VBF name"));
        };
        let n = size.div_ceil(65536) as u128;
        if blocks as u128 + (bi as u128 + n) * 2 > hlen as u128 || offset < hlen {
            return Err(ve("invalid VBF block range"));
        }
        let lengths: Vec<u16> = (0..n as usize)
            .map(|k| {
                let o = blocks + (bi + k) * 2;
                u16::from_le_bytes([head[o], head[o + 1]])
            })
            .collect();
        let name =
            crate::lspk::utf8(&head[start..end]).map_err(|e| format!("UnicodeDecodeError: {e}"))?;
        out.push((name, size, offset, lengths));
    }
    Ok(out)
}

/// `vbf.block(data, n, name)`: one whole zlib stream of n bytes, else a raw LZ4 block
fn block(data: &[u8], n: usize, name: &str) -> Result<Vec<u8>, String> {
    if let Ok(z) = crate::inflate::run_as_zlib(data, crate::inflate::Wrap::Zlib, Some(n + 1))
        && z.eof
        && z.out.len() == n
    {
        return Ok(z.out);
    }
    let mut buf = vec![0u8; n];
    match lz4_flex::block::decompress_into(data, &mut buf) {
        Ok(k) if k == n => Ok(buf),
        Ok(_) => Err(ve(format!("{name}: VBF block size mismatch"))),
        Err(_) => Err(ve(format!(
            "{name}: invalid VBF compressed block (neither zlib nor LZ4)"
        ))),
    }
}

/// `vbf.read(f, entry)`
pub fn read(src: &dyn Source, entry: &Entry) -> Result<Vec<u8>, String> {
    let (name, size, offset, lengths) = entry;
    let mut size = *size;
    let mut off = *offset;
    if off > i64::MAX as u64 {
        // `f.seek(offset)` takes a signed offset
        return Err(ve("cannot fit 'int' into an offset-sized integer"));
    }
    let mut out = Vec::new();
    for &length in lengths {
        let n = size.min(65536) as usize;
        let length = if length == 0 { 65536 } else { length as usize };
        let data = src.read(off, length).map_err(|e| format!("OSError: {e}"))?;
        off += length as u64;
        if data.len() != length {
            return Err(ve(format!("{name}: truncated VBF block")));
        }
        let data = if length != n {
            block(&data, n, name)?
        } else {
            data
        };
        if data.len() != n {
            return Err(ve(format!("{name}: VBF block size mismatch")));
        }
        out.extend_from_slice(&data);
        size -= n as u64;
    }
    Ok(out)
}

/// The members `scan_file_in` reads: compiled effects and loose GPU programs (texture and sound members carry none)
pub fn wanted(name: &str) -> bool {
    name.contains(".fx")
        || name.contains("/shaders/")
        || name.ends_with(".vs.bin")
        || name.ends_with(".ps.bin")
        || name.ends_with(".fs.bin")
}
