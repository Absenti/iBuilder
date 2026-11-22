//! dmsh_entry.fs_uint / uint_entry / fs_dual: the integer-output and dual-source entry points of a pixel shader.
use crate::blend::out_struct;
use crate::side;
use crate::templates as t;
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

fn uty(ty: &str) -> String {
    format!("u{}", ty.replace("float", "int"))
}

/// `color\((\d+)\)` in an attribute
fn color_index(at: &str) -> Option<u32> {
    let p = at.find("color(")?;
    let rest = &at[p + 6..];
    let e = rest.bytes().take_while(|c| c.is_ascii_digit()).count();
    if e == 0 || !rest[e..].starts_with(')') {
        return None;
    }
    rest[..e].parse().ok()
}

/// dmsh_entry.uint_entry
pub fn uint_entry(src: &str) -> Option<String> {
    let st = out_struct(src)?;
    let e = side::entry(src, "main0").ok().flatten()?;
    let mem = out_members(st);
    let is_col = |ty: &str, at: &str| at.contains("color(") && ty.starts_with("float");
    let mut cols: Vec<(&str, &str)> = Vec::new();
    for (ty, nm, at) in &mem {
        if is_col(ty, at) {
            if let Some(c) = cols.iter_mut().find(|(n, _)| n == nm) {
                c.1 = ty;
            } else {
                cols.push((nm, ty));
            }
        }
    }
    let coltype = |nm: &str| cols.iter().find(|(n, _)| *n == nm).map(|(_, t)| *t);
    // col = {nm: index} for the members in cols
    let colidx = |nm: &str, at: &str| -> u32 {
        let _ = nm;
        color_index(at).unwrap_or(0)
    };
    let mut idx: Vec<(&str, u32)> = Vec::new();
    for (_, nm, at) in &mem {
        if coltype(nm).is_some() {
            let v = colidx(nm, at);
            if let Some(c) = idx.iter_mut().find(|(n, _)| n == nm) {
                c.1 = v;
            } else {
                idx.push((nm, v));
            }
        }
    }
    let col = |nm: &str| idx.iter().find(|(n, _)| *n == nm).map(|(_, v)| *v).unwrap();
    let mut set: Vec<u32> = idx.iter().map(|(_, v)| *v).collect();
    set.sort_unstable();
    set.dedup();
    let mut consts = String::new();
    for n in &set {
        consts.push_str(&format!("constant bool dm_ui{n} [[function_constant({})]];\nconstant bool dm_uf{n} = !dm_ui{n};\n", t::UINT_FC + n));
    }
    let mut s = String::from("struct main0_out_u\n{\n");
    for (ty, nm, at) in &mem {
        if coltype(nm).is_some() {
            let a = &at[..at.len() - 2];
            let c = col(nm);
            s.push_str(&format!("    {ty} {nm} {a}, function_constant(dm_uf{c})]];\n    {} {nm}_ui {a}, function_constant(dm_ui{c})]];\n", uty(ty)));
        } else {
            s.push_str(&format!("    {ty} {nm} {at};\n"));
        }
    }
    s.push_str("};\n\n");
    let mut conv = String::from("main0_out_u r = {};\n");
    for (_, nm, _) in &mem {
        if let Some(cty) = coltype(nm) {
            let c = col(nm);
            conv.push_str(&format!(
                "    if (dm_ui{c}) r.{nm}_ui = as_type<{}>(out.{nm}); else r.{nm} = out.{nm};\n",
                uty(cty)
            ));
        } else {
            conv.push_str(&format!("    r.{nm} = out.{nm};\n"));
        }
    }
    conv.push_str("    return r;");
    Some(format!(
        "\n{consts}\n{s}{}",
        side::calling(&e, "main0_u", "main0_out_u", &conv)
    ))
}

/// `out\.(\w+)(?:\.\w+)? = ([^;]+);` matches: (start, name range, value range)
type Span = (usize, usize);

fn stores(src: &str) -> Vec<(usize, Span, Span)> {
    let b = src.as_bytes();
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"out.", from) {
        from = p + 1;
        let n0 = p + 4;
        let n1 = word_end(b, n0);
        if n1 == n0 {
            continue;
        }
        let mut e = n1;
        if e < b.len() && b[e] == b'.' {
            let w1 = word_end(b, e + 1);
            if w1 > e + 1 {
                e = w1;
            } else {
                continue;
            }
        }
        if !starts(b, e, " = ") {
            continue;
        }
        let v0 = e + 3;
        let Some(rel) = memchr::memchr(b';', &b[v0..]) else {
            continue;
        };
        if rel == 0 {
            continue;
        }
        v.push((p, (n0, n1), (v0, v0 + rel)));
        from = v0 + rel + 1;
    }
    v
}

/// locals given integer bits: `\b(_\d+(?:\[\d+\])?) = as_type<float\d?>\(`
fn bits_set(src: &str) -> Vec<&str> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    let needle = b" = as_type<float";
    while let Some(p) = find_b(b, needle, from) {
        from = p + 1;
        let mut i = p + needle.len();
        if i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if !starts(b, i, ">(") {
            continue;
        }
        let mut k = p;
        if k > 0 && b[k - 1] == b']' {
            let mut j = k - 1;
            while j > 0 && b[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j == k - 1 || j == 0 || b[j - 1] != b'[' {
                continue;
            }
            k = j - 1;
        }
        let mut d = k;
        while d > 0 && b[d - 1].is_ascii_digit() {
            d -= 1;
        }
        if d == k || d == 0 || b[d - 1] != b'_' {
            continue;
        }
        let s0 = d - 1;
        if in_word(b, s0) {
            continue;
        }
        out.push(&src[s0..p]);
    }
    out
}

