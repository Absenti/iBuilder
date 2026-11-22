//! `dmsh_mesh.py`: passthrough geometry as one Metal mesh shader (`<vertex key>_<geometry key>_m3`, the pair-level `mesh_finish` of `dmsh_build.py`).
//!
//! `compose(vs_src, gs_src, reads)` takes the vertex shader's library source (`<key>_b.metal`), the lean passthrough kernel (`<geometry key>_bl3.metal`, `geometry.lean`) and the capture
//! words the kernel reads (`geometry.capture_reads` of its json) and returns the mesh library's MSL. The Python's `ValueError`s are `MeshErr::Value(message)` (the build logs
//! `[RED] mesh <vk> <gk>: <message>` and counts the pair as "not composed"); an exception it does not catch (`TypeError`, `IndexError`) is `MeshErr::Crash` (the Python's build stops).
use super::vin::pull_variant;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

/// triangles a threadgroup takes, fewer when their vertices exceed the mesh output limit (`prims_for`)
pub const PRIMS: i64 = 32;
/// Metal's limit on a mesh threadgroup's output: vertices and primitives
pub const MESH_BYTES: i64 = 32768;
/// argument-table slots the vertex stage leaves free in a pulling pipeline
pub const DRAW_SLOT: i64 = 12;
pub const GS_SLOT: i64 = 13;
/// vertex table slots a run record holds (= nvn_geometry.inc geo_mesh_rec)
pub const RUN_SLOTS: i64 = 31;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshErr {
    /// a `ValueError`
    Value(String),
    /// any other Python exception: the Python build stops
    Crash(String),
}

impl std::fmt::Display for MeshErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshErr::Value(m) | MeshErr::Crash(m) => f.write_str(m),
        }
    }
}

fn value<T>(m: impl Into<String>) -> Result<T, MeshErr> {
    Err(MeshErr::Value(m.into()))
}

re!(RE_OUT_STRUCT, r"(?s)struct main0_out\s*\{(.*?)\};");
re!(
    RE_OUT_MEMBER,
    r"^\s*(\w+?)([234]?) \w+(?: \[(\d+)\])?(?: \[\[.*\]\])?\s*$"
);

fn scalar_bytes(t: &str) -> Option<i64> {
    match t {
        "float" | "int" | "uint" => Some(4),
        "half" | "short" | "ushort" => Some(2),
        _ => None,
    }
}

/// `out_bytes(src)`: the bytes of one vertex the mesh writes: the members of `main0_out`, packed as Metal counts the mesh output
pub fn out_bytes(src: &str) -> Result<i64, MeshErr> {
    let Some(m) = RE_OUT_STRUCT.captures(src) else {
        return value("mesh: no main0_out");
    };
    let mut n = 0i64;
    for ln in m[1].split(';') {
        let a = RE_OUT_MEMBER.captures(ln);
        if super::pycompat::strip(ln).is_empty() {
            continue;
        }
        let Some(a) = a else {
            return value(format!(
                "mesh: vertex output member {}",
                super::pycompat::strip(ln)
            ));
        };
        let Some(sz) = scalar_bytes(&a[1]) else {
            return value(format!(
                "mesh: vertex output member {}",
                super::pycompat::strip(ln)
            ));
        };
        let k = if a[2].is_empty() {
            1
        } else {
            a[2].parse::<i64>().unwrap_or(1)
        };
        let c = a
            .get(3)
            .map(|g| g.as_str().parse::<i64>().unwrap_or(1))
            .unwrap_or(1);
        n += sz * k * c;
    }
    Ok(n)
}

/// `prims_for(vbytes)`: the most triangles (32, 16, ... 1) whose vertices and primitives fit one mesh threadgroup
pub fn prims_for(vbytes: i64) -> Result<i64, MeshErr> {
    for p in [32, 16, 8, 4, 2, 1] {
        if 3 * p * vbytes + 4 * p <= MESH_BYTES {
            return Ok(p);
        }
    }
    value(format!(
        "mesh: one triangle of {vbytes}-byte vertices exceeds the mesh output limit"
    ))
}

fn builtin_v(a: &str) -> Option<&'static str> {
    match a {
        "vertex_id" => Some("dm_vid"),
        "instance_id" => Some("dm_iid"),
        "base_instance" => Some("dm_bi"),
        "base_vertex" => Some("dm_bv"),
        _ => None,
    }
}
fn builtin_g(a: &str) -> Option<&'static str> {
    match a {
        "thread_position_in_grid" => Some("uint3(dm_p0 + dm_t, 0u, 0u)"),
        "thread_index_in_simdgroup" => Some("dm_sg"),
        _ => None,
    }
}

