//! The NVN shader containers that need nothing but the bytes: BNSH (and agl's BAHS), sead's primitive drawer, NDSR / NDVE, MSHD, Needle, SHN!, CIVBSF, Sumo's Ninty_NX / NVNsh,
//! Phyre, PlatinumGames' shader objects, Star Fox's binaries, MonoGame's MGFX, Monolith's HCPS / SLCT. Port of the same-named functions of `shader_split.py`.

use crate::bytes::*;
use crate::ctl::*;
use crate::error::{Failure, Res};
use crate::red;
use crate::scan_types::{Ctx, Shader};
use std::collections::BTreeSet;

fn sat(base: usize, off: u64) -> usize {
    usize::try_from(off)
        .ok()
        .and_then(|o| base.checked_add(o))
        .unwrap_or(usize::MAX)
}

fn sh(label: String, st: St, ctl: &[u8], code: &[u8]) -> Shader {
    Shader::new(label, st, ctl, code)
}

// ---- BNSH -------------------------------------------------------------------------------------------------------------------

pub fn bnsh_shaders(d: &[u8], base: usize, label: &str) -> Res<Vec<Shader>> {
    let wh = format!("{label} BNSH @{base:#x}");
    if !eq_at(d, base + 0x60, b"grsc") {
        red!("{wh} has no grsc block");
    }
    field_ok(d, base + 0x7c, 12, &format!("{wh} grsc"))?;
    let nvar = u32le(d, base + 0x7c)?;
    let pvar = u64le(d, base + 0x80)?;
    let mut out = Vec::new();
    for v in 0..nvar as u64 {
        let at_prog = sat(sat(base, pvar), 0x40 * v + 0x10);
        field_ok(d, at_prog, 8, &format!("{wh} variation {v}"))?;
        let prog = u64le(d, at_prog)?;
        if prog == 0 {
            continue;
        }
        for (si, st) in STAGES.iter().enumerate() {
            let a = sat(sat(base, prog), 8 + 8 * si as u64);
            field_ok(d, a, 8, &format!("{wh} variation {v} program"))?;
            let sc = u64le(d, a)?;
            if sc == 0 {
                continue;
            }
            let a = sat(sat(base, sc), 8);
            field_ok(d, a, 24, &format!("{wh} variation {v} {}", st.short()))?;
            let (pctl, pcode, ncode, nctl) = (
                u64le(d, a)?,
                u64le(d, a + 8)?,
                u32le(d, a + 16)? as usize,
                u32le(d, a + 20)? as usize,
            );
            let ctl = span(
                d,
                sat(base, pctl),
                nctl,
                &format!("{wh} variation {v} {} control section", st.short()),
            )?;
            let code = span(
                d,
                sat(base, pcode),
                ncode,
                &format!("{wh} variation {v} {} code area", st.short()),
            )?;
            if let Some(why) = check(*st, ctl, code)? {
                red!(
                    "{label} BNSH @{base:#x} variation {v} {}: {why}",
                    st.short()
                );
            }
            out.push(sh(format!("{label}#BNSH@{base:#x}#var{v}"), *st, ctl, code));
        }
        // NVN2's ray-tracing stage (the rebased engine): the program's 7th pointer, +0x38. Older programs end there; only a pointer whose control header identifies NVN2 stage 8 admits this optional
        // descriptor, and after that identification malformed lengths or metadata are errors
        if sat(sat(base, prog), 0x40) <= d.len() {
            let sc = u64le(d, sat(sat(base, prog), 0x38))?;
            if sc != 0 && sat(sat(base, sc), 0x20) <= d.len() {
                let q = sat(sat(base, sc), 8);
                let (pctl, pcode, ncode, nctl) = (
                    u64le(d, q)?,
                    u64le(d, q + 8)?,
                    u32le(d, q + 16)? as usize,
                    u32le(d, q + 20)? as usize,
                );
                let prefix = at(d, sat(base, pctl), 0x57);
                if nvn2_ctl(prefix) && prefix.len() == 0x57 && prefix[0x56] == 8 {
                    let ctl = span(
                        d,
                        sat(base, pctl),
                        nctl,
                        &format!("{wh} variation {v} rt control section"),
                    )?;
                    let code = span(
                        d,
                        sat(base, pcode),
                        ncode,
                        &format!("{wh} variation {v} rt code area"),
                    )?;
                    if let Some(why) = check(St::Rt, ctl, code)? {
                        red!("{wh} variation {v} rt: {why}");
                    }
                    out.push(sh(
                        format!("{label}#BNSH@{base:#x}#var{v}"),
                        St::Rt,
                        ctl,
                        code,
                    ));
                }
            }
        }
        // NVN2's task and mesh stages follow, at +0x40 and +0x48 of the program
        for (off, st, stage) in [(0x40u64, St::Ts, 6u8), (0x48, St::Ms, 7)] {
            let a = sat(sat(base, prog), off);
            if a.saturating_add(8) > d.len() {
                continue;
            }
            let sc = u64le(d, a)?;
            if sc == 0 || sat(sat(base, sc), 0x20) > d.len() {
                continue;
            }
            let q = sat(sat(base, sc), 8);
            let (pctl, pcode, ncode, nctl) = (
                u64le(d, q)?,
                u64le(d, q + 8)?,
                u32le(d, q + 16)? as usize,
                u32le(d, q + 20)? as usize,
            );
            if !(0x48..0x10000).contains(&nctl)
                || sat(sat(base, pctl), nctl as u64) > d.len()
                || sat(sat(base, pcode), ncode as u64) > d.len()
            {
                continue;
            }
            let ctl = at(d, sat(base, pctl), nctl);
            let code = at(d, sat(base, pcode), ncode);
            if !nvn2_ctl(ctl) || ctl.len() <= 0x56 || ctl[0x56] != stage {
                continue;
            }
            if let Some(why) = check(st, ctl, code)? {
                red!(
                    "{label} BNSH @{base:#x} variation {v} {}: {why}",
                    st.short()
                );
            }
            out.push(sh(format!("{label}#BNSH@{base:#x}#var{v}"), st, ctl, code));
        }
    }
    Ok(out)
}

/// The `for m in re.finditer(b'BNSH\0\0\0\0', d)` loop of `scan_blob`
pub fn bnsh_in(d: &[u8], label: &str, out: &mut Vec<Shader>) -> Res<()> {
    for m in find_all(d, b"BNSH\0\0\0\0") {
        if m + 0x20 > d.len() {
            continue;
        }
        let size = u32le(d, m + 0x1c)? as usize;
        if m + size <= d.len() && eq_at(d, m + 0x60, b"grsc") {
            out.extend(bnsh_shaders(d, m, label)?);
        }
    }
    Ok(())
}

// ---- BAHS (agl's sharcb) ----------------------------------------------------------------------------------------------------

