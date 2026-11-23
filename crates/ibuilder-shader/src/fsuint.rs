//! dmsh_entry.fs_uint / typed_ops / fs_dual: the typed-output and dual-source entry points of a pixel shader.
use crate::blend::out_struct;
use crate::finish::{abi, texts, unit};
use crate::side;
use crate::text::*;

/// `^\s*(\w+)\s+(\w+)\s*(\[\[[^\]]*\]\])?;` per line -> (type, name, attribute or "")
pub fn out_members(body: &str) -> Vec<(&str, &str, &str)> {
    let mut v = Vec::new();
    for (a, e) in lines(body) {
        let l = &body[a..e];
        let b = l.as_bytes();
        let i = skip_space(b, 0);
        let t1 = word_end(b, i);
        if t1 == i {
            continue;
        }
        let j = skip_space(b, t1);
        if j == t1 {
            continue;
        }
        let n1 = word_end(b, j);
        if n1 == j {
            continue;
        }
        let k = skip_space(b, n1);
        if k < b.len() && b[k] == b';' {
            v.push((&l[i..t1], &l[j..n1], ""));
            continue;
        }
        if starts(b, k, "[[") {
            // `\[\[[^\]]*\]\]`: up to the first `]`, which must be followed by `]`
            if let Some(r) = memchr::memchr(b']', &b[k + 2..]) {
                let c = k + 2 + r;
                if starts(b, c, "]];") {
                    v.push((&l[i..t1], &l[j..n1], &l[k..c + 2]));
                }
            }
        }
    }
    v
}

/// `re.fullmatch(r'\[\[color\(([0-7])\)\]\]', at)` -> the colour
fn plain_color(at: &str) -> Option<u32> {
    let d = at.strip_prefix("[[color(")?.strip_suffix(")]]")?;
    (d.len() == 1 && matches!(d.as_bytes()[0], b'0'..=b'7'))
        .then(|| u32::from(d.as_bytes()[0] - b'0'))
}

/// `re.fullmatch(r'(float|uint|int)([234]?)', ty)` -> (scalar type, width suffix)
fn colour_type(ty: &str) -> Option<(&'static str, &str)> {
    let (k, rest) = if let Some(r) = ty.strip_prefix("float") {
        ("float", r)
    } else if let Some(r) = ty.strip_prefix("uint") {
        ("uint", r)
    } else {
        ("int", ty.strip_prefix("int")?)
    };
    matches!(rest, "" | "2" | "3" | "4").then_some((k, rest))
}

