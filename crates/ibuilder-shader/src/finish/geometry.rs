//! `dmsh_build.gs_slice` / `geometry_finish` and `dmsh_geometry.py` (`output_layout`, `reader`, `topology_pointer`, `capture_input`, `passthrough`): the geometry shaders.
//!
//! Round 3: the passthrough geometry shaders' lean kernels (`dmsh_geometry.lean`, `lean.rs`: the `<key>_bl0/1/3` libraries) and the geometry shaders with a transform feedback layout
//! (`vs_tf(geometry=True)` on the vertex reader) are ported; Grizzy has none of either, so they are verified on the engine's own hand-assembled shaders and on synthetic input only
//! (`docs/shaders/SHADER-TESS-MESH-RUST.md`).
use super::pyjson::J;
use super::{Ctx, Finished, R, reflect, spv, t160};
use crate::PostError;
use crate::tools::ShaderTools;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

// ------------------------------------------------------------------------------------------ gs_slice
re!(RE_GS_IN, r"layout\(location = (\d+)\) in vec4 (\w+)\[3\];");
re!(RE_GS_OUT, r"layout\(location = (\d+)\) out vec4 (\w+);");
re!(
    RE_GS_SKIP,
    r"^(?:int \w+ = -?\d+|\w+ = int\(0x80000000\)|int \w+ = \w+|EmitVertex\(\))$"
);
re!(RE_GS_LOAD_IN, r"^float (\w+) = (\w+)\[\w+\]\.([xyzw])$");
re!(
    RE_GS_LOAD_POS,
    r"^float (\w+) = gl_in\[\w+\]\.gl_Position\.([xyzw])$"
);
re!(RE_GS_STORE_POS, r"^gl_Position\.([xyzw]) = (\w+)$");
re!(RE_GS_STORE_OUT, r"^(\w+)\.([xyzw]) = (\w+)$");
re!(RE_GS_LAYER, r"^gl_Layer = floatBitsToInt\((\w+)\)$");

#[derive(Clone, PartialEq, Eq, Debug)]
enum Src {
    Loc(i64, char),
    Pos(char),
}

/// `gs_slice(spv)`: the GLSL spirv-cross writes for a geometry stage (`stdout` of `<spv> --version 450`) -> {loc, comp} of a layer-only geometry shader
pub fn gs_slice(tools: &dyn ShaderTools, spv: &Path) -> Option<(i64, char)> {
    let r = tools.spirv_cross(&[
        spv.to_string_lossy().into_owned(),
        "--version".into(),
        "450".into(),
    ]);
    let g = &r.stdout;
    if r.rc != 0
        || !g.contains("layout(triangles) in;")
        || !g.contains("layout(max_vertices = 3, triangle_strip) out;")
        || g.matches("EmitVertex();").count() != 3
        || g.contains("EndPrimitive")
    {
        return None;
    }
    let ins: BTreeMap<String, i64> = RE_GS_IN
        .captures_iter(g)
        .map(|m| (m[2].to_string(), m[1].parse().unwrap_or(0)))
        .collect();
    let outs: BTreeMap<String, i64> = RE_GS_OUT
        .captures_iter(g)
        .map(|m| (m[2].to_string(), m[1].parse().unwrap_or(0)))
        .collect();
    let body = match g.find("void main()") {
        Some(i) => &g[i..],
        None => g.get(g.len().saturating_sub(1)..).unwrap_or(""),
    };
    let after = match body.find('{') {
        Some(i) => &body[i + 1..],
        None => body,
    };
    let (mut var, mut layer): (BTreeMap<String, Src>, Vec<(i64, char)>) =
        (BTreeMap::new(), Vec::new());
    for st in after.split([';', '{', '}']).map(|s| s.trim()) {
        if st.is_empty() || st == "if (false)" || RE_GS_SKIP.is_match(st) {
            continue;
        }
        if let Some(m) = RE_GS_LOAD_IN.captures(st)
            && let Some(l) = ins.get(&m[2])
        {
            var.insert(m[1].to_string(), Src::Loc(*l, m[3].chars().next().unwrap()));
            continue;
        }
        if let Some(m) = RE_GS_LOAD_POS.captures(st) {
            var.insert(m[1].to_string(), Src::Pos(m[2].chars().next().unwrap()));
            continue;
        }
        if let Some(m) = RE_GS_STORE_POS.captures(st)
            && var.get(&m[2]) == Some(&Src::Pos(m[1].chars().next().unwrap()))
        {
            continue;
        }
        if let Some(m) = RE_GS_STORE_OUT.captures(st)
            && let Some(l) = outs.get(&m[1])
            && var.get(&m[3]) == Some(&Src::Loc(*l, m[2].chars().next().unwrap()))
        {
            continue;
        }
        if let Some(m) = RE_GS_LAYER.captures(st)
            && let Some(Src::Loc(l, c)) = var.get(&m[1])
        {
            if !layer.contains(&(*l, *c)) {
                layer.push((*l, *c));
            }
            continue;
        }
        return None;
    }
    if layer.len() != 1 || g.matches("gl_Layer =").count() != 3 {
        return None;
    }
    Some(layer[0])
}

