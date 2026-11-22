//! GLSLC outputs and the readers built on them: `glslc_shaders` (the one every game's `main` goes through), RAGE's `rgxd`, YEBIS's `.tfxo`, Diablo II's SHAD containers,
//! control sections loose in main (`raw_shaders`), Retro Studios' SNVN. Port of the same-named functions of `shader_split.py`.

use crate::bytes::*;
use crate::ctl::*;
use crate::error::{Failure, Res};
use crate::red;
use crate::scan_types::{Ctx, Shader};
use crate::zstd::ZSTD_MAGIC;
use std::collections::{HashMap, HashSet};

fn sh(label: String, st: St, ctl: &[u8], code: &[u8]) -> Shader {
    Shader::new(label, st, ctl, code)
}

/// `glslc_layout(d, h)`: the header of the GLSLC output at h: (the whole output's length, section count, first section header, distance between section headers)
pub fn glslc_layout(d: &[u8], h: usize) -> Res<(usize, usize, usize, usize)> {
    let w = |o: usize| u32le(d, h + o).map(|v| v as usize);
    if h + 0x68 <= d.len() && w(0x30)? == 0xc9 && w(0x44)? == 0 {
        return Ok((w(0x5c)?, w(0x64)?, 0xa8, 0x98));
    }
    if h + 0x50 <= d.len() {
        Ok((w(0x44)?, w(0x4c)?, 0x90, 0x90))
    } else {
        Ok((0, 0, 0x90, 0x90))
    }
}

/// One GLSLC stage pair: (the two byte blobs of a program).
pub type StagePair = (Vec<u8>, Vec<u8>);

/// `glslc_section(d, h, size)`: the GPU code sections of the GLSLC output at h, from its section headers → [(control, code)] of each GPU code section, or None
pub fn glslc_section(d: &[u8], h: usize, size: usize) -> Res<Option<Vec<StagePair>>> {
    let (_, n, first, stride) = glslc_layout(d, h)?;
    let mut secs = Vec::new();
    if !(0 < n && n <= 16) || h + first + stride * n > d.len() {
        return Ok(None);
    }
    for i in 0..n {
        let s = h + first + stride * i;
        let (ssz, soff, typ) = (
            u32le(d, s)? as usize,
            u32le(d, s + 4)? as usize,
            u32le(d, s + 8)?,
        );
        if typ != 0 {
            continue;
        }
        let (co, ko, kn, cn) = (
            u32le(d, s + 0x30)? as usize,
            u32le(d, s + 0x34)? as usize,
            u32le(d, s + 0x38)? as usize,
            u32le(d, s + 0x3c)? as usize,
        );
        if soff + ssz > size || co + cn > ssz || ko + kn > ssz {
            return Ok(None);
        }
        secs.push((
            at(d, h + soff + co, cn).to_vec(),
            at(d, h + soff + ko, kn).to_vec(),
        ));
    }
    Ok(Some(secs))
}

/// `glslc_program(d, h, size, label)`: a GLSLC output of several stages, compiled together as one program
pub fn glslc_program(d: &[u8], h: usize, size: usize, label: &str) -> Res<Vec<Shader>> {
    let secs = glslc_section(d, h, size)?.unwrap_or_default();
    let mut sts = Vec::new();
    for (ctl, code) in &secs {
        sts.push(glslc_stage(ctl, code)?);
    }
    // `len(set(sts)) < len(sts)` counts None as a value too
    let mut set: Vec<Option<St>> = Vec::new();
    for s in &sts {
        if !set.contains(s) {
            set.push(*s);
        }
    }
    let dup = set.len() < sts.len();
    if secs.len() < 2 || dup {
        return Ok(vec![]);
    }
    for (st, (ctl, code)) in sts.iter().zip(&secs) {
        match st {
            None => return Ok(vec![]),
            Some(st) => {
                if check(*st, ctl, code)?.is_some() {
                    return Ok(vec![]);
                }
            }
        }
    }
    Ok(sts
        .iter()
        .zip(&secs)
        .map(|(st, (ctl, code))| sh(format!("{label}#GLSLC@{h:#x}#var"), st.unwrap(), ctl, code))
        .collect())
}

