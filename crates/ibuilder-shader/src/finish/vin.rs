//! `dmsh_vin.py`: the shadow-input library `<key>_bv` of a vertex shader with integer inputs, and `dmsh_pull.py`: the pulling library `<key>_bp` (no `[[stage_in]]`, the inputs read through a layout block).
use super::pyjson::J;
use super::texts;
use regex::Regex;
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(RE_MAIN0_IN, r"(?s)\nstruct main0_in\n\{\n(.*?)\n\};");
re!(
    RE_IN_MEMBER,
    r"^    (float4|uint4|int4) (m_\d+(?:_\d+)*) \[\[attribute\((\d+)\)\]\];$"
);

fn members(body: &str) -> Result<Vec<(String, String, i64)>, String> {
    let mut v = Vec::new();
    for ln in body.split('\n') {
        let Some(m) = RE_IN_MEMBER.captures(ln) else {
            return Err(format!(
                "an input declaration not understood: {}",
                ln.trim().chars().take(60).collect::<String>()
            ));
        };
        v.push((
            m[1].to_string(),
            m[2].to_string(),
            m[3].parse().unwrap_or(0),
        ));
    }
    Ok(v)
}

const SLOT0: i64 = 16;
const SLOT_END: i64 = 31;

/// `dmsh_vin.variant(msl)` -> (new MSL, shadows) or the reason (the Python's `(None, reason)`)
pub fn vin_variant(msl: &str) -> Result<(String, Vec<J>), String> {
    let Some(m) = RE_MAIN0_IN.captures(msl) else {
        return Err(if msl.contains("[[stage_in]]") {
            "no main0_in struct".into()
        } else {
            "no integer input".into()
        });
    };
    let (whole, g1) = (m.get(0).unwrap(), m.get(1).unwrap().as_str());
    let members = members(g1)?;
    let ints: Vec<&(String, String, i64)> = members.iter().filter(|x| x.0 != "float4").collect();
    if ints.is_empty() {
        return Err("no integer input".into());
    }
    if members.iter().any(|x| x.2 >= SLOT0) {
        return Err("an input at attribute 16 or above".into());
    }
    let (mut slot, mut shadows, mut extra): (i64, Vec<J>, Vec<String>) =
        (SLOT0, Vec::new(), Vec::new());
    for (_, n, l) in &ints {
        if slot >= SLOT_END {
            break;
        }
        extra.push(format!("    float4 {n}_vf [[attribute({slot})]];"));
        shadows.push(J::obj().with("location", *l).with("float", slot));
        slot += 1;
    }
    for (e, (t, n, _)) in shadows.iter_mut().zip(ints.iter()) {
        if slot >= SLOT_END {
            break;
        }
        let o = if t == "uint4" { "int4" } else { "uint4" };
        extra.push(format!("    {o} {n}_vo [[attribute({slot})]];"));
        e.set(if o == "int4" { "sint" } else { "uint" }, slot);
        slot += 1;
    }
    let mut covered: Vec<(i64, J)> = Vec::new();
    for e in &shadows {
        let l = e.get("location").and_then(|x| x.as_i64()).unwrap_or(-1);
        if let Some(c) = covered.iter_mut().find(|(k, _)| *k == l) {
            c.1 = e.clone();
        } else {
            covered.push((l, e.clone()));
        }
    }
    let head = format!(
        "{}\n\nstruct main0_in_v\n{{\n{}\n{}\n}};",
        &msl[..whole.end()],
        g1,
        extra.join("\n")
    );
    let mut body = msl[whole.end()..].to_string();
    if let Some(cut) = body.find("\nvertex void main0_tf(") {
        body.truncate(cut);
        body.push('\n');
    }
    let mut pro = String::from("    main0_in in = {};\n");
    for (t, n, l) in &members {
        let ors = match covered.iter().find(|(k, _)| k == l) {
            None => String::new(),
            Some((_, e)) => {
                format!(" | as_type<{t}>(dm_in.{n}_vf)")
                    + &if e.has("sint") || e.has("uint") {
                        format!(" | as_type<{t}>(dm_in.{n}_vo)")
                    } else {
                        String::new()
                    }
            }
        };
        pro.push_str(&format!("    in.{n} = dm_in.{n}{ors};\n"));
    }
    for entry in ["main0", "main0_z"] {
        let h = super::rx::cached(&format!(
            r"\nvertex main0_out {entry}\(main0_in in \[\[stage_in\]\]([^\n]*)\)\n\{{\n"
        ))
        .map_err(|e| e.to_string())?;
        let Some(hdr) = h.captures(&body) else {
            if entry == "main0" {
                return Err("main0 signature not recognized".into());
            }
            continue;
        };
        let w = hdr.get(0).unwrap();
        let new = format!(
            "\nvertex main0_out {entry}(main0_in_v dm_in [[stage_in]]{})\n{{\n{pro}",
            &hdr[1]
        );
        body = format!("{}{}{}", &body[..w.start()], new, &body[w.end()..]);
    }
    Ok((format!("{head}{body}"), shadows))
}