pub fn bahs_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    field_ok(d, 0x18, 8, &format!("{label} BAHS header"))?;
    let (rsz, region) = (u32le(d, 0x18)? as usize, u32le(d, 0x1c)? as usize);
    if region + rsz > d.len() {
        red!("{label} BAHS code region out of bounds");
    }
    // (entry offset a, control offset c, shader)
    let mut found: Vec<(usize, usize, Shader)> = Vec::new();
    let mut a = 0usize;
    while a + 16 < region {
        let (off, z, csz, rel) = (
            u32le(d, a)? as usize,
            u32le(d, a + 4)?,
            u32le(d, a + 8)? as i64,
            i32le(d, a + 12)? as i64,
        );
        let ai = a as i64;
        let nvn2 = (0x48..0x10000).contains(&csz)
            && 0 <= ai + rel
            && ai + rel + csz <= region as i64
            && nvn2_ctl(at(d, (ai + rel) as usize, csz as usize));
        // control length: Mario 0x880 / 0x88c, 3D World's BAHS version 9 0x848; NVN2 (Donkey Kong Bananza's agl resources) 0xd8
        if z != 0
            || !(nvn2 || csz == 0x880 || csz == 0x88c || (0x760..0x880).contains(&csz))
            || off % 0x100 != 0
        {
            a += 4;
            continue;
        }
        let c = ai + rel;
        if c < 0 || c + csz > region as i64 || !eq_at(d, c as usize, &CTL_MAGIC) {
            a += 4;
            continue;
        }
        let c = c as usize;
        let n = u32le(d, c + if nvn2 { 0x44 } else { 0x704 })? as usize;
        if off + n > rsz {
            a += 4;
            continue;
        }
        let code = at(d, region + off, n);
        let ctl = at(d, c, csz as usize);
        let c4 = sl(code, 0, 4);
        let st = if nvn2 {
            nvn2_stage(ctl, code)
        } else if c4 == CS_MAGIC {
            Some(St::Cs)
        } else if c4 == GFX_MAGIC {
            sph_stage(code)
        } else {
            None
        };
        let st = match st {
            Some(s) => s,
            None => {
                a += 4;
                continue;
            }
        };
        if check(st, ctl, code)?.is_some() {
            a += 4;
            continue;
        }
        found.push((a, c, sh(format!("{label}#BAHS@{a:#x}"), st, ctl, code)));
        a += 4;
    }
    // A program entry never lies inside a control section. Each section counts with its own length (ctl_len)
    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(found.len());
    for (_, c, s) in &found {
        spans.push((*c, *c + ctl_len(&s.ctl)?));
    }
    spans.sort_unstable();
    spans.dedup();
    let inside = |a: usize| -> bool {
        // any (lo, hi) with lo <= a < hi: the spans are sorted by lo, and a control length is at most 0x880 (ctl_len), so only the spans that start within 0x880 before a can hold it
        let k = spans.partition_point(|&(lo, _)| lo <= a);
        spans[..k]
            .iter()
            .rev()
            .take_while(|&&(lo, _)| lo + 0x880 > a)
            .any(|&(_, hi)| a < hi)
    };
    let candidates: Vec<(usize, Shader)> = found
        .into_iter()
        .filter(|(a, _, _)| !inside(*a))
        .map(|(a, _, s)| (a, s))
        .collect();
    // A validated binary array is authoritative: descriptor-shaped words may also occur after the control header, inside a record's other payload (5df3dbe8: Mario Kart World 1.8.0's
    // DrivePassShaderDefault has one outside its 2,580 record entries, pairing compute control with unrelated vertex code)
    let mut offsets: BTreeSet<usize> = candidates.iter().map(|(a, _)| *a).collect();
    if let Some((records, end)) = bahs_binary_records(d, region, &offsets)? {
        let rec_at: BTreeSet<usize> = records.iter().map(|&(a, _)| a).collect();
        offsets.retain(|a| rec_at.contains(a));
        bahs_variations(cx, d, label, &offsets, &records, end)?;
    }
    Ok(candidates
        .into_iter()
        .filter(|(a, _)| offsets.contains(a))
        .map(|(_, s)| s)
        .collect())
}

/// (entry offset, kind) of each binary record, and the binary array's end
pub type BinaryRecords = (Vec<(usize, u32)>, usize);

/// `bahs_binary_records(d, region, taken)`: the validated binary array, (entry offset, kind) of each record and the array's end, or None for a layout it does not recognise
pub fn bahs_binary_records(
    d: &[u8],
    region: usize,
    taken: &BTreeSet<usize>,
) -> Res<Option<BinaryRecords>> {
    let Some(&first) = taken.iter().next() else {
        return Ok(None);
    };
    if first < 0x18 + 0x20 {
        return Ok(None);
    }
    let arr = first - 0x18;
    let (asz, acnt) = (u32le(d, arr)? as usize, u32le(d, arr + 4)? as usize);
    let end = arr + asz;
    let strays = 1.max(taken.len() / 100);
    if acnt == 0 || end > region || (acnt as i64) < taken.len() as i64 - strays as i64 {
        return Ok(None);
    }
    let (mut o, mut recs) = (arr + 8, Vec::new());
    for _ in 0..acnt {
        if o + 0x20 > end {
            return Ok(None);
        }
        let (rs, kind) = (u32le(d, o)? as usize, u32le(d, o + 4)?);
        if rs < 0x20 {
            return Ok(None);
        }
        recs.push((o + 0x10, kind));
        o += rs;
    }
    // Every candidate is a record's entry, apart from stray scan matches (Donkey Kong Bananza's agl_common: 1,556 candidates, 1,555 records): one stray, or up to 1 % in larger arrays
    let rec_at: std::collections::HashSet<usize> = recs.iter().map(|&(a, _)| a).collect();
    let outside = taken.iter().filter(|t| !rec_at.contains(t)).count();
    if o != end || outside > strays {
        return Ok(None);
    }
    Ok(Some((recs, end)))
}

/// `bahs_variations(d, label, taken, recs, o)`: the variations of an agl BAHS file's programs, for `var_pairs` (see the Python's docstring); `o` is the binary array's end, where the program
/// array starts
pub fn bahs_variations(
    cx: &Ctx<'_>,
    d: &[u8],
    label: &str,
    taken: &BTreeSet<usize>,
    recs: &[(usize, u32)],
    o: usize,
) -> Res<()> {
    let acnt = recs.len();
    if o + 8 > d.len() {
        return Ok(());
    }
    let (psz, pcnt) = (u32le(d, o)? as usize, u32le(d, o + 4)? as usize);
    if o + psz > d.len() || pcnt == 0 {
        return Ok(());
    }
    let (mut q, mut bases) = (o + 8, Vec::<usize>::new());
    for _ in 0..pcnt {
        if q + 16 > o + psz {
            return Ok(());
        }
        let (rs, base) = (u32le(d, q)? as usize, u32le(d, q + 12)? as usize);
        if rs < 16 || base > acnt || bases.last().is_some_and(|&l| base < l) {
            return Ok(());
        }
        bases.push(base);
        q += rs;
    }
    if q != o + psz {
        return Ok(());
    }
    bases.push(acnt);
    let mut var = cx.bahs_var.borrow_mut();
    for p in 0..pcnt {
        let mut seen = [0usize; 2];
        for &(a, k) in &recs[bases[p]..bases[p + 1]] {
            if k > 1 {
                continue; // compute and the other kinds pair with nothing
            }
            if taken.contains(&a) {
                var.insert(
                    format!("{label}#BAHS@{a:#x}"),
                    format!("{label}#BAHS#prog{p}#var{}", seen[k as usize]),
                );
            }
            seen[k as usize] += 1;
        }
    }
    Ok(())
}

// ---- sead / NDSR / NDVE / MSHD ----------------------------------------------------------------------------------------------