/// flat inputs: `\b(\w+) \[\[user\(locn\d+\), flat\]\]`
fn flat_set(src: &str) -> Vec<&str> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    let needle = b" [[user(locn";
    while let Some(p) = find_b(b, needle, from) {
        from = p + 1;
        let d0 = p + needle.len();
        let d1 = digits_end(b, d0);
        if d1 == d0 || !starts(b, d1, "), flat]]") {
            continue;
        }
        let mut k = p;
        while k > 0 && is_word(b[k - 1]) {
            k -= 1;
        }
        if k == p {
            continue;
        }
        out.push(&src[k..p]);
    }
    out
}

/// `\bfloat\d? NAME = (in\.\w+(?:\.\w+)?);` -> group 1
fn local_in<'a>(src: &'a str, w: &str) -> Option<&'a str> {
    let b = src.as_bytes();
    let needle = format!(" {w} = in.");
    let mut from = 0;
    while let Some(p) = find_b(b, needle.as_bytes(), from) {
        from = p + 1;
        // before: `float` + optional digit, with \b before it
        let mut k = p;
        if k > 0 && b[k - 1].is_ascii_digit() {
            k -= 1;
        }
        let ok = k >= 5 && &b[k - 5..k] == b"float" && !in_word(b, k - 5);
        // `float\d?` with the optional digit absent: the char before ` w` must be `t`; with a digit, it is the digit
        if !ok {
            continue;
        }
        let g0 = p + needle.len() - 3; // start of `in.`
        let a = g0 + 3;
        let a1 = word_end(b, a);
        if a1 == a {
            continue;
        }
        let mut e = a1;
        if e < b.len() && b[e] == b'.' {
            let w1 = word_end(b, e + 1);
            if w1 > e + 1 {
                e = w1;
            }
        }
        if e < b.len() && b[e] == b';' {
            return Some(&src[g0..e]);
        }
        // the optional `.\w+` may also be absent when it is not followed by `;`: nothing else to try
    }
    None
}

/// `in\.(\w+)(?:\.\w+)?` fullmatch -> group 1
fn in_name(s: &str) -> Option<&str> {
    let r = s.strip_prefix("in.")?;
    let b = r.as_bytes();
    let e = word_end(b, 0);
    if e == 0 {
        return None;
    }
    if e == b.len() {
        return Some(&r[..e]);
    }
    if b[e] != b'.' {
        return None;
    }
    let e2 = word_end(b, e + 1);
    if e2 > e + 1 && e2 == b.len() {
        Some(&r[..e])
    } else {
        None
    }
}

fn is_constant(w: &str) -> bool {
    // `-?[0-9.]+f?|float\d\([-0-9., ]*\)`
    let a = w.strip_prefix('-').unwrap_or(w);
    let a = a.strip_suffix('f').unwrap_or(a);
    if !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return true;
    }
    // `-?[0-9.]+f?` with the `f` optional: also try without stripping when the text ends with `f` inside digits only handled above
    if let Some(r) = w.strip_prefix("float") {
        let rb = r.as_bytes();
        if rb.len() >= 3 && rb[0].is_ascii_digit() && rb[1] == b'(' && rb[rb.len() - 1] == b')' {
            return rb[2..rb.len() - 1]
                .iter()
                .all(|&c| c.is_ascii_digit() || matches!(c, b'-' | b'.' | b',' | b' '));
        }
    }
    false
}

/// dmsh_entry.fs_uint
pub fn fs_uint(src: &str) -> Option<String> {
    // m = re.search(r'^.*\bfragment\s+main0_out\s+main0\s*\(', src, re.M)
    let b = src.as_bytes();
    let mut have = false;
    let mut from = 0;
    while let Some(p) = find_b(b, b"fragment", from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let mut i = skip_space(b, p + 8);
        if i == p + 8 {
            continue;
        }
        if !starts(b, i, "main0_out") {
            continue;
        }
        let j = skip_space(b, i + 9);
        if j == i + 9 {
            continue;
        }
        i = j;
        if !starts(b, i, "main0") {
            continue;
        }
        let q = skip_space(b, i + 5);
        if q < b.len() && b[q] == b'(' {
            have = true;
            break;
        }
    }
    let st = out_struct(src);
    if !have || st.is_none() {
        return None;
    }
    let st = st.unwrap();
    let mem = out_members(st);
    let cols: Vec<&str> = mem
        .iter()
        .filter(|(ty, _, at)| at.contains("color(") && ty.starts_with("float"))
        .map(|(_, n, _)| *n)
        .collect();
    let writes: Vec<&str> = stores(src)
        .into_iter()
        .filter(|(p, n, _)| cols.contains(&&src[n.0..n.1]) && !in_word(b, *p))
        .map(|(_, _, v)| strip(&src[v.0..v.1]))
        .collect();
    let bits = bits_set(src);
    let flat = if contains(src, ", flat]]") {
        flat_set(src)
    } else {
        Vec::new()
    };
    let passed = |w: &str| -> bool {
        let is_word_str = !w.is_empty() && w.bytes().all(is_word);
        let d = if is_word_str { local_in(src, w) } else { None };
        let target = d.unwrap_or(w);
        in_name(target).map(|n| flat.contains(&n)).unwrap_or(false)
    };
    let any_bits = writes
        .iter()
        .any(|w| w.starts_with("as_type<float") || bits.contains(w));
    if !any_bits && !(!writes.is_empty() && writes.iter().all(|w| is_constant(w) || passed(w))) {
        return None;
    }
    uint_entry(src)
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
