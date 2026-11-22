//! Archives and compression wrappers a scan opens member by member: Yaz0, SARC, gzip, zstd (with the dictionaries of Tears of the Kingdom), size-prefixed frames, PACx 403,
//! MT Framework ARC, Bezel Engine BEA, Game Freak GFPAK, Criterion bnd2, Heaps PAK, Koei Tecmo's chunked zlib, Monolith's XBC1 / cmp-zlib / HCPS, UbiArt IPK, XNB.
//! Port of the same-named functions of `shader_split.py`.

use crate::bytes::*;
use crate::ctl::*;
use crate::error::{Failure, Res};
use crate::inflate::{self, Wrap};
use crate::nvn::hcps_shaders;
use crate::scan_types::{Ctx, Shader};
use crate::zlibx;
use crate::zstd::{self, ZSTD_MAGIC};
use crate::{red, unsupported};
use std::collections::HashMap;

pub const NEST: usize = 16;

fn zerr(e: String) -> Failure {
    Failure::Red(format!("zlib.error: {e}"))
}

/// `vec![0u8; n]` for an LZ4 block whose decoded size is the file's word (a `u32`): over [`inflate::MAX_DECODED`] it is refused with the cap named (review r2, 4.1), not reserved
fn lz4_buffer(n: usize) -> Res<Vec<u8>> {
    if n > inflate::MAX_DECODED {
        red!(
            "LZ4BlockError: {} (a block declares {n})",
            inflate::cap_error(inflate::MAX_DECODED)
        );
    }
    Ok(vec![0u8; n])
}

/// `int.from_bytes(s[4:8], 'big')`: a slice shorter than four bytes is read as it is
pub fn yaz0_size(s: &[u8]) -> usize {
    sl(s, 4, 8).iter().fold(0usize, |a, &b| a << 8 | b as usize)
}

pub fn yaz0(s: &[u8], label: &str) -> Res<Vec<u8>> {
    let n = yaz0_size(s);
    let mut out: Vec<u8> = Vec::with_capacity(n.min(64 << 20));
    let mut si = 16usize;
    macro_rules! byte {
        () => {{
            match s.get(si) {
                Some(&b) => {
                    si += 1;
                    b
                }
                None => red!(
                    "{label}: the Yaz0 stream ends at {:#x} with {:#x} of its {n:#x} bytes decoded",
                    s.len(),
                    out.len()
                ),
            }
        }};
    }
    while out.len() < n {
        let mut h = byte!() as u32;
        for _ in 0..8 {
            if h & 0x80 != 0 {
                let b = byte!();
                out.push(b);
            } else {
                let (b1, b2) = (byte!() as usize, byte!() as usize);
                let (dist, mut ln) = (((b1 & 15) << 8 | b2) + 1, b1 >> 4);
                if ln != 0 {
                    ln += 2;
                } else {
                    ln = byte!() as usize + 0x12;
                }
                if dist > out.len() {
                    red!(
                        "{label}: a back-reference at {si:#x} reaches {:#x} bytes before the start",
                        dist - out.len()
                    );
                }
                let st = out.len() - dist;
                for k in 0..ln {
                    let v = out[st + k];
                    out.push(v);
                }
            }
            if out.len() >= n {
                break;
            }
            h <<= 1;
        }
    }
    out.truncate(n);
    Ok(out)
}

/// `sarc_members(d, label)`: (name, bytes) of each member of a SARC
pub fn sarc_members<'a>(d: &'a [u8], label: &str) -> Res<Vec<(String, &'a [u8])>> {
    field_ok(d, 4, 12, &format!("{label} SARC header"))?;
    let (hl, doff) = (u16le(d, 4)? as usize, u32le(d, 12)? as usize);
    field_ok(d, hl + 6, 2, &format!("{label} SFAT header"))?;
    let n = u16le(d, hl + 6)? as usize;
    let names = hl + 12 + 16 * n + 8;
    if !(names <= doff && doff <= d.len()) {
        red!(
            "{label}: SARC data at {doff:#x}, its {n} entries and the names end at {names:#x}, the archive at {:#x}",
            d.len()
        );
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = hl + 12 + 16 * i;
        let (attr, s, e) = (
            u32le(d, o + 4)? as usize,
            u32le(d, o + 8)? as usize,
            u32le(d, o + 12)? as usize,
        );
        if !(s <= e && e <= d.len() - doff) {
            red!(
                "{label}: SARC member {i} at {s:#x}..{e:#x} of the data lies outside its {:#x} bytes",
                d.len() - doff
            );
        }
        let nm = if attr >> 24 != 0 {
            let o = names + (attr & 0xffffff) * 4;
            let z = find(d, &[0], o);
            String::from_utf8_lossy(sl(d, o, z.unwrap_or(d.len()))).into_owned()
        } else {
            format!("#{i}")
        };
        out.push((nm, sl(d, doff + s, doff + e)));
    }
    Ok(out)
}

pub const GZIP_FLOOR: usize = 1 << 30;

/// `ungzip(d, label)`: validate every gzip member and bound decoded bytes
pub fn ungzip(d: &[u8], label: &str) -> Res<Vec<u8>> {
    let limit = GZIP_FLOOR.max(NEST * d.len());
    let out = inflate::gunzip(d, limit)
        .map_err(|e| Failure::Red(format!("{label}: invalid gzip stream ({e})")))?;
    if out.len() > limit {
        red!("{label}: gzip stream expands beyond its {limit:#x} byte budget");
    }
    Ok(out)
}

// ---- zstd files, dictionaries, size-prefixed frames --------------------------------------------------------------------------------

fn zstd_err(e: String) -> Failure {
    Failure::Red(format!("ZstdError: {e}"))
}

/// `unzs_dict(d, did, where)`: zstd frames that name dictionary did, decompressed with it from ZDICTS
pub fn unzs_dict(cx: &Ctx<'_>, d: &[u8], did: u32, wh: &str) -> Res<Vec<u8>> {
    match cx.zdicts.get(&did) {
        Some(dict) => cx.zstd.decompress(d, Some(dict), None).map_err(zstd_err),
        None => red!(
            "{wh}: a frame needs zstd dictionary {did}, which Pack/ZsDic.pack.zs does not have"
        ),
    }
}

/// `unzs(path)`'s decoding of file bytes: with the frame's dictionary when named
pub fn unzs(cx: &Ctx<'_>, d: &[u8], path: &str) -> Res<Vec<u8>> {
    let did = zstd::dict_id(d);
    if did != 0 {
        unzs_dict(cx, d, did, path).map_err(|e| match e {
            Failure::Red(m) if m.starts_with("ZstdError: ") => Failure::Red(format!(
                "{path}: invalid zstd stream ({})",
                &m["ZstdError: ".len()..]
            )),
            o => o,
        })
    } else {
        cx.zstd
            .decompress(d, None, None)
            .map_err(|e| Failure::Red(format!("{path}: invalid zstd stream ({e})")))
    }
}

/// All of an LZ4 frame, at most `cap` bytes (review r2, 4.1: an LZ4 frame is at most ~255:1, so 4 MB of input can make 1 GiB). Past `cap` it is the `Err` that names it, as for the zlib and zstd decoders; the
/// size-prefixed caller turns an `Err` into a `[RED]` line and reads the file as it is. Under `cap` the bytes are the ones `read_to_end` gave. (`cap` is a parameter for the tests.)
pub fn lz4_frame_capped(f: &[u8], cap: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut v = Vec::new();
    // one byte more than the cap is read, to tell "fits exactly" from "passes it"
    lz4_flex::frame::FrameDecoder::new(f)
        .take((cap as u64).saturating_add(1))
        .read_to_end(&mut v)
        .map_err(|e| e.to_string())?;
    if v.len() > cap {
        return Err(inflate::cap_error(cap));
    }
    Ok(v)
}