pub fn sead_prim_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    if d.len() < 0x10 {
        return Ok(vec![]);
    }
    let offs = [
        u32le(d, 0)? as usize,
        u32le(d, 4)? as usize,
        u32le(d, 8)? as usize,
        u32le(d, 12)? as usize,
    ];
    let mut out = Vec::new();
    for (st, c, k) in [(St::Vs, offs[0], offs[2]), (St::Ps, offs[1], offs[3])] {
        if !(0x10 <= c && c + 0x48 <= d.len() && nvn2_ctl(at(d, c, 0x48))) {
            return Ok(vec![]);
        }
        let own = u32le(d, c + 0x30)? as usize;
        let ctl = at(d, c, own.max(0x48));
        let n = u32le(ctl, 0x44)? as usize;
        let code = at(d, k, n);
        if code.len() != n || nvn2_stage(ctl, code) != Some(st) || check(st, ctl, code)?.is_some() {
            return Ok(vec![]);
        }
        out.push(sh(format!("{label}#SEADPRIM@{c:#x}"), st, ctl, code));
    }
    Ok(out)
}

pub fn ndsr_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    if !starts(d, b"NDSR") || d.len() < 0x20 {
        return Ok(vec![]);
    }
    // how many of the first records point at an NVN2 control section
    let records_from = |o: usize| -> usize {
        let count = 16.min((d.len().saturating_sub(o)) / 16);
        (0..count)
            .filter(|i| {
                u32le(d, o + 16 * i)
                    .map(|c| c as usize + 0x48 <= d.len() && nvn2_ctl(at(d, c as usize, 0x48)))
                    .unwrap_or(false)
            })
            .count()
    };
    // `max((0x10, 0x14), key=records_from)`: the first maximal one
    let o0 = if records_from(0x14) > records_from(0x10) {
        0x14
    } else {
        0x10
    };
    let mut o = o0;
    let mut out = Vec::new();
    let mut prog: Option<String> = None;
    while o + 16 <= d.len() {
        let (c, cl, k, x) = (
            u32le(d, o)? as usize,
            u32le(d, o + 4)? as usize,
            u32le(d, o + 8)? as usize,
            u32le(d, o + 12)?,
        );
        if c == 0 && cl == 0 && k == 0 && x == 0 {
            break;
        }
        // Entries are 8 bytes, a shader's control and code areas two of them, so data entries can shift the next shader by 8
        if !(c + 0x48 <= d.len() && nvn2_ctl(at(d, c, 0x48))) {
            o += 8;
            continue;
        }
        o += 16;
        let ctl = at(d, c, (cl >> 4).max(0x48));
        let code = at(d, k, u32le(ctl, 0x44)? as usize);
        let st = match nvn2_stage(ctl, code) {
            Some(s) => s,
            None => continue,
        };
        if check(st, ctl, code)?.is_some() {
            continue;
        }
        let wh = format!("{label}#NDSR@{c:#x}");
        out.push(sh(wh.clone(), st, ctl, code));
        if st == St::Vs {
            prog = Some(wh);
        } else if st == St::Ps
            && let Some(p) = prog.take()
        {
            let v = format!(
                "{label}#NDSR#{}",
                p.rsplit_once('@').map(|x| x.1).unwrap_or("")
            );
            let mut m = cx.bahs_var.borrow_mut();
            m.insert(p, v.clone());
            m.insert(wh, v);
        }
    }
    Ok(out)
}

pub fn ndve_shaders(cx: &Ctx<'_>, d: &[u8], label: &str) -> Res<Vec<Shader>> {
    if d.len() < 0x18 {
        return Ok(vec![]);
    }
    let pack = 0x18 + 8 * u32le(d, 0x14)? as usize;
    let nets = pack + u32le(d, 0xc)? as usize;
    let mut out = ndsr_shaders(cx, sl(d, pack, nets), label)?;
    let mut c = find(d, &CTL_MAGIC, nets);
    while let Some(cc) = c {
        if cc & 3 == 0 && cc + 0x48 <= d.len() && nvn2_ctl(at(d, cc, 0x48)) {
            let (own, n) = (u32le(d, cc + 0x30)? as usize, u32le(d, cc + 0x44)? as usize);
            let ctl = at(d, cc, own);
            let code = at(d, cc + own, n);
            if let Some(st) = nvn2_stage(ctl, code)
                && code.len() == n
                && check(st, ctl, code)?.is_none()
            {
                out.push(sh(format!("{label}#NDVE@{cc:#x}"), st, ctl, code));
            }
        }
        c = find(d, &CTL_MAGIC, cc + 4);
    }
    Ok(out)
}

pub fn mshd_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out: Vec<Shader> = Vec::new();
    let mut c = find(d, &CTL_MAGIC, 0);
    while let Some(cc) = c {
        field_ok(
            d,
            cc + 0x20,
            4,
            &format!("{label} MSHD control section @{cc:#x}"),
        )?;
        let nctl = u32le(d, cc + 0x20)? as usize;
        let g = find(d, &GFX_MAGIC, cc + nctl);
        match g {
            Some(g) if nctl >= 0x880 && g - cc - nctl <= 0x400 => {
                let ncode = u32le(d, cc + 0x704)? as usize;
                let ctl = at(d, cc, nctl);
                let code = span(d, g, ncode, &format!("{label} MSHD @{cc:#x} code area"))?;
                if ncode < 0x34 {
                    red!(
                        "{label} MSHD @{cc:#x}: a code area of {ncode:#x} bytes, shorter than its header"
                    );
                }
                let st = sph_stage(code);
                let want = if out.len().is_multiple_of(2) {
                    St::Vs
                } else {
                    St::Ps
                };
                if st != Some(want) {
                    red!(
                        "{label} MSHD @{cc:#x}: stage {} is {}, not alternating vertex / pixel",
                        out.len(),
                        st.map(|s| s.short()).unwrap_or("None")
                    );
                }
                let st = st.unwrap();
                if let Some(why) = check(st, ctl, code)? {
                    red!("{label} MSHD @{cc:#x} {}: {why}", st.short());
                }
                out.push(sh(
                    format!("{label}#MSHD#var{}", out.len() / 2),
                    st,
                    ctl,
                    code,
                ));
                c = find(d, &CTL_MAGIC, g + ncode);
            }
            _ => red!(
                "{label} MSHD control section @{cc:#x}: length {nctl:#x}, no code area header after it"
            ),
        }
    }
    if !out.len().is_multiple_of(2) {
        red!("{label} MSHD: {} stages, not in pairs", out.len());
    }
    Ok(out)
}

// ---- Needle -----------------------------------------------------------------------------------------------------------------

pub const NEEDLE: &[u8] = b"HNSHV002";