/// `glslc_shaders(d, label, need, skip)`: GLSLC compiler outputs, from the magic 0x19866891, +0x44 = the whole output's length; only outputs that pass the triple check are kept
pub fn glslc_shaders(
    d: &[u8],
    label: &str,
    need: bool,
    skip: &[(usize, usize)],
) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for h in find_all(d, &GLSLC_MAGIC) {
        if skip.iter().any(|&(a, b)| a <= h && h < b) {
            continue;
        }
        if h + 0x48 > d.len() {
            continue;
        }
        let (size, _, first, _) = glslc_layout(d, h)?;
        if first == 0xa8 {
            // NVN2: its section headers say everything; its control sections are too short for the rule below
            if !(0 < size && size <= d.len() - h) {
                continue;
            }
            let secs = glslc_section(d, h, size)?.unwrap_or_default();
            if secs.len() > 1 {
                out.extend(glslc_program(d, h, size, label)?);
            } else if let Some((ctl, code)) = secs.first()
                && let Some(st) = glslc_stage(ctl, code)?
                && check(st, ctl, code)?.is_none()
            {
                out.push(sh(format!("{label}#GLSLC@{h:#x}"), st, ctl, code));
            }
            continue;
        }
        let c = if 0x760 < size && size <= d.len() - h {
            find_in(d, &CTL_MAGIC, h, h + size)
        } else {
            None
        };
        let c = match c {
            Some(c) => c,
            None => continue,
        };
        if find_in(d, &CTL_MAGIC, c + 4, h + size).is_some() {
            out.extend(glslc_program(d, h, size, label)?);
            continue;
        }
        // The output's own section header says where its control section and code area are (glslc_section), and is read first
        if let Some(sec) = glslc_section(d, h, size)?
            && sec.len() == 1
            && let Some(st) = glslc_stage(&sec[0].0, &sec[0].1)?
            && check(st, &sec[0].0, &sec[0].1)?.is_none()
        {
            out.push(sh(
                format!("{label}#GLSLC@{h:#x}"),
                st,
                &sec[0].0,
                &sec[0].1,
            ));
            continue;
        }
        // Without a usable section header: the code area right after the control section's +0x20 length
        let (nctl, ncode) = if c + 0x708 <= h + size {
            (u32le(d, c + 0x20)? as usize, u32le(d, c + 0x704)? as usize)
        } else {
            (0, 0)
        };
        let (ctl, code) = (at(d, c, nctl), at(d, c + nctl, ncode));
        let st = if nctl >= 0x760 && nctl & 3 == 0 && c + nctl + ncode <= h + size {
            glslc_stage(ctl, code)?
        } else {
            None
        };
        let st = match st {
            Some(s) => s,
            None => continue,
        };
        if check(st, ctl, code)?.is_some() {
            continue;
        }
        out.push(sh(format!("{label}#GLSLC@{h:#x}"), st, ctl, code));
    }
    if out.is_empty() && need {
        red!("{label}: no GLSLC output");
    }
    Ok(out)
}

/// `rgxd_shaders(d, label)`: a RAGE effect: each shader is a record {u32 length, u32 0x19866891, u32 code length, u32 control length}, the code area, the control section
pub fn rgxd_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let n = find_all(d, &CTL_MAGIC).count();
    let mut out = Vec::new();
    for m in find_all(d, &GLSLC_MAGIC) {
        if m < 4 {
            continue;
        }
        let h = m - 4;
        if h + 16 > d.len() {
            continue;
        }
        let (nk, nc) = (u32le(d, h + 8)? as usize, u32le(d, h + 12)? as usize);
        if h + 16 + nk + nc > d.len() {
            continue;
        }
        let (code, ctl) = (at(d, h + 16, nk), at(d, h + 16 + nk, nc));
        let st = if ctl.len() >= 4 && ctl[..4] == CTL_MAGIC && ctl.len() >= 0x760 {
            glslc_stage(ctl, code)?
        } else {
            None
        };
        if let Some(st) = st
            && check(st, ctl, code)?.is_none()
        {
            out.push(sh(format!("{label}#RGXD@{h:#x}"), st, ctl, code));
        }
    }
    if out.len() != n {
        cx.print(&format!(
            "[RED] {label}: {n} control sections, {} shader records read; the rest are left out",
            out.len()
        ));
    }
    Ok(out)
}