/// `unsized(d, where)`: a size-prefixed frame (HAL's .cmp containers) → the frame decompressed; bytes that are not such a container are returned as they are
pub fn unsized_frame<'a>(cx: &Ctx<'_>, d: &'a [u8], wh: &str) -> Res<std::borrow::Cow<'a, [u8]>> {
    use std::borrow::Cow;
    let n = u32le(d, 0)? as u64;
    let f = &d[4..];
    if let Some(fs) = zstd::frame_size(f)
        && fs != n
    {
        return Ok(Cow::Borrowed(d));
    }
    let did = zstd::dict_id(f);
    let result: Result<Option<Vec<u8>>, String> = if f.len() >= 4 && f[..4] == zstd::LZ4_FRAME_MAGIC
    {
        lz4_frame_capped(f, inflate::MAX_DECODED).map(Some)
    } else if did != 0 && !cx.zdicts.contains_key(&did) {
        Ok(None)
    } else if did != 0 {
        cx.zstd
            .decompress(f, cx.zdicts.get(&did).map(|v| v.as_slice()), None)
            .map(Some)
    } else {
        cx.zstd.decompress(f, None, None).map(Some)
    };
    let why = match result {
        Ok(None) => {
            format!("the frame needs zstd dictionary {did}, which Pack/ZsDic.pack.zs does not have")
        }
        Ok(Some(dec)) => {
            if dec.len() as u64 == n {
                return Ok(Cow::Owned(dec));
            }
            format!(
                "the size prefix says {n} bytes, the frame holds {}",
                dec.len()
            )
        }
        Err(e) => e,
    };
    cx.print(&format!(
        "[RED] {wh}: a size-prefixed frame that did not open ({why}): the file is read as it is"
    ));
    Ok(Cow::Borrowed(d))
}

// ---- PACx 403 ---------------------------------------------------------------------------------------------------------------------

pub const PACX4: &[u8] = b"PACx403L";
pub const PACX4_ROOT: &[u8] = b"PACx402L";

fn pacx4_unchunk(
    d: &[u8],
    mut a: usize,
    chunks: &[(usize, usize)],
    n: usize,
    wh: &str,
) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    for &(c, u) in chunks {
        let raw = span(d, a, c, wh)?;
        if c == u {
            out.extend_from_slice(raw);
        } else {
            let mut buf = lz4_buffer(u)?;
            let k = lz4_flex::block::decompress_into(raw, &mut buf)
                .map_err(|e| Failure::Red(format!("LZ4BlockError: {e}")))?;
            buf.truncate(k);
            out.extend_from_slice(&buf);
        }
        a += c;
    }
    if out.len() != n {
        red!(
            "{wh}: its chunks give {:#x} bytes, its length is {n:#x}",
            out.len()
        );
    }
    Ok(out)
}

/// `pacx4_parts(d, label)`: (name, bytes) of the root and of each block of a PACx 403 archive whose root names a shader type
pub fn pacx4_parts(d: &[u8], label: &str) -> Res<Vec<(String, Vec<u8>)>> {
    let h = span(d, 0x10, 12, &format!("{label}: PACx 403 header"))?;
    let (roff, rstored, rlen) = (
        u32le(h, 0)? as usize,
        u32le(h, 4)? as usize,
        u32le(h, 8)? as usize,
    );
    let (deps, ctab) = (u32le(d, 0x20)? as usize, u32le(d, 0x24)? as usize);
    let nb = span(
        d,
        0x30 + deps,
        4,
        &format!("{label}: PACx 403 root chunk table"),
    )?;
    let n = u32le(nb, 0)? as usize;
    if n == 0 || 4 + 8 * n > ctab {
        red!("{label}: PACx 403 root chunk count {n} in a table of {ctab:#x} bytes");
    }
    let mut chunks = Vec::new();
    for i in 0..n {
        chunks.push((
            u32le(d, 0x34 + deps + 8 * i)? as usize,
            u32le(d, 0x38 + deps + 8 * i)? as usize,
        ));
    }
    if chunks.iter().map(|c| c.0).sum::<usize>() != rstored {
        red!("{label}: the PACx 403 root chunks do not add up to its stored length {rstored:#x}");
    }
    let root = pacx4_unchunk(d, roff, &chunks, rlen, &format!("{label}: PACx 403 root"))?;
    if !starts(&root, PACX4_ROOT) {
        red!("{label}: the PACx 403 root is not a PACx 402 archive");
    }
    if !contains(&root, b"Shader") {
        return Ok(vec![]); // the type tree names no shader type
    }
    let (trees, info) = (u32le(&root, 0x10)? as usize, u32le(&root, 0x14)? as usize);
    let mut out = vec![("root".to_string(), root.clone())];
    if info == 0 {
        return Ok(out);
    }
    let t = span(
        &root,
        0x30 + trees,
        16,
        &format!("{label}: PACx 403 block table"),
    )?;
    let (nblocks, tab) = (u64le(t, 0)? as usize, u64le(t, 8)? as usize);
    let mut blocks = Vec::new();
    for i in 0..nblocks {
        let b = span(
            &root,
            tab + 0x20 * i,
            0x20,
            &format!("{label}: PACx 403 block {i}"),
        )?;
        blocks.push((
            u64le(b, 0)? as usize,
            u32le(b, 8)? as usize,
            u32le(b, 12)? as usize,
            u32le(b, 16)? as usize,
            u32le(b, 20)? as usize,
            u64le(b, 24)? as usize,
        ));
    }
    if 0x10 + blocks.iter().map(|b| 0x20 + 8 * b.4).sum::<usize>() != info {
        red!("{label}: the PACx 403 block table is not {info:#x} bytes");
    }
    for (i, (name, stored, ln, off, nch, cptr)) in blocks.into_iter().enumerate() {
        let nm = if name < root.len() {
            let z = find(&root, &[0], name).unwrap_or(root.len());
            String::from_utf8_lossy(&root[name..z]).into_owned()
        } else {
            i.to_string()
        };
        let mut cks = Vec::new();
        for k in 0..nch {
            let b = span(&root, cptr + 8 * k, 8, &format!("{label}/{nm}: chunk {k}"))?;
            cks.push((u32le(b, 0)? as usize, u32le(b, 4)? as usize));
        }
        if cks.iter().map(|c| c.0).sum::<usize>() != stored {
            red!("{label}/{nm}: the chunks do not add up to the block's stored length {stored:#x}");
        }
        out.push((
            nm.clone(),
            pacx4_unchunk(d, off, &cks, ln, &format!("{label}/{nm}"))?,
        ));
    }
    Ok(out)
}

// ---- MT Framework ARC, Bezel Engine BEA, Game Freak GFPAK ----------------------------------------------------------------------------

pub const MTARC: &[u8] = b"ARC\0";
pub const MTSPK: &[u8] = b"SPK\0";

/// `mtarc_members(d, label)`: (path, data) of each entry of Capcom's MT Framework archive
pub fn mtarc_members(d: &[u8], label: &str) -> Res<Vec<(String, Vec<u8>)>> {
    field_ok(d, 4, 4, label)?;
    let (ver, n) = (u16le(d, 4)?, u16le(d, 6)? as usize);
    if ver != 7 {
        red!("{label}: ARC version {ver}: only version 7 is known");
    }
    if n == 0 {
        return Ok(vec![]);
    }
    // The version does not name the width. Check every member against the end of its candidate directory and the archive
    let directory_rank = |width: usize| -> Res<i32> {
        let step = width + 16;
        let table_end = 8 + n * step;
        if table_end > d.len() {
            return Ok(-1);
        }
        let mut has_data = false;
        let mut o = 8;
        while o < table_end {
            let (stored, size, offset) = (
                u32le(d, o + width + 4)? as usize,
                u32le(d, o + width + 8)? as usize,
                u32le(d, o + width + 12)? as usize,
            );
            o += step;
            if stored == 0 && size & 0x1fffffff == 0 && offset == 0 {
                continue; // an empty member needs no offset
            }
            if offset < table_end || offset + stored > d.len() {
                return Ok(-1);
            }
            has_data |= stored != 0;
        }
        Ok(has_data as i32)
    };
    let wide = directory_rank(128)?;
    let width = if wide == 1 {
        128
    } else {
        let short = directory_rank(64)?;
        if wide < 0 && short < 0 {
            red!("{label}: ARC v7 has no complete directory with in-bounds members");
        }
        if wide >= short { 128 } else { 64 }
    };
    let mut out = Vec::new();
    for i in 0..n {
        let o = 8 + i * (width + 16);
        let pb = at(d, o, width);
        let pb = pb.split(|&b| b == 0).next().unwrap_or(&[]);
        let path: String = pb.iter().map(|&b| b as char).collect(); // latin-1
        let (csz, usz, off) = (
            u32le(d, o + width + 4)? as usize,
            (u32le(d, o + width + 8)? & 0x1fffffff) as usize,
            u32le(d, o + width + 12)? as usize,
        );
        let mut data = at(d, off, csz).to_vec();
        if csz != usz {
            data = inflate::zlib_decompress(&data).map_err(|e| {
                Failure::Red(format!("{label}#{path}: invalid ARC zlib stream: {e}"))
            })?;
        }
        if data.len() != usz {
            red!(
                "{label}#{path}: {:#x} bytes, the entry says {usz:#x}",
                data.len()
            );
        }
        out.push((path.replace('\\', "/"), data));
    }
    Ok(out)
}