pub fn needle_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    // by nvn2_ctl: (the descriptor's length (its control offset), the least control length, where the control holds its own length)
    let rec = |nvn2: bool| -> (usize, usize, usize) {
        if nvn2 {
            (0x98, NVN2_KEY_CTL, 0x30)
        } else {
            (0x90, 0x760, 0x20)
        }
    };
    let min_size = rec(false).0 + rec(false).1;
    let min_size = min_size.min(rec(true).0 + rec(true).1);
    let mut out = Vec::new();
    for h in find_all(d, NEEDLE) {
        if h + 12 > d.len() {
            continue;
        }
        let size = u32be(d, h + 8)? as usize;
        if size < min_size || size > d.len() - h {
            continue; // a literal magic string in executable code is not a container
        }
        let end = h + size;
        let mut c = find_in(d, &CTL_MAGIC, h + 12, end);
        while let Some(cc) = c {
            let (desc, cmin, own) = rec(nvn2_ctl(at(
                d,
                cc,
                NVN2_KEY_CTL.min(end.saturating_sub(cc)),
            )));
            if size < desc + cmin {
                break; // too small for this kind of payload: not a container
            }
            if cc < desc || cc - desc < h + 12 || cc + cmin > end {
                red!("{label} NEEDLE @{h:#x}: incomplete NVN section at {cc:#x}");
            }
            let s = cc - desc;
            let (co, ko, kn, cn) = (
                u32le(d, s + 0x30)? as usize,
                u32le(d, s + 0x34)? as usize,
                u32le(d, s + 0x38)? as usize,
                u32le(d, s + 0x3c)? as usize,
            );
            if co != desc || cn < cmin || cn != u32le(d, cc + own)? as usize {
                red!("{label} NEEDLE @{h:#x}: invalid control offset or control length at {s:#x}");
            }
            if ko < co + cn || kn == 0 || (co + cn).max(ko + kn) > end - s {
                red!(
                    "{label} NEEDLE @{h:#x}: NVN section at {s:#x} is outside its HNSHV002 container"
                );
            }
            let (ctl, code) = (at(d, cc, cn), at(d, s + ko, kn));
            let st = glslc_stage(ctl, code)?;
            let w = match st {
                Some(st) => check(st, ctl, code)?.map(str::to_string),
                None => Some("unknown shader stage".to_string()),
            };
            if let Some(w) = w {
                red!("{label} NEEDLE @{h:#x}: NVN section at {s:#x}: {w}");
            }
            out.push(sh(format!("{label}#NEEDLE@{s:#x}"), st.unwrap(), ctl, code));
            c = find_in(d, &CTL_MAGIC, s + ko + kn, end);
        }
    }
    Ok(out)
}

// ---- CIVBSF -----------------------------------------------------------------------------------------------------------------

pub const CIVBSF: &[u8] = b"CIVBSF\0\0";

pub fn civbsf_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let wh = format!("{label} CIVBSF");
    span(d, 0, 0x90, &wh)?;
    field_ok(d, 8, 4, &wh)?;
    let version = u32le(d, 8)?;
    // revision 0x1118 (Civilization VI 1.2.19): 0x48-byte entries holding four platform descriptors; 0x1218 (Civilization VII): 0x50-byte entries holding one data descriptor
    let stride = match version {
        0x1118 => 0x48usize,
        0x1218 => 0x50,
        _ => red!("{wh}: unknown revision {version:#x}"),
    };
    field_ok(d, 0xc, 4, &wh)?;
    let size = u32le(d, 0xc)? as usize;
    if size != d.len() {
        red!(
            "{wh}: file size {size:#x} does not match {:#x} bytes",
            d.len()
        );
    }
    field_ok(d, 0x20, 8, &wh)?;
    let table = u64le(d, 0x20)?;
    field_ok(d, 0x68, 4, &wh)?;
    let count = u32le(d, 0x68)? as usize;
    if table < 0x90 {
        red!("{wh}: shader table overlaps the header");
    }
    let table = sat(0, table);
    span(d, table, count * stride, &format!("{wh} shader table"))?;
    let mut out = Vec::new();
    for i in 0..count {
        let entry = table + i * stride;
        let source = format!("{wh} shader {i}");
        field_ok(d, entry + 8, 8, &source)?;
        let descriptor = sat(0, u64le(d, entry + 8)?);
        if version == 0x1118 {
            span(
                d,
                descriptor,
                0x40,
                &format!("{source} platform descriptors"),
            )?;
            for platform in 0..4usize {
                let at = descriptor + platform * 16;
                field_ok(d, at, 16, &source)?;
                let (offset, length, kind) = (
                    sat(0, u64le(d, at)?),
                    u32le(d, at + 8)? as usize,
                    u32le(d, at + 12)? as usize,
                );
                if kind != platform {
                    red!("{source}: platform descriptor {platform} has kind {kind}");
                }
                let payload = span(
                    d,
                    offset,
                    length,
                    &format!("{source} platform {platform} data"),
                )?;
                if platform != 3 {
                    continue;
                }
                span(payload, 0, 0x28, &format!("{source} NVBC"))?;
                if payload[..8] != *b"NVBC\0\0\0\0" {
                    red!("{source}: unknown NVBC tag");
                }
                field_ok(payload, 8, 16, &source)?;
                let (nout, nrefl) = (u64le(payload, 8)?, u64le(payload, 16)?);
                if 0x28u128 + nout as u128 + nrefl as u128 != length as u128 {
                    red!("{source}: NVBC output and reflection lengths do not match the data size");
                }
                let output = at_range(payload, 0x28, nout);
                if output.len() < 4
                    || output[..4] != GLSLC_MAGIC
                    || crate::glslc::glslc_layout(output, 0)?.0 as u64 != nout
                {
                    red!("{source}: invalid NVBC GLSLC output length or tag");
                }
                out.extend(crate::glslc::glslc_shaders(
                    output,
                    &format!("{label}#CIVBSF@{entry:#x}"),
                    true,
                    &[],
                )?);
            }
            continue;
        }
        field_ok(d, descriptor, 16, &format!("{source} descriptor"))?;
        let (offset, length) = (
            sat(0, u64le(d, descriptor)?),
            sat(0, u64le(d, descriptor + 8)?),
        );
        let payload = span(d, offset, length, &format!("{source} data"))?;
        field_ok(payload, 0, 16, &source)?;
        let (tag, ncode, nctl) = (
            u32le(payload, 0)?,
            u32le(payload, 4)? as usize,
            u32le(payload, 8)? as usize,
        );
        if tag != 0xff00ff00 {
            red!("{source}: unknown data tag {tag:#x}");
        }
        if 16 + ncode + nctl != length {
            red!("{source}: code and control lengths do not match the data size");
        }
        let (code, ctl) = (&payload[16..16 + ncode], &payload[16 + ncode..]);
        let st = glslc_stage(ctl, code)?;
        let w = match st {
            Some(st) => check(st, ctl, code)?.map(str::to_string),
            None => Some("unknown shader stage".to_string()),
        };
        if let Some(w) = w {
            red!("{source}: {w}");
        }
        out.push(sh(
            format!("{label}#CIVBSF@{entry:#x}"),
            st.unwrap(),
            ctl,
            code,
        ));
    }
    Ok(out)
}

/// `payload[a:a + n]` for a u64 length (a slice clamps)
fn at_range(d: &[u8], a: usize, n: u64) -> &[u8] {
    sl(d, a, sat(a, n))
}

// ---- SHN! (Shin'en) ---------------------------------------------------------------------------------------------------------

