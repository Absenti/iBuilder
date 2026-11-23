//! A registry of the chain's steps under their Python names, for differential testing: `step_diff` (src/bin) runs a case list through it and the Python driver
//! (`tools/difftest/stepdiff.py`) runs the same list through the engine's functions; the two result files must be identical (docs/shaders/SHADER-RUST.md, section 3).
//!
//! `run(step, msl, args)` -> the text the step returns (Python: its first return value) and a JSON `meta` with everything else it returns (counts, the secondary strings).
use serde_json::{Value, json};

pub struct Outcome {
    pub ok: bool,
    pub text: String,
    pub meta: Value,
    pub msg: String,
}

fn ok(text: impl Into<String>, meta: Value) -> Option<Outcome> {
    Some(Outcome {
        ok: true,
        text: text.into(),
        meta,
        msg: String::new(),
    })
}
fn err(e: crate::PostError) -> Option<Outcome> {
    let msg = match e {
        crate::PostError::Failed { msg, .. } => msg,
        crate::PostError::Unported(s) => format!("unported: {s}"),
    };
    Some(Outcome {
        ok: false,
        text: String::new(),
        meta: Value::Null,
        msg,
    })
}

fn local(args: &Value) -> Option<Vec<i64>> {
    args.get("local")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
}

/// every step name `run` knows
// the steps are kept grouped by round as written (rustfmt would put one per line)
#[rustfmt::skip]
pub const STEPS: &[&str] = &[
    "check_msl", "precise", "bit0_mask", "select_guards", "tg_fence", "border_emu", "uniform_tests", "blend_zero", "max_threads",
    "vs_init", "vs_layer", "vs_depth", "vs_discard", "vs_layer_entry", "capture", "fs_uint", "fs_dual", "shared_body",
    "vs_base_instance", "drop_point_size", "first_store", "tg_spill", "vs_tf", "tf_layout",
    "chain", "hmma_msl", "frag_z_snap", "pool_arrays", "wide_loads", "mul_extended", "float_add_msl", "gather_offsets", "shared_store_msl", "per_vertex", "wide_atomics",
    // the steps of upstream 033793f0's chain that the 2026-11-16 engine did not have, and `rewrites` (its finish() order over the steps of this crate)
    "constant_reads", "compute_loops", "raw_noinline", "mesh_subgroup_noinline", "rewrites",
    // round 3 (tessellation, task / mesh shaders, passthrough geometry): `tools/tessmesh/`
    "tess_hull", "tess_domain", "tess_mode", "mesh_info", "mesh_to_stage", "mesh_pixel_reads", "mesh_compose", "geo_lean", "mesh_out_bytes", "mesh_prims_for",
];

/// `crc32` as zlib computes it (the analysis text's checksum in the chain's meta)
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            t[i as usize] = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// `{"read_only": [s numbers]}` (constant_reads' read-only storage buffers)
fn read_only(args: &Value) -> std::collections::HashSet<u32> {
    args.get("read_only")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u32))
                .collect()
        })
        .unwrap_or_default()
}

/// `{"mode": ...}`: dmsh_chain.storage_constant_mode() (DMSH_SCONST), default `fp`
fn mode(args: &Value) -> &str {
    args.get("mode").and_then(|v| v.as_str()).unwrap_or("fp")
}