/// `mtspk_shaders(d, label)`: MT Framework's shader package ('SPK\0')
pub fn mtspk_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out: Vec<Shader> = Vec::new();
    let mut end: Option<usize> = None;
    let mut c = match find(d, &CTL_MAGIC, 0) {
        Some(c) => c,
        None => red!("{label} SPK: no shader control section"),
    };
    field_ok(d, c + 0x704, 4, &format!("{label} SPK"))?;
    let ncode0 = u32le(d, c + 0x704)? as usize;
    // In a code-first package the next shader's code also follows this control. Its *own* code comes before it at the declared distance.
    let control_first = c < ncode0 || !eq_at(d, c - ncode0, &GFX_MAGIC);
    loop {
        if c + 0x708 > d.len() || !eq_at(d, c, &CTL_MAGIC) {
            red!("{label} SPK: no complete control header at {c:#x}");
        }
        let (nctl, ncode) = (u32le(d, c + 0x20)? as usize, u32le(d, c + 0x704)? as usize);
        let g: i64 = if control_first {
            (c + nctl) as i64
        } else {
            c as i64 - ncode as i64
        };
        let start = if control_first { c as i64 } else { g };
        if nctl < 0x708
            || g < 0
            || c + nctl > d.len()
            || g as usize + ncode > d.len()
            || !eq_at(d, g as usize, &GFX_MAGIC)
        {
            red!("{label} SPK: no matching graphics code area for control at {c:#x}");
        }
        let g = g as usize;
        if let Some(e) = end
            && start != e as i64
        {
            red!("{label} SPK: noncontiguous shader at {start:#x} after {e:#x}");
        }
        let (ctl, code) = (at(d, c, nctl), at(d, g, ncode));
        let st = sph_stage(code);
        let why = match st {
            Some(st) => check(st, ctl, code)?.map(str::to_string),
            None => Some("no stage in the SPH".to_string()),
        };
        if let Some(w) = why {
            red!("{label} SPK @{start:#x}: {w}");
        }
        out.push(Shader::new(
            format!("{label}#MTSPK@{start:#x}"),
            st.unwrap(),
            ctl,
            code,
        ));
        let e = if control_first { g + ncode } else { c + nctl };
        end = Some(e);
        if e == d.len() {
            return Ok(out);
        }
        match if control_first {
            Some(e)
        } else {
            find(d, &CTL_MAGIC, e)
        } {
            Some(nc) => c = nc,
            None => break,
        }
    }
    red!(
        "{label} SPK: last shader ends at {}, before the end of the package",
        end.unwrap_or(0)
    )
}

pub fn mtarc_shaders(d: &[u8], label: &str, out: &mut Vec<Shader>) -> Res<()> {
    for (path, m) in mtarc_members(d, label)? {
        if starts(&m, MTSPK) {
            out.extend(mtspk_shaders(&m, &format!("{label}#{path}"))?);
        }
    }
    Ok(())
}

/// `bea_members(d, label)`: (path, data) of each entry of a NintendoWare Bezel Engine archive
pub fn bea_members(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<(String, Vec<u8>)>> {
    let (n, arr) = (u64le(d, 0x20)? as usize, u64le(d, 0x28)? as usize);
    let inline = eq_at(d, arr, b"ASST");
    let mut o = arr;
    let mut out = Vec::new();
    for i in 0..n {
        if !inline {
            o = u64le(d, arr + 8 * i)? as usize;
        }
        if !eq_at(d, o, b"ASST") {
            red!("{label}: entry {i} @{o:#x} is not an ASST entry");
        }
        let size = if inline {
            u32le(d, o + 4)? as usize
        } else {
            0x30
        };
        let (csz, usz) = (u32le(d, o + 0x14)? as usize, u32le(d, o + 0x18)? as usize);
        let (off, nm) = (
            u64le(d, o + size - 0x10)? as usize,
            u64le(d, o + size - 8)? as usize,
        );
        if inline {
            o += size;
        }
        let nl = u16le(d, nm)? as usize;
        let path = String::from_utf8_lossy(at(d, nm + 2, nl)).into_owned();
        let mut data = at(d, off, csz).to_vec();
        if data.len() >= 4 && data[..4] == ZSTD_MAGIC {
            data = cx.zstd.decompress(&data, None, None).map_err(zstd_err)?;
        }
        if data.len() != usz {
            red!(
                "{label}/{path}: {:#x} bytes, the entry says {usz:#x}",
                data.len()
            );
        }
        out.push((path, data));
    }
    Ok(out)
}

pub const GFPAK: &[u8] = b"GFLXPACK";

/// `gfpak_members(d, label)`: (#index, data) of each entry of a Game Freak GFPAK archive (Oodle Kraken entries are not decoded here)
pub fn gfpak_members(d: &[u8], label: &str) -> Res<Vec<(String, Vec<u8>)>> {
    if !eq_at(d, 8, b"\0\x10\0\0") {
        red!("{label}: GFPAK version {}, not 0x1000", hex(at(d, 8, 4)));
    }
    let (n, files) = (u32le(d, 0x10)? as usize, u64le(d, 0x18)? as usize);
    let mut out = Vec::new();
    for i in 0..n {
        let b = files + 0x18 * i;
        let (ct, usz, csz, off) = (
            u16le(d, b + 2)?,
            u32le(d, b + 4)? as usize,
            u32le(d, b + 8)? as usize,
            u64le(d, b + 16)? as usize,
        );
        let raw = at(d, off, csz);
        let data: Vec<u8> = match ct {
            2 => {
                let mut buf = lz4_buffer(usz)?;
                let k = lz4_flex::block::decompress_into(raw, &mut buf)
                    .map_err(|e| Failure::Red(format!("LZ4BlockError: {e}")))?;
                buf.truncate(k);
                buf
            }
            1 => inflate::zlib_decompress(raw).map_err(zerr)?,
            0 => raw.to_vec(),
            3 => unsupported!(
                "Oodle Kraken (GFPAK entry {i} of {label}: the game's own decoder in the Mac host)"
            ),
            _ => red!("{label}: entry {i} has compression {ct}, which this reader does not decode"),
        };
        if data.len() != usz {
            red!(
                "{label}#{i}: {:#x} bytes, the entry says {usz:#x}",
                data.len()
            );
        }
        out.push((format!("#{i}"), data));
    }
    Ok(out)
}

// ---- Criterion bnd2, Heaps PAK -----------------------------------------------------------------------------------------------------

/// `BND2_LAYOUTS`: version -> (blocks per resource, entry size): 2 in Burnout Paradise Remastered, 3 in Need for Speed: Hot Pursuit Remastered
fn bnd2_layout(ver: usize) -> (usize, usize) {
    match ver {
        2 => (3, 0x40),
        3 => (4, 0x50),
        _ => (0, 0),
    }
}

/// One BND2 resource: (id, flags, index, bytes).
pub type Bnd2Resource = (u64, u32, usize, Vec<u8>);

/// `bnd2_resources(d, label)`: the resources of a Criterion Bundle 2 → [(resource ID, type, block, data)]. The version sets the number of blocks a resource has, B (3 or 4), and every offset
/// after the first array follows from it: header +0x04 version, +0x10 entry count, +0x14 entry table offset, +0x18 B block data offsets (u32), then the flags (bit 0: zlib-compressed);
/// an entry (0x40 bytes in v2, 0x50 in v3): +0x00 resource ID u64, +0x10 B uncompressed sizes, then B sizes on disk, then B offsets, then the import offset and the type ID
pub fn bnd2_resources(d: &[u8], label: &str) -> Res<Vec<Bnd2Resource>> {
    field_ok(d, 4, 20, &format!("{label}: bnd2 header"))?;
    let g = |i: usize| u32le(d, 4 + 4 * i).map(|v| v as usize);
    let (ver, cnt, eoff) = (g(0)?, g(3)?, g(4)?);
    let (blocks, entry) = bnd2_layout(ver);
    let table_end = eoff + cnt * entry;
    if blocks == 0 || eoff < 0x30 || table_end > d.len() {
        red!(
            "{label}: bnd2 version {ver} with {cnt} entries at {eoff:#x} is not a layout this reader knows"
        );
    }
    field_ok(d, 0x18, 4 * (blocks + 1), &format!("{label}: bnd2 header"))?;
    let datoff: Vec<usize> = (0..blocks)
        .map(|k| u32le(d, 0x18 + 4 * k).map(|v| v as usize))
        .collect::<Res<_>>()?;
    let flags = u32le(d, 0x18 + 4 * blocks)? as usize;
    let mut out = Vec::new();
    for i in 0..cnt {
        let e = eoff + i * entry;
        let rid = u64le(d, e)?;
        let typ = u32le(d, e + 0x14 + 12 * blocks)?;
        let arr = |a: usize| -> Res<Vec<usize>> {
            (0..blocks)
                .map(|k| u32le(d, e + 0x10 + 4 * blocks * a + 4 * k).map(|v| v as usize))
                .collect()
        };
        let (unpacked, sizes, offs) = (arr(0)?, arr(1)?, arr(2)?);
        for k in 0..blocks {
            let n = sizes[k] & 0x0FFF_FFFF;
            let expected = unpacked[k] & 0x0FFF_FFFF;
            let wh = format!("{label}: bnd2 resource {rid:016x} block {k}");
            if n == 0 {
                if expected != 0 {
                    red!("{wh}: declares {expected} bytes and has none stored");
                }
                continue;
            }
            let start = datoff[k] + offs[k];
            if start < table_end {
                red!("{wh}: its data at {start:#x} starts inside the resource table");
            }
            let mut raw = span(d, start, n, &wh)?.to_vec();
            if flags & 1 != 0 {
                let z = zlibx::decompress(&raw, Some(expected + 1))
                    .map_err(|e| Failure::Red(format!("{wh}: damaged zlib stream ({e})")))?;
                if !z.eof || z.out.len() != expected {
                    red!(
                        "{wh}: incomplete zlib stream or decompressed size differs from declared size {expected}"
                    );
                }
                raw = z.out;
            } else if raw.len() != expected {
                red!("{wh}: stored size differs from declared size {expected}");
            }
            out.push((rid, typ, k, raw));
        }
    }
    Ok(out)
}

pub fn bnd2_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut found = Vec::new();
    for (rid, typ, k, r) in bnd2_resources(d, label)? {
        if contains(&r, &GLSLC_MAGIC) {
            found.extend(crate::glslc::glslc_shaders(
                &r,
                &format!("{label}/{rid:016x}.{typ:x}.{k}"),
                false,
                &[],
            )?);
        }
    }
    Ok(found)
}

