//! NXCX texture archives (TSUKIHIME's `w_allpac.mrg`, 15.6 GiB; nxnative 0ec204f5): `nxcx_shaders`. The archive is searched in chunks, never read whole: when another embedded format may
//! be in it the file goes the loose-file way (`None`); else its shaders are sead's primitive drawer at its start or the GLSLC outputs at the magics the search found.

use crate::bytes::*;
use crate::ctl::{GFX_MAGIC, GLSLC_MAGIC, St, check, nvn2_ctl, nvn2_stage};
use crate::error::{Failure, Res};
use crate::glslc::{glslc_shaders_window, glslc_window};
use crate::nvn::{MGFX, NEEDLE, NVNSH, sumo_nx};
use crate::scan_types::Shader;
use crate::source::Source;
use crate::zstd::{LZ4_FRAME_MAGIC, ZSTD_MAGIC};

fn io(e: std::io::Error) -> Failure {
    Failure::Red(format!("OSError: {e}"))
}

/// The branch of `scan_file_in`: 'NXCX', four zero bytes at +12, and neither a code header at +8 nor a size-prefixed frame at +4
pub fn is_nxcx(head: &[u8]) -> bool {
    starts(head, b"NXCX")
        && sl(head, 12, 16) == [0, 0, 0, 0]
        && sl(head, 8, 12) != GFX_MAGIC
        && sl(head, 4, 8) != ZSTD_MAGIC
        && sl(head, 4, 8) != LZ4_FRAME_MAGIC
}

/// `nxcx_shaders(f, label)`: None when the loose-file reader must read the file (a BNSH, SLCT, MGFX 5 / 8, Needle, Ninty_NX or NVNsh magic is in it)
pub fn nxcx_shaders(src: &dyn Source, label: &str) -> Res<Option<Vec<Shader>>> {
    let mut m5 = MGFX.to_vec();
    m5.push(5);
    let mut m8 = MGFX.to_vec();
    m8.push(8);
    let sumo = sumo_nx();
    let other: [&[u8]; 7] = [
        b"BNSH\0\0\0\0",
        b"SLCT\x03\x10\0\0",
        &m5,
        &m8,
        NEEDLE,
        &sumo,
        NVNSH,
    ];
    let finders: Vec<memchr::memmem::Finder<'_>> =
        other.iter().map(memchr::memmem::Finder::new).collect();
    let overlap = other.iter().map(|m| m.len()).max().unwrap_or(1) - 1;
    const CHUNK: usize = 16 << 20;
    let len = src.len();
    let mut offsets: Vec<usize> = Vec::new();
    let mut loose = false;
    let mut buf = vec![0u8; CHUNK + overlap];
    let mut at = 0u64;
    // the window at `at` holds the last `overlap` bytes before it too, as the Python's `tail + block`
    let mut tail = 0usize;
    while at < len {
        let start = at - tail as u64;
        let want = (tail as u64 + (len - at).min(CHUNK as u64)) as usize;
        let mut got = 0;
        while got < want {
            let k = src
                .read_at(start + got as u64, &mut buf[got..want])
                .map_err(io)?;
            if k == 0 {
                break;
            }
            got += k;
        }
        let w = &buf[..got];
        if finders.iter().any(|f| f.find(w).is_some()) {
            return Ok(None);
        }
        loose = loose || contains(w, b"BAHS") || contains(w, MGFX);
        for m in find_all(w, &GLSLC_MAGIC) {
            if m + 4 > tail {
                offsets.push(start as usize + m);
            }
        }
        if got < tail + 1 {
            break;
        }
        at = start + got as u64;
        tail = overlap.min(got);
    }
    if !loose && offsets.is_empty() {
        return Ok(Some(vec![]));
    }
    let sead = sead_prim_src(src, label)?;
    if !sead.is_empty() {
        return Ok(Some(sead));
    }
    let mut out = Vec::new();
    for h in offsets {
        let left = len - h as u64;
        let head = src.read(h as u64, 0x68).map_err(io)?;
        let n = glslc_window(&head, left)?;
        let w = src.read(h as u64, n as usize).map_err(io)?;
        out.extend(glslc_shaders_window(&w, label, h)?);
    }
    Ok(Some(out))
}

/// `sead_prim_shaders(d, label)` over a source: four u32 offsets (vertex control, pixel control, vertex code, pixel code), each control naming its own length (+0x30) and its code's (+0x44)
fn sead_prim_src(src: &dyn Source, label: &str) -> Res<Vec<Shader>> {
    let len = src.len();
    if len < 0x10 {
        return Ok(vec![]);
    }
    let h = src.read(0, 0x10).map_err(io)?;
    let offs = [
        u32le(&h, 0)? as u64,
        u32le(&h, 4)? as u64,
        u32le(&h, 8)? as u64,
        u32le(&h, 12)? as u64,
    ];
    let mut out = Vec::new();
    for (st, c, k) in [(St::Vs, offs[0], offs[2]), (St::Ps, offs[1], offs[3])] {
        if !(0x10 <= c && c + 0x48 <= len) {
            return Ok(vec![]);
        }
        let c48 = src.read(c, 0x48).map_err(io)?;
        if !nvn2_ctl(&c48) {
            return Ok(vec![]);
        }
        let own = u32le(&c48, 0x30)? as u64;
        let ctl = src.read(c, own.max(0x48) as usize).map_err(io)?;
        let n = u32le(&ctl, 0x44)? as usize;
        let code = src.read(k, n).map_err(io)?;
        if code.len() != n
            || nvn2_stage(&ctl, &code) != Some(st)
            || check(st, &ctl, &code)?.is_some()
        {
            return Ok(vec![]);
        }
        out.push(Shader::new(
            format!("{label}#SEADPRIM@{c:#x}"),
            st,
            &ctl,
            &code,
        ));
    }
    Ok(out)
}