pub fn shn_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    span(d, 0, 0x44, &format!("{label} SHN! header"))?;
    let block = |a: usize| -> Res<Vec<u8>> {
        let wh = format!("{label} SHN! block");
        field_ok(d, a, 12, &wh)?;
        let (off, stored, size) = (
            u32le(d, a)? as usize,
            u32le(d, a + 4)? as usize,
            u32le(d, a + 8)? as usize,
        );
        if stored != 0 && off < 0x44 {
            red!("{label} SHN! block at {a:#x}: data overlaps the header");
        }
        let raw = span(d, off, stored, &format!("{label} SHN! block at {a:#x}"))?;
        if stored == size {
            return Ok(raw.to_vec());
        }
        // LZ4's match-length extensions add at most 255 decoded bytes per stored byte
        if stored == 0 || size > 255 * stored {
            red!(
                "{label} SHN! block at {a:#x}: impossible LZ4 length {size:#x} for {stored:#x} bytes"
            );
        }
        let mut out = vec![0u8; size];
        match lz4_flex::block::decompress_into(raw, &mut out) {
            Ok(n) => out.truncate(n),
            Err(e) => red!("{label} SHN! block at {a:#x}: invalid LZ4 data: {e}"),
        }
        if out.len() != size {
            red!(
                "{label} SHN! block at {a:#x}: decoded {:#x} bytes, expected {size:#x}",
                out.len()
            );
        }
        Ok(out)
    };
    let meta = block(4)?;
    let objects: Vec<usize> = find_all(&meta, b"SHDROUNC").collect();
    if objects.is_empty() {
        return Ok(vec![]);
    }
    let data = block(0x24)?;
    let mut out = Vec::new();
    for h in objects {
        field_ok(&meta, h + 0xc, 4, &format!("{label} SHDROUNC header"))?;
        let count = u32le(&meta, h + 0xc)? as usize;
        field_ok(&meta, h + 0x30, 8, &format!("{label} SHDROUNC header"))?;
        let table = sat(0, u64le(&meta, h + 0x30)?);
        span(
            &meta,
            table,
            count * 9 * 0x50,
            &format!("{label} SHDROUNC stage table"),
        )?;
        for v in 0..count {
            for slot in 0..9 {
                let r = table + (v * 9 + slot) * 0x50;
                let (k, c) = (sat(0, u64le(&meta, r)?), sat(0, u64le(&meta, r + 8)?));
                if c == 0 {
                    continue;
                }
                let (nk, nc) = (
                    u32le(&meta, r + 0x40)? as usize,
                    u32le(&meta, r + 0x44)? as usize,
                );
                let ctl = span(&meta, c, nc, &format!("{label} SHDROUNC control section"))?;
                let code = span(&data, k, nk, &format!("{label} SHDROUNC code area"))?;
                if !nvn2_ctl(ctl) {
                    red!("{label} SHDROUNC: invalid NVN2 control section at {c:#x}");
                }
                let st = nvn2_stage(ctl, code);
                let w = match st {
                    Some(st) => check(st, ctl, code)?.map(str::to_string),
                    None => Some("unknown shader stage".to_string()),
                };
                if let Some(w) = w {
                    red!("{label} SHDROUNC variant {v} slot {slot}: {w}");
                }
                out.push(sh(
                    format!("{label}#SHN@{h:#x}#var{v}"),
                    st.unwrap(),
                    ctl,
                    code,
                ));
            }
        }
    }
    Ok(out)
}

// ---- Sumo -------------------------------------------------------------------------------------------------------------------

pub fn sumo_nx() -> Vec<u8> {
    let mut v = b"Ninty_NX".to_vec();
    v.extend_from_slice(&[0u8; 8]);
    v
}
pub const NVNSH: &[u8] = b"NVNsh\0";
pub const SUMO_MUX: &[u8] = b"MuxFDMR\x02";

/// `sumo_members(d, label)`: Sumo's MuxFDMR v2: two buffer lengths at +24/+32, one or two zlib streams whose stored lengths start at +40.
pub fn sumo_members(d: &[u8], label: &str) -> Res<Vec<(String, Vec<u8>)>> {
    if d.len() < 44 {
        red!("{label}: truncated MuxFDMR header");
    }
    let (mut head, n) = (u32le(d, 8)? as usize, u8at(d, 18)? as usize);
    if u8at(d, 17)? != 2 || (n != 1 && n != 2) || head != 44 + 4 * n || head > d.len() {
        red!("{label}: unsupported MuxFDMR layout");
    }
    let mut sizes = Vec::new();
    for i in 0..n {
        sizes.push(u32le(d, 40 + 4 * i)? as usize);
    }
    if head + sizes.iter().sum::<usize>() != d.len() {
        red!("{label}: MuxFDMR streams do not fit the file");
    }
    let mut out = Vec::new();
    for (i, size) in sizes.into_iter().enumerate() {
        let expected = u32le(d, 24 + 8 * i)? as usize;
        let z = crate::inflate::run(
            at(d, head, size),
            crate::inflate::Wrap::Zlib,
            Some(expected + 1),
        )
        .map_err(|e| Failure::Red(format!("zlib.error: {e}")))?;
        if !z.eof || z.used != size || z.out.len() != expected {
            red!("{label}: MuxFDMR stream {i} length does not match its header");
        }
        out.push((format!("#MUX{i}"), z.out));
        head += size;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn sumo_program(
    d: &[u8],
    base: i64,
    size: i64,
    co: u64,
    ko: u64,
    cn: usize,
    kn: usize,
    label: String,
    stage: Option<St>,
) -> Res<Shader> {
    let (co_i, ko_i) = (co as i128, ko as i128);
    if base < 0
        || size < 0x34
        || base + size > d.len() as i64
        || cn < 0x760
        || kn < 0x40
        || co_i.min(ko_i) < 0x34
        || (co_i + cn as i128).max(ko_i + kn as i128) > size as i128
        || !(co_i + cn as i128 <= ko_i || ko_i + kn as i128 <= co_i)
    {
        red!("{label}: shader ranges do not fit the container");
    }
    let (base, co, ko) = (base as usize, co as usize, ko as usize);
    let (ctl, code) = (at(d, base + co, cn), at(d, base + ko, kn));
    let st = glslc_stage(ctl, code)?;
    let st = match st {
        Some(s) if stage.is_none() || stage == Some(s) => s,
        _ => red!("{label}: invalid shader stage"),
    };
    if let Some(w) = check(st, ctl, code)? {
        red!("{label}: {w}");
    }
    Ok(sh(label, st, ctl, code))
}

pub fn sumo_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for m in find_all(d, &sumo_nx()) {
        if m < 0x30 {
            continue;
        }
        let r = m - 0x30;
        let base = r as i64 - u64le(d, r + 0x28)? as i64;
        if base < 0 || (r as i64) < base + 0x38 {
            continue;
        }
        let size = u64le(d, base as usize + 8)? as i64;
        if r as i64 + 0x40 > base + size || base + size > d.len() as i64 {
            continue;
        }
        if u32le(d, base as usize + 4)? != 1 {
            red!("{label}: unsupported Ninty_NX container version");
        }
        let (co, ko, cn, kn) = (
            u64le(d, r)?,
            u64le(d, r + 8)?,
            u32le(d, r + 16)? as usize,
            u32le(d, r + 20)? as usize,
        );
        out.push(sumo_program(
            d,
            base,
            size,
            co,
            ko,
            cn,
            kn,
            format!("{label}#SUMO@{r:#x}"),
            None,
        )?);
    }
    for h in find_all(d, NVNSH) {
        if h + 0x34 > d.len() || !(0x34..=d.len() - h).contains(&(u32le(d, h + 8)? as usize)) {
            continue;
        }
        if u16le(d, h + 6)? != 1 {
            red!("{label}: unsupported NVNsh version");
        }
        let (size, stage) = (u32le(d, h + 8)? as i64, u32le(d, h + 28)? as usize);
        if stage >= STAGES.len() {
            red!("{label}: invalid NVNsh stage {stage}");
        }
        let (co, ko, kn, cn) = (
            u32le(d, h + 32)? as u64,
            u32le(d, h + 36)? as u64,
            u32le(d, h + 40)? as usize,
            u32le(d, h + 44)? as usize,
        );
        out.push(sumo_program(
            d,
            h as i64,
            size,
            co,
            ko,
            cn,
            kn,
            format!("{label}#NVNSH@{h:#x}"),
            Some(STAGES[stage]),
        )?);
    }
    Ok(out)
}

// ---- Phyre, PlatinumGames, Star Fox -----------------------------------------------------------------------------------------

/// `phyre_shaders(d, label)`: Phyre's NVN code immediately precedes its control section, unaligned in RYHP effects; loose blobs also prefix their two lengths (FFX/X-2). Unravel TWO also has
/// headerless compute code (the stage is `glslc_stage`'s; in a file other than an RYHP effect, or with a control magic inside its code, such a code area is not taken)
pub fn phyre_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for c in find_all(d, &CTL_MAGIC) {
        if c + 0x708 > d.len() {
            continue;
        }
        let (cn, kn) = (u32le(d, c + 0x20)? as usize, u32le(d, c + 0x704)? as usize);
        if kn > c {
            continue;
        }
        let k = c - kn;
        if kn < 0x80 || cn < 0x760 || c + cn > d.len() {
            continue;
        }
        let (ctl, code) = (at(d, c, cn), sl(d, k, c));
        let st = glslc_stage(ctl, code)?;
        if st == Some(St::Cs)
            && sl(code, 0, 4) != CS_MAGIC
            && (sl(d, 0, 4) != b"RYHP" || contains(code, &CTL_MAGIC))
        {
            continue;
        }
        if let Some(st) = st
            && check(st, ctl, code)?.is_none()
        {
            out.push(sh(format!("{label}#PHYRE@{k:#x}"), st, ctl, code));
        }
    }
    Ok(out)
}

