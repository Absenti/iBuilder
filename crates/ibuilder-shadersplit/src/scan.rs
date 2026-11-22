//! `scan_blob`, `scan_file_in`, `scan_file`, the Unity readers on top of `unity.rs`, and the pair tables: the middle of `shader_split.py` (l.3104-3300, 3470-3660).

use crate::archive::*;
use crate::bytes::*;
use crate::ctl::*;
use crate::error::{Failure, Res};
use crate::glslc::*;
use crate::image::Loc;
use crate::nvn::*;
use crate::scan_types::*;
use crate::source::{MemSource, Source, contains_each};
use crate::unity::{self, Trees, Val};
use crate::zstd::{LZ4_FRAME_MAGIC, ZSTD_MAGIC};
use crate::{red, unsupported};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// A file to scan: the root it belongs to (the files of one version / DLC root), its path below it and where its bytes are
#[derive(Clone)]
pub struct FileRef {
    pub root: usize,
    pub rel: String,
    pub loc: Loc,
}

// ---- Unity ------------------------------------------------------------------------------------------------------------------

/// One Unity program: (stage, the two byte blobs).
type UnityProgram = (St, Vec<u8>, Vec<u8>);

/// `unity_program(code, where, stage)`: Unity's Switch program code → [(stage, control, code)]; stage = the subprogram's stage from m_ParsedForm, which only the pre-2019.3 layout needs
fn unity_program(code: &[u8], wh: &str, stage: Option<St>) -> Res<Vec<UnityProgram>> {
    if code.len() >= 0x14 && code[0x10..0x14] == CTL_MAGIC {
        // before 2019.3: 0x10 header + control section + code area, one stage
        let stage = match stage {
            Some(s) => s,
            None => red!("{wh}: pre-2019.3 subprogram whose stage is not in m_ParsedForm"),
        };
        let (nctl, ncode) = (i32le(code, 8)? as i64, i32le(code, 12)? as i64);
        let a = 0x10i64;
        let end = a + nctl + ncode;
        // slices with Python's semantics (negative lengths give empty slices)
        let sub = |x: i64, y: i64| -> &[u8] {
            if x < 0 || y < x {
                &[]
            } else {
                sl(code, x as usize, y as usize)
            }
        };
        let (ctl, cd) = (sub(a, a + nctl), sub(a + nctl, end));
        let why = if end <= code.len() as i64 {
            check(stage, ctl, cd)?
        } else {
            Some("block out of bounds")
        };
        if let Some(w) = why {
            red!("{wh} {}: {w}", stage.short());
        }
        if end != code.len() as i64 {
            red!(
                "{wh}: program code is {:#x} bytes, but control section + code area end at {end:#x}",
                code.len()
            );
        }
        return Ok(vec![(stage, ctl.to_vec(), cd.to_vec())]);
    }
    let mut h = [0i32; 24];
    for (i, v) in h.iter_mut().enumerate() {
        *v = i32le(code, 4 * i)?;
    }
    let mut out = Vec::new();
    let mut end = 0x60i64;
    for (i, st) in NV_STAGES.iter().enumerate() {
        let (off, nctl, ncode) = (h[6 + i] as i64, h[12 + i] as i64, h[18 + i] as i64);
        if off == -1 {
            continue;
        }
        let a = 0x60 + off;
        let sub = |x: i64, y: i64| -> &[u8] {
            if x < 0 || y < x {
                &[]
            } else {
                sl(code, x as usize, y as usize)
            }
        };
        let (ctl, cd) = (sub(a, a + nctl), sub(a + nctl, a + nctl + ncode));
        let why = if a + nctl + ncode <= code.len() as i64 {
            check(*st, ctl, cd)?
        } else {
            Some("block out of bounds")
        };
        if let Some(w) = why {
            red!("{wh} {}: {w}", st.short());
        }
        out.push((*st, ctl.to_vec(), cd.to_vec()));
        end = end.max(a + nctl + ncode);
    }
    if end != code.len() as i64 {
        red!(
            "{wh}: program code is {:#x} bytes, but the stage blocks end at {end:#x}",
            code.len()
        );
    }
    Ok(out)
}

fn passes<'a, 'b>(o: &'b Val<'a>) -> Vec<&'b Val<'a>> {
    let mut v = Vec::new();
    if let Some(pf) = o.get("m_ParsedForm") {
        for sub in pf.get("m_SubShaders").map(Val::list).unwrap_or(&[]) {
            for p in sub.get("m_Passes").map(Val::list).unwrap_or(&[]) {
                v.push(p);
            }
        }
    }
    v
}

const PROG_FIELDS: [(&str, St); 5] = [
    ("progVertex", St::Vs),
    ("progFragment", St::Ps),
    ("progGeometry", St::Gs),
    ("progHull", St::Hs),
    ("progDomain", St::Ds),
];

/// `unity_stages(o)`: Shader object → {subprogram index: stage}, from the passes of m_ParsedForm
fn unity_stages(o: &Val<'_>) -> HashMap<i64, St> {
    let mut m = HashMap::new();
    for p in passes(o) {
        for (field, st) in PROG_FIELDS {
            if let Some(f) = p.get(field) {
                for sp in f.get("m_SubPrograms").map(Val::list).unwrap_or(&[]) {
                    if let Some(bi) = sp.get("m_BlobIndex").and_then(Val::int) {
                        m.entry(bi).or_insert(st);
                    }
                }
            }
        }
    }
    m
}

/// `unity_code_blobs(o)`: the subprogram indices that hold program code, or None when every entry does (before Unity 2022)
fn unity_code_blobs(o: &Val<'_>) -> Option<HashSet<i64>> {
    // a program that is absent or empty contributes nothing (the Python's `p.get(f) or {}`)
    let progs: Vec<&Val<'_>> = passes(o)
        .into_iter()
        .flat_map(|p| PROG_FIELDS.iter().filter_map(move |(f, _)| p.get(f)))
        .collect();
    if !progs
        .iter()
        .any(|p| p.get("m_ParameterBlobIndices").is_some())
    {
        return None;
    }
    let mut set = HashSet::new();
    for p in progs {
        for tier in p.get("m_PlayerSubPrograms").map(Val::list).unwrap_or(&[]) {
            for sp in tier.list() {
                if let Some(bi) = sp.get("m_BlobIndex").and_then(Val::int) {
                    set.insert(bi);
                }
            }
        }
    }
    Some(set)
}