/// Python's `base64.b64decode(s)` for text matching `[A-Za-z0-9+/]+={0,2}`
pub fn b64decode(s: &[u8]) -> Result<Vec<u8>, String> {
    let pads = s.iter().rev().take_while(|&&c| c == b'=').count();
    let body = &s[..s.len() - pads];
    if !(body.len() + pads).is_multiple_of(4) {
        return Err("Incorrect padding".into());
    }
    if body.len() % 4 == 1 {
        return Err("Invalid base64-encoded string: number of data characters cannot be 1 more than a multiple of 4".into());
    }
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            _ => 63,
        }
    };
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for ch in body.chunks(4) {
        let mut n = 0u32;
        for &c in ch {
            n = n << 6 | val(c);
        }
        n <<= 6 * (4 - ch.len()) as u32;
        let b = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&b[..(ch.len() * 6 / 8)]);
    }
    Ok(out)
}

pub const HEAPS_CACHE: &str = "shaders.cache.nx";

/// `heaps_pak_files(f)`: the files of a Heaps PAK archive, open at +0 → [(path, offset in the archive, size)], or None when it is not one
pub fn heaps_pak_files(src: &dyn crate::source::Source) -> Res<Option<Vec<(String, u64, u32)>>> {
    let head = src
        .read(0, 12)
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    if head.len() < 12 || &head[..3] != b"PAK" || head[3] > 1 {
        return Ok(None);
    }
    let (size, dsize) = (i32le(&head, 4)?, i32le(&head, 8)?);
    if !(12 < size && size <= 1 << 28) || dsize < 0 {
        return Ok(None);
    }
    let size = size as usize;
    let h = src
        .read(12, size - 12)
        .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    let mut out = Vec::new();
    fn entry(
        h: &[u8],
        p: usize,
        path: &str,
        size: usize,
        out: &mut Vec<(String, u64, u32)>,
        depth: usize,
    ) -> Option<usize> {
        if depth > 200 {
            return None;
        }
        let n = *h.get(p)? as usize;
        let name = std::str::from_utf8(h.get(p + 1..p + 1 + n)?).ok()?;
        let fl = *h.get(p + 1 + n)?;
        let mut p = p + n + 2;
        let path = if name.is_empty() {
            path.to_string()
        } else {
            format!("{path}/{name}")
        };
        if fl & 1 != 0 {
            let cnt = i32::from_le_bytes(h.get(p..p + 4)?.try_into().ok()?);
            p += 4;
            for _ in 0..cnt.max(0) {
                p = entry(h, p, &path, size, out, depth + 1)?;
            }
            return Some(p);
        }
        let pos: i64 = if fl & 2 != 0 {
            let v = f64::from_le_bytes(h.get(p..p + 8)?.try_into().ok()?);
            p += 8;
            v as i64
        } else {
            let v = i32::from_le_bytes(h.get(p..p + 4)?.try_into().ok()?);
            p += 4;
            v as i64
        };
        let n = i32::from_le_bytes(h.get(p..p + 4)?.try_into().ok()?);
        out.push((path, (size as i64 + pos) as u64, n as u32));
        Some(p + 8)
    }
    let end = match entry(&h, if head[3] != 0 { 64 } else { 0 }, "", size, &mut out, 0) {
        Some(e) => e,
        None => return Ok(None),
    };
    Ok(if eq_at(&h, end, b"DATA") {
        Some(out)
    } else {
        None
    })
}

/// `heaps_shaders(d, label)`: Heaps' hxsl.CacheFile for the Switch: a GLSL that was compiled ends with a comment holding the base64 of its GLSLC output, `//BIN=<base64>#`
pub fn heaps_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut found = Vec::new();
    for m in find_all(d, b"//BIN=") {
        let s = m + 6;
        let mut e = s;
        while e < d.len() && (d[e].is_ascii_alphanumeric() || d[e] == b'+' || d[e] == b'/') {
            e += 1;
        }
        if e == s {
            continue;
        }
        let mut pe = e;
        while pe < d.len() && pe < e + 2 && d[pe] == b'=' {
            pe += 1;
        }
        if d.get(pe) != Some(&b'#') {
            continue;
        }
        let bin = b64decode(&d[s..pe]).map_err(|e| Failure::Red(format!("binascii.Error: {e}")))?;
        found.extend(crate::glslc::glslc_shaders(
            &bin,
            &format!("{label}@{m:#x}"),
            false,
            &[],
        )?);
    }
    if found.is_empty() {
        red!("{label}: no GLSLC output in //BIN= comments");
    }
    Ok(found)
}

// ---- Koei Tecmo's chunked zlib ---------------------------------------------------------------------------------------------------

/// `kt_gz(d)`: the older KTGL's chunked zlib → the inflated data, or None when d is not in that form
pub fn kt_gz(d: &[u8]) -> Option<Vec<u8>> {
    if d.len() < 16 {
        return None;
    }
    let (cs, n, tot) = (
        u32le(d, 0).ok()? as usize,
        u32le(d, 4).ok()? as usize,
        u32le(d, 8).ok()? as usize,
    );
    if cs < 0x10000 || cs & (cs - 1) != 0 || n == 0 || n != tot.div_ceil(cs) || 12 + 4 * n > d.len()
    {
        return None;
    }
    let mut sizes = Vec::new();
    for i in 0..n {
        sizes.push(u32le(d, 12 + 4 * i).ok()? as usize);
    }
    let mut o = (12 + 4 * n + 0x7f) & !0x7f;
    let mut out = Vec::new();
    for (i, &s) in sizes.iter().enumerate() {
        if o + s > d.len() {
            return None;
        }
        if s == cs.min(tot - i * cs) {
            out.extend_from_slice(&d[o..o + s]);
        } else {
            let ln = u32le(d, o).ok()? as usize;
            out.extend_from_slice(&inflate::zlib_decompress(at(d, o + 4, ln)).ok()?);
        }
        o = (o + s + 0x7f) & !0x7f;
    }
    Some(out)
}

