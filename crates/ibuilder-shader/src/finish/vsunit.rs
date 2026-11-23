//! `dmsh_side.vs_unit` / `vs_entries`: a vertex shader's libraries from one compile unit, which holds the body once (`dm_vs_body`, always inline, and its out-of-line copy
//! `dm_vs_body_o` for the wide-line entries and the capture entry main0_gs), and `dmsh_build.vs_libraries` as source mode makes the iOS image: the unit under every library
//! name it is cut into (`<key>_b`, `<key>_bp`, `<key>_bv`), or, when the source is not of the shape the unit knows, each library from its own source with a red line.
use crate::finish::pyjson::J;
use crate::finish::texts;
use crate::finish::vin;
use regex::{NoExpand, Regex};
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(VENTRY, r"(?m)^vertex (\S+) (\w+)\((.*)\)\n\{\n");
re!(VEND, r"\n\}(?:\n|$)");
re!(PARAM_ATTR, r"\s*\[\[.*\]\]$");
re!(PARAM_NAME, r"(\w+)$");
re!(EPILOGUE, r"\{ dm_cw\(.*? return; \}");
re!(
    CULL,
    r"\{ if \(\(dm_vertex_viewport_mask & 1u\) == 0u\) out\.\w+ = float4\(2\.0, 0\.0, 0\.0, 1\.0\); return out; \}"
);
re!(MAIN0_IN_V, r"(?s)\nstruct main0_in_v\n\{\n.*?\n\};");
re!(LAYER_ENTRY, r"(?m)^vertex \S+ main0_l\(");

/// `VS_DEPTH`: the line `dmsh_entry.vs_depth` puts before every `return out;` of main0
pub const VS_DEPTH: &str =
    "out.gl_Position.z = fma(out.gl_Position.z, 0.5, out.gl_Position.w * 0.5);";
pub const VS_BODY: &str = "static __attribute__((always_inline)) main0_out dm_vs_body(";
pub const VS_BODY_OUT: &str = "static main0_out dm_vs_body_o(";
/// the pulling entry points' names in the unit
pub const VS_PULLED: [(&str, &str); 2] = [("main0", "main0_p"), ("main0_z", "main0_zp")];
/// the shadow-input ones
pub const VS_SHADOW: [(&str, &str); 2] = [("main0", "main0_v"), ("main0_z", "main0_zv")];

/// one vertex entry point as `vs_entries` gives it (byte offsets)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VEntry {
    pub ret: String,
    /// the text between its parentheses
    pub params: String,
    pub start: usize,
    /// after its opening brace's line
    pub body: usize,
    /// after its closing brace's line
    pub end: usize,
}

/// `vs_entries(src)`: the vertex entry points by name, in the order the dict holds them (a later one of the same name replaces the earlier's value in its place)
pub fn vs_entries(src: &str) -> Result<Vec<(String, VEntry)>, String> {
    let mut out: Vec<(String, VEntry)> = Vec::new();
    for m in VENTRY.captures_iter(src) {
        let w = m.get(0).unwrap();
        let Some(e) = VEND.find_at(src, w.end() - 1) else {
            return Err(format!("no end of {}", &m[2]));
        };
        let v = VEntry {
            ret: m[1].to_string(),
            params: m[3].to_string(),
            start: w.start(),
            body: w.end(),
            end: e.end(),
        };
        match out.iter_mut().find(|(n, _)| n == &m[2]) {
            Some(x) => x.1 = v,
            None => out.push((m[2].to_string(), v)),
        }
    }
    Ok(out)
}

fn get<'a>(es: &'a [(String, VEntry)], n: &str) -> Option<&'a VEntry> {
    es.iter().find(|(k, _)| k == n).map(|(_, v)| v)
}

/// `_params(s)`: a parameter list's parameters; commas inside `<>`, `()` or `[]` do not separate
fn params(s: &str) -> Vec<String> {
    let (mut out, mut at, mut depth) = (Vec::new(), 0usize, 0i64);
    for (i, ch) in s.char_indices() {
        depth += matches!(ch, '<' | '(' | '[') as i64 - matches!(ch, '>' | ')' | ']') as i64;
        if ch == ',' && depth == 0 {
            out.push(s[at..i].trim().to_string());
            at = i + 1;
        }
    }
    if !s[at..].trim().is_empty() {
        out.push(s[at..].trim().to_string());
    }
    out
}