// ---- YEBIS .tfxo ---------------------------------------------------------------------------------------------------------------

pub const TFXO: &[u8] = b".tfxo file\0";

pub fn tfxo_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for a in find_all(d, &CTL_MAGIC) {
        if a + 0x24 > d.len() {
            continue; // the magic too near the end to start a control section
        }
        let n = u32le(d, a + 0x20)? as usize;
        if !(0x760..=0x880).contains(&n) {
            continue;
        }
        let ctl = span(d, a, n, &format!("{label}: the control section at {a:#x}"))?;
        let ncode = u32le(ctl, 0x704)? as usize;
        let code = at(d, a + n, ncode);
        let c4 = sl(code, 0, 4);
        let st = if c4 == CS_MAGIC {
            Some(St::Cs)
        } else if c4 == GFX_MAGIC {
            sph_stage(code)
        } else {
            None
        };
        let st = match st {
            Some(s) => s,
            None => red!("{label}: the control section at {a:#x} is not followed by a code area"),
        };
        if let Some(why) = check(st, ctl, code)? {
            red!("{label}: shader at {a:#x}: {why}");
        }
        out.push(sh(format!("{label}#TFXO@{a:#x}"), st, ctl, code));
    }
    Ok(out)
}

/// `ZLIB_HEAD.finditer(d)`: the offsets of `\x78[\x01\x5e\x9c\xda]`
pub fn zlib_heads(d: &[u8]) -> impl Iterator<Item = usize> + '_ {
    memchr::memchr_iter(0x78, d)
        .filter(move |&i| matches!(d.get(i + 1), Some(0x01 | 0x5e | 0x9c | 0xda)))
}

/// `tfxo_streams(d, label)`: YEBIS effect files linked into a module as zlib streams: a stream is taken when its first bytes inflate to the '.tfxo file' header
pub fn tfxo_streams(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for m in zlib_heads(d) {
        let head =
            match crate::inflate::run(at(d, m, 4096), crate::inflate::Wrap::Zlib, Some(TFXO.len()))
            {
                Ok(r) => r.out,
                Err(_) => continue,
            };
        if head != TFXO {
            continue;
        }
        let x = crate::inflate::run(&d[m..], crate::inflate::Wrap::Zlib, None)
            .map_err(|e| Failure::Red(format!("zlib.error: {e}")))?;
        out.extend(tfxo_shaders(&x.out, &format!("{label}@{m:#x}"))?);
    }
    Ok(out)
}

// ---- Diablo II: Resurrected SHAD --------------------------------------------------------------------------------------------------

pub const SHAD: &[u8] = b"SHADSHAD";

/// `shad_record(d, at, end)`: {u32 5, control length, code length + 0xc, code length, u32 4}, the code area, the control section → (control, code), or None
pub fn shad_record(d: &[u8], a: usize, end: usize) -> Res<Option<(&[u8], &[u8])>> {
    if a + 0x14 > end {
        return Ok(None);
    }
    let w: Vec<usize> = (0..5)
        .map(|i| u32le(d, a + 4 * i).map(|v| v as usize))
        .collect::<Res<_>>()?;
    if w[0] != 5 || w[4] != 4 || w[2] != w[3] + 0xc || a + 0x14 + w[3] + w[1] != end {
        return Ok(None);
    }
    Ok(Some((sl(d, a + 0x14 + w[3], end), at(d, a + 0x14, w[3]))))
}