/// `ps_inputs(code)`: generic attributes a pixel shader reads: SPH words 6-13 are the input map, 8 bits for each of the 32 attributes
fn ps_inputs(code: &[u8]) -> Res<BTreeSet<u32>> {
    let mut s = BTreeSet::new();
    for i in 0..32u32 {
        if (u32le(code, 0x30 + 4 * (6 + (i as usize >> 2)))? >> ((i & 3) * 8)) & 0xff != 0 {
            s.insert(i);
        }
    }
    Ok(s)
}

/// `pixel_input_cover(shaders)`: {pixel key: code} → a deterministic greedy cover of their generic inputs; ties take the smaller key
pub fn pixel_input_cover_pub(shaders: &BTreeMap<String, Vec<u8>>) -> Res<Vec<String>> {
    pixel_input_cover(shaders)
}

/// The bytes of a pixel shader's code area that [`ps_inputs`] reads: the input map is SPH words 6-13, at `0x30 + 4 * (6 + i / 4)`, so the first `PS_INPUTS_PREFIX` bytes are all that matter
/// (a code area shorter than that is read whole and fails in `ps_inputs` exactly as before). The split keeps its shaders in a spill file and reads only this much of each pixel shader.
pub const PS_INPUTS_PREFIX: usize = 0x30 + 4 * (6 + 7) + 4;

/// `ps_inputs(code)` for the split's pixel cover (`code` may be the first [`PS_INPUTS_PREFIX`] bytes of the code area)
pub fn ps_inputs_pub(code: &[u8]) -> Res<BTreeSet<u32>> {
    ps_inputs(code)
}

/// The cover of pixel shaders given by their generic inputs ({pixel key: inputs}): what `pixel_input_cover` does after reading the inputs
pub fn pixel_cover_of_inputs(ins: &BTreeMap<String, BTreeSet<u32>>) -> Vec<String> {
    let ins: BTreeMap<&String, &BTreeSet<u32>> = ins.iter().collect();
    let mut left: BTreeSet<u32> = BTreeSet::new();
    for v in ins.values() {
        left.extend(v.iter().copied());
    }
    cover_greedy(&ins, left)
}

fn pixel_input_cover(shaders: &BTreeMap<String, Vec<u8>>) -> Res<Vec<String>> {
    let mut owned: BTreeMap<&String, BTreeSet<u32>> = BTreeMap::new();
    let mut left: BTreeSet<u32> = BTreeSet::new();
    for (k, code) in shaders {
        let i = ps_inputs(code)?;
        left.extend(i.iter().copied());
        owned.insert(k, i);
    }
    let ins: BTreeMap<&String, &BTreeSet<u32>> = owned.iter().map(|(k, v)| (*k, v)).collect();
    Ok(cover_greedy(&ins, left))
}

/// The greedy loop: the shader that covers the most of what is left, ties to the smaller key
fn cover_greedy(ins: &BTreeMap<&String, &BTreeSet<u32>>, mut left: BTreeSet<u32>) -> Vec<String> {
    let mut cover = Vec::new();
    while !left.is_empty() {
        let k = ins
            .iter()
            // ties to the smaller key: compared as `&str` (the same order as the `String` the key was copied into before, without a copy per candidate and step)
            .min_by_key(|(k, v)| (std::cmp::Reverse(v.intersection(&left).count()), k.as_str()))
            .map(|(k, _)| (*k).clone())
            .unwrap();
        for x in ins[&k].iter() {
            left.remove(x);
        }
        cover.push(k);
    }
    cover
}

type Legacy = HashMap<(i64, i64), (St, Vec<u8>, Vec<u8>)>;

/// `unity_pass_pairs(o, programs)`: legacy one-stage programs → vertex / pixel-input-cover pairs within each parsed pass and platform
fn unity_pass_pairs(
    cx: &Ctx<'_>,
    o: &Val<'_>,
    programs: &Legacy,
) -> Res<BTreeSet<(String, String)>> {
    let mut pairs = BTreeSet::new();
    let plats: BTreeSet<i64> = programs.keys().map(|k| k.0).collect();
    for p in passes(o) {
        for &plat in &plats {
            let mut vs: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            let mut ps: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            for (field, stage) in [("progVertex", St::Vs), ("progFragment", St::Ps)] {
                if let Some(f) = p.get(field) {
                    for sp in f.get("m_SubPrograms").map(Val::list).unwrap_or(&[]) {
                        let bi = sp.get("m_BlobIndex").and_then(Val::int);
                        if let Some(program) = bi.and_then(|b| programs.get(&(plat, b)))
                            && program.0 == stage
                        {
                            let k = cx.skey(&program.1, &program.2)?;
                            if stage == St::Vs { &mut vs } else { &mut ps }
                                .insert(k, program.2.clone());
                        }
                    }
                }
            }
            let cover = pixel_input_cover(&ps)?;
            for v in vs.keys() {
                for f in &cover {
                    pairs.insert((v.clone(), f.clone()));
                }
            }
        }
    }
    Ok(pairs)
}