pub fn pg_stage(magic: &[u8]) -> Option<St> {
    Some(match magic {
        b"VSO\0" => St::Vs,
        b"HSO\0" => St::Hs,
        b"DSO\0" => St::Ds,
        b"GSO\0" => St::Gs,
        b"PSO\0" => St::Ps,
        b"CSO\0" => St::Cs,
        _ => return None,
    })
}

/// One PGDAT member: (name, bytes).
pub type PgdatMember = (String, Vec<u8>);

pub fn pgdat_members(d: &[u8], _label: &str) -> Res<Option<Vec<PgdatMember>>> {
    let (n, ot, _et, nt, st) = (
        u32le(d, 4)? as usize,
        u32le(d, 8)? as usize,
        u32le(d, 12)? as usize,
        u32le(d, 16)? as usize,
        u32le(d, 20)? as usize,
    );
    if n == 0 {
        return Ok(Some(vec![]));
    }
    if (ot.saturating_add(4 * n))
        .max(st.saturating_add(4 * n))
        .max(nt + 4)
        > d.len()
    {
        return Ok(None);
    }
    let w = u32le(d, nt)? as usize;
    let mut offs = Vec::new();
    let mut sizes = Vec::new();
    for i in 0..n {
        offs.push(u32le(d, ot + 4 * i)? as usize);
        sizes.push(u32le(d, st + 4 * i)? as usize);
    }
    if (nt + 4).saturating_add(w.saturating_mul(n)) > d.len()
        || offs.iter().zip(&sizes).any(|(o, s)| o + s > d.len())
    {
        return Ok(None);
    }
    let mut out = Vec::new();
    for i in 0..n {
        let nm = sl(d, nt + 4 + w * i, nt + 4 + w * (i + 1));
        let nm = nm.split(|&b| b == 0).next().unwrap_or(&[]);
        out.push((
            String::from_utf8_lossy(nm).into_owned(),
            at(d, offs[i], sizes[i]).to_vec(),
        ));
    }
    Ok(Some(out))
}

pub fn pg_shader(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let st = pg_stage(&d[..4]).unwrap();
    let (nctl, octl, ncode, ocode) = (
        u32le(d, 8)? as usize,
        u32le(d, 12)? as usize,
        u32le(d, 16)? as usize,
        u32le(d, 20)? as usize,
    );
    if octl + nctl > d.len() || ocode + ncode > d.len() {
        red!("{label}: shader object sections out of bounds");
    }
    let (ctl, code) = (at(d, octl, nctl), at(d, ocode, ncode));
    let mut why = check(st, ctl, code)?.map(str::to_string);
    if why.is_none() && st != St::Cs && sph_stage(code) != Some(st) {
        why = Some(format!(
            "the SPH stage {} does not match the object's magic",
            sph_stage(code).map(|s| s.short()).unwrap_or("None")
        ));
    }
    if let Some(w) = why {
        red!("{label} ({}): {w}", st.short());
    }
    Ok(vec![sh(format!("{label}#PGSO"), st, ctl, code)])
}

pub fn pg_sized_shader(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    if d.len() < 12 {
        return Ok(vec![]);
    }
    let prefix = u32le(d, 0)? as usize;
    if prefix < 4 || prefix & 3 != 0 || prefix + 8 > d.len() {
        return Ok(vec![]);
    }
    let ncode = u32le(d, prefix)? as usize;
    let (codeoff, ctlsize) = (prefix + 4, prefix + 4 + ncode);
    let m = sl(d, codeoff, codeoff + 4);
    if ncode < 0x80 || ctlsize + 4 > d.len() || (m != GFX_MAGIC && m != CS_MAGIC) {
        return Ok(vec![]);
    }
    let nctl = u32le(d, ctlsize)? as usize;
    let ctloff = ctlsize + 4;
    if nctl < 0x760 || ctloff + nctl != d.len() || !eq_at(d, ctloff, &CTL_MAGIC) {
        return Ok(vec![]);
    }
    let (ctl, code) = (&d[ctloff..], &d[codeoff..ctlsize]);
    let st = if sl(code, 0, 4) == CS_MAGIC {
        Some(St::Cs)
    } else {
        sph_stage(code)
    };
    let st = match st {
        Some(s) => s,
        None => red!("{label}: binding-table shader has an unknown SPH stage"),
    };
    if let Some(w) = check(st, ctl, code)? {
        red!("{label} ({}): {w}", st.short());
    }
    Ok(vec![sh(format!("{label}#PGSO"), st, ctl, code)])
}

pub fn viper_shader(d: &[u8], label: &str) -> Res<Option<Vec<Shader>>> {
    if d.len() < 0x18 + NVN2_KEY_CTL || d[..4] != GLSLC_MAGIC {
        return Ok(None);
    }
    let (ncode, nctl) = (u32le(d, 4)? as usize, u32le(d, 8)? as usize);
    if 0x18 + ncode + nctl != d.len() || ncode == 0 || nctl == 0 {
        return Ok(None);
    }
    let (code, ctl) = (&d[0x18..0x18 + ncode], &d[0x18 + ncode..]);
    if !nvn2_ctl(ctl) {
        return Ok(None);
    }
    let st = match nvn2_stage(ctl, code) {
        Some(s) => s,
        None => red!(
            "{label}: a shader binary of no known stage (control +0x56 {:?}, code {})",
            ctl.get(0x56),
            hex(sl(code, 0, 4))
        ),
    };
    if let Some(w) = check(st, ctl, code)? {
        red!("{label} ({}): {w}", st.short());
    }
    Ok(Some(vec![sh(format!("{label}#VIPER"), st, ctl, code)]))
}

// ---- MonoGame MGFX ----------------------------------------------------------------------------------------------------------