/// the names the unit gives the side libraries' entry points (`{'entry', 'entry_z', ...}` in insertion order)
pub type Names = Vec<(String, String)>;

fn names_j(n: &Names) -> J {
    let mut j = J::obj();
    for (k, v) in n {
        j.set(k, v.as_str());
    }
    j
}

/// `dmsh_side.vs_unit(full, pulled, shadowed)` → (the compile unit, the pulling entry points' names, the shadow-input ones'); Err is the ValueError's message
pub fn vs_unit(
    full: &str,
    pulled: Option<&str>,
    shadowed: Option<&str>,
) -> Result<(String, Names, Names), String> {
    let es = vs_entries(full)?;
    let (Some(e0), Some(ez)) = (get(&es, "main0"), get(&es, "main0_z")) else {
        return Err("no main0 and main0_z".into());
    };
    if e0.end > ez.start || e0.ret != "main0_out" || ez.ret != "main0_out" || e0.params != ez.params
    {
        return Err("main0 and main0_z differ in more than their bodies".into());
    }
    let gl = &full[e0.body..e0.end];
    let z = &full[ez.body..ez.end];
    if !z.contains("return out;")
        || gl != z.replace("return out;", &format!("{VS_DEPTH}\n    return out;"))
    {
        return Err("main0 is not main0_z with the depth line".into());
    }
    let mut taken: Vec<&str> = [
        "main0_p",
        "main0_zp",
        "main0_v",
        "main0_zv",
        "main0_wl",
        "main0_z_wl",
    ]
    .into_iter()
    .filter(|n| get(&es, n).is_some())
    .collect();
    taken.extend(
        ["dm_vs_body", "dm_wl_in"]
            .into_iter()
            .filter(|n| full.contains(&format!("{n}("))),
    );
    if let Some(t) = taken.first() {
        return Err(format!("the source has a {t} already"));
    }
    let (mut decl, mut args) = (Vec::new(), Vec::new());
    for p in params(&e0.params) {
        let d = PARAM_ATTR.replace_all(&p, "").into_owned();
        let a = PARAM_NAME.captures(&d).map(|c| c[1].to_string());
        let Some(a) = a.filter(|_| !p.contains("function_constant")) else {
            return Err(format!(
                "a parameter of main0: {}",
                p.chars().take(80).collect::<String>()
            ));
        };
        decl.push(d);
        args.push(a);
    }
    let (decl, args) = (decl.join(", "), args.join(", "));
    // text with its copies of the body as calls of f; n_gl, n_z: how many copies of each form it must hold
    let calling = |text: &str, f: &str, n_gl: usize, n_z: usize| -> Result<String, String> {
        let (cg, cz) = (text.matches(gl).count(), text.matches(z).count());
        if (cg, cz) != (n_gl, n_z) {
            return Err(format!(
                "{cg} and {cz} copies of the body where {n_gl} and {n_z} belong"
            ));
        }
        Ok(text
            .replace(
                gl,
                &format!("    main0_out out = {f}({args});\n    {VS_DEPTH}\n    return out;\n}}\n"),
            )
            .replace(z, &format!("    return {f}({args});\n}}\n")))
    };
    // a variant's main0 / main0_z under the unit's names: each keeps its prologue and calls the body
    let renamed = |variant: &str, names: &[(&str, &str)]| -> Result<(String, Names), String> {
        let ve = vs_entries(variant)?;
        let (mut text, mut got) = (String::new(), Names::new());
        if get(&ve, "main0").is_none() {
            return Err("a variant without main0".into());
        }
        for (n, new) in names {
            let Some(e) = get(&ve, n) else { continue };
            let one = &variant[e.start..e.end];
            let head = format!("vertex main0_out {n}(");
            if !one.starts_with(&head) {
                return Err(format!("the variant's {n} does not return main0_out"));
            }
            text.push('\n');
            text.push_str(&calling(
                &format!("vertex main0_out {new}({}", &one[head.len()..]),
                "dm_vs_body",
                (*n == "main0") as usize,
                (*n == "main0_z") as usize,
            )?);
            let k = if *n == "main0" { "entry" } else { "entry_z" };
            match got.iter_mut().find(|(x, _)| x == k) {
                Some(x) => x.1 = new.to_string(),
                None => got.push((k.to_string(), new.to_string())),
            }
        }
        Ok((text, got))
    };
    let wide = pulled.is_some_and(|p| p.contains("\nstatic inline main0_in dm_wl_in("));
    let mut rest = full[ez.end..].to_string();
    let mut gs_call = false;
    if let Some(eg) = get(&es, "main0_gs") {
        // the capture entry: its prologue, then the body with the capture epilogue where each `return out;` was
        let gs = &full[eg.body..eg.end];
        let epi = EPILOGUE.find(gs).map(|m| m.as_str());
        let params_ok = e0.params.is_empty()
            || eg.params == e0.params
            || eg.params.starts_with(&format!("{}, ", e0.params));
        let bad = || "main0_gs is not the body with the capture epilogue".to_string();
        let Some(epi) = epi else { return Err(bad()) };
        if eg.start < ez.end || eg.ret != "void" || !params_ok {
            return Err(bad());
        }
        let copy = z.replace("return out;", epi);
        if gs.ends_with(&copy) {
            gs_call = true;
            rest = format!(
                "{}{}    main0_out out = dm_vs_body_o({args});\n    {epi}\n}}\n{}",
                &full[ez.end..eg.body],
                &gs[..gs.len() - copy.len()],
                &full[eg.end..]
            );
        } else if z.contains("dm_vertex_viewport_mask")
            && gs.ends_with(CULL.replace_all(z, NoExpand(epi)).as_ref())
        {
            // vs_viewport_mask culls raster entries only: main0_gs stays a whole unculled copy
        } else {
            return Err(bad());
        }
    }
    let mut head = full[..e0.start].to_string();
    if pulled.is_some() {
        let Some(ui) = head.find("using namespace metal;") else {
            return Err("no \"using namespace metal;\"".into());
        };
        let ui = ui + "using namespace metal;".len();
        head = format!(
            "{}\n\n{}{}{}",
            &head[..ui],
            texts::PULL_FETCH,
            if wide { texts::PULL_WL_MSL } else { "" },
            &head[ui..]
        );
    }
    let mut src = format!("{head}{VS_BODY}{decl})\n{{\n{z}\n");
    if wide || gs_call {
        src.push_str(&format!(
            "{VS_BODY_OUT}{decl})\n{{\n    return dm_vs_body({args});\n}}\n\n"
        ));
    }
    src.push_str(&calling(&full[e0.start..ez.end], "dm_vs_body", 1, 1)?);
    src.push_str(&rest);
    let (mut pn, mut vn) = (Names::new(), Names::new());
    if let Some(pv) = pulled {
        let (text, n) = renamed(pv, &VS_PULLED)?;
        pn = n;
        src.push_str(&text);
        if wide {
            let wl = &pv[pv.find("\nstatic inline main0_in dm_wl_in(").unwrap()..];
            for (n, k) in [("main0", "entry_wl"), ("main0_z", "entry_z_wl")] {
                if wl.contains(&format!("\nvertex main0_out {n}_wl(")) {
                    match pn.iter_mut().find(|(x, _)| x == k) {
                        Some(x) => x.1 = format!("{n}_wl"),
                        None => pn.push((k.to_string(), format!("{n}_wl"))),
                    }
                }
            }
            let has = |k: &str| pn.iter().any(|(x, _)| x == k) as usize;
            src.push_str(&calling(
                wl,
                "dm_vs_body_o",
                has("entry_wl"),
                has("entry_z_wl"),
            )?);
        }
    }
    if let Some(sv) = shadowed {
        let Some(st) = MAIN0_IN_V.find(sv) else {
            return Err("a shadow-input variant without main0_in_v".into());
        };
        let (text, n) = renamed(sv, &VS_SHADOW)?;
        vn = n;
        src.push('\n');
        src.push_str(st.as_str());
        src.push('\n');
        src.push_str(&text);
    }
    Ok((src, pn, vn))
}