/// `unity_compute(x, base, trees)`: ComputeShader objects (class 72) of a serialized file → [(where, 'cs', control, code)]
fn unity_compute(cx: &Ctx<'_>, x: &dyn Source, base: &str, trees: &mut Trees) -> Res<Vec<Shader>> {
    let mut out: Vec<Shader> = Vec::new();
    let r = unity::for_each_object(
        x,
        cx.common,
        &[unity::CLASS_COMPUTE_SHADER],
        Some(trees),
        |_cid, _pid, o| {
            let name = o.get("m_Name").map(Val::str).unwrap_or_default();
            for v in o
                .get("variants")
                .map(Val::list)
                .ok_or_else(|| Failure::Red("KeyError: 'variants'".into()))?
            {
                for k in v
                    .get("kernels")
                    .map(Val::list)
                    .ok_or_else(|| Failure::Red("KeyError: 'kernels'".into()))?
                {
                    let variants: Vec<&Val<'_>> = if let Some(u) = k.get("uniqueVariants") {
                        u.list().iter().collect()
                    } else if let Some(m) = k.get("variantMap") {
                        let mut v = Vec::new();
                        for p in m.list() {
                            v.push(
                                p.get("second")
                                    .ok_or_else(|| Failure::Red("KeyError: 'second'".into()))?,
                            );
                        }
                        v
                    } else {
                        vec![k]
                    };
                    for (i, u) in variants.iter().enumerate() {
                        let code = u
                            .get("code")
                            .ok_or_else(|| Failure::Red("KeyError: 'code'".into()))?;
                        if !code.truthy() {
                            continue;
                        }
                        let kname = k
                            .get("name")
                            .map(Val::str)
                            .ok_or_else(|| Failure::Red("KeyError: 'name'".into()))?;
                        let wh = format!("{base}#{name}#{kname}.{i}");
                        for (st, ctl, cd) in unity_program(code.bytes(), &wh, Some(St::Cs))? {
                            out.push(Shader {
                                label: wh.clone(),
                                st,
                                ctl,
                                code: cd,
                            });
                        }
                    }
                }
            }
            Ok(())
        },
    );
    match r {
        Ok(()) => {}
        Err(Failure::NoTree(e)) => {
            if cx.borrowed.is_empty() {
                return Err(Failure::NoTree(e));
            }
            cx.print(&format!("[RED] {base}: ComputeShader objects that no bundle lends a type tree to and unityfs.py has no layout for, their kernels are left out ({e})"));
            return Ok(vec![]);
        }
        Err(e) => return Err(e),
    }
    // The NSP script may derive shader-only tables when a new reader adds compute kernels to a registered build: the reader's key/source evidence
    for s in &out {
        cx.print(&format!(
            "unity compute source: [{}, {}]",
            crate::pyjson::string(&cx.skey(&s.ctl, &s.code)?),
            crate::pyjson::string(&s.label)
        ));
    }
    Ok(out)
}