// ------------------------------------------------------------------------------------------ dmsh_geometry
/// `output_layout(code, input_topology)` -> {vertices, invocations, output, index_stride}
pub fn output_layout(code: &[u8], input_topology: usize) -> Result<J, String> {
    if code.len() < 20 || input_topology >= 5 {
        return Err("invalid geometry header or input topology".into());
    }
    let w = |o: usize| u32::from_le_bytes([code[o], code[o + 1], code[o + 2], code[o + 3]]);
    let (w2, w3, w4) = (w(8), w(12), w(16));
    let (mut vertices, invocations, mut topology) = (
        (w4 & 4095) as i64,
        (w2 >> 24) as i64,
        ((w3 >> 24) & 15) as i64,
    );
    if w(0) & (1 << 24) != 0 {
        vertices = [1, 2, 2, 3, 3][input_topology];
        topology = [1, 6, 6, 7, 7][input_topology];
    }
    if !matches!(topology, 1 | 6 | 7) || vertices == 0 || invocations == 0 {
        return Err("invalid geometry output declaration".into());
    }
    Ok(J::obj()
        .with("vertices", vertices)
        .with("invocations", invocations)
        .with("output", topology)
        .with("index_stride", vertices * 2))
}

pub fn passthrough(code: &[u8]) -> bool {
    code.len() >= 4 && (u32::from_le_bytes([code[0], code[1], code[2], code[3]]) >> 24) & 1 != 0
}

re!(
    RE_VMASK_STORE,
    r"\bspvArrayCopyFromStackToStack\(\s*gl_BuiltIn_5253\s*,\s*as_type<int>\((\w+)\)\.elements\s*\);"
);
re!(RE_STRUCT_OUT, r"(?s)struct main0_out\s*\{(.*?)\};");
re!(
    RE_POSITION,
    r"\bfloat4 (\w+) \[\[position(?:, invariant)?\]\];"
);
re!(
    RE_VS_ENTRY,
    r"\bvertex\s+main0_out\s+main0\s*\([^{}]*\)\s*\{"
);