/// `gust_gz(d)`: Gust's chunked zlib → the inflated data, or None when d is not in that form
pub fn gust_gz(d: &[u8]) -> Option<Vec<u8>> {
    let (mut o, mut out) = (0usize, Vec::new());
    while o + 4 <= d.len() {
        let n = u32le(d, o).ok()? as usize;
        o += 4;
        if n == 0 {
            return if !out.is_empty() && o == d.len() {
                Some(out)
            } else {
                None
            };
        }
        if o + n > d.len() || d[o] != 0x78 {
            return None;
        }
        let z = inflate::run(&d[o..o + n], Wrap::Zlib, None).ok()?;
        if !z.eof || z.used != n {
            return None;
        }
        out.extend_from_slice(&z.out);
        o += n;
    }
    None
}

/// `kt_shaders(d, label, out)`: Three Houses' KTGL: every inflated file or entry is searched for GLSLC outputs, and its SARC archives for BNSH
pub fn kt_shaders(cx: &Ctx<'_>, d: &[u8], label: &str, out: &mut Vec<Shader>) -> Res<()> {
    let z = kt_gz(d);
    let d: &[u8] = z.as_deref().unwrap_or(d);
    if contains(d, &GLSLC_MAGIC) {
        out.extend(crate::glslc::glslc_shaders(d, label, false, &[])?);
    }
    if starts(d, b"SARC") || contains(d, b"BNSH\0\0\0\0") {
        crate::scan::scan_blob(cx, d, label, out, 0, None)?;
    }
    Ok(())
}

// ---- Monolith ------------------------------------------------------------------------------------------------------------------

/// `xbc_data(d, off)`: an XBC1 stream: 48-byte header, stored / zlib / zstd payload → (decoded bytes, end), or None for bytes that merely resemble a header
fn xbc_data(cx: &Ctx<'_>, d: &[u8], off: usize) -> Res<Option<(Vec<u8>, usize)>> {
    if off + 48 > d.len() || !eq_at(d, off, b"xbc1") {
        return Ok(None);
    }
    let (kind, raw, size) = (
        u32le(d, off + 4)?,
        u32le(d, off + 8)? as usize,
        u32le(d, off + 12)? as usize,
    );
    if !matches!(kind, 0 | 1 | 3) || size == 0 || off + 48 + size > d.len() {
        return Ok(None);
    }
    let data = &d[off + 48..off + 48 + size];
    let data: Vec<u8> = match kind {
        1 => inflate::zlib_decompress(data).map_err(zerr)?,
        3 => cx.zstd.decompress(data, None, None).map_err(zstd_err)?,
        _ => data.to_vec(),
    };
    if data.len() != raw {
        red!(
            "XBC1 @{off:#x}: expected {raw} bytes, decoded {}",
            data.len()
        );
    }
    Ok(Some((data, off + 48 + size)))
}

/// `monolith_cmp_data(d, off)`: Firehawk effect archives: cmp\0zlib, stored/raw sizes, zlib payload
fn monolith_cmp_data(d: &[u8], off: usize) -> Res<Option<(Vec<u8>, usize)>> {
    if off + 16 > d.len() || !eq_at(d, off, b"cmp\0zlib") {
        return Ok(None);
    }
    let (size, raw) = (u32le(d, off + 8)? as usize, u32le(d, off + 12)? as usize);
    if size == 0 || size > d.len() - off - 16 {
        return Ok(None);
    }
    let z = inflate::run(&d[off + 16..off + 16 + size], Wrap::Zlib, None)
        .map_err(|e| Failure::Red(format!("cmp zlib @{off:#x}: damaged payload ({e})")))?;
    if !z.eof || z.used != size || z.out.len() != raw {
        red!("cmp zlib @{off:#x}: payload does not match stored/raw sizes");
    }
    Ok(Some((z.out, off + 16 + size)))
}

/// `monolith_shaders(d, label, out)`: decoded archive entries and their XBC1 streams, without relying on extensions
pub fn monolith_shaders(
    cx: &Ctx<'_>,
    d: &[u8],
    label: &str,
    out: &mut Vec<Shader>,
    depth: usize,
) -> Res<()> {
    if depth > 64 {
        red!("{label}: XBC1 streams nest too deep");
    }
    let mut pos = 0usize;
    let mut compressed: Vec<(usize, usize)> = Vec::new();
    let mut markers: Vec<(usize, bool, usize)> = Vec::new();
    for (magic, xbc) in [(&b"xbc1"[..], true), (&b"cmp\0zlib"[..], false)] {
        for m in find_all(d, magic) {
            markers.push((m, xbc, magic.len()));
        }
    }
    markers.sort();
    for (off, xbc, size) in markers {
        if off < pos {
            continue;
        }
        let result = if xbc {
            xbc_data(cx, d, off)?
        } else {
            monolith_cmp_data(d, off)?
        };
        match result {
            None => pos = off + size,
            Some((data, end)) => {
                pos = end;
                compressed.push((off, pos));
                monolith_shaders(
                    cx,
                    &data,
                    &format!("{label}#{}@{off:#x}", if xbc { "XBC1" } else { "cmp-zlib" }),
                    out,
                    depth + 1,
                )?;
            }
        }
    }
    let mut ci = 0usize;
    for base in find_all(d, b"HCPS") {
        while ci < compressed.len() && compressed[ci].1 <= base {
            ci += 1;
        }
        if ci < compressed.len() && compressed[ci].0 <= base {
            continue; // a zstd literal is not a decoded container
        }
        if base + 8 <= d.len() && matches!(u32le(d, base + 4)?, 1000 | 1010) {
            out.extend(hcps_shaders(d, base, label)?);
        }
    }
    // Effect asset packs keep complete GLSLC outputs alongside HCPS programs
    out.extend(crate::glslc::glslc_shaders(d, label, false, &compressed)?);
    Ok(())
}

// ---- Zs dictionaries --------------------------------------------------------------------------------------------------------------

/// `zs_dicts(romfs)`'s decoding of `Pack/ZsDic.pack.zs`: a SARC (itself plain zstd) of `*.zsdic` files → {ID: dictionary}
pub fn zs_dicts_from(zstd_impl: &dyn crate::zstd::Zstd, file: &[u8]) -> Res<HashMap<u32, Vec<u8>>> {
    let d = zstd_impl.decompress(file, None, None).map_err(zstd_err)?;
    let mut out = HashMap::new();
    for (_, m) in sarc_members(&d, "/Pack/ZsDic.pack.zs")? {
        if m.len() >= 4 && m[..4] == [0x37, 0xa4, 0x30, 0xec] {
            out.insert(u32le(m, 4)?, m.to_vec());
        }
    }
    Ok(out)
}

// ---- XNB -----------------------------------------------------------------------------------------------------------------------

/// `xnb_decompress(d, first_block)`: the decompressed content of a compressed XNB (flags 0x80 LZX, 0x40 LZ4)
pub fn xnb_decompress(d: &[u8], label: &str) -> Res<Vec<u8>> {
    let dec_size = u32le(d, 10)? as usize;
    let body = sl(d, 14, u32le(d, 6)? as usize);
    if d[5] & 0x80 != 0 {
        unsupported!("XNB with LZX compression (MonoGame/XNA ContentCompressedLzx, {label})");
    }
    if d[5] & 0x40 != 0 {
        let mut buf = lz4_buffer(dec_size)?;
        let k = lz4_flex::block::decompress_into(body, &mut buf)
            .map_err(|e| Failure::Red(format!("LZ4BlockError: {e}")))?;
        buf.truncate(k);
        return Ok(buf);
    }
    Ok(sl(d, 14, d.len()).to_vec())
}

// ---- Koei Tecmo RDB (IDRK / PDRK), UbiArt IPK, Double Eleven packs ---------------------------------------------------------------------

pub const IDRK: &[u8] = b"IDRK0000";
pub const PDRK: &[u8] = b"PDRK0000";

