//! `dmsh_side.py` (`entries`, `entry`, `body`, `shared_body`, `calling`, `unit`) with `dmsh_alpha.py` and `dmsh_logic.py`: a pixel shader's compile unit with its alpha test and logic operation entry points.
//! (`cut`, which splits a compiled Metal library, is not needed: source mode writes the unit's text under every library name.)
//! A self-contained port over the regex crate; the chain's own `crate::side` (F1) keeps main0's shared body for `fs_uint` / `fs_dual`.
use super::{abi, texts};
use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(
    RE_ENTRY,
    r"(?m)^((?:\[\[[^\]]*\]\]\s*)?)fragment\s+(\w+)\s+(\w+)\s*\("
);
re!(
    RE_BODY,
    r"static (__attribute__\(\(always_inline\)\) )?main0_out dm_body_main0\("
);
re!(RE_PARAM_ATTR, r"\s*\[\[.*\]\]$");
re!(RE_LAST_WORD, r"(\w+)$");
pub const BODY: &str = "static __attribute__((always_inline)) main0_out dm_body_main0(";

#[derive(Debug, Clone)]
pub struct Entry {
    pub attr: String,
    pub ret: String,
    pub name: String,
    pub params: Vec<String>,
    pub decl: String,
    pub args: String,
    pub start: usize,
    pub head: usize,
    pub end: usize,
}

/// `_close(src, i, o, c)`: the index after the bracket closing the one opened just before `i`
pub fn close(src: &str, i: usize, o: u8, c: u8) -> Option<usize> {
    let b = src.as_bytes();
    let mut depth = 1i32;
    let mut k = i;
    while k < b.len() {
        let p = memchr::memchr2(o, c, &b[k..])? + k;
        if b[p] == o {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(p + 1);
            }
        }
        k = p + 1;
    }
    None
}