/// The MSL rewrites of upstream 033793f0's `finish()` in its order, from check_msl to the stage tails this crate owns (dmsh_build.py l.880-937): the order post.rs takes.
/// args `{stage, local, read_only, mode, mesh_kind}`: `mesh_kind` (`ts` / `ms`) for a task / mesh shader, whose `dmsh_meshstage.to_stage` (between the chain and raw_noinline)
/// is not part of this step; meta `{blend0, first, spill, loop_sites}`.
fn rewrites(msl: &str, args: &Value) -> Option<Outcome> {
    use crate::*;
    use std::borrow::Cow;
    let stage = args["stage"].as_str().unwrap_or("?");
    let mesh_kind = args.get("mesh_kind").and_then(|v| v.as_str());
    let ps = stage == "ps";
    let lc = local(args);
    let run = || -> Result<(String, Value), PostError> {
        let mut cur: Cow<str> = Cow::Borrowed(msl);
        macro_rules! step {
            ($e:expr) => {{
                if let Cow::Owned(o) = $e {
                    cur = Cow::Owned(o);
                }
            }};
        }
        step!(steps::check_msl(&cur)?);
        if ps {
            step!(pervertex::per_vertex(&cur)?);
        }
        step!(steps::precise(&cur));
        step!(bit0::bit0_mask(&cur));
        step!(guards::select_guards(&cur)?);
        step!(ampere::mul_extended(&cur));
        step!(ampere::float_add_msl(&cur));
        step!(ampere::gather_offsets(&cur)?);
        step!(hmma::hmma_msl(&cur)?);
        step!(ampere::shared_store_msl(&cur)?);
        step!(pool::pool_arrays(&cur)?);
        step!(wide::wide_loads(&cur));
        step!(wide::constant_reads(&cur, &read_only(args), mode(args)));
        step!(steps::tg_fence(&cur)?);
        step!(border::border_emu(&cur)?);
        if ps {
            step!(zsnap::frag_z_snap(&cur).0);
        }
        step!(utest::uniform_tests(&cur)?);
        let mut blend0 = 0;
        if ps {
            let (n, k) = blend::blend_zero(&cur)?;
            blend0 = k;
            step!(n);
        }
        let mut text = cur.into_owned();
        let (mut first, mut spill, mut sites) = (Vec::new(), 0, Vec::new());
        if mesh_kind.is_none() && stage == "cs" {
            let (s, f) = cs::first_store(&text, lc.as_deref());
            first = f;
            let (s, n) = cs::tg_spill(&s)?;
            spill = n;
            let (s, l) = cs::compute_loops(&s)?;
            sites = l;
            text = steps::max_threads(&s, lc.as_deref()).into_owned();
        }
        if text.contains("dmsh_raw_find") {
            text = steps::raw_noinline(&text, mesh_kind.unwrap_or(stage)).into_owned();
        }
        if mesh_kind == Some("ms") {
            text = steps::mesh_subgroup_noinline(&text).into_owned();
        }
        Ok((
            text,
            json!({"blend0": blend0, "first": first.iter().map(|(n, w, v)| json!([n, w, v])).collect::<Vec<_>>(), "spill": spill,
                   "loop_sites": sites.iter().map(|s| s.to_json()).collect::<Vec<_>>()}),
        ))
    };
    match run() {
        Ok((t, m)) => ok(t, m),
        Err(e) => err(e),
    }
}