/// What `vs_libraries` gives the iOS image and the reflection
#[derive(Debug, Clone, Default)]
pub struct VsLibraries {
    /// the image's files: `<key>_b.metal` (the unit, or the plain source), `<key>_bp.metal`, `<key>_bv.metal`
    pub outputs: Vec<(String, Vec<u8>)>,
    /// the reflection's `pull` and `vin`, set in that order after the rest of the reflection (`refl.update(side)`)
    pub pull: Option<J>,
    pub vin: Option<J>,
    /// the iOS base library was cut from a unit: `<key>_b.json` gets `chain_metal_cut: true` (after chain_metal / chain_compiler)
    pub cut: bool,
    /// the red lines the build prints, in order
    pub log: Vec<String>,
}

/// `vin_finish` in source mode: the shadow-input library `<key>_bv` from its own source
fn vin_separate(full: &str, key: &str, r: &mut VsLibraries) {
    match vin::vin_variant(full) {
        Ok((v, shadows)) => {
            r.outputs.push((format!("{key}_bv.metal"), v.into_bytes()));
            r.vin = Some(
                J::obj()
                    .with("entry", "main0")
                    .with("entry_z", "main0_z")
                    .with("shadow", J::Arr(shadows)),
            );
        }
        Err(why) => {
            if why != "no integer input" {
                r.log.push(format!("[RED] {key}: no shadow-input library ({why}): its integer inputs keep the CPU conversion"));
            }
        }
    }
}