fn params(s: &str) -> Vec<String> {
    let (mut out, mut at, mut depth) = (Vec::new(), 0usize, 0i32);
    for (i, ch) in s.bytes().enumerate() {
        if matches!(ch, b'<' | b'(' | b'[') {
            depth += 1;
        }
        if matches!(ch, b'>' | b')' | b']') {
            depth -= 1;
        }
        if ch == b',' && depth == 0 {
            out.push(s[at..i].trim().to_string());
            at = i + 1;
        }
    }
    let tail = s[at..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn build(msl: &str, m: &regex::Captures) -> Result<Entry, String> {
    let whole = m.get(0).unwrap();
    let (attr, ret, name) = (
        m.get(1).unwrap().as_str(),
        m.get(2).unwrap().as_str(),
        m.get(3).unwrap().as_str(),
    );
    let head = close(msl, whole.end(), b'(', b')');
    let body = head.and_then(|h| msl[h..].find('{').map(|p| p + h));
    let end = body.and_then(|b| close(msl, b + 1, b'{', b'}'));
    let (Some(head), Some(end)) = (head, end) else {
        return Err(format!("no body for {name}"));
    };
    let ps = params(&msl[whole.end()..head - 1]);
    let (mut decl, mut args) = (Vec::new(), Vec::new());
    for p in &ps {
        let d = RE_PARAM_ATTR.replace(p, "").to_string();
        let a = RE_LAST_WORD.captures(&d).map(|c| c[1].to_string());
        match a {
            Some(a) if !p.contains("function_constant") => {
                args.push(a);
                decl.push(d);
            }
            _ => {
                return Err(format!(
                    "a parameter of {name}: {}",
                    p.chars().take(80).collect::<String>()
                ));
            }
        }
    }
    Ok(Entry {
        attr: attr.to_string(),
        ret: ret.to_string(),
        name: name.to_string(),
        params: ps,
        decl: decl.join(", "),
        args: args.join(", "),
        start: whole.start(),
        head,
        end,
    })
}

/// `entries(msl)`: the fragment entry points in source order; Err on a shape it does not know
pub fn entries(msl: &str) -> Result<Vec<Entry>, String> {
    RE_ENTRY
        .captures_iter(msl)
        .map(|m| build(msl, &m))
        .collect()
}

/// `entry(msl, name)`: one entry point, None when there is none (the others are not read)
pub fn entry(msl: &str, name: &str) -> Result<Option<Entry>, String> {
    for m in RE_ENTRY.captures_iter(msl) {
        if m.get(3).unwrap().as_str() == name {
            return build(msl, &m).map(Some);
        }
    }
    Ok(None)
}

/// `body(msl)`: dm_body_main0's definition: (its start, the end of its parameter list, the end of its body)
pub fn body(msl: &str) -> Option<(usize, usize, usize)> {
    let m = RE_BODY.find(msl)?;
    let head = close(msl, m.end(), b'(', b')')?;
    let o = msl[head..].find('{')? + head;
    let end = close(msl, o + 1, b'{', b'}')?;
    Some((m.start(), head, end))
}

pub fn calling(e: &Entry, name: &str, ret: &str, tail: &str) -> String {
    format!(
        "{}fragment {} {}({})\n{{\n    main0_out out = dm_body_main0({});\n    {}\n}}\n",
        e.attr,
        ret,
        name,
        e.params.join(", "),
        e.args,
        tail
    )
}

pub fn shared_body(msl: &str) -> Option<String> {
    if let Some(m) = RE_BODY.captures(msl) {
        let w = m.get(0).unwrap();
        return Some(if m.get(1).is_some() {
            msl.to_string()
        } else {
            format!("{}{}{}", &msl[..w.start()], BODY, &msl[w.end()..])
        });
    }
    let e = entry(msl, "main0").ok()??;
    if e.ret != "main0_out" {
        return None;
    }
    Some(
        format!("{}{}{})", &msl[..e.start], BODY, e.decl)
            + &msl[e.head..e.end]
            + "\n"
            + &calling(&e, "main0", "main0_out", "return out;")
            + &msl[e.end..],
    )
}

// ------------------------------------------------------------------------------------------ alpha
re!(RE_STRUCT_MAIN0_OUT, r"(?s)struct main0_out\s*\{(.*?)\};");
re!(RE_COLOR0_ANY, r"\[\[color\(0\)\]\]");
re!(
    RE_COLOR0,
    r"(?m)^\s*(float4|half4|uint4)\s+(\w+)\s*\[\[color\(0\)(?:, index\(0\))?(?:, function_constant\((\w+)\))?\]\];"
);

fn colour0(msl: &str, strukt: &str) -> Vec<(String, String, Option<String>)> {
    // one compiled pattern per struct name and thread (`rx::cached`), not one per call
    let Ok(r) = super::rx::cached(&format!(
        r"(?s)struct {}\s*\{{(.*?)\}};",
        regex::escape(strukt)
    )) else {
        return vec![];
    };
    let Some(st) = r.captures(msl) else {
        return vec![];
    };
    RE_COLOR0
        .captures_iter(st.get(1).unwrap().as_str())
        .map(|m| {
            (
                m[1].to_string(),
                m[2].to_string(),
                m.get(3)
                    .map(|g| g.as_str().to_string())
                    .filter(|s| !s.is_empty()),
            )
        })
        .collect()
}

fn alpha_refused(msl: &str) -> Option<String> {
    let Some(st) = RE_STRUCT_MAIN0_OUT.captures(msl) else {
        return Some("no colour output".into());
    };
    let tys: Vec<String> = colour0(msl, "main0_out").into_iter().map(|c| c.0).collect();
    if tys.is_empty() || tys == ["uint4"] {
        return Some(if RE_COLOR0_ANY.is_match(st.get(1).unwrap().as_str()) {
            "colour 0 has no float alpha".into()
        } else {
            "no colour 0 output".into()
        });
    }
    if msl.contains(&format!("[[buffer({})]]", abi::ALPHA_BUFFER)) {
        return Some(format!("fragment buffer {} is taken", abi::ALPHA_BUFFER));
    }
    None
}

fn alpha_entry(msl: &str, e: &Entry) -> Option<String> {
    let cols = colour0(msl, &e.ret);
    if cols.is_empty() {
        return None;
    }
    let read = |ty: &str, nm: &str| {
        if ty == "uint4" {
            format!("as_type<float>(dm_r.{nm}.w)")
        } else {
            format!("float(dm_r.{nm}.w)")
        }
    };
    let alpha: Vec<String> = cols
        .iter()
        .map(|(ty, nm, fc)| match fc {
            Some(fc) => format!("if ({fc}) dm_a = {};", read(ty, nm)),
            None => format!("dm_a = {};", read(ty, nm)),
        })
        .collect();
    let mut ps = e.params.clone();
    ps.push(format!(
        "constant dm_alpha_state& dm_alpha [[buffer({})]]",
        abi::ALPHA_BUFFER
    ));
    Some(format!(
        "{}fragment {} {}_a({})\n{{\n    {} dm_r = dm_body_{}({});\n    float dm_a = 1.0f; {}\n    if (!dm_alpha_pass(dm_a, dm_alpha)) discard_fragment();\n    return dm_r;\n}}\n",
        e.attr,
        e.ret,
        e.name,
        ps.join(", "),
        e.ret,
        e.name,
        e.args,
        alpha.join(" ")
    ))
}

// ------------------------------------------------------------------------------------------ logic
re!(RE_ATTACH_READ, r"\[\[color\(\d+\)\]\]\s*[,)]");
re!(
    RE_LOGIC_OUT,
    r"(?m)^\s*((?:uint|float)[234]?)\s+(\w+)\s*\[\[color\((\d+)\)(?:, function_constant\((\w+)\))?\]\];"
);

fn logic_refused(msl: &str) -> Option<String> {
    if msl.contains(&format!("[[buffer({})]]", abi::LOGIC_BUFFER)) {
        return Some(format!("fragment buffer {} is taken", abi::LOGIC_BUFFER));
    }
    if RE_ATTACH_READ.is_match(msl) {
        return Some("an entry point already reads an attachment".into());
    }
    None
}

fn logic_constants(targets: &BTreeSet<i64>) -> String {
    targets
        .iter()
        .map(|n| {
            format!(
                "constant bool dm_lfb{n} [[function_constant({})]];\n",
                abi::LOGIC_FC + n
            )
        })
        .collect()
}

fn logic_entry(msl: &str, e: &Entry) -> Option<(String, BTreeSet<i64>)> {
    if e.name != "main0" && e.name != "main0_u" {
        return None;
    }
    let r = super::rx::cached(&format!(
        r"(?s)struct {}\s*\{{(.*?)\}};",
        regex::escape(&e.ret)
    ))
    .ok()?;
    let st = r.captures(msl)?;
    let outs: Vec<(String, String, i64, Option<String>)> = RE_LOGIC_OUT
        .captures_iter(st.get(1).unwrap().as_str())
        .map(|m| {
            (
                m[1].to_string(),
                m[2].to_string(),
                m[3].parse().unwrap_or(0),
                m.get(4)
                    .map(|g| g.as_str().to_string())
                    .filter(|s| !s.is_empty()),
            )
        })
        .collect();
    if outs.is_empty() {
        return None;
    }
    let scalar = |ty: &str| {
        if ty.starts_with("uint") {
            "uint"
        } else {
            "float"
        }
    };
    let on = |n: i64, fc: &Option<String>| match fc {
        Some(fc) => format!("dm_lfb{n}_{fc}"),
        None => format!("dm_lfb{n}"),
    };
    let consts: String = outs
        .iter()
        .filter(|o| o.3.is_some())
        .map(|(_, _, n, fc)| {
            format!(
                "constant bool {} = dm_lfb{n} && {};\n",
                on(*n, fc),
                fc.as_ref().unwrap()
            )
        })
        .collect();
    let fetch: Vec<String> = outs
        .iter()
        .map(|(ty, _, n, fc)| {
            format!(
                "{}4 {} [[color({n}), function_constant({})]]",
                scalar(ty),
                on(*n, fc).replace("dm_lfb", "dm_fb"),
                on(*n, fc)
            )
        })
        .collect();
    let mut apply = Vec::new();
    for (ty, nm, n, fc) in &outs {
        let s = scalar(ty);
        let width: usize = if ty.len() > s.len() {
            ty[s.len()..].parse().unwrap_or(1)
        } else {
            1
        };
        let zero = if s == "uint" { "0u" } else { "0.0f" };
        let pad = format!(", {zero}").repeat(4 - width);
        let sw = if width < 4 {
            format!(".{}", &"xyzw"[..width])
        } else {
            String::new()
        };
        apply.push(format!(
            "if ({}) dm_r.{nm} = {ty}({}({s}4(dm_r.{nm}{pad}), {}, dm_lop, {n}u){sw});",
            on(*n, fc),
            if s == "uint" {
                "dm_logic"
            } else {
                "dm_logic_norm"
            },
            on(*n, fc).replace("dm_lfb", "dm_fb")
        ));
    }
    let mut ps = e.params.clone();
    ps.extend(fetch);
    ps.push(format!(
        "constant dm_logic_state& dm_lop [[buffer({})]]",
        abi::LOGIC_BUFFER
    ));
    let text = format!(
        "{consts}{}fragment {} {}_l({})\n{{\n    {} dm_r = dm_body_{}({});\n    {}\n    return dm_r;\n}}\n",
        e.attr,
        e.ret,
        e.name,
        ps.join(", "),
        e.ret,
        e.name,
        e.args,
        apply.join(" ")
    );
    Some((text, outs.iter().map(|o| o.2).collect()))
}

// ------------------------------------------------------------------------------------------ unit
#[derive(Debug, Default, Clone)]
pub struct UnitInfo {
    /// entry point names of the alpha test / logic operation entry points
    pub alpha: Vec<String>,
    pub logic: Vec<String>,
    /// why there is no unit
    pub why: Option<String>,
}

/// `unit(msl)` -> (the compile unit, info); None unit with `info.why`
pub fn unit(msl: &str) -> (Option<String>, UnitInfo) {
    let es = match entries(msl) {
        Ok(e) => e,
        Err(e) => {
            return (
                None,
                UnitInfo {
                    why: Some(e),
                    ..Default::default()
                },
            );
        }
    };
    if es.is_empty() || es[0].name != "main0" {
        return (
            None,
            UnitInfo {
                why: Some("no fragment entry point main0".into()),
                ..Default::default()
            },
        );
    }
    let (no_alpha, no_logic) = (alpha_refused(msl), logic_refused(msl));
    let mut alpha: Vec<(String, Option<String>)> = Vec::new();
    let mut no_alpha = no_alpha;
    if no_alpha.is_none() {
        for e in &es {
            alpha.push((format!("{}_a", e.name), alpha_entry(msl, e)));
        }
    }
    if !alpha.iter().all(|a| a.1.is_some()) {
        alpha.clear();
        no_alpha = Some("an entry point without a colour 0".into());
    }
    let mut logic: Vec<(String, (String, BTreeSet<i64>))> = Vec::new();
    for e in &es {
        if no_logic.is_some() {
            continue;
        }
        if let Some(o) = logic_entry(msl, e) {
            logic.push((format!("{}_l", e.name), o));
        }
    }
    if alpha.is_empty() && logic.is_empty() {
        let why = no_alpha
            .or(no_logic)
            .unwrap_or_else(|| "no colour output".into());
        return (
            None,
            UnitInfo {
                why: Some(why),
                ..Default::default()
            },
        );
    }
    let mut thin: Vec<String> = alpha.iter().filter_map(|a| a.1.clone()).collect();
    thin.extend(logic.iter().map(|o| o.1.0.clone()));
    let targets: BTreeSet<i64> = logic.iter().flat_map(|o| o.1.1.iter().copied()).collect();
    let shared = RE_BODY.is_match(msl);
    let mut src = String::new();
    let mut at = es[0].start;
    for e in &es {
        // `msl[at:e.start]`: two entry points can share a body (a header without one is followed by the next entry point's: both find its brace), and then `at` is past
        // `e.start`, where the Python's slice is empty (this slice panicked: found by the before / after harness of 2026-11-21 on mutated fixtures)
        src.push_str(msl.get(at..e.start).unwrap_or(""));
        if !(shared && e.name == "main0") {
            src.push_str(&format!("static {} dm_body_{}({})", e.ret, e.name, e.decl));
            src.push_str(&msl[e.head..e.end]);
        }
        at = e.end;
    }
    let mut out = String::with_capacity(msl.len() * 2);
    out.push_str(&msl[..es[0].start]);
    out.push_str(texts::ALPHA_HELPER);
    out.push_str(texts::LOGIC_HELPER);
    out.push_str(&logic_constants(&targets));
    out.push_str(&src);
    out.push_str(&msl[at..]);
    out.push('\n');
    for e in &es {
        out.push_str(&format!(
            "{}fragment {} {}({})\n{{\n    return dm_body_{}({});\n}}\n",
            e.attr,
            e.ret,
            e.name,
            e.params.join(", "),
            e.name,
            e.args
        ));
    }
    for t in thin {
        out.push_str(&t);
    }
    (
        Some(out),
        UnitInfo {
            alpha: alpha.into_iter().map(|a| a.0).collect(),
            logic: logic.into_iter().map(|l| l.0).collect(),
            why: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_entry_points_sharing_a_body_do_not_panic() {
        // a header without a body, then a whole entry point: both read the second one's body, so the first one ends past the second one's start (`msl[at:e.start]` is
        // then empty in the Python; the Rust slice panicked)
        let msl = "struct main0_out\n{\n    float4 m_3 [[color(0)]];\n};\n\nfragment main0_out main0(main0_in in [[stage_in]], uint pid [[primitive_id]])\nfragment main0_out main0(main0_in in [[stage_in]], uint pid [[primitive_id]])\n{\n    main0_out out = {};\n    return out;\n}\n";
        let es = entries(msl).unwrap();
        assert_eq!(es.len(), 2);
        assert!(es[0].end > es[1].start);
        let (u, _) = unit(msl);
        let u = u.expect("a unit");
        // the shared body is written for each entry point, as the Python's string concatenation does
        assert_eq!(u.matches("static main0_out dm_body_main0(").count(), 2);
    }
}