/// the whole chain (`post_rewrites`) as one step: args `{stage, local, tf, dual_want, partner: [locations] | null, res: path of the answer json}`
fn chain(msl: &str, args: &Value) -> Option<Outcome> {
    use crate::post::{Job, Stage, post_rewrites};
    let res: Value = args
        .get("res")
        .and_then(|p| p.as_str())
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(json!({"notes": [], "sbufs": []}));
    let partner: Option<std::collections::BTreeSet<u32>> =
        args.get("partner").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().map(|n| n as u32))
                .collect()
        });
    let tf = args.get("tf").filter(|v| !v.is_null());
    let has_tf = tf
        .map(|v| match v {
            Value::Array(a) => !a.is_empty(),
            Value::Null => false,
            _ => true,
        })
        .unwrap_or(false);
    let job = Job {
        stage: Stage::parse(args["stage"].as_str().unwrap_or("?")),
        local: local(args),
        has_tf,
        tf,
        dual_want: args
            .get("dual_want")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        partner_locs: partner.as_ref(),
        res: Some(&res),
        mesh: false,
    };
    match post_rewrites(msl, &job) {
        Ok(o) => {
            let meta = json!({"blend0": o.blend_zero, "fu": o.fs_uint, "fd": o.fs_dual, "shared": o.shared_body, "pad": o.vs_pad, "discard": o.entry_discard,
                "lay": o.layer_from.map(|(l, c)| json!({"loc": l, "comp": c.to_string()})), "first": o.first_store.iter().map(|(n, w, v)| json!([n, w, v])).collect::<Vec<_>>(), "spill": o.tg_spill,
                "analysis_crc": o.analysis.as_ref().map(|a| crc32(a.as_bytes())), "plain_len": o.plain_len,
                "loop_sites": o.loop_sites.iter().map(|s| s.to_json()).collect::<Vec<_>>(), "viewport_mask": o.viewport_mask});
            ok(o.out, meta)
        }
        Err(e) => err(e),
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn msg(e: impl std::fmt::Display) -> Option<Outcome> {
    Some(Outcome {
        ok: false,
        text: String::new(),
        meta: Value::Null,
        msg: e.to_string(),
    })
}

/// the round-3 steps (`finish::{tess, meshstage, mesh, lean}`); `args` are documented in `tools/tessmesh/stepdiff_ext.py`
fn tessmesh(step: &str, msl: &str, args: &Value) -> Option<Outcome> {
    use crate::finish::{lean, mesh, meshstage, tess};
    match step {
        "tess_hull" => {
            let spv = std::fs::read(args["spv"].as_str()?).ok()?;
            match tess::hull(msl, &spv) {
                Ok((t, f)) => ok(
                    t,
                    json!({"facts": serde_json::from_str::<Value>(&f.to_j().dumps_compact()).ok()}),
                ),
                Err(e) => msg(tess::plain(&e)),
            }
        }
        "tess_domain" => {
            let spv = std::fs::read(args["spv"].as_str()?).ok()?;
            let mode = tess::Mode::from_json(&args["mode"]);
            match tess::domain(msl, &spv, mode.as_ref()) {
                Ok((t, f)) => ok(
                    t,
                    json!({"facts": serde_json::from_str::<Value>(&f.to_j().dumps_compact()).ok()}),
                ),
                Err(e) => msg(tess::plain(&e)),
            }
        }
        "tess_mode" => {
            let ctl = unhex(args["ctl"].as_str()?);
            match tess::control_mode(&ctl) {
                Some(m) => ok(
                    "",
                    json!({"mode": m.to_json(), "bits": tess::mode_bits(&m).ok()}),
                ),
                None => ok("", json!({"mode": null})),
            }
        }
        "mesh_info" => {
            let (ctl, area, code) = (
                unhex(args["ctl"].as_str()?),
                unhex(args["area"].as_str()?),
                unhex(args["code"].as_str().unwrap_or("")),
            );
            let i = match meshstage::info(&ctl, &area) {
                Ok(i) => i,
                Err(e) => return msg(e),
            };
            if i.kind.is_empty() {
                return msg("KeyError: kind is neither ts nor ms");
            }
            let (lay, size) = meshstage::memory(&i);
            ok(
                "",
                json!({"info": i.to_json(), "memory": lay, "size": size, "own_shared": meshstage::own_shared(&code)}),
            )
        }
        "mesh_to_stage" => {
            let mut i = match meshstage::MeshInfo::from_json(&args["info"]) {
                Ok(i) => i,
                Err(e) => return msg(e),
            };
            let reads: Option<std::collections::BTreeMap<i64, i64>> =
                args.get("reads").and_then(|r| r.as_object()).map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.parse().ok()?, v.as_i64()?)))
                        .collect()
                });
            match meshstage::to_stage(msl, &mut i, reads.as_ref()) {
                Ok(t) => ok(t, json!({"spill": i.spill})),
                Err(e) => msg(e),
            }
        }
        "mesh_pixel_reads" => {
            let sph = unhex(args["sph"].as_str()?);
            match meshstage::pixel_reads(&sph) {
                Ok(r) => ok(
                    "",
                    json!({"reads": r.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>()}),
                ),
                Err(e) => msg(e),
            }
        }
        "mesh_compose" => {
            let gs = std::fs::read_to_string(args["gs"].as_str()?).ok()?;
            let reads: Vec<i64> = args["reads"]
                .as_array()?
                .iter()
                .filter_map(|x| x.as_i64())
                .collect();
            match mesh::compose(msl, &gs, &reads) {
                Ok(t) => ok(t, json!({})),
                Err(e) => msg(e),
            }
        }
        "geo_lean" => match lean::lean(msl, args["stride"].as_i64()?) {
            Ok((t, reads)) => ok(t, json!({"reads": reads})),
            Err(e) => msg(e),
        },
        "mesh_out_bytes" => match mesh::out_bytes(msl) {
            Ok(n) => ok("", json!({"n": n})),
            Err(e) => msg(e),
        },
        "mesh_prims_for" => match mesh::prims_for(args["n"].as_i64()?) {
            Ok(n) => ok("", json!({"p": n})),
            Err(e) => msg(e),
        },
        _ => None,
    }
}