/// `unity_text_glslc(x, base)`: whole GLSLC outputs that a game keeps in a TextAsset of a serialized file
fn unity_text_glslc(cx: &Ctx<'_>, x: &dyn Source, base: &str) -> Res<Vec<Shader>> {
    let has =
        contains_each(x, &[&GLSLC_MAGIC]).map_err(|e| Failure::Red(format!("OSError: {e}")))?;
    if !has[0] {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    unity::for_each_text_asset(x, cx.common, |_pid, name, b| {
        if contains(b, &GLSLC_MAGIC) {
            out.extend(glslc_shaders(b, &format!("{base}#{name}"), false, &[])?);
        }
        Ok(())
    })?;
    Ok(out)
}

/// `unity_shaders(d, label, out, trees)`: shaders and compute shaders in a UnityFS bundle or serialized file → out and legacy pass pairs; NoTree when no type tree is available
pub fn unity_shaders(
    cx: &Ctx<'_>,
    src: &dyn Source,
    bundle: bool,
    label: &str,
    out: &mut Vec<Shader>,
    trees: &mut Trees,
) -> Res<BTreeSet<(String, String)>> {
    if cx.common.is_none() {
        red!("{label}: a Unity asset, but main has no Unity common string table");
    }
    let mut pairs = BTreeSet::new();
    let mut owned: Vec<(String, Vec<u8>)> = Vec::new();
    if bundle {
        let all = src
            .read_all()
            .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
        owned = unity::bundle_files(&all)?;
    }
    let mems: Vec<(String, MemSource<'_>)> = owned
        .iter()
        .map(|(n, b)| (n.clone(), MemSource(b)))
        .collect();
    let mut sers: Vec<(&str, &dyn Source)> = Vec::new();
    if bundle {
        for (n, m) in &mems {
            sers.push((n.as_str(), m));
        }
    } else {
        sers.push(("", src));
    }
    for (node, x) in sers {
        let head = x
            .read(0, 48)
            .map_err(|e| Failure::Red(format!("OSError: {e}")))?;
        if !unity::is_serialized(&head, x.len()) {
            continue;
        }
        let base = format!(
            "{label}{}",
            if node.is_empty() {
                String::new()
            } else {
                format!(":{node}")
            }
        );
        unity::for_each_object(
            x,
            cx.common,
            &[unity::CLASS_SHADER],
            Some(&mut *trees),
            |_cid, _pid, o| {
                let name = match o.get("m_ParsedForm") {
                    Some(pf) => pf
                        .get("m_Name")
                        .map(Val::str)
                        .ok_or_else(|| Failure::Red("KeyError: 'm_Name'".into()))?,
                    None => o
                        .get("m_Name")
                        .map(Val::str)
                        .unwrap_or_else(|| "?".to_string()),
                };
                let stages = unity_stages(o);
                let mut legacy: Legacy = HashMap::new();
                let only = unity_code_blobs(o);
                let progs = unity::shader_programs(o, only.as_ref())?;
                for (plat, k, sp) in progs.iter() {
                    let wh = format!("{base}#{name}#var{plat}.{k}");
                    let code = unity::subprogram_code(sp)?;
                    let programs = unity_program(code, &wh, stages.get(&k).copied())?;
                    if code.len() >= 0x14 && code[0x10..0x14] == CTL_MAGIC {
                        // before 2019.3: the one program is also kept for the pass pairs
                        for (st, ctl, cd) in &programs {
                            out.push(Shader {
                                label: wh.clone(),
                                st: *st,
                                ctl: ctl.clone(),
                                code: cd.clone(),
                            });
                        }
                        let p0 = programs.into_iter().next().unwrap();
                        legacy.insert((plat, k), p0);
                    } else {
                        for (st, ctl, cd) in programs {
                            out.push(Shader {
                                label: wh.clone(),
                                st,
                                ctl,
                                code: cd,
                            });
                        }
                    }
                }
                pairs.extend(unity_pass_pairs(cx, o, &legacy)?);
                Ok(())
            },
        )?;
        out.extend(unity_compute(cx, x, &base, &mut *trees)?);
        out.extend(unity_text_glslc(cx, x, &base)?);
    }
    Ok(pairs)
}

// ---- scan_blob ------------------------------------------------------------------------------------------------------------------

/// the archives-in-archives budget: [members, Yaz0 expansion]
type Left = [i64; 2];

/// `scan_blob(d, label, out, depth, left)`
#[allow(unused_assignments)]
pub fn scan_blob(
    cx: &Ctx<'_>,
    d: &[u8],
    label: &str,
    out: &mut Vec<Shader>,
    depth: usize,
    left: Option<&mut Left>,
) -> Res<()> {
    let nest = NEST as i64;
    if depth > NEST {
        red!("{label}: archives nested more than {NEST} deep");
    }
    if starts(d, b"SHN!") {
        out.extend(shn_shaders(d, label)?);
        return Ok(());
    }
    // Media.Vision's NXVS / NXFS / NXCS container, a file of its own (nxsh_shader, strict)
    if d.len() >= 4 && crate::mvgl::nxsh_stage(&d[..4]).is_some() {
        out.extend(crate::mvgl::nxsh_shader(d, 0, label, true)?);
        return Ok(());
    }
    if starts(d, CIVBSF) {
        out.extend(civbsf_shaders(d, label)?);
        return Ok(());
    }
    if starts(d, SUMO_MUX) {
        let mut own: Left = [0, 0];
        let members = sumo_members(d, label)?;
        let lref: &mut Left = match left {
            Some(l) => l,
            None => {
                let t: i64 = members.iter().map(|m| m.1.len() as i64).sum();
                own = [nest * t, nest * t]; // as with Yaz0, count decoded bytes, not compressed bytes
                &mut own
            }
        };
        for (nm, member) in &members {
            scan_blob(
                cx,
                member,
                &format!("{label}{nm}"),
                out,
                depth + 1,
                Some(&mut *lref),
            )?;
        }
        return Ok(());
    }
    if starts(d, b"HCPS") || starts(d, b"xbc1") || starts(d, b"DRSM") || starts(d, b"cmp\0zlib") {
        return monolith_shaders(cx, d, label, out, 0);
    }
    if starts(d, PACX4) {
        let mut own: Left = [0, 0];
        // LZ4 chunks: as with Yaz0, the budget counts decoded bytes
        let parts = pacx4_parts(d, label)?;
        let lref: &mut Left = match left {
            Some(l) => l,
            None => {
                let t = (d.len() as i64).max(parts.iter().map(|p| p.1.len() as i64).sum());
                own = [nest * t, nest * t];
                &mut own
            }
        };
        for (nm, p) in &parts {
            scan_blob(
                cx,
                p,
                &format!("{label}/{nm}"),
                out,
                depth + 1,
                Some(&mut *lref),
            )?;
        }
        return Ok(());
    }
    for m in find_all(d, b"SLCT\x03\x10\0\0") {
        out.extend(slct_shaders(d, m, label)?);
    }
    let stored = d.len() as i64;
    let yaz_buf: Vec<u8>;
    let mut d = d;
    let mut left = left;
    if starts(d, b"Yaz0") {
        if let Some(l) = left.as_deref_mut() {
            // charged by its header before it is decoded
            l[1] -= yaz0_size(d) as i64;
            if l[1] < 0 {
                red!("{label}: the Yaz0 archive members expand to more than {NEST} times the file");
            }
        }
        yaz_buf = yaz0(d, label)?;
        d = &yaz_buf;
    }
    let mut own: Left = [0, 0];
    let lref: &mut Left = match left {
        Some(l) => {
            l[0] -= stored;
            if l[0] < 0 {
                red!(
                    "{label}: the archive members add up to more than {NEST} times the file (members that overlap)"
                );
            }
            l
        }
        None => {
            own = [nest * d.len() as i64, nest * d.len() as i64]; // the file itself: what its archives' members may add up to, and what their Yaz0 may expand to
            &mut own
        }
    };
    if starts(d, b"SARC") {
        for (nm, m) in sarc_members(d, label)? {
            scan_blob(
                cx,
                m,
                &format!("{label}/{nm}"),
                out,
                depth + 1,
                Some(&mut *lref),
            )?;
        }
        return Ok(());
    }
    if starts(d, b"NARC") {
        unsupported!("Camelot NARC container (cra.py: {label})");
    }
    if starts(d, b"BAHS") {
        out.extend(bahs_shaders(cx, d, label)?);
        return Ok(());
    }
    if starts(d, b"rgxd") {
        out.extend(rgxd_shaders(cx, d, label)?);
        return Ok(());
    }
    if starts(d, b"NDSR") {
        out.extend(ndsr_shaders(cx, d, label)?);
        return Ok(());
    }
    if starts(d, b"NDVE") {
        out.extend(ndve_shaders(cx, d, label)?);
        return Ok(());
    }
    let sp = sead_prim_shaders(d, label)?;
    if !sp.is_empty() {
        out.extend(sp);
        return Ok(());
    }
    if starts(d, TFXO) {
        out.extend(tfxo_shaders(d, label)?);
        return Ok(());
    }
    if starts(d, SHAD) {
        out.extend(shad_shaders(d, label)?);
        return Ok(());
    }
    if starts(d, b"MSHD") {
        out.extend(mshd_shaders(d, label)?);
        return Ok(());
    }
    if starts(d, b"DAT\0")
        && d.len() >= 0x1c
        && let Some(mem) = pgdat_members(d, label)?
    {
        for (nm, m) in &mem {
            scan_blob(
                cx,
                m,
                &format!("{label}/{nm}"),
                out,
                depth + 1,
                Some(&mut *lref),
            )?;
        }
        return Ok(());
    }
    if d.len() >= 0x18 && pg_stage(&d[..4]).is_some() {
        let o = u32le(d, 0xc)? as usize;
        if eq_at(d, o, &CTL_MAGIC) {
            out.extend(pg_shader(d, label)?);
            return Ok(());
        }
    }
    let sized = pg_sized_shader(d, label)?;
    if !sized.is_empty() {
        out.extend(sized);
        return Ok(());
    }
    if let Some(v) = viper_shader(d, label)? {
        out.extend(v);
        return Ok(());
    }
    if starts(d, b"RYHP") || eq_at(d, 8, &GFX_MAGIC) {
        // Phyre's effects and loose programs (FFX / X-2, Unravel TWO): when none is found the file goes on through the other readers
        let ph = phyre_shaders(d, label)?;
        if !ph.is_empty() {
            out.extend(ph);
            return Ok(());
        }
        // an RYHP container that wraps whole GLSLC outputs (Trails into Reverie's .fx*.phyre)
        if starts(d, b"RYHP") && contains(d, &GLSLC_MAGIC) {
            let g = glslc_shaders(d, label, false, &[])?;
            if !g.is_empty() {
                out.extend(g);
                return Ok(());
            }
        }
    }
    if starts(d, b"VIPR") {
        // Star Fox's resource containers: two of them wrap the engine's NDSR upscaler networks, 40 shaders each, as a file inside
        for m in find_all(d, b"NDSR") {
            if m + 0x20 <= d.len() && eq_at(d, m + 4, b"\x00\x01") {
                out.extend(ndsr_shaders(cx, &d[m..], &format!("{label}#NDSR@{m:#x}"))?);
            }
        }
        return Ok(());
    }
    bnsh_in(d, label, out)?;
    if contains(d, MGFX) {
        out.extend(mgfx_shaders(d, label)?);
    }
    if contains(d, NEEDLE) {
        out.extend(needle_shaders(d, label)?);
    }
    if contains(d, &sumo_nx()) || contains(d, NVNSH) {
        out.extend(sumo_shaders(d, label)?);
    }
    if crate::mvgl::has_embedded(d) {
        out.extend(crate::mvgl::nxsh_embedded(d, label)?);
    }
    Ok(())
}

// ---- pairs ---------------------------------------------------------------------------------------------------------------------

/// `var_pairs(found)`: (vertex or mesh key, pixel key) within the same BNSH variation; (vertex key, pixel key, 'prog') within the same BAHS program variation
fn var_pairs(cx: &Ctx<'_>, found: &[Shader], keys: &[String]) -> Vec<Pair> {
    let mut var: HashMap<String, HashMap<St, String>> = HashMap::new();
    let bv = cx.bahs_var.borrow();
    for (s, key) in found.iter().zip(keys) {
        let g = if s.label.contains("#var") {
            Some(s.label.clone())
        } else {
            bv.get(&s.label).cloned()
        };
        if let Some(g) = g {
            var.entry(g).or_default().insert(s.st, key.clone());
        }
    }
    let mut out = BTreeSet::new();
    for (g, v) in &var {
        for st in [St::Vs, St::Ms] {
            if let (Some(a), Some(b)) = (v.get(&st), v.get(&St::Ps)) {
                out.insert(Pair {
                    a: a.clone(),
                    b: b.clone(),
                    prog: g.contains("#BAHS#prog"),
                });
            }
        }
    }
    out.into_iter().collect()
}

/// `var_pairs(found)` on its own (the keys are computed here)
pub fn var_pairs_of(cx: &Ctx<'_>, found: &[Shader]) -> Res<Vec<Pair>> {
    let mut keys = Vec::with_capacity(found.len());
    for s in found {
        keys.push(cx.skey(&s.ctl, &s.code)?);
    }
    Ok(var_pairs(cx, found, &keys))
}

/// the three `return rel, uniq_keys(found), var_pairs(found), trees` of the readers, computing each key once
pub fn finish(
    cx: &Ctx<'_>,
    rel: &str,
    found: Vec<Shader>,
    extra_pairs: impl IntoIterator<Item = (String, String)>,
    trees: Trees,
) -> Res<FileResult> {
    let mut keys = Vec::with_capacity(found.len());
    for s in &found {
        keys.push(cx.skey(&s.ctl, &s.code)?);
    }
    let mut pairs = var_pairs(cx, &found, &keys);
    for (a, b) in extra_pairs {
        pairs.push(Pair { a, b, prog: true });
    }
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut uniq: Vec<(String, Shader)> = Vec::new();
    for (s, key) in found.into_iter().zip(keys) {
        if let Some(&i) = index.get(&key) {
            if !cx.same_shader(&uniq[i].1, &s)? {
                return Err(Failure::Red(format!(
                    "key {key} is the same but the control section or code area differs ({})",
                    s.label
                )));
            }
            continue;
        }
        index.insert(key.clone(), uniq.len());
        uniq.push((key, s));
    }
    Ok(FileResult {
        rel: rel.to_string(),
        uniq,
        pairs,
        extra: Extra::Trees(trees),
    })
}

// ---- scan_file ------------------------------------------------------------------------------------------------------------------

fn splitext(p: &str) -> (&str, &str) {
    // os.path.splitext: the extension of the last component, ignoring its leading dots
    let base_start = p.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &p[base_start..];
    let lead = base.len() - base.trim_start_matches('.').len();
    match base[lead..].rfind('.') {
        Some(i) => p.split_at(base_start + lead + i),
        None => (p, ""),
    }
}

fn ioerr(e: std::io::Error) -> Failure {
    Failure::Red(format!("OSError: {e}"))
}

/// below this size a file is read whole at once; above it, a file no container magic opens is only searched in chunks
const STREAM_MIN: u64 = 16 << 20;

/// `scan_file_in(args)` for a plain file job
fn scan_file_in(cx: &Ctx<'_>, f: &FileRef) -> Res<FileResult> {
    let rel = &f.rel;
    let (stem, ext) = splitext(rel);
    {
        // an NXSD container, by its magic before anything else (TETRIS THE GRAND MASTER 4 keeps them beside a .dict)
        let src = f.loc.open().map_err(ioerr)?;
        if src.read(0, 4).map_err(ioerr)? == crate::nxsd::MAGIC {
            let d = src.read_all().map_err(ioerr)?;
            let found = crate::nxsd::nxsd_shaders(&d, &format!("/{rel}"))?;
            return finish(cx, rel, found, [], Trees::new());
        }
    }
    if matches!(ext, ".data" | ".patch" | ".debug" | ".nxpc")
        && cx.files.exists(f.root, &format!("{stem}.dict"))
    {
        return Ok(FileResult::empty(rel)); // read through its Next Level Games index (nlg_shaders)
    }
    let src = f.loc.open().map_err(ioerr)?;
    let head = src.read(0, 16).map_err(ioerr)?;
    let mut found: Vec<Shader> = Vec::new();
    let trees = Trees::new();
    let label = format!("/{rel}");
    let h4 = sl(&head, 0, 4);
    // ---- formats read through their own file layout (the branches of scan_file_in before the generic read)
    if h4 == [0x58, 0x24, 0xf3, 0xa9] {
        unsupported!("Next Level Games archive index (.dict beside .data: {rel})");
    }
    if rel.ends_with(".pkdswitch") {
        let idx_rel = format!("{}.pkiswitch", &rel[..rel.len() - ".pkdswitch".len()]);
        if let Some(il) = cx.files.get(f.root, &idx_rel) {
            // Double Eleven's resource pack, read through its index (d11_pack_shaders)
            let idx = il.open().and_then(|s| s.read_all()).map_err(ioerr)?;
            let d = src.read_all().map_err(ioerr)?;
            d11_pack_shaders(cx, &idx, &d, &label, &mut found)?;
            return finish(cx, rel, found, [], trees);
        }
    }
    if h4 == [0x50, 0xec, 0x12, 0xba] {
        // a UbiArt IPK archive: its .fxb effect files, without reading the rest
        ipk_shaders(cx, &src, rel, &mut found)?;
        return finish(cx, rel, found, [], trees);
    }
    if h4 == crate::apf::MAGIC {
        // a Vector Unit APF package: every entry decoded and scanned, without reading the package whole
        crate::apf::apf_shaders(cx, &src, rel, &mut found)?;
        return finish(cx, rel, found, [], trees);
    }
    if sl(&head, 0, 8) == IDRK || sl(&head, 0, 8) == PDRK {
        idrk_shaders(&src, &label, &mut found)?;
        return finish(cx, rel, found, [], trees);
    }
    if starts(&head, b"bnd2") {
        let d = src.read_all().map_err(ioerr)?;
        found.extend(bnd2_shaders(&d, &label)?);
        return finish(cx, rel, found, [], trees);
    }
    if starts(&head, b"PAK")
        && let Some(members) = heaps_pak_files(&src)?
    {
        for (nm, off, n) in members {
            if nm.ends_with(&format!("/{HEAPS_CACHE}")) {
                let b = src.read(off, n as usize).map_err(ioerr)?;
                found.extend(heaps_shaders(&b, &format!("/{rel}{nm}"))?);
            }
        }
        return finish(cx, rel, found, [], trees);
    }
    if h4 == b"SPKG" || (h4 == [0, 0, 1, 0] && rel.ends_with(".gz")) {
        let d = src.read_all().map_err(ioerr)?;
        kt_shaders(cx, &d, &label, &mut found)?;
        return finish(cx, rel, found, [], trees);
    }
    if h4 == b"BMSG"
        || (h4 == UE3_TAG && head.len() >= 6 && {
            let v = u16::from_le_bytes([head[4], head[5]]);
            0 < v && v < 0x8000
        })
    {
        // an Unreal Engine 3 global shader cache or package (ue3_body, ue3_shaders)
        let d = src.read_all().map_err(ioerr)?;
        let body: std::borrow::Cow<'_, [u8]> = if starts(&d, &UE3_TAG) {
            std::borrow::Cow::Owned(ue3_body(&d, &label)?)
        } else {
            std::borrow::Cow::Borrowed(&d)
        };
        found.extend(ue3_shaders(&body, &label)?);
        return finish(cx, rel, found, [], trees);
    }
    if h4 == RFRM {
        let d = src.read_all().map_err(ioerr)?;
        found.extend(rfrm_shaders(cx, &d, &label)?);
        return finish(cx, rel, found, [], trees);
    }
    if h4 == b"BERG" {
        // Snowdrop's SDF data slices: shader objects in individual compressed records
        let d = src.read_all().map_err(ioerr)?;
        found.extend(snowdrop_shaders(cx, &d, &label, true)?);
        return finish(cx, rel, found, [], trees);
    }
    if h4 == MTARC {
        let d = src.read_all().map_err(ioerr)?;
        mtarc_shaders(&d, &label, &mut found)?;
        return finish(cx, rel, found, [], trees);
    }
    if h4 == b"SCNE" {
        let d = src.read_all().map_err(ioerr)?;
        for (nm, m) in bea_members(cx, &d, &label)? {
            scan_blob(cx, &m, &format!("/{rel}/{nm}"), &mut found, 0, None)?;
        }
        return finish(cx, rel, found, [], trees);
    }
    if sl(&head, 0, 8) == GFPAK {
        let d = src.read_all().map_err(ioerr)?;
        for (nm, m) in gfpak_members(&d, &label)? {
            scan_blob(cx, &m, &format!("/{rel}{nm}"), &mut found, 0, None)?;
        }
        return finish(cx, rel, found, [], trees);
    }
    if h4 == b"SRYK" {
        unsupported!("Phyre VBF archive (vbf.py: {rel})");
    }
    if h4 == crate::falcom::PKA_MAGIC {
        // a Falcom .pka no job took (falcom_pka_jobs plans every one): its blobs, read one at a time
        let es = crate::falcom::falcom_pka_index(&src, &label)?;
        crate::falcom::falcom_pka_shaders(cx, &src, rel, &es, &mut found)?;
        return finish(cx, rel, found, [], trees);
    }
    if head.len() >= 8 && h4 == [0, 0, 0, 0] && u32le(&head, 4)? != 0 {
        // a Falcom .pkg (no magic): its table, then its entries, without reading the package whole
        let n = u32le(&head, 4)? as u64;
        if 8 + 0x50 * n <= src.len()
            && u32le(&src.read(8 + 0x48, 4).map_err(ioerr)?, 0)? as u64 == 8 + 0x50 * n
            && let Some(es) = crate::falcom::falcom_pkg(&src)?
        {
            crate::falcom::falcom_pkg_shaders(cx, &src, &es, &label, &mut found)?;
            return finish(cx, rel, found, [], trees);
        }
    }
    // ---- the generic path: the bytes, decoded when they are zstd
    let big = src.len() > STREAM_MIN;
    let zstd_head = h4 == ZSTD_MAGIC;
    // a big file nothing at its start opens is searched in chunks: it is read whole only when a container magic is in it. A Unity serialized file of any size is read the same way
    // (through its tables and the objects of the classes asked for): the same readers run on the same bytes, without holding the whole file for each of the threads
    if !zstd_head && plain_start(&head, rel) && (big || is_plain_serialized(&src, &head)) {
        return scan_plain_big(cx, f, &src, &head, trees);
    }
    let owned: Vec<u8>;
    {
        let raw = src.read_all().map_err(ioerr)?;
        owned = if zstd_head {
            unzs(cx, &raw, &label)?
        } else {
            raw
        };
    }
    scan_after_read(cx, f, owned, &head, trees)
}

/// Is a file's start none of the magics the generic path opens (zstd, gzip, SHN!, CIVBSF, HCPS, xbc1, DRSM, cmp-zlib, RYHP / GFX at +8, a size-prefixed frame, Yaz0, XNB, UnityFS)?
fn plain_start(head: &[u8], rel: &str) -> bool {
    let h = |a: usize, b: usize| sl(head, a, b);
    !(h(0, 4) == b"SHN!"
        || h(0, 8) == CIVBSF
        || h(0, 3) == [0x1f, 0x8b, 0x08]
        || (h(4, 5) == [0x78] && rel.ends_with(".gz"))
        || h(0, 4) == b"HCPS"
        || h(0, 4) == b"xbc1"
        || h(0, 4) == b"DRSM"
        || h(0, 8) == b"cmp\0zlib"
        || h(0, 4) == b"RYHP"
        || h(8, 12) == GFX_MAGIC
        || h(4, 8) == ZSTD_MAGIC
        || h(4, 8) == LZ4_FRAME_MAGIC
        || h(0, 4) == b"Yaz0"
        || h(0, 3) == b"XNB"
        || h(0, 8) == b"UnityFS\0")
}

/// Is this a Unity serialized file (the test `scan_plain_big` and `scan_after_read` both make: three zero bytes at +8 and a plausible header)?
fn is_plain_serialized(src: &dyn Source, head: &[u8]) -> bool {
    sl(head, 8, 11) == [0, 0, 0]
        && src
            .read(0, 48)
            .map(|h| unity::is_serialized(&h, src.len()))
            .unwrap_or(false)
}

/// The tail of `scan_file_in` for a big file whose start is plain: a Unity serialized file is read through its tables, anything else is searched for the container magics first
fn scan_plain_big(
    cx: &Ctx<'_>,
    f: &FileRef,
    src: &dyn Source,
    head: &[u8],
    mut trees: Trees,
) -> Res<FileResult> {
    let rel = &f.rel;
    let label = format!("/{rel}");
    let mut found = Vec::new();
    let h48 = src.read(0, 48).map_err(ioerr)?;
    if sl(head, 8, 11) == [0, 0, 0] && unity::is_serialized(&h48, src.len()) {
        let r = {
            let mut t = if cx.borrowed.is_empty() {
                std::mem::take(&mut trees)
            } else {
                cx.borrowed.clone()
            };
            let r = unity_shaders(cx, src, false, &label, &mut found, &mut t);
            if cx.borrowed.is_empty() {
                trees = t;
            }
            r
        };
        return match r {
            Ok(pairs) => finish(cx, rel, found, pairs, trees),
            Err(Failure::NoTree(_)) if cx.borrowed.is_empty() => Ok(FileResult {
                rel: rel.clone(),
                uniq: vec![],
                pairs: vec![],
                extra: Extra::Defer,
            }),
            Err(e) => Err(e),
        };
    }
    // BNSH, BAHS, a GLSLC output, MGFX, Needle, Ninty_NX, NVNsh, and 'Yaz0' for a SARC
    let sumo = sumo_nx();
    let ns: [&[u8]; 8] = [
        b"BNSH\0\0\0\0",
        b"BAHS",
        &GLSLC_MAGIC,
        MGFX,
        NEEDLE,
        NVNSH,
        &sumo,
        b"Yaz0",
    ];
    let seen = contains_each(src, &ns).map_err(ioerr)?;
    let pre = |a: usize| sl(head, 0, a);
    let by_head = pre(4) == b"MSHD"
        || pre(4) == b"NDSR"
        || pre(4) == b"NARC"
        || pre(4) == b"NDVE"
        || pre(8) == PACX4
        || pre(8) == SUMO_MUX;
    let sarc_yaz = pre(4) == b"SARC" && seen[7];
    if !(seen[..7].iter().any(|&b| b) || by_head || sarc_yaz) {
        return Ok(FileResult::empty(rel));
    }
    let d = src.read_all().map_err(ioerr)?;
    scan_loaded(cx, rel, &d, &mut found)?;
    finish(cx, rel, found, std::iter::empty(), trees)
}

/// the last branch of `scan_file_in`: `scan_blob`, then whole GLSLC outputs in a file no other reader takes
fn scan_loaded(cx: &Ctx<'_>, rel: &str, d: &[u8], found: &mut Vec<Shader>) -> Res<()> {
    let label = format!("/{rel}");
    scan_blob(cx, d, &label, found, 0, None)?;
    // whole GLSLC outputs in a file no other reader takes (FINAL FANTASY TACTICS: its engine's technique files). Metroid Dread's MSHD files hold GLSLC outputs too, which mshd_shaders already reads
    if found.is_empty() && !starts(d, b"MSHD") && contains(d, &GLSLC_MAGIC) {
        found.extend(glslc_shaders(d, &label, false, &[])?);
    }
    Ok(())
}

/// everything `scan_file_in` does once the file's bytes (`d`: decoded when the file is zstd) are in memory
fn scan_after_read(
    cx: &Ctx<'_>,
    f: &FileRef,
    mut d: Vec<u8>,
    head: &[u8],
    mut trees: Trees,
) -> Res<FileResult> {
    let rel = &f.rel;
    let label = format!("/{rel}");
    let mut found: Vec<Shader> = Vec::new();
    let mut head: Vec<u8> = head.to_vec();
    if starts(&d, b"SHN!") || starts(&d, CIVBSF) {
        scan_blob(cx, &d, &label, &mut found, 0, None)?;
        return finish(cx, rel, found, [], trees);
    }
    if starts(&d, &[0x1f, 0x8b, 0x08]) {
        // standard gzip: Skyward Sword HD's Shaders.arc.gz wraps paired GLSLC outputs
        d = ungzip(&d, &label)?;
        head = sl(&d, 0, 16).to_vec(); // later format probes inspect decoded bytes, not the gzip timestamp
    }
    if eq_at(&head, 4, b"\x78")
        && rel.ends_with(".gz")
        && let Some(z) = gust_gz(&d)
    {
        d = z;
    }
    // containers the generic prefilter below would drop, read whole by scan_blob: Monolith's, and Phyre's effects and loose programs (their two lengths before the code)
    if starts(&d, b"HCPS")
        || starts(&d, b"xbc1")
        || starts(&d, b"DRSM")
        || starts(&d, b"RYHP")
        || starts(&d, b"cmp\0zlib")
        || eq_at(&d, 8, &GFX_MAGIC)
    {
        scan_blob(cx, &d, &label, &mut found, 0, None)?;
        return finish(cx, rel, found, [], trees);
    }
    if !starts(&head, &ZSTD_MAGIC)
        && (eq_at(&head, 4, &ZSTD_MAGIC) || eq_at(&head, 4, &LZ4_FRAME_MAGIC))
    {
        // HAL's .cmp containers (Kirby and the Forgotten Land)
        let u = unsized_frame(cx, &d, &label)?;
        if let std::borrow::Cow::Owned(v) = u {
            d = v;
        }
    }
    if starts(&d, b"Yaz0") {
        d = yaz0(&d, &label)?;
    }
    if starts(&d, b"XNB") && d.len() > 14 && u8at(&d, 4)? == 5 && u8at(&d, 5)? & 0xC0 != 0 {
        d = xnb_decompress(&d, &label)?;
    }
    let is_ufs = starts(&d, b"UnityFS\0");
    if is_ufs || (eq_at(&d, 8, b"\0\0\0") && unity::is_serialized(sl(&d, 0, 48), d.len() as u64)) {
        let m = MemSource(&d);
        let mut t = if cx.borrowed.is_empty() {
            std::mem::take(&mut trees)
        } else {
            cx.borrowed.clone()
        };
        let r = unity_shaders(cx, &m, is_ufs, &label, &mut found, &mut t);
        if cx.borrowed.is_empty() {
            trees = t;
        }
        return match r {
            Ok(pairs) => finish(cx, rel, found, pairs, trees),
            Err(Failure::NoTree(_)) if cx.borrowed.is_empty() => Ok(FileResult {
                rel: rel.clone(),
                uniq: vec![],
                pairs: vec![],
                extra: Extra::Defer,
            }),
            Err(e) => Err(e),
        };
    }
    let sumo = sumo_nx();
    let pre = |a: usize| sl(&d, 0, a);
    let nothing = !contains(&d, b"BNSH\0\0\0\0")
        && !contains(&d, b"BAHS")
        && pre(4) != b"MSHD"
        && pre(4) != b"NDSR"
        && pre(4) != b"NARC"
        && pre(4) != b"NDVE"
        && !(pre(4) == b"SARC" && contains(&d, b"Yaz0")) // a SARC may hold Yaz0 members, a NARC packed ones (cra.undo)
        && !contains(&d, &GLSLC_MAGIC)
        && !contains(&d, MGFX)
        && !contains(&d, NEEDLE)
        && pre(8) != PACX4
        && pre(8) != SUMO_MUX
        && !contains(&d, &sumo)
        && !contains(&d, NVNSH);
    if nothing {
        return Ok(FileResult::empty(rel));
    }
    scan_loaded(cx, rel, &d, &mut found)?;
    finish(cx, rel, found, std::iter::empty(), trees)
}

/// `scan_file(args)`: a file no reader can read is left out, and the split goes on (its red line, or the exception it raised); an unsupported container ends the split
pub fn scan_file(cx: &Ctx<'_>, f: &FileRef) -> Result<FileResult, String> {
    match scan_file_in(cx, f) {
        Ok(r) => Ok(r),
        Err(Failure::Unsupported(m)) => Err(m),
        Err(e) => {
            let msg = e.to_string();
            let name = f.rel.as_str();
            let why = if msg
                .trim_start_matches('/')
                .starts_with(name.trim_start_matches('/'))
            {
                msg
            } else {
                format!("{name}: {msg}")
            };
            cx.print(&format!("[RED] {why} (its shaders are left out)"));
            Ok(FileResult {
                rel: f.rel.clone(),
                uniq: vec![],
                pairs: vec![],
                extra: Extra::Unreadable { why, count: 1 },
            })
        }
    }
}

/// the scan of a block of bytes in memory the way a `main` is scanned (`scan_blob`, then `glslc_shaders`, ...): used by the main part of the split
pub fn scan_main_image(cx: &Ctx<'_>, img: &[u8], label: &str) -> Res<Vec<Shader>> {
    let mut found = Vec::new();
    scan_blob(cx, img, label, &mut found, 0, None)?;
    found.extend(glslc_shaders(img, label, false, &[])?);
    found.extend(tfxo_streams(img, label)?);
    found.extend(retro_main_forms(cx, img, label)?);
    if contains(img, b"rgxd") {
        found.extend(rgxd_shaders(cx, img, label)?);
    }
    let known: HashSet<Vec<u8>> = found
        .iter()
        .map(|s| -> Res<Vec<u8>> {
            let n = u32le(&s.ctl, 0x20)? as usize;
            Ok(sl(&s.ctl, 0, n).to_vec())
        })
        .collect::<Res<_>>()?;
    found.extend(raw_shaders(cx, img, label, &known)?);
    Ok(found)
}