/// `idrk_entries(d)` over a source: (start of the stored data, its end, flags) of each entry. `find` of the next entry's magic goes in chunks.
pub fn idrk_entries(src: &dyn crate::source::Source) -> Res<Vec<(u64, u64, u32)>> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let len = src.len();
    let (mut o, mut out) = (0u64, Vec::new());
    while o + 0x30 <= len {
        let head = src.read(o, 8).map_err(io)?;
        if head != IDRK {
            match crate::source::find_from(src, IDRK, o).map_err(io)? {
                Some(p) => o = p,
                None => return Ok(out),
            }
        }
        let h = src.read(o + 8, 0x28).map_err(io)?;
        if h.len() < 0x28 {
            return Err(short("idrk"));
        }
        let (esz, csz) = (u64le(&h, 0)?, u64le(&h, 8)?);
        out.push(((o + esz).saturating_sub(csz), o + esz, u32le(&h, 0x24)?));
        o += esz;
        if esz == 0 {
            red!("IDRK entry of length 0 at {o:#x}");
        }
    }
    Ok(out)
}

/// `idrk_data(d, a, b, flags, head)`: an entry's data (head: only the first bytes)
pub fn idrk_data(
    src: &dyn crate::source::Source,
    mut a: u64,
    b: u64,
    flags: u32,
    head: Option<usize>,
) -> Res<Vec<u8>> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let (new, lz4b) = (flags & 0x400000 != 0, flags & 0x500000 == 0x500000);
    if flags & 0x500000 == 0 {
        let end = match head {
            Some(h) if h != 0 => b.min(a + h as u64),
            _ => b,
        };
        return src.read(a, end.saturating_sub(a) as usize).map_err(io);
    }
    let mut out: Vec<u8> = Vec::new();
    while a + 4 <= b {
        let cs;
        if new {
            cs = u16le(&src.read(a, 2).map_err(io)?, 0)? as u64;
            a += 10;
        } else {
            cs = u32le(&src.read(a, 4).map_err(io)?, 0)? as u64;
            a += 4;
        }
        if cs == 0 {
            break;
        }
        let raw = src.read(a, cs as usize).map_err(io)?;
        let r: Vec<u8>;
        if lz4b {
            let mut buf = vec![0u8; 0x4000];
            let k = lz4_flex::block::decompress_into(&raw, &mut buf)
                .map_err(|e| Failure::Red(format!("LZ4BlockError: {e}")))?;
            buf.truncate(k);
            if let Some(h) = head.filter(|&h| h != 0) {
                return Ok(buf[..h.min(buf.len())].to_vec());
            }
            r = buf;
        } else if let Some(h) = head.filter(|&h| h != 0) {
            let z = inflate::run(at(&raw, 0, 1024.min(cs as usize)), Wrap::Zlib, Some(h))
                .map_err(zerr)?;
            return Ok(z.out);
        } else {
            r = inflate::zlib_decompress(&raw).map_err(zerr)?;
        }
        out.extend_from_slice(&r);
        a += cs;
    }
    Ok(out)
}

/// `idrk_shaders(path, label, out)`: G2S containers and bare GLSLC outputs in the entries of a Koei Tecmo RDB file
pub fn idrk_shaders(
    src: &dyn crate::source::Source,
    label: &str,
    out: &mut Vec<Shader>,
) -> Res<()> {
    for (a, b, flags) in idrk_entries(src)? {
        let h = idrk_data(src, a, b, flags, Some(4))?;
        if h == b"_S2G" || h == b"_DOK" {
            out.extend(crate::glslc::glslc_shaders(
                &idrk_data(src, a, b, flags, None)?,
                &format!("{label}#IDRK@{a:#x}"),
                false,
                &[],
            )?);
        }
    }
    Ok(())
}

/// `ipk_shaders(path, rel, out)`: the `.fxb` effect files of a UbiArt IPK archive (big-endian, version 7), each scanned like a file
pub fn ipk_shaders(
    cx: &Ctx<'_>,
    src: &dyn crate::source::Source,
    rel: &str,
    out: &mut Vec<Shader>,
) -> Res<()> {
    let io = |e: std::io::Error| Failure::Red(format!("OSError: {e}"));
    let label = format!("/{rel}");
    let h = src.read(0, 0x38).map_err(io)?;
    if h.len() < 0x38 {
        red!(
            "{label}: {} bytes, shorter than a UbiArt IPK header (0x38)",
            h.len()
        );
    }
    let (ver, base, n) = (
        u32be(&h, 4)?,
        u32be(&h, 12)? as u64,
        u32be(&h, 16)? as usize,
    );
    if ver != 7 {
        red!("{label}: UbiArt IPK version {ver} (only version 7 is read)");
    }
    let t = src
        .read(0x38, base.saturating_sub(0x38) as usize)
        .map_err(io)?;
    if base < 0x38 || t.len() as u64 != base - 0x38 {
        red!("{label}: the IPK header puts its data at {base:#x}, outside the file");
    }
    let mut p = 0usize;
    for i in 0..n {
        let r = (|| -> Res<(String, u32, u32, u64)> {
            let (noff, size, csize) =
                (u32be(&t, p)? as usize, u32be(&t, p + 4)?, u32be(&t, p + 8)?);
            p += 20;
            let off = if noff != 0 { u64be(&t, p)? } else { 0 };
            p += 8 * noff;
            let pl = u32be(&t, p)? as usize;
            let dname: String = at(&t, p + 4, pl).iter().map(|&b| b as char).collect();
            p += 4 + pl;
            let nl = u32be(&t, p)? as usize;
            let name = format!(
                "{dname}{}",
                at(&t, p + 4, nl)
                    .iter()
                    .map(|&b| b as char)
                    .collect::<String>()
            );
            p += 4 + nl + 8;
            Ok((name, size, csize, off))
        })();
        let (name, size, csize, off) = match r {
            Ok(x) => x,
            Err(_) => red!(
                "{label}: entry {i} of {n} runs past the IPK table ({:#x} bytes): not the layout of a version 7 archive",
                t.len() + 0x38
            ),
        };
        if !name.ends_with(".fxb") || format!("/{name}").contains("/shaders_dev/") {
            continue;
        }
        let want = if csize != 0 { csize } else { size } as usize;
        let mut b = src.read(base + off, want).map_err(io)?;
        if b.len() != want {
            red!(
                "{label}/{name}: the entry at {:#x} runs past the end of the archive",
                base + off
            );
        }
        if csize != 0 {
            b = inflate::zlib_decompress(&b)
                .map_err(|e| Failure::Red(format!("{label}/{name}: invalid zlib stream ({e})")))?;
        }
        if b.len() != size as usize {
            red!(
                "{label}/{name}: {} bytes decoded, the entry says {size}",
                b.len()
            );
        }
        let mut found = Vec::new();
        crate::scan::scan_blob(cx, &b, &format!("{label}/{name}"), &mut found, 0, None)?;
        if found.is_empty() {
            found = crate::glslc::glslc_shaders(&b, &format!("{label}/{name}"), false, &[])?;
        }
        out.extend(found);
    }
    Ok(())
}

/// `d11_pack_shaders(path, label, out)`: Double Eleven's resource pack: the data file with its index (`.pkiswitch`) beside it
pub fn d11_pack_shaders(
    cx: &Ctx<'_>,
    idx: &[u8],
    d: &[u8],
    label: &str,
    out: &mut Vec<Shader>,
) -> Res<()> {
    if idx.len() < 12 {
        red!("{label}: the pack index is cut short ({} bytes)", idx.len());
    }
    let (ver, n) = (u32le(idx, 0)?, u32le(idx, 8)? as usize);
    if ver != 3 {
        red!("{label}: pack index version {ver}: only 3 is known");
    }
    if 12 + 16 * n > idx.len() {
        red!(
            "{label}: the pack index lists {n} members but holds {}",
            (idx.len() - 12) / 16
        );
    }
    for k in 0..n {
        let (size, stored, off, h) = (
            u32le(idx, 12 + 16 * k)? as usize,
            u32le(idx, 16 + 16 * k)? as usize,
            u32le(idx, 20 + 16 * k)? as usize,
            u32le(idx, 24 + 16 * k)?,
        );
        if off + stored > d.len() {
            red!("{label}: member {k} runs past the end of the data file");
        }
        let mut m = d[off..off + stored].to_vec();
        if stored != size {
            m = inflate::zlib_decompress(&m).map_err(|e| {
                Failure::Red(format!(
                    "{label}: member {k} ({h:08x}) is no zlib stream: {e}"
                ))
            })?;
            if m.len() != size {
                red!(
                    "{label}: member {k} ({h:08x}) inflates to {} bytes, the index says {size}",
                    m.len()
                );
            }
        }
        let lb = format!("{label}#{h:08x}");
        let mut found = Vec::new();
        crate::scan::scan_blob(cx, &m, &lb, &mut found, 0, None)?;
        if found.is_empty() {
            found = crate::glslc::glslc_shaders(&m, &lb, false, &[])?;
        }
        out.extend(found);
    }
    Ok(())
}