pub fn shad_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    fn walk(
        d: &[u8],
        label: &str,
        out: &mut Vec<Shader>,
        mut a: usize,
        end: usize,
        depth: usize,
    ) -> Res<()> {
        if depth > 64 {
            red!("{label}: chunks nest too deep");
        }
        while a < end {
            if a + 28 > end {
                red!("{label}: chunk header at {a:#x} runs past {end:#x}");
            }
            let tag = at(d, a, 4);
            let (cnt, hl, ds, size) = (
                u32le(d, a + 8)? as usize,
                u32le(d, a + 16)? as usize,
                u32le(d, a + 20)? as usize,
                u32le(d, a + 24)? as usize,
            );
            if !(28 <= hl && hl <= ds && ds <= size) || a + size > end {
                red!(
                    "{label}: chunk {:?} at {a:#x} (header {hl:#x}, data {ds:#x}, length {size:#x}) does not fit {end:#x}",
                    tag
                );
            }
            if tag == b"BTCD" {
                let sh_ = match shad_record(d, a + hl, a + size)? {
                    Some(x) => x,
                    None => red!("{label}: BTCD at {a:#x} does not hold a record of its length"),
                };
                let (ctl, code) = sh_;
                let wh = format!("{label}#SHAD@{a:#x}");
                let st = glslc_stage(ctl, code)?;
                let why = match st {
                    Some(st) => check(st, ctl, code)?.map(str::to_string),
                    None => Some("no stage in the code area header".to_string()),
                };
                if let Some(w) = why {
                    red!("{wh}: {w}");
                }
                out.push(sh(wh, st.unwrap(), ctl, code));
            } else if tag == b"PRM[" {
                if hl + 4 * cnt > ds {
                    red!(
                        "{label}: PRM[ at {a:#x}: {cnt} offsets do not fit before its data at {ds:#x}"
                    );
                }
                for i in 0..cnt {
                    let o = u32le(d, a + hl + 4 * i)? as usize;
                    walk(d, label, out, a + ds + o, a + size, depth + 1)?;
                }
            } else if tag == b"SHAD" || tag == b"SHMD" {
                walk(d, label, out, a + ds, a + size, depth + 1)?;
            }
            a += size;
            if tag == b"BTCD" && depth != 0 {
                return Ok(()); // a PRM[ offset names one chunk
            }
        }
        Ok(())
    }
    walk(d, label, &mut out, 0, d.len(), 0)?;
    Ok(out)
}

// ---- control sections loose in main --------------------------------------------------------------------------------------------

/// `rel_targets(img)`: a module image's relative relocations `{slot: target}` in the Python dict's order (RELA R_AARCH64_RELATIVE, then DT_RELR)
fn rel_targets(img: &[u8]) -> Res<Vec<(u64, u64)>> {
    let m = u32le(img, 4)? as usize;
    let o0 = m as i64 + i32le(img, m + 4)? as i64;
    let mut o = o0 as usize;
    let mut t: HashMap<i64, u64> = HashMap::new();
    while o + 16 <= img.len() {
        let (tag, v) = (i64le(img, o)?, u64le(img, o + 8)?);
        o += 16;
        if tag == 0 {
            break;
        }
        t.entry(tag).or_insert(v);
    }
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut idx: HashMap<u64, usize> = HashMap::new();
    let mut put = |k: u64, v: u64, out: &mut Vec<(u64, u64)>| {
        if let Some(&i) = idx.get(&k) {
            out[i].1 = v;
        } else {
            idx.insert(k, out.len());
            out.push((k, v));
        }
    };
    if t.contains_key(&7) {
        let n = t.get(&8).copied().unwrap_or(0) as usize;
        let base = t[&7] as usize;
        let mut i = 0usize;
        while i < n {
            let (off, info, add) = (
                u64le(img, base + i)?,
                u64le(img, base + i + 8)?,
                i64le(img, base + i + 16)?,
            );
            if info & 0xffff_ffff == 0x403 {
                put(off, add as u64, &mut out);
            }
            i += 24;
        }
    }
    let mut nxt = 0u64;
    if t.contains_key(&36) {
        let n = t.get(&35).copied().unwrap_or(0) as usize;
        let base = t[&36] as usize;
        let mut i = 0usize;
        while i < n {
            let e = u64le(img, base + i)?;
            i += 8;
            if e & 1 == 0 {
                let v = u64le(img, e as usize)?;
                put(e, v, &mut out);
                nxt = e + 8;
                continue;
            }
            for b in 0..63u64 {
                if (e >> (b + 1)) & 1 != 0 {
                    let s = nxt + 8 * b;
                    let v = u64le(img, s as usize)?;
                    put(s, v, &mut out);
                }
            }
            nxt += 8 * 63;
        }
    }
    Ok(out)
}