/// `pull_finish` in source mode: the pulling library `<key>_bp` from its own source (written only with `pulling`, the reflection's `pull` either way)
fn pull_separate(full: &str, key: &str, pulling: bool, r: &mut VsLibraries) {
    match vin::pull_variant(full) {
        Ok((v, info)) => {
            if pulling {
                r.outputs.push((format!("{key}_bp.metal"), v.into_bytes()));
            }
            r.pull = Some(without_nulls(&info));
        }
        Err(why) => {
            if why != "no stage input" {
                r.log.push(format!("[RED] {key}: no pulling library ({why}): its pipelines are always built for the draw's vertex layout"));
            }
        }
    }
}

fn without_nulls(info: &J) -> J {
    let mut p = J::obj();
    if let J::Obj(m) = info {
        for (k, v) in m {
            if *v != J::Null {
                p.set(k, v.clone());
            }
        }
    }
    p
}

/// `dmsh_build.vs_libraries(key, msl, full)` for the iOS target in source mode (`pulling`: DMSH_IOS_PULL). `full`: the text of `<key>.metal` (every entry point)
pub fn vs_libraries(key: &str, full: &str, pulling: bool) -> VsLibraries {
    let mut r = VsLibraries::default();
    let mut why: Option<String> = None;
    if let Ok((pv, pinfo)) = vin::pull_variant(full) {
        let (vv, shadows) = match vin::vin_variant(full) {
            Ok((v, s)) => (Some(v), s),
            Err(_) => (None, Vec::new()),
        };
        let vu = if LAYER_ENTRY.is_match(full) {
            None
        } else {
            vv.as_deref()
        };
        let built = vs_unit(full, Some(&pv), vu).and_then(|whole| {
            let rest = match vu {
                Some(_) => Some(vs_unit(full, None, vu)?),
                None => None,
            };
            Ok((whole, rest))
        });
        match built {
            Err(e) => why = Some(e.chars().take(120).collect()),
            Ok((whole, rest)) => {
                let target = if pulling { Some(&whole) } else { rest.as_ref() };
                let b = format!("{key}_b.metal");
                match target {
                    Some((src, pn, vn)) => {
                        r.outputs.push((b, src.clone().into_bytes()));
                        if !pn.is_empty() {
                            r.outputs
                                .push((format!("{key}_bp.metal"), src.clone().into_bytes()));
                        }
                        if !vn.is_empty() {
                            r.outputs
                                .push((format!("{key}_bv.metal"), src.clone().into_bytes()));
                        }
                        r.cut = true;
                    }
                    None => r.outputs.push((b, full.as_bytes().to_vec())),
                }
                let mut pull = pinfo.clone();
                for (k, v) in &whole.1 {
                    pull.set(k, v.as_str());
                }
                r.pull = Some(without_nulls(&pull));
                if vu.is_none() {
                    vin_separate(full, key, &mut r); // none, or the layer shader's own compile
                } else {
                    let mut v = J::obj().with("entry", "main0").with("entry_z", "main0_z");
                    v.extend(&names_j(&whole.2));
                    v.set("shadow", J::Arr(shadows));
                    r.vin = Some(v);
                }
                return r;
            }
        }
    }
    r.outputs
        .push((format!("{key}_b.metal"), full.as_bytes().to_vec()));
    if let Some(w) = &why {
        r.log.push(format!("[RED] {key}: its libraries do not build from one compile ({w}): each is compiled from its own source"));
    }
    vin_separate(full, key, &mut r);
    let mut pull = VsLibraries::default();
    pull_separate(full, key, pulling, &mut pull);
    r.outputs.extend(pull.outputs);
    r.log.extend(pull.log);
    r.pull = pull.pull;
    r
}