// ---- Snowdrop BERG -----------------------------------------------------------------------------------------------------------------

fn snowdrop_record_at(d: &[u8], h: usize) -> bool {
    // SNOWDROP_RECORD = rb'[\xc5\xc6]\x00\xcd\xab'
    h + 4 <= d.len()
        && (d[h] == 0xc5 || d[h] == 0xc6)
        && d[h + 1] == 0
        && d[h + 2] == 0xcd
        && d[h + 3] == 0xab
}

/// `snowdrop_wrapped(d, label)`: shader cache records that are themselves zstd chunks: (start, end, decoded bytes)
fn snowdrop_wrapped(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<(usize, usize, Vec<u8>)>> {
    let mut out = Vec::new();
    for h in find_all(d, &ZSTD_MAGIC) {
        // peek at the decoded header of the frame: 20 bytes; a magic inside another compressed asset gives a decoder error or a short frame
        // a frame whose block headers run past the data is cut short: the Python's streaming decoder still gives the first 20 bytes when the first block is whole, and then reads on to the end of the data
        let flen = zstd::frame_len(d, h).filter(|l| h + l <= d.len());
        let src = match flen {
            Some(l) => &d[h..h + l],
            None => &d[h..],
        };
        let cut = if flen.is_none() {
            match zstd::peek_cut(src, 20) {
                Ok(v) => v,
                Err(()) => continue, // a ZstdError: a magic inside another compressed asset
            }
        } else {
            None
        };
        let head = match cut {
            Some(v) => v,
            None => match cx.zstd.decompress(src, None, Some(20)) {
                Ok(v) => v,
                Err(_) => continue,
            },
        };
        if head.len() != 20 || !snowdrop_record_at(&head, 0) {
            continue;
        }
        let Some(flen) = flen else {
            red!("{label} Snowdrop chunk @{h:#x}: truncated frame");
        };
        let full = cx
            .zstd
            .decompress(src, None, None)
            .map_err(|e| Failure::Red(format!("{label} Snowdrop chunk @{h:#x}: {e}")))?;
        out.push((h, h + flen, full));
    }
    Ok(out)
}

/// `snowdrop_objects(raw, where)`: the shader objects of one decoded Snowdrop cache record: each GLSLC control section sits immediately before its code area; control +0x20 and +0x704 give their lengths
fn snowdrop_objects(raw: &[u8], wh: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for c in find_all(raw, &CTL_MAGIC) {
        if c + 0x708 > raw.len() {
            continue;
        }
        let (nc, nk) = (
            u32le(raw, c + 0x20)? as usize,
            u32le(raw, c + 0x704)? as usize,
        );
        if nc < 0x760 || nc & 3 != 0 || nk < 0x34 || c + nc + nk > raw.len() {
            continue;
        }
        let (ctl, code) = (&raw[c..c + nc], &raw[c + nc..c + nc + nk]);
        if let Some(st) = glslc_stage(ctl, code)?
            && check(st, ctl, code)?.is_none()
        {
            out.push(Shader::new(format!("{wh}:{c:#x}"), st, ctl, code));
        }
    }
    Ok(out)
}

/// `SNOWDROP_CACHE`: 0xabcd00d4, the decoded head of a bare cache record (`.mcache`)
const SNOWDROP_CACHE: [u8; 4] = [0xd4, 0x00, 0xcd, 0xab];

/// `snowdrop_zlib(d, label)` (`paches/engine/0104`, upstream nxnative PR 843): older SDF data slices (South Park: The Fractured But Whole) store each shader cache file
/// (`camel/shadercache/nx/*.mcache`) as a bare zlib stream, without the 20-byte c5 / c6 header. Each stream is found by its zlib header (`ZLIB_CMF` byte, FCHECK) and a peek at its first 20 decoded
/// bytes (the 0xabcd00d4 head, kind, 0, twice the length that follows), then decoded in full: a file over 64 KB is a run of consecutive 64 KB streams, decoded one after another up to the length
/// in its head. The bytes inside a decoded record are skipped by position (`end`). zlib's own `inflate` semantics matter here (`zlibx`): the peek stops at 20 bytes of output, and the text
/// of a damaged stream is part of the `[RED]` line.
fn snowdrop_zlib(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    snowdrop_zlib_capped(d, label, SNOWDROP_ZLIB_MAX_RAW)
}

/// The most ONE bare cache file may decode to: [`inflate::MAX_DECODED`], 1 GiB (review r2, 4.1). `nraw` is the file's own word (a `u32`: up to 4 GiB) and one zlib stream of at most 0x10500 input
/// bytes can carry ~68 MB (zlib's best ratio is ~1032:1), so a few MB of well-formed input made `raw` grow to GBs before `raw.len() != nraw` was ever looked at (measured: 0.5 MB in -> 937 MB RSS for
/// 512 MiB; the engine's Python, which has the same loop with no cap, 0.5 MB in -> 950 MB). A truthful bomb costs about twice its size in memory, and no real `.mcache` comes near 1 GiB; past the cap
/// the file is refused with a `[RED]` line that names it, instead of the process being killed for memory. Nothing under the cap behaves differently: `decompress(seg, Some(room + 1))` is
/// `decompress(seg, None)` whenever the turn's output fits in `room`, so the text of a damaged stream or of a wrong `nraw` is the one the engine's Python gives.
pub const SNOWDROP_ZLIB_MAX_RAW: usize = inflate::MAX_DECODED;

/// `snowdrop_zlib` with the decoded-size cap as a parameter (the tests use a small one)
pub fn snowdrop_zlib_capped(d: &[u8], label: &str, max_raw: usize) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    let mut end = 0usize;
    for h in 0..d.len() {
        // ZLIB_CMF = rb'[\x08\x18\x28\x38\x48\x58\x68\x78]': deflate, any window
        if d[h] & 0x8f != 0x08 {
            continue;
        }
        if h < end
            || h + 26 > d.len()
            || !(((d[h] as u32) << 8) | d[h + 1] as u32).is_multiple_of(31)
        {
            continue;
        }
        let head = match zlibx::decompress(&d[h..(h + 256).min(d.len())], Some(20)) {
            Ok(z) => z.out,
            Err(_) => continue,
        };
        if head.len() < 20
            || head[..4] != SNOWDROP_CACHE
            || head[8..12] != [0u8; 4]
            || head[12..16] != head[16..20]
        {
            continue; // +4: the record's kind (0x13c, 0x65, ...)
        }
        let nraw = u32le(&head, 12)? as usize + 20; // the length after the 20-byte header
        let mut raw: Vec<u8> = Vec::new();
        let (mut got, mut pos) = (0usize, h);
        while got < nraw {
            // a file over 64 KB is stored as consecutive zlib streams of 64 KB each (the list's Block column)
            let lim = pos + (nraw - got).min(0x10000) + 0x500;
            let seg = &d[pos.min(d.len())..lim.min(d.len())];
            // one turn may carry ~1000x its input: ask for no more than the room left under the cap (+1, to tell "fits exactly" from "passes it")
            let room = max_raw.saturating_sub(raw.len());
            let part = match zlibx::decompress(seg, Some(room.saturating_add(1))) {
                Ok(z) => z,
                Err(e) => red!("{label} Snowdrop zlib record @{h:#x}, block @{pos:#x}: {e}"),
            };
            if part.out.len() > room {
                red!(
                    "{label} Snowdrop zlib record @{h:#x}: block @{pos:#x} decodes past {max_raw} bytes, the most one cache file may take (its head announces {nraw})"
                );
            }
            if !part.eof || part.out.is_empty() {
                red!("{label} Snowdrop zlib record @{h:#x}: block @{pos:#x} not finished");
            }
            got += part.out.len();
            raw.extend_from_slice(&part.out);
            pos = pos.min(d.len()) + part.used;
        }
        if raw.len() != nraw {
            red!(
                "{label} Snowdrop zlib record @{h:#x}: expected {nraw} raw bytes, got {}",
                raw.len()
            );
        }
        end = pos;
        out.extend(snowdrop_objects(&raw, &format!("{label}#SDFZLIB@{h:#x}"))?);
    }
    Ok(out)
}