/// `adrp_targets(d, lo, hi)`: addresses in [lo, hi) that the code builds with adrp + add on the same register (the add within the next 8 instructions) → {target: offsets of those adds}
fn adrp_targets(d: &[u8], lo: i64, hi: i64) -> HashMap<i64, Vec<i64>> {
    let n = d.len() / 4;
    let w = |i: usize| u32::from_le_bytes(d[4 * i..4 * i + 4].try_into().unwrap());
    let mut out: HashMap<i64, Vec<i64>> = HashMap::new();
    for i in 0..n {
        let x = w(i);
        if x & 0x9f00_0000 != 0x9000_0000 {
            continue;
        }
        let rd = x & 31;
        // the Python shifts the 21-bit immediate left and right by 44 in unbounded integers: no sign extension
        let imm = (((x >> 5) & 0x7ffff) << 2 | (x >> 29) & 3) as i64;
        let page = ((i as i64 * 4) & !0xfff) + (imm << 12);
        if !(lo - 0x1000 <= page && page < hi) {
            continue;
        }
        for j in 0..8 {
            if i + 1 + j >= n {
                break;
            }
            let y = w(i + 1 + j);
            if y & 0xffc0_0000 == 0x9100_0000 && (y >> 5) & 31 == rd {
                let t = page + ((y >> 10) & 0xfff) as i64;
                if lo <= t && t < hi {
                    out.entry(t).or_default().push(((i + 1 + j) * 4) as i64);
                }
                break;
            }
        }
    }
    out
}

/// `raw_shaders(d, label, known)`: control sections in main that are in no GLSLC output and no BNSH file (see the Python's docstring for the whole story)
pub fn raw_shaders(
    cx: &Ctx<'_>,
    d: &[u8],
    label: &str,
    known: &HashSet<Vec<u8>>,
) -> Res<Vec<Shader>> {
    // the start of the headed code area of the control section's length that ends right at it, else None
    let adjacent = |c: usize| -> Res<Option<usize>> {
        let (nctl, n) = (u32le(d, c + 0x20)? as usize, u32le(d, c + 0x704)? as usize);
        if n < 0x80 || nctl < 0x760 || c + nctl > d.len() || n > c {
            return Ok(None);
        }
        let k = c - n;
        let m = at(d, k, 4);
        if (m == GFX_MAGIC || m == CS_MAGIC) && !contains(sl(d, k, c), &CTL_MAGIC) {
            Ok(Some(k))
        } else {
            Ok(None)
        }
    };
    let mut skip: Vec<(usize, usize)> = Vec::new();
    for m in find_all(d, &GLSLC_MAGIC) {
        let (size, _, first, _) = glslc_layout(d, m)?;
        let lower = if first == 0xa8 { 0 } else { 0x760 };
        if lower < size && size <= d.len() - m {
            skip.push((m, m + size));
        }
    }
    for m in find_all(d, b"BNSH\0\0\0\0") {
        skip.push((m, m + u32le(d, m + 0x1c)? as usize));
    }
    let in_skip = |c: usize| skip.iter().any(|&(a, b)| a <= c && c < b);
    let mut ctls: Vec<usize> = Vec::new();
    for c in find_all(d, &CTL_MAGIC) {
        if c + 0x708 > d.len() || in_skip(c) {
            continue;
        }
        if !(c & 3 == 0 || adjacent(c)?.is_some()) {
            continue;
        }
        let n = u32le(d, c + 0x20)? as usize;
        if known.contains(at(d, c, n)) {
            continue;
        }
        ctls.push(c);
    }
    if ctls.is_empty() {
        return Ok(vec![]);
    }
    let rel = rel_targets(d)?;
    let rel_map: HashMap<u64, u64> = rel.iter().copied().collect();
    let cset: HashSet<usize> = ctls.iter().copied().collect();
    let mut code_of: HashMap<usize, usize> = HashMap::new();
    for &(s, c) in &rel {
        let cu = c as usize;
        if cset.contains(&cu)
            && rel_map.contains_key(&(s.wrapping_sub(8)))
            && !code_of.contains_key(&cu)
            && u32le(d, s as usize + 8)? == u32le(d, cu + 0x704)?
        {
            code_of.insert(cu, rel_map[&(s - 8)] as usize);
        }
    }
    let built = if ctls.iter().any(|c| !code_of.contains_key(c)) {
        adrp_targets(
            d,
            *ctls.iter().min().unwrap() as i64 - 0x100000,
            *ctls.iter().max().unwrap() as i64 + 0x100000,
        )
    } else {
        HashMap::new()
    };
    let targets: HashSet<u64> = rel.iter().map(|&(_, c)| c).collect();
    let mut out = Vec::new();
    for &c in &ctls {
        let (nctl, n) = (u32le(d, c + 0x20)? as usize, u32le(d, c + 0x704)? as usize);
        let r = c as i64 - n as i64 - 0x14; // a SHAD record's start (shad_record), when this control section ends one
        if !code_of.contains_key(&c)
            && r >= 0
            && (targets.contains(&(r as u64)) || built.contains_key(&r))
        {
            let ru = r as usize;
            let rec_end = ru + 0x14 + n + u32le(d, ru + 4)? as usize;
            if shad_record(d, ru, rec_end)?.is_some() {
                code_of.insert(c, c - n);
            }
        }
        let ctl = at(d, c, nctl);
        let mut why = true;
        let near = |k: i64| -> i64 {
            let bc = &built[&(c as i64)];
            built[&k]
                .iter()
                .flat_map(|a| bc.iter().map(move |b| (a - b).abs()))
                .min()
                .unwrap_or(i64::MAX)
        };
        let cands: Vec<i64> = if let Some(&k) = code_of.get(&c) {
            vec![k as i64]
        } else {
            let mut v: Vec<i64> = [((c + nctl + 15) & !15) as i64, c as i64 - n as i64]
                .into_iter()
                .filter(|k| {
                    built.contains_key(k) && built.contains_key(&(c as i64)) && near(*k) < 0x400
                })
                .collect();
            v.sort_by_key(|&k| near(k)); // stable, as sorted(key=near)
            v
        };
        for k in cands {
            let code = if k >= 0 {
                at(d, k as usize, n)
            } else {
                &[][..]
            };
            let st = if k >= 0 && !contains(code, &CTL_MAGIC) {
                glslc_stage(ctl, code)?
            } else {
                None
            };
            if let Some(st) = st
                && check(st, ctl, code)?.is_none()
            {
                out.push(sh(format!("{label}#RAW@{c:#x}"), st, ctl, code));
                why = false;
                break;
            }
        }
        if why
            && !code_of.contains_key(&c)
            && let Some(k) = adjacent(c)?
        {
            // nothing references it: the code area that ends right at it (Phyre's form)
            let code = sl(d, k, c);
            if let Some(st) = glslc_stage(ctl, code)?
                && check(st, ctl, code)?.is_none()
            {
                out.push(sh(format!("{label}#PHYRE@{k:#x}"), st, ctl, code));
                why = false;
            }
        }
        if why {
            cx.print(&format!("[RED] {label} control section @{c:#x}: no code area: no data record points at one, the code builds the address of none right after or right before it, and none of its length ends at it; left out"));
        }
    }
    Ok(out)
}