pub fn run(step: &str, msl: &str, args: &Value) -> Option<Outcome> {
    use crate::*;
    match step {
        s if (s.starts_with("tess_") || s.starts_with("mesh_") || s == "geo_lean")
            && s != "mesh_subgroup_noinline" =>
        {
            tessmesh(s, msl, args)
        }
        "chain" => chain(msl, args),
        "check_msl" => match steps::check_msl(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "precise" => ok(steps::precise(msl).into_owned(), json!({})),
        "bit0_mask" => ok(bit0::bit0_mask(msl).into_owned(), json!({})),
        "select_guards" => match guards::select_guards(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "tg_fence" => match steps::tg_fence(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "border_emu" => match border::border_emu(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "uniform_tests" => match utest::uniform_tests(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "blend_zero" => match blend::blend_zero(msl) {
            Ok((t, k)) => ok(t.into_owned(), json!({"k": k})),
            Err(e) => err(e),
        },
        "max_threads" => ok(
            steps::max_threads(msl, local(args).as_deref()).into_owned(),
            json!({}),
        ),
        "vs_init" => ok(vs::vs_init(msl), json!({})),
        "vs_layer" => match vs::vs_layer(msl.to_string()) {
            Ok(t) => ok(t, json!({})),
            Err(e) => err(e),
        },
        "vs_depth" => match vs::vs_depth(msl) {
            Ok((s, f)) => ok(s, json!({"full": f})),
            Err(e) => err(e),
        },
        "vs_discard" => match vs::vs_discard(msl) {
            Ok(t) => ok(t, json!({})),
            Err(e) => err(e),
        },
        "vs_layer_entry" => match vs::vs_layer_entry(msl) {
            Ok((t, lay)) => ok(
                t,
                json!({"lay": lay.map(|(l, c)| json!({"loc": l, "comp": c.to_string()}))}),
            ),
            Err(e) => err(e),
        },
        "capture" => match vs::capture(msl) {
            Ok(t) => ok(t, json!({})),
            Err(e) => err(e),
        },
        "fs_uint" => {
            let r = fsuint::fs_uint(msl);
            ok(r.clone().unwrap_or_default(), json!({"none": r.is_none()}))
        }
        "fs_dual" => {
            let r = fsuint::fs_dual(msl);
            ok(r.clone().unwrap_or_default(), json!({"none": r.is_none()}))
        }
        "shared_body" => {
            let r = side::shared_body(msl);
            ok(r.clone().unwrap_or_default(), json!({"none": r.is_none()}))
        }
        "vs_base_instance" => ok(vs::vs_base_instance(msl), json!({})),
        "drop_point_size" => match vs::drop_point_size(msl) {
            Ok(t) => ok(t, json!({})),
            Err(e) => err(e),
        },
        "first_store" => {
            let (t, st) = cs::first_store(msl, local(args).as_deref());
            ok(
                t,
                json!({"stores": st.iter().map(|(n, w, v)| json!([n, w, v])).collect::<Vec<_>>()}),
            )
        }
        "tg_spill" => match cs::tg_spill(msl) {
            Ok((t, total)) => ok(t, json!({"total": total})),
            Err(e) => err(e),
        },
        "tf_layout" => {
            let hex = msl.trim();
            let ctl: Vec<u8> = (0..hex.len() / 2)
                .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
                .collect();
            match tf::tf_layout("k", &ctl) {
                Ok((l, red)) => ok(
                    sorted_json(serde_json::to_value(&l).unwrap()),
                    json!({"red": red}),
                ),
                Err(e) => err(crate::PostError::Failed {
                    step: "tf_layout",
                    msg: e,
                }),
            }
        }
        "vs_tf" => match tf::vs_tf(msl, args.get("tf").unwrap_or(&Value::Null), false) {
            Ok(t) => ok(t, json!({})),
            Err(e) => err(e),
        },
        "hmma_msl" => match hmma::hmma_msl(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "frag_z_snap" => {
            let (t, n) = zsnap::frag_z_snap(msl);
            ok(t.into_owned(), json!({"n": n}))
        }
        "pool_arrays" => match pool::pool_arrays(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "wide_loads" => ok(wide::wide_loads(msl).into_owned(), json!({})),
        "mul_extended" => ok(ampere::mul_extended(msl).into_owned(), json!({})),
        "float_add_msl" => ok(ampere::float_add_msl(msl).into_owned(), json!({})),
        "gather_offsets" => match ampere::gather_offsets(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "shared_store_msl" => match ampere::shared_store_msl(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "per_vertex" => match pervertex::per_vertex(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        "constant_reads" => ok(
            wide::constant_reads(msl, &read_only(args), mode(args)).into_owned(),
            json!({}),
        ),
        "compute_loops" => match cs::compute_loops(msl) {
            Ok((t, sites)) => ok(
                t,
                json!({"sites": sites.iter().map(|s| s.to_json()).collect::<Vec<_>>()}),
            ),
            Err(e) => err(e),
        },
        "raw_noinline" => ok(
            steps::raw_noinline(
                msl,
                args.get("stage").and_then(|v| v.as_str()).unwrap_or("cs"),
            )
            .into_owned(),
            json!({}),
        ),
        "mesh_subgroup_noinline" => ok(steps::mesh_subgroup_noinline(msl).into_owned(), json!({})),
        "rewrites" => rewrites(msl, args),
        "wide_atomics" => match atomic::wide_atomics(msl) {
            Ok(t) => ok(t.into_owned(), json!({})),
            Err(e) => err(e),
        },
        _ => None,
    }
}

/// `json.dumps(x, sort_keys=True)` spelling: keys in sorted order at every depth, whatever order `serde_json::Map` keeps (the workspace enables
/// serde_json's `preserve_order`, so a plain `to_string` follows insertion order there).
pub fn sorted_json(v: Value) -> String {
    fn sorted(v: Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<String> = m.keys().cloned().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    let x = sorted(m[&k].clone());
                    out.insert(k, x);
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
            other => other,
        }
    }
    sorted(v).to_string()
}