/// `snowdrop_shaders(d, label, wrapped)`: BERG data slices: compressed shader records with a 20-byte header {0xabcd00c5 / 0xabcd00c6, flags, 0, raw length, stored length}, then zlib / zstd data;
/// and, in the older slices (SDF version 22 with the ubisoft tag), bare zlib streams (`snowdrop_zlib`)
pub fn snowdrop_shaders(cx: &Ctx<'_>, d: &[u8], label: &str, wrapped: bool) -> Res<Vec<Shader>> {
    let chunks = if wrapped {
        snowdrop_wrapped(cx, d, label)?
    } else {
        Vec::new()
    };
    let mut out = if wrapped {
        snowdrop_zlib(d, label)?
    } else {
        Vec::new()
    };
    let mut ci = 0usize;
    let mut p = 0usize;
    while let Some(i) = memchr::memchr2(0xc5, 0xc6, &d[p.min(d.len())..]) {
        let h = p + i;
        p = h + 1;
        if !snowdrop_record_at(d, h) {
            continue;
        }
        while ci < chunks.len() && h >= chunks[ci].1 {
            ci += 1;
        }
        if ci < chunks.len() && h >= chunks[ci].0 {
            continue; // literal bytes inside a zstd block are not a separately stored record
        }
        if h + 24 > d.len() {
            continue;
        }
        let is_zstd = d[h] == 0xc6;
        if is_zstd {
            if d[h + 20..h + 24] != ZSTD_MAGIC {
                continue;
            }
        } else if d[h + 20] & 0xf != 8
            || !((d[h + 20] as u32) << 8 | d[h + 21] as u32).is_multiple_of(31)
        {
            continue;
        }
        let (nraw, nstored) = (u32le(d, h + 12)? as usize, u32le(d, h + 16)? as usize);
        if nstored == 0 || h + 20 + nstored > d.len() {
            red!("{label} Snowdrop record @{h:#x}: stored bytes out of bounds");
        }
        let stored = &d[h + 20..h + 20 + nstored];
        let raw = if is_zstd {
            cx.zstd.decompress(stored, None, None).map_err(zstd_err)?
        } else {
            inflate::zlib_decompress(stored).map_err(zerr)?
        };
        if raw.len() != nraw {
            red!(
                "{label} Snowdrop record @{h:#x}: expected {nraw} raw bytes, got {}",
                raw.len()
            );
        }
        out.extend(snowdrop_objects(&raw, &format!("{label}#SNOWDROP@{h:#x}"))?);
    }
    for (h, _, raw) in chunks {
        out.extend(snowdrop_shaders(
            cx,
            &raw,
            &format!("{label}#SDFZSTD@{h:#x}"),
            false,
        )?);
    }
    Ok(out)
}

// ---- Unreal Engine 3 caches ---------------------------------------------------------------------------------------------------------

pub const UE3_TAG: [u8; 4] = [0xc1, 0x83, 0x2a, 0x9e];

/// `ue3_body(d, label)`: an Unreal Engine 3 package with its compressed chunks decoded (zlib blocks here; the others are Oodle: the game's own decoder)
pub fn ue3_body(d: &[u8], label: &str) -> Res<Vec<u8>> {
    let i0 = match find(d, &UE3_TAG, 4) {
        Some(i) => i,
        None => return Ok(d.to_vec()),
    };
    let mut out: Vec<u8> = d[..i0].to_vec();
    let mut i = i0;
    while i < d.len() {
        if !eq_at(d, i, &UE3_TAG) || i + 16 > d.len() {
            red!(
                "{label}: no compressed chunk at {i:#x} (the chunks do not run to the end of the package)"
            );
        }
        let (bs, _cs, us) = (
            u32le(d, i + 4)? as usize,
            u32le(d, i + 8)?,
            u32le(d, i + 12)? as usize,
        );
        if bs == 0 {
            red!("{label}: chunk at {i:#x} has block size 0");
        }
        let nb = us.div_ceil(bs);
        let mut o = i + 16 + 8 * nb;
        for k in 0..nb {
            let (c, u) = (
                u32le(d, i + 16 + 8 * k)? as usize,
                u32le(d, i + 20 + 8 * k)? as usize,
            );
            let blk = span(d, o, c, &format!("{label} chunk at {i:#x} block {k}"))?;
            if c == u {
                out.extend_from_slice(blk);
            } else if blk.first() == Some(&0x78) {
                out.extend_from_slice(&inflate::zlib_decompress(blk).map_err(zerr)?);
            } else {
                unsupported!(
                    "Oodle block in an Unreal Engine 3 package ({label} chunk at {i:#x} block {k}: the game's own decoder in the Mac host)"
                );
            }
            o += c;
        }
        i = o;
    }
    Ok(out)
}

/// `ue3_shaders(d, label)`: shader records of an Unreal Engine 3 game's shader caches
pub fn ue3_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for c in find_all(d, &CTL_MAGIC) {
        if c + 0x708 > d.len() {
            continue;
        }
        let n = u32le(d, c + 0x704)? as usize;
        if n > c {
            continue;
        }
        let a = c - n;
        if a < 12 || !(eq_at(d, a, &GFX_MAGIC) || eq_at(d, a, &CS_MAGIC)) {
            continue;
        }
        let mut nctl = None;
        for k in [4usize, 8] {
            if a >= k + 4 {
                let (ln, lc) = (u32le(d, a - k - 4)? as usize, u32le(d, a - k)? as usize);
                if ln == n && (0x760..=0x1000).contains(&lc) {
                    nctl = Some(lc);
                    break;
                }
            }
        }
        let nctl = match nctl {
            Some(x) if c + x <= d.len() => x,
            _ => continue,
        };
        let (ctl, code) = (&d[c..c + nctl], &d[a..c]);
        let st = if sl(code, 0, 4) == CS_MAGIC {
            Some(St::Cs)
        } else {
            sph_stage(code)
        };
        let st = match st {
            Some(s) => s,
            None => red!("{label}: shader record at {c:#x}: no stage in the code area header"),
        };
        if let Some(w) = check(st, ctl, code)? {
            red!("{label}: shader record at {c:#x} ({}): {w}", st.short());
        }
        out.push(Shader::new(format!("{label}#UE3@{c:x}"), st, ctl, code));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_python() {
        assert_eq!(b64decode(b"aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(b64decode(b"aGk=").unwrap(), b"hi");
        assert_eq!(b64decode(b"aGk").unwrap_err(), "Incorrect padding");
        assert!(b64decode(b"a").is_err());
        assert_eq!(b64decode(b"AAEC/w==").unwrap(), [0, 1, 2, 255]);
    }

    #[test]
    fn yaz0_literals_and_a_back_reference() {
        // 'abc' as literals, then a copy of 3 bytes from distance 3, then a literal
        let mut s = b"Yaz0".to_vec();
        s.extend_from_slice(&7u32.to_be_bytes());
        s.extend_from_slice(&[0; 8]);
        s.push(0b1110_1000); // literal, literal, literal, reference, literal
        s.extend_from_slice(b"abc");
        s.extend_from_slice(&[0x10, 0x02]); // length (1 + 2) = 3, distance 2 + 1 = 3
        s.push(b'z');
        assert_eq!(yaz0(&s, "t").unwrap(), b"abcabcz");
        s.truncate(s.len() - 1);
        assert!(yaz0(&s, "t").is_err());
    }

    #[test]
    fn zlib_streams_follow_pythons_eof_and_unused_data() {
        let z = {
            use std::io::Write;
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&[7u8; 1000]).unwrap();
            e.finish().unwrap()
        };
        let r = inflate::run(&z, Wrap::Zlib, None).unwrap();
        assert!(r.eof && r.out.len() == 1000 && r.used == z.len());
        let mut t = z.clone();
        t.extend_from_slice(b"junk");
        let r = inflate::run(&t, Wrap::Zlib, None).unwrap();
        assert!(r.eof && r.used == z.len());
        assert!(inflate::zlib_decompress(&z[..z.len() - 5]).is_err());
        let r = inflate::run(&z[..z.len() - 5], Wrap::Zlib, None).unwrap();
        assert!(!r.eof);
        assert_eq!(
            inflate::run(&z, Wrap::Zlib, Some(11)).unwrap().out.len(),
            11
        );
    }
}