// ---- Retro Studios (RFRM / SNVN) ---------------------------------------------------------------------------------------------------

pub const RFRM: &[u8] = b"RFRM";
pub const SNVN: &[u8] = b"SNVN";

/// the byte order of the RFRM form or chunk at o, from its u64 length at +4: the reading that is a length is the small one
pub fn rfrm_order(d: &[u8], o: usize) -> Res<bool> {
    // true: little-endian
    Ok(u64le(d, o + 4)? < u64be(d, o + 4)?)
}

/// `rfrm_chunk(d, o)`: (fourcc, data start, data end, little-endian)
pub fn rfrm_chunk(d: &[u8], o: usize) -> Res<(&[u8], usize, usize, bool)> {
    let le = rfrm_order(d, o)?;
    let len = if le {
        u64le(d, o + 4)?
    } else {
        u64be(d, o + 4)?
    } as usize;
    Ok((at(d, o, 4), o + 0x18, (o + 0x18).saturating_add(len), le))
}

type Stages = Vec<(Vec<u8>, Vec<u8>)>;

/// `snvn_records(d, a, b, label, e)`: the program records of an SNVN chunk's data d[a:b]: [(offset, [(ctl, code), ...])]
pub fn snvn_records(
    cx: &Ctx<'_>,
    d: &[u8],
    a: usize,
    b: usize,
    label: &str,
    le: bool,
) -> Res<Vec<(usize, Stages)>> {
    let mut out = Vec::new();
    let mut i = a + 12;
    loop {
        // SNVN_STREAM.search(d, i + 1, b): 'x' or the zstd magic
        let mut m = None;
        let mut p = i + 1;
        while p < b && p < d.len() {
            if d[p] == b'x' || (p + 4 <= b && d[p..].starts_with(&ZSTD_MAGIC)) {
                m = Some(p);
                break;
            }
            p += 1;
        }
        let m = match m {
            Some(m) => m,
            None => break,
        };
        i = m;
        if i < 13 {
            return Err(crate::bytes::short("snvn"));
        }
        let rd = |o: usize| if le { u32le(d, o) } else { u32be(d, o) };
        let (ncode, nctl, _z, nz) = (
            rd(i - 13)? as usize,
            rd(i - 9)? as usize,
            u8at(d, i - 5)?,
            rd(i - 4)? as usize,
        );
        if !(nctl != 0
            && nctl % 4 == 0
            && nctl < 1 << 20
            && ncode < 1 << 24
            && 0 < nz
            && nz <= b - i)
        {
            continue;
        }
        let y: Vec<u8>;
        if d[i] == 0x78 {
            match crate::inflate::run(at(d, i, nz), crate::inflate::Wrap::Zlib, None) {
                Ok(z) if z.eof && z.used == nz => y = z.out,
                _ => continue,
            }
        } else {
            match cx.zstd.decompress(at(d, i, nz), None, None) {
                Ok(v) => y = v,
                Err(_) => continue, // not a stream of whole frames: the magic's four bytes inside other data
            }
        }
        if y.len() != ncode + nctl {
            continue;
        }
        let (mut stages, mut k, mut c) = (Vec::new(), 0usize, ncode);
        while c < y.len() {
            if !eq_at(&y, c, &CTL_MAGIC) {
                red!("{label} SNVN record @{i:#x}: no control section at {c:#x}");
            }
            let (mut n, nk) = (
                u32le(&y, c + 0x20)? as usize,
                u32le(&y, c + 0x704)? as usize,
            );
            let (x, xn) = (
                u32le(&y, c + 0x7e8)? as usize,
                u32le(&y, c + 0x7ec)? as usize,
            ); // data GLSLC put after the section, not in its own length
            if xn != 0 && x >= n {
                n = x + xn;
            }
            stages.push((at(&y, c, n).to_vec(), at(&y, k, nk).to_vec()));
            k += nk;
            c += n;
            if c < y.len() && !eq_at(&y, c, &CTL_MAGIC) {
                c = ncode + ((c - ncode + 15) & !15); // Metroid Prime 4: the next section starts on 16 bytes
            }
        }
        if k != ncode {
            red!(
                "{label} SNVN record @{i:#x}: the code areas end at {k:#x}, the control sections start at {ncode:#x}"
            );
        }
        out.push((i, stages));
        i += nz - 1;
    }
    Ok(out)
}