/// dmsh_entry.fs_uint: the typed-output entry point main0_u (with the helpers it uses), to append to the source with its body shared; None when main0 is not of a
/// shape `side::entry` knows, does not return main0_out, or has no colour output
pub fn fs_uint(src: &str) -> Option<String> {
    let st = out_struct(src);
    let e = side::entry(src, "main0").ok().flatten();
    let (Some(e), Some(st)) = (e, st) else {
        return None;
    };
    if e.ret != "main0_out" {
        return None;
    }
    let mem = out_members(st);
    // member -> (scalar type, width suffix, colour), as the Python's dict (a later member of the same name takes the earlier one's place)
    let mut col: Vec<(&str, (&str, &str, u32))> = Vec::new();
    for (ty, nm, at) in &mem {
        if let (Some(c), Some((k, w))) = (plain_color(at), colour_type(ty)) {
            match col.iter_mut().find(|(n, _)| n == nm) {
                Some(slot) => slot.1 = (k, w, c),
                None => col.push((nm, (k, w, c))),
            }
        }
    }
    if col.is_empty() {
        return None;
    }
    let col_of = |nm: &str| col.iter().find(|(n, _)| *n == nm).map(|(_, v)| *v);
    let mut ns: Vec<u32> = col.iter().map(|(_, (_, _, n))| *n).collect();
    ns.sort_unstable();
    ns.dedup();
    let mut consts = String::new();
    for n in &ns {
        consts.push_str(&format!(
            "constant bool dm_ui{n} [[function_constant({})]];\nconstant bool dm_si{n} [[function_constant({})]];\nconstant bool dm_uf{n} = !dm_ui{n} && !dm_si{n};\n",
            abi::UINT_FC + i64::from(*n),
            abi::SINT_FC + i64::from(*n)
        ));
    }
    const KINDS: [(&str, &str, &str); 3] = [
        ("float", "", "dm_uf"),
        ("uint", "_ui", "dm_ui"),
        ("int", "_si", "dm_si"),
    ];
    let mut members = String::new();
    for (ty, nm, at) in &mem {
        match col_of(nm) {
            None if at.is_empty() => members.push_str(&format!("    {ty} {nm};\n")),
            None => members.push_str(&format!("    {ty} {nm} {at};\n")),
            Some((_, w, n)) => {
                for (k, sfx, fc) in KINDS {
                    members.push_str(&format!(
                        "    {k}{w} {nm}{sfx} [[color({n}), function_constant({fc}{n})]];\n"
                    ));
                }
            }
        }
    }
    let mut stores = String::new();
    for (_, nm, _) in &mem {
        match col_of(nm) {
            None => stores.push_str(&format!("    r.{nm} = out.{nm};\n")),
            Some((k0, w, n)) => {
                let put = |k: &str, sfx: &str| {
                    if k == k0 {
                        format!("r.{nm}{sfx} = out.{nm};")
                    } else {
                        format!("r.{nm}{sfx} = as_type<{k}{w}>(out.{nm});")
                    }
                };
                stores.push_str(&format!(
                    "    if (dm_ui{n}) {} else if (dm_si{n}) {} else {}\n",
                    put("uint", "_ui"),
                    put("int", "_si"),
                    put("float", "")
                ));
            }
        }
    }
    let alpha = unit::alpha_test(src, "out");
    let logic = if unit::logic_refused(src).is_some() {
        None
    } else {
        unit::logic_parts(&members, "r", Some("dm_lo"))
    };
    let (mut helpers, mut params, mut pre, mut post) = (
        String::new(),
        e.params.clone(),
        String::new(),
        String::new(),
    );
    if let Some(a) = alpha {
        helpers.push_str(texts::ALPHA_HELPER);
        consts.push_str(&format!(
            "constant bool dm_at [[function_constant({})]];\n",
            abi::TYPED_ALPHA_FC
        ));
        params.push(format!(
            "constant dm_alpha_state& dm_alpha [[buffer({}), function_constant(dm_at)]]",
            abi::ALPHA_BUFFER
        ));
        pre = format!("    if (dm_at) {{ {a} }}\n");
    }
    if let Some(l) = logic {
        helpers.push_str(texts::LOGIC_HELPER);
        consts.push_str(&format!(
            "constant bool dm_lo [[function_constant({})]];\n",
            abi::TYPED_LOGIC_FC
        ));
        consts.push_str(&unit::logic_constants(&l.targets));
        consts.push_str(&l.consts);
        params.extend(l.fetch);
        params.push(format!(
            "constant dm_logic_state& dm_lop [[buffer({}), function_constant(dm_lo)]]",
            abi::LOGIC_BUFFER
        ));
        post = format!("    {}\n", l.apply.join(" "));
    }
    Some(format!(
        "\n{helpers}{consts}\nstruct main0_out_u\n{{\n{members}}};\n\n{}fragment main0_out_u main0_u({})\n{{\n    main0_out out = dm_body_main0({});\n{pre}    main0_out_u r = {{}};\n{stores}{post}    return r;\n}}\n",
        e.attr,
        params.join(", "),
        e.args
    ))
}

/// dmsh_entry.typed_ops: what the typed-output entry point `msl` (fs_uint's text) holds besides its outputs -> (alpha test, logic operation): the reflection's
/// `entry_u_alpha`, `entry_u_logic`
pub fn typed_ops(msl: &str) -> (bool, bool) {
    (
        msl.contains(&format!(
            "dm_at [[function_constant({})]]",
            abi::TYPED_ALPHA_FC
        )),
        msl.contains(&format!(
            "dm_lo [[function_constant({})]]",
            abi::TYPED_LOGIC_FC
        )),
    )
}

/// dmsh_entry.fs_dual (the profile's dual_want.tsv lists the shaders it applies to)
pub fn fs_dual(src: &str) -> Option<String> {
    let st = out_struct(src)?;
    let e = side::entry(src, "main0").ok().flatten()?;
    let mem = out_members(st);
    let mut ats: Vec<&str> = mem
        .iter()
        .filter(|(_, _, at)| at.contains("color("))
        .map(|(_, _, at)| *at)
        .collect();
    ats.sort_unstable();
    if ats != ["[[color(0)]]", "[[color(1)]]"] {
        return None;
    }
    let mut s = String::from("struct main0_out_d\n{\n");
    for (ty, nm, at) in &mem {
        s.push_str(&format!(
            "    {ty} {nm} {};\n",
            if *at == "[[color(1)]]" {
                "[[color(0), index(1)]]"
            } else {
                at
            }
        ));
    }
    s.push_str("};\n\n");
    let ret = format!(
        "return main0_out_d{{{}}};",
        mem.iter()
            .map(|(_, nm, _)| format!("out.{nm}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Some(format!(
        "\n{s}{}",
        side::calling(&e, "main0_d", "main0_out_d", &ret)
    ))
}