/// `dmsh_geometry.reader(src)`
pub fn reader(src: &str) -> Result<String, String> {
    if !src.contains("gl_BuiltIn_5253") {
        return Ok(src.to_string());
    }
    let n = RE_VMASK_STORE.find_iter(src).count();
    let lowered = RE_VMASK_STORE
        .replace_all(src, "dm_gs_viewport_mask = as_type<uint>($1);")
        .into_owned();
    if n == 0 || lowered.contains("gl_BuiltIn_5253") || src.contains("dm_gs_viewport_mask") {
        return Err("geometry reader: unsupported viewport-mask expression".into());
    }
    let output = RE_STRUCT_OUT.captures(&lowered);
    let position = output.as_ref().and_then(|o| {
        RE_POSITION
            .captures(o.get(1).unwrap().as_str())
            .map(|p| p[1].to_string())
    });
    let entry = RE_VS_ENTRY.find(&lowered);
    let (Some(position), Some(entry)) = (position, entry) else {
        return Err("geometry reader: missing vertex position or entry".into());
    };
    let (begin, mut depth, mut end) = (entry.end(), 1i32, entry.end());
    let b = lowered.as_bytes();
    while depth != 0 && end < b.len() {
        depth += (b[end] == b'{') as i32 - (b[end] == b'}') as i32;
        end += 1;
    }
    if depth != 0 {
        return Err("geometry reader: unterminated vertex entry".into());
    }
    let body = &lowered[begin..end - 1];
    if body.matches("dm_gs_viewport_mask").count() != n || !body.contains("return out;") {
        return Err("geometry reader: viewport mask outside the vertex entry".into());
    }
    let body = body.replace("return out;", &format!("{{ if ((dm_gs_viewport_mask & 1u) == 0u) out.{position} = float4(2.0, 0.0, 0.0, 1.0); return out; }}"));
    Ok(format!(
        "{}\n    uint dm_gs_viewport_mask = 1u;{}{}",
        &lowered[..begin],
        body,
        &lowered[end - 1..]
    ))
}

re!(RE_TRB_TYPE, r"texture_buffer<float> trb_data");
re!(RE_TRB_READ, r"(?:spvDescriptorSet0\.)?trb_data\.read\(");

/// `dmsh_geometry.topology_pointer(src)`
pub fn topology_pointer(src: &str) -> Result<String, String> {
    let n = RE_TRB_TYPE.find_iter(src).count();
    let mut src = RE_TRB_TYPE
        .replace_all(src, "device const uint* trb_data")
        .into_owned();
    if n == 0 || !src.contains("trb_data [[id(") {
        return Err("missing geometry topology buffer".into());
    }
    while let Some(m) = RE_TRB_READ.find(&src) {
        let at = m.end();
        let (mut depth, mut end) = (1i32, at);
        let b = src.as_bytes();
        while depth != 0 {
            if end >= b.len() {
                return Err("string index out of range".into());
            }
            if b[end] == b'(' {
                depth += 1;
            }
            if b[end] == b')' {
                depth -= 1;
            }
            end += 1;
        }
        if src.get(end..end + 2) != Some(".x") {
            return Err("unexpected topology read".into());
        }
        let table = src[m.start()..m.end() - ".read(".len()].to_string();
        src = format!(
            "{}as_type<float>({table}[{}]){}",
            &src[..m.start()],
            &src[at..end - 1],
            &src[end + 2..]
        );
    }
    Ok(src)
}

re!(RE_VOUT_DECL, r"\bdevice (\w+)\* vertex_output\b");

/// `dmsh_geometry.capture_input(src)`
pub fn capture_input(src: &str) -> Result<String, String> {
    if !src.contains("vertex_output") {
        return Ok(src.to_string());
    }
    if RE_VOUT_DECL.find_iter(src).count() != 1 {
        return Err("unknown geometry capture-input declaration".into());
    }
    Ok(RE_VOUT_DECL
        .replace_all(src, "device volatile $1* vertex_output")
        .into_owned())
}

// ------------------------------------------------------------------------------------------ the chain's steps over a geometry kernel
/// `uniform_tests(select_guards(bit0_mask(precise(check_msl(text)))))` and `pool_arrays` (the chain's own functions)
fn selected(text: &str) -> Result<String, PostError> {
    let a = crate::steps::check_msl(text)?;
    let b = crate::steps::precise(&a);
    let c = crate::bit0::bit0_mask(&b);
    let d = crate::guards::select_guards(&c)?;
    let e = crate::utest::uniform_tests(&d)?;
    Ok(e.into_owned())
}

fn pool_arrays(text: String) -> Result<String, PostError> {
    Ok(crate::pool::pool_arrays(&text)?.into_owned())
}