pub fn snvn_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    let mut p = find(d, SNVN, 0);
    while let Some(pp) = p {
        let (_, a, b, le) = rfrm_chunk(d, pp)?;
        for (i, stages) in snvn_records(cx, d, a, b, label, le)? {
            for (ctl, code) in stages {
                let st = glslc_stage(&ctl, &code)?;
                let why = match st {
                    Some(st) => check(st, &ctl, &code)?.map(str::to_string),
                    None => Some("no stage in the code area".to_string()),
                };
                if let Some(w) = why {
                    red!("{label} SNVN record @{i:#x}: {w}");
                }
                out.push(Shader {
                    label: format!("{label}#SNVN@{i:#x}#var"),
                    st: st.unwrap(),
                    ctl,
                    code,
                });
            }
        }
        p = find(d, SNVN, b);
    }
    Ok(out)
}

/// `retro_lzss8(s, n)`: Retro's LZSS on 8-bit units → the first n bytes, or None when the stream runs out or points before its start
pub fn retro_lzss8(s: &[u8], n: usize) -> Option<Vec<u8>> {
    let (mut out, mut i, mut flags, mut left) = (Vec::<u8>::new(), 0usize, 0u8, 0u32);
    while out.len() < n {
        if left == 0 {
            flags = *s.get(i)?;
            i += 1;
            left = 8;
        }
        if flags & 0x80 != 0 {
            let (b0, b1) = (*s.get(i)?, *s.get(i + 1)?);
            let (ln, dist) = (
                (b0 >> 4) as usize + 3,
                ((b0 & 0xf) as usize) << 8 | b1 as usize,
            );
            i += 2;
            if !(0 < dist && dist <= out.len()) {
                return None;
            }
            for _ in 0..ln {
                let v = out[out.len() - dist];
                out.push(v);
            }
        } else {
            out.push(*s.get(i)?);
            i += 1;
        }
        flags <<= 1;
        left -= 1;
    }
    out.truncate(n);
    Some(out)
}