re!(RE_PARAM, r"^(.+?) (\w+) \[\[(\w+)(?:\((\d+)\))?\]\]$");
re!(RE_THREADGROUP, r"\bthreadgroup\b|\bthreadgroup_barrier\b");
re!(
    RE_VOUT_READ,
    r"\(\*spvDescriptorSet0\.vertex_output\)\._m0\[(\w+)\]"
);
re!(
    RE_MASK_STORE,
    r"\(\*spvDescriptorSet0\.geometry_vb_output\)\._m0\[\w+\] = (\w+);"
);
re!(
    RE_IB_STORE,
    r"\n\s*\(\*spvDescriptorSet0\.geometry_ib_output\)\._m0\[\w+\] = [^;]+;"
);
re!(
    RE_LEFT,
    r"vertex_output|geometry_vb_output|geometry_ib_output|trb_data\["
);
re!(RE_ATTRS_TAIL, r" \[\[.*\]\]$");
re!(
    RE_RUN_RES,
    r"^constant (.+?)& (\w+) \[\[buffer\((\d+)\)\]\]$"
);
re!(RE_INCLUDE, r"^(?:\s*#include\b|\s*using namespace metal;)");

/// `_entry(src, kind, name)`: `\n<kind> (\w+) <name>\(([^\n]*)\)\n\{\n` -> (start, end, the parameter text)
fn entry_of(src: &str, kind: &str, name: &str) -> Result<(usize, usize, String), MeshErr> {
    let re = super::rx::cached(&format!(
        r"\n{} (\w+) {}\(([^\n]*)\)\n\{{\n",
        regex::escape(kind),
        regex::escape(name)
    ))
    .map_err(|e| MeshErr::Crash(e.to_string()))?;
    match re.captures(src) {
        Some(m) => Ok((
            m.get(0).unwrap().start(),
            m.get(0).unwrap().end(),
            m[2].to_string(),
        )),
        None => value(format!("mesh: no {kind} {name}")),
    }
}