fn fatal(e: impl std::fmt::Display) -> Finished {
    Finished::fail("geometry failed", t160(&e.to_string()))
}

#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
fn cross(tools: &dyn ShaderTools, spv: &Path) -> Result<Result<String, String>, Finished> {
    let data = std::fs::read(spv).map_err(|e| fatal(format!("{}: {e}", spv.display())))?;
    let mut args = spv::cross_args(&data, &spv.to_string_lossy()).map_err(fatal)?;
    args.push(spv.to_string_lossy().into_owned());
    let r = tools.spirv_cross(&args);
    Ok(if r.rc != 0 {
        Err(t160(r.stderr.trim()))
    } else {
        Ok(r.stdout)
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `finish` for a geometry shader: the layer-only fold, else the five topology variants and the vertex reader
#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_large_err)] // Err is the finished-job record (flow control); boxing it would reshape every caller
pub fn finish_gs(
    ctx: &Ctx,
    key: &str,
    work: &Path,
    job: &mut Value,
    c1: &[u8],
    tools: &dyn ShaderTools,
    source: &str,
    _log: &mut Vec<String>,
) -> R<Finished> {
    let has_tf = job
        .get("tf")
        .map(|v| match v {
            Value::Null => false,
            Value::Array(a) => !a.is_empty(),
            Value::Bool(b) => *b,
            _ => true,
        })
        .unwrap_or(false);
    let pc = ctx.pc_of.get(key).cloned();
    if !has_tf && let Some((loc, comp)) = gs_slice(tools, &work.join(format!("{key}.spv"))) {
        let refl = J::obj()
            .with("key", format!("{key}_b"))
            .with("route", "B")
            .with("chain", ctx.chain.as_str())
            .with("dm_ctl", key)
            .with("source", source)
            .with("pc_paired", pc.as_deref())
            .with("stage", "gs")
            .with(
                "gs_slice",
                J::obj().with("loc", loc).with("comp", comp.to_string()),
            )
            .with("notes", J::Arr(vec![]))
            .with("drv_reads", "")
            .with("drv_unfillable", J::Arr(vec![]))
            .with("bindings", J::Arr(vec![]));
        return Ok(super::finalize(
            ctx,
            key,
            refl,
            vec![],
            Default::default(),
            false,
        ));
    }
    let code = match std::fs::read(work.join(format!("{key}.bin"))).or_else(|_| {
        job.get("code")
            .and_then(|c| c.as_str())
            .map(std::fs::read)
            .unwrap_or_else(|| Err(std::io::ErrorKind::NotFound.into()))
    }) {
        Ok(c) => c,
        Err(e) => return Err(fatal(e)),
    };
    output_layout(&code, 0).map_err(fatal)?; // validate before running the translator
    let jobs: Vec<Value> = (0..5)
        .map(|t| {
            let mut j = job.clone();
            j["key"] = Value::String(format!("{key}_g{t}"));
            j["geometry_topology"] = Value::from(t);
            j
        })
        .collect();
    let jp = work.join(format!("{key}_geometry.json"));
    std::fs::write(&jp, serde_json::to_vec(&jobs).unwrap_or_default()).map_err(fatal)?;
    let rx = tools.xlate(&[
        jp.to_string_lossy().into_owned(),
        work.to_string_lossy().into_owned(),
    ]);
    if rx.rc != 0 {
        return Err(Finished::fail("geometry failed", t160(rx.stderr.trim())));
    }
    let mut outputs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut base: Option<J> = None;
    let is_pass = passthrough(&code);
    let mut save = |suffix: &str, text: String, meta: &J| {
        outputs.push((format!("{key}{suffix}.metal"), text.into_bytes()));
        let mut w = meta.clone();
        w.set("chain_metal", ctx.opts.chain_metal.as_str());
        w.set("chain_compiler", ctx.opts.chain_compiler.as_str());
        outputs.push((format!("{key}{suffix}.json"), w.dump().into_bytes()));
    };
    for t in 0..5usize {
        let layout = output_layout(&code, t).map_err(fatal)?;
        let k = format!("{key}_g{t}");
        let spvp = work.join(format!("{k}.spv"));
        let flat = std::fs::read(&spvp)
            .map_err(|e| fatal(format!("{}: {e}", spvp.display())))
            .and_then(|d| spv::flatten_sets(&d).map_err(fatal))?;
        std::fs::write(&spvp, flat).map_err(fatal)?;
        let text = match cross(tools, &spvp)? {
            Ok(t) => t,
            Err(e) => return Err(Finished::fail("geometry MSL failed", e)),
        };
        let sel = match selected(&text) {
            Ok(s) => s,
            Err(PostError::Unported(w)) => {
                return Err(Finished::unported(format!(
                    "chain step {w} (geometry kernel)"
                )));
            }
            Err(PostError::Failed { msg, .. }) => {
                return Err(Finished::fail("geometry MSL failed", t160(&msg)));
            }
        };
        let plain = match pool_arrays(sel) {
            Ok(p) => topology_pointer(&p).map_err(fatal)?,
            Err(PostError::Unported(w)) => {
                return Err(Finished::unported(format!(
                    "chain step {w} (geometry kernel)"
                )));
            }
            Err(e) => return Err(fatal(e)),
        };
        let src = capture_input(&plain).map_err(fatal)?;
        let res: Value = serde_json::from_slice(
            &std::fs::read(work.join(format!("{k}.res.json"))).map_err(fatal)?,
        )
        .map_err(fatal)?;
        let stride = res
            .get("geometry_stride")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| fatal("geometry_stride"))?;
        let notes: Vec<J> = res
            .get("notes")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(J::from_value).collect())
            .unwrap_or_default();
        let binds = reflect::reflect(key, "gs", &src, &res).map_err(fatal)?;
        let mut refl = J::obj()
            .with("key", format!("{key}_bg{t}"))
            .with("route", "B")
            .with("chain", ctx.chain.as_str())
            .with("dm_ctl", key)
            .with("source", source)
            .with("pc_paired", pc.as_deref())
            .with("stage", "gs")
            .with("entry", "main0")
            .with("msl_entry_kind", "kernel")
            .with(
                "threadgroup_size",
                J::Arr(vec![J::Int(32), J::Int(1), J::Int(1)]),
            )
            .with("geometry", {
                let mut g = layout.clone();
                g.set("stride", stride);
                g
            })
            .with("c1", hex(c1))
            .with("notes", J::Arr(notes))
            .with("drv_reads", reflect::drv_reads(&src))
            .with("drv_unfillable", J::Arr(vec![]))
            .with("bindings", J::Arr(binds))
            .with("vertex_inputs", J::Arr(vec![]))
            .with("raster", J::obj());
        save(&format!("_bg{t}"), src.clone(), &refl);
        // Passthrough input without adjacency: a lean kernel the host can pair with the game's own vertex shader (dmsh_geometry.lean)
        if is_pass && matches!(t, 0 | 1 | 3) {
            match super::lean::lean(&plain, stride) {
                Ok((lean_src, reads)) => {
                    let mut g = layout.clone();
                    g.set("stride", 1);
                    g.set("lean", true);
                    g.set(
                        "capture_reads",
                        J::Arr(reads.iter().map(|r| J::Int(*r)).collect()),
                    );
                    let mut m = refl.clone();
                    m.set("key", format!("{key}_bl{t}"));
                    m.set("geometry", g);
                    m.set("drv_reads", reflect::drv_reads(&lean_src));
                    m.set(
                        "bindings",
                        J::Arr(reflect::reflect(key, "gs", &lean_src, &res).map_err(fatal)?),
                    );
                    save(&format!("_bl{t}"), lean_src, &m);
                }
                Err(e) => {
                    // `refl['notes'] = refl['notes'] + [str(e)]`: the base library written below (t == 0) carries the note, the `_bg0` written above does not
                    let mut notes = refl
                        .get("notes")
                        .and_then(|n| n.as_arr())
                        .cloned()
                        .unwrap_or_default();
                    notes.push(J::Str(e));
                    refl.set("notes", J::Arr(notes));
                }
            }
        }
        if t == 0 {
            let mut b = refl.clone();
            b.set("key", format!("{key}_b"));
            save("_b", src.clone(), &b);
            base = Some(b);
            let rd = work.join(format!("{k}.reader.spv"));
            let flat = std::fs::read(&rd)
                .map_err(|e| fatal(format!("{}: {e}", rd.display())))
                .and_then(|d| spv::flatten_sets(&d).map_err(fatal))?;
            std::fs::write(&rd, flat).map_err(fatal)?;
            let text = match cross(tools, &rd)? {
                Ok(t) => t,
                Err(e) => return Err(Finished::fail("geometry reader failed", e)),
            };
            let sel = match selected(&text) {
                Ok(s) => s,
                Err(PostError::Unported(w)) => {
                    return Err(Finished::unported(format!(
                        "chain step {w} (geometry reader)"
                    )));
                }
                Err(PostError::Failed { msg, .. }) => {
                    return Err(Finished::fail("geometry reader failed", t160(&msg)));
                }
            };
            let sel = match pool_arrays(sel) {
                Ok(s) => s,
                Err(PostError::Unported(w)) => {
                    return Err(Finished::unported(format!(
                        "chain step {w} (geometry reader)"
                    )));
                }
                Err(e) => return Err(fatal(e)),
            };
            let rdr = crate::vs::vs_layer(crate::vs::vs_init(&reader(&sel).map_err(fatal)?))
                .map_err(fatal)?;
            let (analysis, full) = crate::vs::vs_depth(&rdr).map_err(fatal)?;
            let rres = serde_json::json!({"sbufs": [], "textures": res.get("textures").cloned().unwrap_or(Value::Array(vec![])), "images": res.get("images").cloned().unwrap_or(Value::Array(vec![]))});
            let mut meta = J::obj()
                .with("key", format!("{key}_bgv"))
                .with("route", "B")
                .with("chain", ctx.chain.as_str())
                .with("dm_ctl", key)
                .with("stage", "vs")
                .with("entry", "main0")
                .with("entry_z", "main0_z")
                .with("msl_entry_kind", "vertex")
                .with("geometry_reader", true)
                .with(
                    "geometry_output",
                    layout.get("output").cloned().unwrap_or(J::Null),
                );
            if crate::vs::layer_out(&analysis) {
                meta.set("layer_out", true);
            }
            meta.set("c1", "");
            meta.set("notes", J::Arr(vec![]));
            meta.set("drv_reads", J::obj());
            meta.set("drv_unfillable", J::Arr(vec![]));
            meta.set(
                "bindings",
                J::Arr(reflect::reflect(key, "vs", &analysis, &rres).map_err(fatal)?),
            );
            meta.set("vertex_inputs", J::Arr(vec![]));
            meta.set("raster", J::obj());
            let mut full = full;
            if has_tf {
                let tfv = job.get("tf").cloned().unwrap_or(Value::Null);
                full.push_str(
                    &crate::tf::vs_tf(&analysis, &tfv, true)
                        .map_err(|e| Finished::fail("fatal", format!("vs_tf: {e}")))?,
                );
                meta.set("entry_tf", "main0_tf");
                meta.set(
                    "tf_strides",
                    J::Arr(
                        tfv.as_array()
                            .map(|a| {
                                a.iter()
                                    .map(|e| {
                                        J::Int(
                                            e.get("stride").and_then(|x| x.as_i64()).unwrap_or(0),
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    ),
                );
            }
            save("_bgv", full, &meta);
        }
    }
    let base = base.ok_or_else(|| fatal("no base"))?;
    Ok(super::finalize_with(
        ctx,
        key,
        base,
        outputs,
        Default::default(),
    ))
}