/// `retro_main_forms(d, label)`: RFRM forms compressed into main with CBufferedCompression
pub fn retro_main_forms(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for p in find_all(d, RFRM) {
        if p < 5 || d[p - 5] != 1 || at(d, p - 4, 3) != [0, 0, 0] {
            continue;
        }
        let head = match retro_lzss8(&d[p - 1..], 0x20) {
            Some(h) => h,
            None => continue,
        };
        if head.is_empty() || &head[..4] != RFRM || &head[0x14..0x18] != b"MTRL" {
            continue;
        }
        let le = rfrm_order(&head, 0)?;
        let ln = if le {
            u64le(&head, 4)?
        } else {
            u64be(&head, 4)?
        } as usize;
        if let Some(x) = retro_lzss8(&d[p - 1..], 0x20usize.saturating_add(ln)) {
            out.extend(snvn_shaders(cx, &x, &format!("{label}@{:#x}", p - 5))?);
        }
    }
    Ok(out)
}

/// `rfrm_shaders(d, label)`: a Retro Studios RFRM file: a 'PACK' archive or a single form
pub fn rfrm_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    if !eq_at(d, 0x14, b"PACK") {
        return snvn_shaders(cx, d, label);
    }
    let mut out = Vec::new();
    let a = match find(d, b"ADIR", 0x20) {
        Some(a) => a,
        None => red!("{label}: RFRM PACK without an ADIR chunk"),
    };
    let (_, _, end, le) = rfrm_chunk(d, a)?;
    let rd32 = |o: usize| if le { u32le(d, o) } else { u32be(d, o) };
    let rd64 = |o: usize| if le { u64le(d, o) } else { u64be(d, o) };
    let n = rd32(a + 0x18)? as usize;
    let size = end.saturating_sub(a + 0x1c).checked_div(n).unwrap_or(36);
    if ![36, 52, 60].contains(&size) || a + 0x1c + n * size != end {
        red!(
            "{label}: ADIR of {n} entries in {:#x} bytes, neither 36, 52 nor 60 bytes each",
            end.saturating_sub(a + 0x1c)
        );
    }
    for i in 0..n {
        let e = a + 0x1c + size * i;
        if !eq_at(d, e, b"MTRL") {
            continue;
        }
        let (off, ln) = (
            rd64(e + if size == 36 { 20 } else { 28 })? as usize,
            rd64(e + if size == 36 { 28 } else { 36 })? as usize,
        );
        let stored = if size == 36 {
            ln
        } else {
            rd64(e + 44)? as usize
        };
        let guid = hex(at(d, e + 4, 16));
        if stored != ln {
            red!(
                "{label}: MTRL resource {guid} is stored compressed ({stored:#x} of {ln:#x} bytes), which this reader does not decode"
            );
        }
        let r = at(d, off, ln);
        if !starts(r, RFRM) {
            red!("{label}: MTRL resource {guid} is not an RFRM form");
        }
        let r2: Vec<u8>;
        let r: &[u8] = if eq_at(r, 0x20, b"\x78") {
            r2 = crate::inflate::zlib_decompress(sl(r, 0x20, r.len()))
                .map_err(|e| Failure::Red(format!("zlib.error: {e}")))?;
            &r2
        } else if eq_at(r, 0x20, &ZSTD_MAGIC) {
            r2 = cx
                .zstd
                .decompress(sl(r, 0x20, r.len()), None, None)
                .map_err(|e| Failure::Red(format!("ZstdError: {e}")))?;
            &r2
        } else {
            r
        };
        out.extend(snvn_shaders(cx, r, &format!("{label}/{guid}"))?);
    }
    Ok(out)
}