pub const MGFX: &[u8] = b"MGFX";

/// `_Rd`: a little-endian reader over bytes, with .NET's BinaryReader strings (7-bit encoded length + UTF-8). `u8` past the end is the Python's IndexError,
/// `raw` slices silently clamp, `un` is a struct.unpack.
struct RdNet<'a> {
    d: &'a [u8],
    p: usize,
}
impl<'a> RdNet<'a> {
    fn u8(&mut self) -> Res<u8> {
        self.p += 1;
        self.d
            .get(self.p - 1)
            .copied()
            .ok_or_else(|| Failure::Red("IndexError: index out of range".into()))
    }
    fn u32(&mut self) -> Res<u32> {
        let v = u32le(self.d, self.p)?;
        self.p += 4;
        Ok(v)
    }
    fn raw(&mut self, n: usize) -> &'a [u8] {
        self.p += n;
        sl(self.d, self.p - n, self.p)
    }
    fn str(&mut self) -> Res<&'a [u8]> {
        let (mut n, mut sh) = (0usize, 0u32);
        loop {
            let b = self.u8()?;
            n |= ((b & 0x7f) as usize) << sh;
            sh += 7;
            if b < 0x80 {
                return Ok(self.raw(n));
            }
            if sh > 63 {
                red!("OverflowError: a 7-bit length that does not end");
            }
        }
    }
}

/// `mgfx_effect(d, p, label)`: a MonoGame effect at d[p:]; None when the header is not version 8's (or 5's) or its first shader is not an NVN one
pub fn mgfx_effect(d: &[u8], p: usize, label: &str) -> Res<Option<Vec<Shader>>> {
    let v5 = eq_at(d, p + 4, b"\x05");
    if !eq_at(d, p + 4, b"\x08") && !v5 {
        return Ok(None);
    }
    let mut r = RdNet {
        d,
        p: p + if v5 { 6 } else { 10 },
    };
    let head = (|| -> Res<bool> {
        for _ in 0..r.u8()? {
            if !v5 {
                r.str()?;
            }
            r.raw(2);
            let k = r.u8()? as usize;
            r.raw(3 * k);
        }
        // shader count, then the first shader's stage, length (and control length)
        let bad = if v5 {
            !eq_at(d, r.p + 6, &GLSLC_MAGIC)
        } else {
            !eq_at(d, r.p + 10, &CTL_MAGIC)
        };
        Ok(bad)
    })();
    match head {
        Ok(false) => {}
        _ => return Ok(None),
    }
    let xnb = label.ends_with(".xnb");
    fn no_annotations(r: &mut RdNet<'_>, label: &str, p: usize) -> Res<()> {
        if r.u8()? != 0 {
            red!("{label} MGFX @{p:#x}: annotations (MonoGame writes none)");
        }
        Ok(())
    }
    let mut out: Vec<Shader> = Vec::new();
    let nsh = r.u8()?;
    for i in 0..nsh {
        let vs = r.u8()? as usize;
        let n = r.u32()? as usize;
        if r.p + n > d.len() {
            // a shader whose length runs past the data: a stray 'MGFX' in other content, not an effect
            if xnb {
                red!(
                    "{label} MGFX @{p:#x}: shader {i} claims {n:#x} bytes past the end of the file"
                );
            }
            return Ok(None);
        }
        let bc = r.raw(n);
        let (st, ctl, code): (Option<St>, Vec<u8>, Vec<u8>);
        if v5 {
            let g = crate::glslc::glslc_shaders(
                bc,
                &format!("{label} MGFX @{p:#x} shader {i}"),
                false,
                &[],
            )?;
            if vs > 1 || g.len() != 1 || bc[..4] != GLSLC_MAGIC || u32le(bc, 0x44)? as usize != n {
                red!(
                    "{label} MGFX @{p:#x} shader {i}: {n:#x} bytes, not one GLSLC output of one stage"
                );
            }
            let g0 = &g[0];
            st = Some(g0.st);
            ctl = g0.ctl.clone();
            code = g0.code.clone();
        } else {
            let nctl = u32le(bc, 0)? as i64;
            let ncode: i64 = if 8 + nctl <= n as i64 {
                u32le(bc, 4 + nctl as usize)? as i64
            } else {
                -1
            };
            if vs > 1 || 8 + nctl + ncode != n as i64 {
                red!(
                    "{label} MGFX @{p:#x} shader {i}: {n:#x} bytes, not a control section and a code area"
                );
            }
            ctl = sl(bc, 4, 4 + nctl as usize).to_vec();
            code = sl(bc, 8 + nctl as usize, bc.len()).to_vec();
            st = if sl(&code, 0, 4) == GFX_MAGIC {
                sph_stage(&code)
            } else {
                None
            };
        }
        let want = if vs == 0 { St::Ps } else { St::Vs };
        let why = if st == Some(want) {
            check(st.unwrap(), &ctl, &code)?.map(str::to_string)
        } else {
            Some(format!(
                "the SPH stage is {}, the effect says {}",
                st.map(|s| s.short()).unwrap_or("None"),
                if vs == 0 { "pixel" } else { "vertex" }
            ))
        };
        if let Some(w) = why {
            red!("{label} MGFX @{p:#x} shader {i}: {w}");
        }
        out.push(Shader {
            label: format!("{label}#MGFX@{p:#x}#{i}"),
            st: st.unwrap(),
            ctl,
            code,
        });
        for _ in 0..r.u8()? {
            r.raw(3);
            if r.u8()? != 0 {
                if v5 {
                    red!(
                        "{label} MGFX @{p:#x} shader {i}: a sampler state (none in the version 5 effects seen)"
                    );
                } else {
                    r.raw(20); // address u / v / w, border colour, filter, max anisotropy, max mip level, mip LOD bias
                }
            }
            if !v5 {
                r.str()?;
            }
            r.u8()?;
        }
        let k = r.u8()? as usize;
        r.raw(k);
        if !v5 {
            for _ in 0..r.u8()? {
                r.str()?;
                r.raw(4);
            }
        }
    }
    fn params(r: &mut RdNet<'_>, label: &str, p: usize, depth: usize) -> Res<u8> {
        if depth > 64 {
            red!("{label} MGFX @{p:#x}: parameters nest too deep");
        }
        let n = r.u8()?;
        for _ in 0..n {
            let two = r.raw(2);
            let typ = *two
                .get(1)
                .ok_or_else(|| Failure::Red("IndexError: index out of range".into()))?;
            r.str()?;
            r.str()?;
            no_annotations(r, label, p)?;
            let (rows, cols) = (r.u8()? as usize, r.u8()? as usize);
            let a = params(r, label, p, depth + 1)? as usize;
            let b = params(r, label, p, depth + 1)? as usize;
            if a + b != 0 {
                continue;
            }
            if (1..=3).contains(&typ) {
                r.raw(4 * rows * cols);
            } else if typ == 4 {
                r.str()?;
            }
        }
        Ok(n)
    }
    params(&mut r, label, p, 0)?;
    for _ in 0..r.u8()? {
        // techniques
        r.str()?;
        no_annotations(&mut r, label, p)?;
        for _ in 0..r.u8()? {
            // passes: name, annotations, vertex / pixel shader index, then the blend, depth-stencil and rasterizer states when present
            r.str()?;
            no_annotations(&mut r, label, p)?;
            let (v, f) = (r.u8()? as usize, r.u8()? as usize);
            if (v != 255 && v >= out.len()) || (f != 255 && f >= out.len()) {
                red!(
                    "{label} MGFX @{p:#x}: a pass names shader {v} / {f} of {}",
                    out.len()
                );
            }
            for n in [18usize, 25, 12] {
                if r.u8()? != 0 {
                    r.raw(n);
                }
            }
        }
    }
    // in Bastion's 24 XNB files the length in front of 'MGFX' runs to the file's end, about 0xff0 bytes past the effect, all zeros
    if r.p > d.len() || (xnb && sl(d, r.p, d.len()).iter().any(|&b| b != 0)) {
        red!(
            "{label} MGFX @{p:#x}: the effect ends at {:#x}, the file at {:#x}",
            r.p,
            d.len()
        );
    }
    Ok(Some(out))
}