/// `_strip_head(src)`: without its `#include` and `using namespace metal;` lines
fn strip_head(src: &str) -> String {
    src.split('\n')
        .filter(|l| !RE_INCLUDE.is_match(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `dmsh_mesh.compose(vs_src, gs_src, reads)`
pub fn compose(vs_src: &str, gs_src: &str, reads: &[i64]) -> Result<String, MeshErr> {
    if reads.contains(&2) || reads.is_empty() {
        return value("mesh: the kernel reads clip z or nothing");
    }
    let (pulled, info) = match pull_variant(vs_src) {
        Ok(x) => x,
        Err(why) => return value(format!("mesh: no pulling entry: {why}")),
    };
    let words = match crate::vs::output_words(&pulled) {
        Ok(w) => w.into_iter().collect::<BTreeMap<usize, String>>(),
        Err(e) => {
            return value(match e {
                crate::PostError::Failed { msg, .. } => msg,
                other => other.to_string(),
            });
        }
    };
    let mut pulled = pulled.replace(" [[point_size]];", ";");
    let p_count = prims_for(out_bytes(&pulled)?)?;
    let mut entry_names = vec![
        info.get("entry")
            .and_then(|e| e.as_str())
            .unwrap_or("main0")
            .to_string(),
    ];
    if let Some(z) = info.get("entry_z").and_then(|e| e.as_str()) {
        entry_names.push(z.to_string());
    }
    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    let mut res: Option<Vec<String>> = None;
    for name in &entry_names {
        let (ms, me, ptext) = entry_of(&pulled, "vertex", name)?;
        let (mut args, mut fparams, mut rparams) = (
            Vec::<String>::new(),
            Vec::<String>::new(),
            Vec::<String>::new(),
        );
        for p in super::meshstage::params_of(&ptext) {
            let Some(a) = RE_PARAM.captures(&p) else {
                return value(format!("mesh: vertex parameter {p}"));
            };
            fparams.push(format!("{} {}", &a[1], &a[2]));
            if let Some(b) = builtin_v(&a[3]) {
                args.push(b.to_string());
            } else if &a[3] == "buffer" {
                let Some(slot) = a.get(4) else {
                    return Err(MeshErr::Crash("TypeError: int() argument must be a string, a bytes-like object or a real number, not 'NoneType'".into()));
                };
                let slot: i64 = slot.as_str().parse().unwrap_or(i64::MAX);
                if slot == DRAW_SLOT || slot == GS_SLOT {
                    return value(format!("mesh: vertex parameter {p}"));
                }
                args.push(a[2].to_string());
                rparams.push(p.clone());
            } else {
                return value(format!("mesh: vertex parameter {p}"));
            }
        }
        if let Some(r) = &res
            && *r != rparams
        {
            return value("mesh: entries differ in resources");
        }
        res = Some(rparams);
        pulled = format!(
            "{}\nstatic inline main0_out dm_vs_{name}({})\n{{\n{}",
            &pulled[..ms],
            fparams.join(", "),
            &pulled[me..]
        );
        entries.push((name.clone(), args));
    }
    let (ks, ke, gtext) = entry_of(gs_src, "kernel", "main0")?;
    let mut gargs: Vec<String> = Vec::new();
    for p in super::meshstage::params_of(&gtext) {
        let a = RE_PARAM.captures(&p);
        if let Some(b) = a.as_ref().and_then(|a| builtin_g(&a[3])) {
            gargs.push(b.to_string());
        } else if a.is_some()
            && p == "constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]"
        {
            gargs.push("dm_gset".into());
        } else {
            return value(format!("mesh: kernel parameter {p}"));
        }
    }
    let fparams: Vec<String> = super::meshstage::params_of(&gtext)
        .iter()
        .map(|p| RE_ATTRS_TAIL.replace(p, "").into_owned())
        .collect();
    let mut body = gs_src[ke..].to_string();
    if RE_THREADGROUP.is_match(&body) {
        return value("mesh: kernel uses threadgroup memory");
    }
    body = RE_VOUT_READ
        .replace_all(&body, "dm_rec[uint(${1}) - dm_capture_base]")
        .into_owned();
    let prefix = "spvDescriptorSet0.trb_data[";
    while let Some(s) = body.find(prefix) {
        let at = s + prefix.len();
        let (mut e, mut depth) = (at, 1i32);
        let b = body.as_bytes();
        while depth != 0 {
            let Some(&c) = b.get(e) else {
                return Err(MeshErr::Crash(
                    "IndexError: string index out of range".into(),
                ));
            };
            depth += (c == b'[') as i32 - (c == b']') as i32;
            e += 1;
        }
        body = format!("{}uint({}){}", &body[..s], &body[at..e - 1], &body[e..]);
    }
    let n_mask = RE_MASK_STORE.find_iter(&body).count();
    body = RE_MASK_STORE
        .replace_all(&body, "dm_mask = as_type<uint>(${1});")
        .into_owned();
    if n_mask == 0 {
        return value("mesh: no mask store");
    }
    body = RE_IB_STORE.replace_all(&body, "").into_owned();
    if RE_LEFT.is_match(&body) {
        return value("mesh: kernel output not understood");
    }
    let mut gparams = fparams;
    gparams.extend([
        "threadgroup float* dm_rec".to_string(),
        "uint dm_capture_base".to_string(),
        "thread uint& dm_mask".to_string(),
    ]);
    let gfn = format!(
        "{}\nstatic inline void dm_gs({})\n{{\n{}",
        &gs_src[..ks],
        gparams.join(", "),
        body
    );
    let word = |w: i64| -> String {
        if let Some(e) = usize::try_from(w).ok().and_then(|w| words.get(&w)) {
            return format!("dm_o.{}", &e[4..]);
        }
        if w == 3 || w == 4 || (w >= 19 && w % 4 == 3) {
            "0x3f800000u".into()
        } else {
            "0u".into()
        }
    };
    let n = reads.len() as i64;
    let stores = reads
        .iter()
        .enumerate()
        .map(|(i, w)| format!("dm_rec[dm_t * {n}u + {i}u] = as_type<float>({});", word(*w)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out: Vec<String> = vec![
        "#include <metal_stdlib>".into(), "#include <simd/simd.h>".into(), "#include <metal_atomic>".into(), "using namespace metal;".into(),
        "namespace dm_v {".into(), strip_head(&pulled), "}".into(), "namespace dm_g {".into(), strip_head(&gfn), "}".into(), "using namespace dm_v;".into(),
        "struct dm_mesh_draw { device const uchar* ib; uint itype; uint first; uint count; int base_vertex; uint base_instance; };".into(),
        "struct dm_mesh_prim { bool culled [[primitive_culled]]; };".into(),
        format!("struct dm_mesh_rec {{ constant uchar* slot[{RUN_SLOTS}]; dm_mesh_draw d; uint group_end; uint n; }};"),
    ];
    let Some(res) = res else {
        return value("mesh: no vertex entries");
    };
    let run_res: Vec<(String, String, i64)> = res
        .iter()
        .filter_map(|p| RE_RUN_RES.captures(p))
        .filter_map(|m| {
            let s: i64 = m[3].parse().unwrap_or(i64::MAX);
            if s < RUN_SLOTS {
                Some((m[1].to_string(), m[2].to_string(), s))
            } else {
                None
            }
        })
        .collect();
    let run_ok = run_res.len() == res.len();
    let head = format!(
        "metal::mesh<main0_out, dm_mesh_prim, {}, {p_count}, metal::topology::triangle> dm_m",
        3 * p_count
    );
    for (name, args) in &entries {
        let sfx = format!(
            "{}{}",
            &name[5.min(name.len())..],
            if p_count == PRIMS {
                String::new()
            } else {
                format!("_p{p_count}")
            }
        );
        let body = format!(
            "\n    threadgroup float dm_rec[{}];\n    uint dm_p0 = dm_tg * {p_count}u, dm_np = min({p_count}u, dm_d.count / 3u - dm_p0);\n    if (dm_t < dm_np * 3u) {{\n        uint dm_i = dm_d.first + dm_p0 * 3u + dm_t;\n        uint dm_ix = dm_d.itype == 0u ? dm_i : dm_d.itype == 1u ? uint(dm_d.ib[dm_i]) : dm_d.itype == 2u ? uint(((device const ushort*)dm_d.ib)[dm_i]) : ((device const uint*)dm_d.ib)[dm_i];\n        uint dm_vid = dm_ix + uint(dm_d.base_vertex), dm_bv = uint(dm_d.base_vertex), dm_bi = dm_d.base_instance, dm_iid = dm_bi;\n        main0_out dm_o = dm_vs_{name}({});\n        dm_m.set_vertex(dm_t, dm_o);\n        {stores}\n    }}\n    if (dm_t == 0u) dm_m.set_primitive_count(dm_np);\n    threadgroup_barrier(mem_flags::mem_threadgroup);\n    if (dm_t < dm_np) {{\n        uint dm_mask = 0u;\n        dm_g::dm_gs({}, dm_rec, dm_p0 * {}u, dm_mask);\n        dm_m.set_index(dm_t * 3u, dm_t * 3u); dm_m.set_index(dm_t * 3u + 1u, dm_t * 3u + 1u); dm_m.set_index(dm_t * 3u + 2u, dm_t * 3u + 2u);\n        dm_m.set_primitive(dm_t, dm_mesh_prim{{(dm_mask & 1u) == 0u}});\n    }}\n}}",
            3 * p_count * n,
            args.join(", "),
            gargs.join(", "),
            3 * n
        );
        out.push(format!("[[mesh]] void dm_mesh{sfx}({head}, {},\n    constant dm_mesh_draw& dm_d [[buffer({DRAW_SLOT})]], constant dm_g::spvDescriptorSetBuffer0& dm_gset [[buffer({GS_SLOT})]],\n    uint dm_t [[thread_index_in_threadgroup]], uint dm_tg [[threadgroup_position_in_grid]], uint dm_sg [[thread_index_in_simdgroup]])\n{{{body}", res.join(", ")));
        if !run_ok {
            continue;
        }
        let takes: String = run_res
            .iter()
            .map(|(t, nm, s)| format!("\n    constant {t}& {nm} = *(constant {t}*)dm_r.slot[{s}];"))
            .collect();
        out.push(format!("[[mesh]] void dm_mesh_run{sfx}({head}, constant dm_mesh_rec* dm_run [[buffer({DRAW_SLOT})]],\n    uint dm_t [[thread_index_in_threadgroup]], uint dm_tgr [[threadgroup_position_in_grid]], uint dm_sg [[thread_index_in_simdgroup]])\n{{\n    uint dm_lo = 0u, dm_hi = dm_run[0].n - 1u;   // the draw this threadgroup belongs to: the first whose group_end is past it\n    while (dm_lo < dm_hi) {{ uint dm_mid = (dm_lo + dm_hi) / 2u; if (dm_run[dm_mid].group_end <= dm_tgr) dm_lo = dm_mid + 1u; else dm_hi = dm_mid; }}\n    constant dm_mesh_rec& dm_r = dm_run[dm_lo];\n    uint dm_tg = dm_tgr - (dm_lo ? dm_run[dm_lo - 1u].group_end : 0u);\n    constant dm_mesh_draw& dm_d = dm_r.d;\n    constant dm_g::spvDescriptorSetBuffer0& dm_gset = *(constant dm_g::spvDescriptorSetBuffer0*)dm_r.slot[{GS_SLOT}];{takes}{body}"));
    }
    Ok(out.join("\n") + "\n")
}