// ------------------------------------------------------------------------------------------ pulling
const SLOT: i64 = 16;
const WL_LINE: i64 = 15;
const WL_ATTRS: [&str; 7] = [
    "buffer",
    "texture",
    "sampler",
    "vertex_id",
    "instance_id",
    "base_instance",
    "base_vertex",
];

re!(RE_UINT_VID, r"uint (\w+) \[\[vertex_id\]\]");
re!(RE_UINT_IID, r"uint (\w+) \[\[instance_id\]\]");
re!(RE_UINT_BI, r"uint (\w+) \[\[base_instance\]\]");
re!(
    RE_POS_MEMBER,
    r"struct main0_out\n\{[^}]*?float4 (\w+) \[\[position"
);
re!(RE_VERTEX_LINE, r"^vertex \S+ (\w+)\(");
re!(RE_WL_PARAM, r"^(.*?)(\w+) \[\[(\w+)(?:\(\d+\))?\]\]$");
re!(
    RE_MAIN0_OUT_BLOCK,
    r"(?s)\nstruct main0_out\n\{\n(.*?)\n\};"
);
re!(
    RE_WL_MEMBER,
    r"^    (\w+) (\w+)(?: \[\[([^\]]*)\]\])?(?: \[(\d+)\])?;$"
);

fn pull_params(rest: &str) -> Vec<String> {
    let (mut out, mut depth, mut cur) = (Vec::new(), 0i32, String::new());
    for ch in rest.chars() {
        if ch == '<' || ch == '(' {
            depth += 1;
        } else if ch == '>' || ch == ')' {
            depth -= 1;
        }
        if ch == ',' && depth == 0 {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `_wl_mix(msl)`
fn wl_mix(msl: &str) -> Option<String> {
    let m = RE_MAIN0_OUT_BLOCK.captures(msl)?;
    let mut body = String::new();
    for ln in m[1].split('\n') {
        let mm = RE_WL_MEMBER.captures(ln)?;
        let (t, n, attrs, count) = (
            &mm[1],
            &mm[2],
            mm.get(3).map(|g| g.as_str()).unwrap_or(""),
            mm.get(4).map(|g| g.as_str()),
        );
        if !(t.starts_with("float") || t.starts_with("half")) || attrs.contains("flat") {
            continue;
        }
        match count {
            Some(c) => body.push_str(&format!(
                "    for (int i = 0; i < {c}; i++) r.{n}[i] = mix(a.{n}[i], b.{n}[i], t);\n"
            )),
            None => body.push_str(&format!("    r.{n} = mix(a.{n}, b.{n}, t);\n")),
        }
    }
    Some(format!(
        "\nstatic inline main0_out dm_wl_mix(main0_out a, main0_out b, float t)\n{{\n    main0_out r = a;\n{body}    return r;\n}}\n"
    ))
}

/// `_wide_line(entry, params, fbody, pos, inn, bn)`
fn wide_line(
    entry: &str,
    params: &[String],
    fbody: &str,
    pos: &str,
    inn: &str,
    bn: &str,
) -> Option<String> {
    let (mut decl, mut args) = (Vec::new(), Vec::new());
    for p in params {
        let m = RE_WL_PARAM.captures(p)?;
        if !WL_ATTRS.contains(&&m[3]) {
            return None;
        }
        decl.push(format!("{}{}", &m[1], &m[2]));
        args.push(match &m[3] {
            "vertex_id" => "{v}".to_string(),
            "base_vertex" => "uint(dm_wl.base)".to_string(),
            _ => m[2].to_string(),
        });
    }
    let mut wparams: Vec<String> = params
        .iter()
        .filter(|p| !(p.ends_with("[[vertex_id]]") || p.ends_with("[[base_vertex]]")))
        .cloned()
        .collect();
    wparams.push("uint dm_wcorner [[vertex_id]]".into());
    wparams.push(format!("constant dm_wline& dm_wl [[buffer({WL_LINE})]]"));
    let call = |v: &str| {
        format!(
            "dm_wlb_{entry}(dm_wl_in(dm_lay, {v}, {inn}, {bn}), {})",
            args.iter()
                .map(|a| a.replace("{v}", v))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut s = format!(
        "\nstatic inline main0_out dm_wlb_{entry}(main0_in in, {})\n{{\n{fbody}\nvertex main0_out {entry}_wl({})\n{{\n",
        decl.join(", "),
        wparams.join(", ")
    );
    s.push_str("    bool dm_cut = false; uint3 dm_e = dm_wl_ends(dm_wl, dm_wcorner, dm_cut);\n");
    s.push_str(
        "    if (dm_cut) return main0_out{};   // a segment with a restart index: nothing drawn\n",
    );
    s.push_str(&format!(
        "    main0_out out = {}, dm_o = {};\n",
        call("dm_e.x"),
        call("dm_e.y")
    ));
    s.push_str(&format!(
        "    float4 dm_p = out.{pos}, dm_q = dm_o.{pos};\n"
    ));
    s.push_str("    float2 dm_t = dm_wl_cut(dm_p, dm_q, dm_wl);\n");
    s.push_str("    if (dm_t.x > 1.0f) return main0_out{};   // the segment is wholly behind the eye (or the near plane)\n");
    s.push_str("    if (dm_t.x > 0.0f) out = dm_wl_mix(out, dm_o, dm_t.x);\n");
    s.push_str(&format!(
        "    out.{pos} = dm_wl_offset(out.{pos}, mix(dm_q, dm_p, dm_t.y), dm_wl, dm_e.z);\n"
    ));
    s.push_str("    return out;\n}\n");
    Some(s)
}

/// `_drop_entries(msl, keep)`
fn drop_entries(msl: &str, keep: &[&str]) -> String {
    let lines: Vec<&str> = msl.split('\n').collect();
    let (mut out, mut i) = (Vec::new(), 0usize);
    while i < lines.len() {
        if let Some(m) = RE_VERTEX_LINE.captures(lines[i])
            && !keep.contains(&&m[1])
        {
            let mut j = i;
            while j < lines.len() && lines[j] != "}" {
                j += 1;
            }
            i = j + 1;
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }
    out.join("\n")
}

/// `dmsh_pull.variant(msl)` -> (new MSL, info) or the reason
pub fn pull_variant(msl: &str) -> Result<(String, J), String> {
    let Some(m) = RE_MAIN0_IN.captures(msl) else {
        return Err("no stage input".into());
    };
    let (whole, g1) = (m.get(0).unwrap(), m.get(1).unwrap().as_str());
    if msl.contains(&format!("[[buffer({SLOT})]]")) {
        return Err(format!("buffer {SLOT} is taken"));
    }
    let members = members(g1)?;
    if members.iter().any(|x| x.2 >= 16) {
        return Err("an input at attribute 16 or above".into());
    }
    let mut head = format!(
        "{}\nstruct main0_in\n{{\n{}\n}};",
        &msl[..whole.start()],
        members
            .iter()
            .map(|(t, n, _)| format!("    {t} {n};"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let mut body = drop_entries(&msl[whole.end()..], &["main0", "main0_z"]);
    let mut entries: Vec<&str> = Vec::new();
    let (mut wl, mut wle): (Vec<String>, Vec<&str>) = (Vec::new(), Vec::new());
    let pos = RE_POS_MEMBER.captures(msl).map(|c| c[1].to_string());
    for entry in ["main0", "main0_z"] {
        let h = super::rx::cached(&format!(
            r"\nvertex main0_out {entry}\(main0_in in \[\[stage_in\]\]((?:, [^\n]*)?)\)\n\{{\n"
        ))
        .map_err(|e| e.to_string())?;
        let Some(hdr) = h.captures(&body) else {
            if entry == "main0" {
                return Err("main0 signature not recognized".into());
            }
            continue;
        };
        let w = hdr.get(0).unwrap();
        let rest = hdr[1].to_string();
        let vid = RE_UINT_VID.captures(&rest).map(|c| c[1].to_string());
        let iid = RE_UINT_IID.captures(&rest).map(|c| c[1].to_string());
        let bi = RE_UINT_BI.captures(&rest).map(|c| c[1].to_string());
        let mut params = if !rest.is_empty() {
            pull_params(&rest[2..])
        } else {
            vec![]
        };
        params.push(format!("constant dm_vlayout& dm_lay [[buffer({SLOT})]]"));
        if vid.is_none() {
            params.push("uint dm_vid [[vertex_id]]".into());
        }
        if iid.is_none() {
            params.push("uint dm_iid [[instance_id]]".into());
        }
        if bi.is_none() {
            params.push("uint dm_bi [[base_instance]]".into());
        }
        let (vn, inn, bn) = (
            vid.unwrap_or_else(|| "dm_vid".into()),
            iid.unwrap_or_else(|| "dm_iid".into()),
            bi.unwrap_or_else(|| "dm_bi".into()),
        );
        let mut pro = String::from("    main0_in in;\n");
        for (t, n, l) in &members {
            pro.push_str(&format!(
                "    in.{n} = as_type<{t}>(dm_fetch(dm_lay, {l}u, {vn}, {inn}, {bn}));\n"
            ));
        }
        let Some(fn_end) = body[w.end()..].find("\n}\n").map(|p| p + w.end()) else {
            return Err(format!("end of {entry} not found"));
        };
        let fbody = body[w.end()..fn_end + 3].to_string();
        body = format!(
            "{}\nvertex main0_out {entry}({})\n{{\n{pro}{}",
            &body[..w.start()],
            params.join(", "),
            &body[w.end()..]
        );
        entries.push(entry);
        if let Some(p) = &pos
            && let Some(x) = wide_line(entry, &params, &fbody, p, &inn, &bn)
        {
            wl.push(x);
            wle.push(entry);
        }
    }
    let Some(ui) = head.find("using namespace metal;") else {
        return Err("no \"using namespace metal;\"".into());
    };
    let ui = ui + "using namespace metal;".len();
    let mix = if !wl.is_empty() { wl_mix(msl) } else { None };
    if mix.is_none() {
        wl.clear();
        wle.clear();
    }
    head = format!(
        "{}\n\n{}{}{}",
        &head[..ui],
        texts::PULL_FETCH,
        if !wl.is_empty() {
            texts::PULL_WL_MSL
        } else {
            ""
        },
        &head[ui..]
    );
    if let (false, Some(mix)) = (wl.is_empty(), &mix) {
        let mut first = String::from(
            "\nstatic inline main0_in dm_wl_in(constant dm_vlayout& dm_lay, uint dm_v, uint dm_i, uint dm_b)\n{\n    main0_in in;\n",
        );
        for (t, n, l) in &members {
            first.push_str(&format!(
                "    in.{n} = as_type<{t}>(dm_fetch(dm_lay, {l}u, dm_v, dm_i, dm_b));\n"
            ));
        }
        first.push_str("    return in;\n}\n");
        first.push_str(mix);
        wl.insert(0, first);
    }
    let mut info = J::obj()
        .with("entry", "main0")
        .with(
            "entry_z",
            if entries.contains(&"main0_z") {
                J::from("main0_z")
            } else {
                J::Null
            },
        )
        .with("slot", SLOT)
        .with(
            "inputs",
            J::Arr(members.iter().map(|m| J::Int(m.2)).collect()),
        );
    if wle.contains(&"main0") {
        info.set("entry_wl", "main0_wl");
        info.set(
            "entry_z_wl",
            if wle.contains(&"main0_z") {
                J::from("main0_z_wl")
            } else {
                J::Null
            },
        );
    }
    Ok((format!("{head}{body}{}", wl.concat()), info))
}