pub fn mgfx_shaders(d: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut out = Vec::new();
    for m in find_all(d, MGFX) {
        out.extend(mgfx_effect(d, m, label)?.unwrap_or_default());
    }
    if label.ends_with(".xnb") && contains(at(d, 0, 0x200), b"EffectReader") && out.is_empty() {
        red!("{label}: an XNB effect without an MGFX version 5 or 8 effect of NVN shaders");
    }
    Ok(out)
}

// ---- Monolith HCPS / SLCT -----------------------------------------------------------------------------------------------------

pub fn hcps_shaders(d: &[u8], base: usize, label: &str) -> Res<Vec<Shader>> {
    let spn = |off: usize, n: usize| -> Res<&[u8]> {
        if off < base || off > d.len() || n > d.len() - off {
            red!("{label} HCPS @{base:#x}: section out of bounds");
        }
        Ok(&d[off..off + n])
    };
    spn(base, 72)?;
    let (table, count) = (u32le(d, base + 8)? as usize, u32le(d, base + 12)? as usize);
    let g = |i: usize| -> Res<usize> { Ok(u32le(d, base + 24 + 4 * i)? as usize) };
    let (slct_off, slct_size, code_off, code_size, ctl_off, ctl_size) =
        (g(0)?, g(1)?, g(2)?, g(3)?, g(4)?, g(5)?);
    spn(base + table, count * 8)?;
    let (slcts, codes, ctls) = (
        spn(base + slct_off, slct_size)?,
        spn(base + code_off, code_size)?,
        spn(base + ctl_off, ctl_size)?,
    );
    if code_size == 0 && ctl_size == 0 {
        return Ok(vec![]); // source-only GLSL programs (DLC model streams), no NVN binaries to extract
    }
    let mut out = Vec::new();
    for i in 0..count {
        let s = u32le(d, base + table + 8 * i)? as usize;
        if s + 72 > slcts.len() || &slcts[s..s + 4] != b"SLCT" {
            red!("{label} HCPS: invalid SLCT {i}");
        }
        let (num, ptr) = (
            u32le(slcts, s + 16)? as usize,
            u32le(slcts, s + 20)? as usize,
        );
        let (mut c, cn, mut k, kn) = (
            u32le(slcts, s + 40)? as usize,
            u32le(slcts, s + 44)? as usize,
            u32le(slcts, s + 48)? as usize,
            u32le(slcts, s + 52)? as usize,
        );
        let (ce, ke) = (c + cn, k + kn);
        if s + ptr + num * 8 > slcts.len() || ce > ctls.len() || ke > codes.len() {
            red!("{label} HCPS SLCT {i}: variant range out of bounds");
        }
        for j in 0..num {
            let (po, pn) = (
                u32le(slcts, s + ptr + j * 8)? as usize,
                u32le(slcts, s + ptr + j * 8 + 4)? as usize,
            );
            let p = s + po;
            if pn < 32 || p + pn > slcts.len() {
                red!("{label} HCPS: invalid NVSD");
            }
            let (vo, vn) = (
                u32le(slcts, p + 24)? as usize,
                u32le(slcts, p + 28)? as usize,
            );
            if vn == 0 || vo + vn * 20 > pn {
                red!("{label} HCPS: invalid NVSD stage records");
            }
            let q = p + vo + (vn - 1) * 20;
            let (vs, ps, vc, pc) = (
                u32le(slcts, q + 4)? as usize,
                u32le(slcts, q + 8)? as usize,
                u32le(slcts, q + 12)? as usize,
                u32le(slcts, q + 16)? as usize,
            );
            let (pend, kend) = (c + vc + pc, k + vs + ps);
            if pend > ce || kend > ke {
                red!("{label} HCPS: program exceeds SLCT range");
            }
            while c < pend {
                // One asset can contain both Maxwell and NVN2 HCPS programs (Xenoblade X DE). Read each control's own version and lengths.
                if c + NVN2_KEY_CTL > pend {
                    red!("{label} HCPS: truncated control");
                }
                let nvn2 = nvn2_ctl(at(ctls, c, NVN2_KEY_CTL));
                if !nvn2 && c + 0x708 > pend {
                    red!("{label} HCPS: truncated control");
                }
                let nc = u32le(ctls, c + if nvn2 { 0x30 } else { 0x20 })? as usize;
                let nk = u32le(ctls, c + if nvn2 { 0x44 } else { 0x704 })? as usize;
                if nc < (if nvn2 { NVN2_KEY_CTL } else { 0x760 })
                    || c + nc > pend
                    || nk == 0
                    || k + nk > kend
                {
                    red!("{label} HCPS: shader exceeds program range");
                }
                let (ctl, code) = (at(ctls, c, nc), at(codes, k, nk));
                c += (nc + 7) & !7;
                k += nk;
                let st = if nvn2 {
                    nvn2_stage(ctl, code)
                } else {
                    glslc_stage(ctl, code)?
                };
                let why = match st {
                    Some(st) => check(st, ctl, code)?.map(str::to_string),
                    None => Some("unknown stage".to_string()),
                };
                if let Some(w) = why {
                    red!("{label} HCPS SLCT {i} variant {j}: {w}");
                }
                out.push(sh(
                    format!("{label}#HCPS@{base:#x}#var{i}.{j}"),
                    st.unwrap(),
                    ctl,
                    code,
                ));
            }
            if k != kend {
                red!("{label} HCPS: unused program code bytes");
            }
        }
        if c != ce || k != ke {
            red!("{label} HCPS SLCT {i}: unused control/code bytes");
        }
    }
    Ok(out)
}

pub fn slct_shaders(d: &[u8], base: usize, label: &str) -> Res<Vec<Shader>> {
    if base + 72 > d.len() {
        return Ok(vec![]);
    }
    let (c, cn, k, kn) = (
        u32le(d, base + 40)? as usize,
        u32le(d, base + 44)? as usize,
        u32le(d, base + 48)? as usize,
        u32le(d, base + 52)? as usize,
    );
    let size = (c + cn).max(k + kn);
    if c.min(k) < 72 || size > d.len() - base {
        return Ok(vec![]);
    }
    let mut header = vec![0u8; 80];
    header[..4].copy_from_slice(b"HCPS");
    header[8..12].copy_from_slice(&72u32.to_le_bytes());
    header[12..16].copy_from_slice(&1u32.to_le_bytes());
    for (i, v) in [80u32, size as u32, 80, size as u32, 80, size as u32]
        .iter()
        .enumerate()
    {
        header[24 + 4 * i..28 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    header.extend_from_slice(at(d, base, size));
    hcps_shaders(&header, 0, &format!("{label}#SLCT@{base:#x}"))
}
