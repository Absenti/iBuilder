//! The compute-only rewrites of dmsh_rewrite.py: `first_store` (the first invocation's stores leave the shader) and `tg_spill` (threadgroup memory past Metal's 32 KB in a device buffer).
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};

// ------------------------------------------------------------------------------------------------------------------------------ first_store

/// `float (\w+) = as_type<float>\(<what>\);\s*` at `c`: the captured name; the cursor moves past the whitespace
fn float_line<'a>(c: &mut Cur<'a>, what: &str) -> Option<&'a str> {
    if !c.lit("float ") {
        return None;
    }
    let n = c.word()?;
    if !(c.lit(" = as_type<float>(") && c.lit(what) && c.lit(");")) {
        return None;
    }
    c.ws0();
    Some(n)
}

/// `<ty> (\w+) = <pieces>;\s*`: the pieces are literal text (a back-reference is the captured name's text); the last one ends with the `;`
fn decl_line<'a>(c: &mut Cur<'a>, ty: &str, pieces: &[&str]) -> Option<&'a str> {
    if !c.lit(ty) || !c.lit(" ") {
        return None;
    }
    let n = c.word()?;
    if !c.lit(" = ") {
        return None;
    }
    for p in pieces {
        if !c.lit(p) {
            return None;
        }
    }
    c.ws0();
    Some(n)
}

/// `int (\w+) = as_type<int>\(<src_name>\) << (\d+);\s*` -> (name, digits)
fn shift_line<'a>(c: &mut Cur<'a>, src_name: &str) -> Option<(&'a str, &'a str)> {
    if !c.lit("int ") {
        return None;
    }
    let n = c.word()?;
    if !(c.lit(" = as_type<int>(") && c.lit(src_name) && c.lit(") << ")) {
        return None;
    }
    let d = c.digits()?;
    if !c.lit(";") {
        return None;
    }
    c.ws0();
    Some((n, d))
}

/// `1 << int(digits)` as Python computes it, None when it cannot equal a local size (past 63 bits)
fn shl_value(digits: &str) -> Option<i64> {
    let n: u64 = digits.parse().ok()?;
    if n >= 63 { None } else { Some(1i64 << n) }
}

/// FIRST_STORE at `p` (the start of its first `float`) -> (end of the match, the shift's digits, the guard variable)
fn first_store_1d(src: &str, p: usize) -> Option<(usize, &str, &str)> {
    let mut c = Cur::at(src, p);
    let g1 = float_line(&mut c, "gl_WorkGroupID.x")?;
    let g2 = float_line(&mut c, "gl_LocalInvocationID.x")?;
    let (g3, d4) = shift_line(&mut c, g1)?;
    let g5 = decl_line(&mut c, "int", &["~as_type<int>(", g2, ");"])?;
    let g6 = decl_line(&mut c, "int", &[g5, " + 1;"])?;
    let g7 = decl_line(&mut c, "bool", &[g6, " != ", g3, ";"])?;
    Some((c.i, d4, g7))
}

/// FIRST_STORE_2D at `p` -> (end of the match, group 7 digits (the y shift), group 12 digits (the x shift), the guard text: `!name` (group 20) or `name` (group 19))
fn first_store_2d(src: &str, p: usize) -> Option<(usize, &str, &str, String)> {
    let mut c = Cur::at(src, p);
    let g1 = float_line(&mut c, "gl_WorkGroupID.y")?;
    let g2 = float_line(&mut c, "gl_LocalInvocationID.y")?;
    let g3 = float_line(&mut c, "gl_LocalInvocationID.x")?;
    let g4 = float_line(&mut c, "gl_WorkGroupID.x")?;
    let g5 = float_line(&mut c, "gl_WorkGroupID.z")?;
    let (g6, d7) = shift_line(&mut c, g1)?;
    let g8 = decl_line(&mut c, "int", &["~as_type<int>(", g2, ");"])?;
    let g9 = decl_line(&mut c, "int", &[g8, " + 1;"])?;
    let g10 = decl_line(&mut c, "bool", &[g9, " != ", g6, ";"])?;
    let (g11, d12) = shift_line(&mut c, g4)?;
    let g13 = decl_line(&mut c, "int", &["~as_type<int>(", g3, ");"])?;
    let g14 = decl_line(&mut c, "int", &[g13, " + 1;"])?;
    let g15 = decl_line(&mut c, "bool", &[g14, " == ", g11, ";"])?;
    let g16 = decl_line(&mut c, "bool", &["!", g10, ";"])?;
    let g17 = decl_line(&mut c, "bool", &[g15, " && ", g16, ";"])?;
    // bool (\w+) = (?:as_type<int>\(\5\) == 0|0 == as_type<int>\(\5\));\s*
    let alt1 = format!("as_type<int>({g5}) == 0;");
    let alt2 = format!("0 == as_type<int>({g5});");
    let g18 = {
        let mut a = c;
        let mut r = None;
        for alt in [&alt1, &alt2] {
            let mut x = c;
            if x.lit("bool ")
                && let Some(n) = x.word()
                && x.lit(" = ")
                && x.lit(alt)
            {
                x.ws0();
                a = x;
                r = Some(n);
                break;
            }
        }
        c = a;
        r?
    };
    let g19 = decl_line(&mut c, "bool", &[g18, " && ", g17, ";"])?;
    // (?:bool (\w+) = !\19;\s*)?
    let mut g20: Option<&str> = None;
    {
        let mut c2 = c;
        if c2.lit("bool ")
            && let Some(n) = c2.word()
            && c2.lit(" = !")
            && c2.lit(g19)
            && c2.lit(";")
        {
            c2.ws0();
            g20 = Some(n);
            c = c2;
        }
    }
    let guard = match g20 {
        Some(n) => format!("!{n}"),
        None => g19.to_string(),
    };
    Some((c.i, d7, d12, guard))
}

/// the leftmost match of a pattern whose first line is `float (\w+) = as_type<float>\(<what>\);`: its start, by the pattern's own matcher
fn search_from_anchor(src: &str, what: &str, ok: &dyn Fn(usize) -> bool) -> Option<usize> {
    let b = src.as_bytes();
    let needle = format!(" = as_type<float>({what});");
    let mut from = 0;
    while let Some(p) = find_b(b, needle.as_bytes(), from) {
        from = p + 1;
        let mut k = p;
        while k > 0 && is_word(b[k - 1]) {
            k -= 1;
        }
        if k == p || k < 6 || &b[k - 6..k] != b"float " {
            continue;
        }
        if ok(k - 6) {
            return Some(k - 6);
        }
    }
    None
}

/// dmsh_rewrite.first_store(msl, local) -> (the shader, [(storage binding's MSL name, word, value)])
pub fn first_store(src: &str, local: Option<&[i64]>) -> (String, Vec<(String, i64, i64)>) {
    let unchanged = || (src.to_string(), Vec::new());
    let Some(l) = local else { return unchanged() };
    if l.len() != 3 || l[2] != 1 {
        return unchanged();
    }
    // m = FIRST_STORE.search(msl); if m: (conditions) else: m = FIRST_STORE_2D.search(msl)
    let (mend, guard): (usize, String) = if let Some(p) =
        search_from_anchor(src, "gl_WorkGroupID.x", &|s| {
            first_store_1d(src, s).is_some()
        }) {
        let (end, d4, g7) = first_store_1d(src, p).unwrap();
        if l[1] != 1 || shl_value(d4) != Some(l[0]) {
            return unchanged();
        }
        (end, format!("!{g7}"))
    } else if let Some(p) = search_from_anchor(src, "gl_WorkGroupID.y", &|s| {
        first_store_2d(src, s).is_some()
    }) {
        let (end, d7, d12, guard) = first_store_2d(src, p).unwrap();
        if shl_value(d7) != Some(l[1]) || shl_value(d12) != Some(l[0]) {
            return unchanged();
        }
        (end, guard)
    } else {
        return unchanged();
    };
    let mut at = mend;
    let mut out = String::from(&src[..mend]);
    let mut stores: Vec<(String, i64, i64)> = Vec::new();
    let mut values: Vec<(&str, i64)> = Vec::new();
    loop {
        if let Some((hend, name, val)) = held(src, at, &guard) {
            if let Some(v) = values.iter_mut().find(|(n, _)| *n == name) {
                v.1 = val;
            } else {
                values.push((name, val));
            }
            out.push_str(&src[at..hend]);
            at = hend;
            continue;
        }
        if let Some((bend, items)) = block(src, at, &guard) {
            for (n, w, v) in items {
                let w: i64 = w.parse().unwrap_or(i64::MAX);
                if let Some(d) = v.strip_suffix('u') {
                    stores.push((n.to_string(), w, d.parse().unwrap_or(i64::MAX)));
                } else {
                    let name = &v[5..v.len() - 1];
                    match values.iter().find(|(k, _)| *k == name) {
                        Some((_, x)) => stores.push((n.to_string(), w, *x)),
                        None => return unchanged(),
                    }
                }
            }
            at = bend;
            continue;
        }
        let Some(pend) = plain(src, at) else { break };
        if held(src, at, &guard).is_some() {
            break;
        }
        out.push_str(&src[at..pend]);
        at = pend;
    }
    let rest = &src[at..];
    if stores.is_empty() || stores.len() > 4 || contains(rest, &format!("if ({guard})")) {
        return unchanged();
    }
    out.push_str(rest);
    (out, stores)
}

/// `int (\w+) = \w+;\s*if \(G\)\s*\{\s*\1 = (\d+);\s*\}\s*int (\w+) = \1;\s*` at `at` -> (end, the new local's name, its value)
fn held<'a>(src: &'a str, at: usize, guard: &str) -> Option<(usize, &'a str, i64)> {
    let mut c = Cur::at(src, at);
    if !c.lit("int ") {
        return None;
    }
    let x = c.word()?;
    if !c.lit(" = ") || c.word().is_none() || !c.lit(";") {
        return None;
    }
    c.ws0();
    if !(c.lit("if (") && c.lit(guard) && c.lit(")")) {
        return None;
    }
    c.ws0();
    if !c.lit("{") {
        return None;
    }
    c.ws0();
    if !(c.lit(x) && c.lit(" = ")) {
        return None;
    }
    let d = c.digits()?;
    if !c.lit(";") {
        return None;
    }
    c.ws0();
    if !c.lit("}") {
        return None;
    }
    c.ws0();
    if !c.lit("int ") {
        return None;
    }
    let y = c.word()?;
    if !(c.lit(" = ") && c.lit(x) && c.lit(";")) {
        return None;
    }
    c.ws0();
    Some((c.i, y, d.parse().unwrap_or(i64::MAX)))
}

/// `if \(G\)\s*\{\s*((?:\(\*\w+\.\w+\)\._m0\[\d+\] = (?:\d+u|uint\(\w+\));\s*)+)\}\s*` at `at` -> (end, the statements (name, word, value text) of group 1)
/// A statement of a guarded block: (name, word, value text).
type Stmt<'a> = (&'a str, &'a str, &'a str);

fn block<'a>(src: &'a str, at: usize, guard: &str) -> Option<(usize, Vec<Stmt<'a>>)> {
    let mut c = Cur::at(src, at);
    if !(c.lit("if (") && c.lit(guard) && c.lit(")")) {
        return None;
    }
    c.ws0();
    if !c.lit("{") {
        return None;
    }
    c.ws0();
    let mut items = Vec::new();
    loop {
        let mut d = c;
        if !d.lit("(*") || d.word().is_none() || !d.lit(".") {
            break;
        }
        let Some(n) = d.word() else { break };
        if !d.lit(")._m0[") {
            break;
        }
        let Some(w) = d.digits() else { break };
        if !d.lit("] = ") {
            break;
        }
        // (?:\d+u|uint\(\w+\))
        let v0 = d.i;
        let mut e = d;
        if e.digits().is_some() && e.lit("u") {
        } else {
            e = d;
            if !(e.lit("uint(") && e.word().is_some() && e.lit(")")) {
                break;
            }
        }
        let v = &src[v0..e.i];
        if !e.lit(";") {
            break;
        }
        e.ws0();
        c = e;
        items.push((n, w, v));
    }
    if items.is_empty() {
        return None;
    }
    if !c.lit("}") {
        return None;
    }
    c.ws0();
    Some((c.i, items))
}

/// `int \w+ = [^;{}]*;\s*` at `at` -> end
fn plain(src: &str, at: usize) -> Option<usize> {
    let mut c = Cur::at(src, at);
    if !c.lit("int ") || c.word().is_none() || !c.lit(" = ") {
        return None;
    }
    let b = src.as_bytes();
    while c.i < b.len() && !matches!(b[c.i], b';' | b'{' | b'}') {
        c.i += 1;
    }
    if !c.lit(";") {
        return None;
    }
    c.ws0();
    Some(c.i)
}

// ------------------------------------------------------------------------------------------------------------------------------ tg_spill

pub fn tg_bytes(elem: &str) -> Option<u64> {
    Some(match elem {
        "uint" | "int" | "float" => 4,
        "uint2" | "int2" | "float2" => 8,
        "uint3" | "int3" | "float3" | "uint4" | "int4" | "float4" => 16,
        "half" | "short" | "ushort" => 2,
        "half2" => 4,
        "half4" => 8,
        "char" | "uchar" => 1,
        _ => return None,
    })
}

/// one threadgroup declaration of main0's scope: `^([ \t]*)threadgroup (spvUnsafeArray<(\w+), (\d+)>|\w+) (\w+)(?:\[(\d+)\])?;[ \t]*$` (re.M)
pub struct TgDecl<'a> {
    pub start: usize,
    pub end: usize,
    pub indent: &'a str,
    pub ty: &'a str,
    pub elem: &'a str,
    pub n: u64,
    pub name: &'a str,
    pub suffix: Option<&'a str>,
}

pub fn tg_decls<'a>(src: &'a str, from: usize) -> Vec<TgDecl<'a>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    // the first line start at or after `from` (Python's `^` with a pos argument still needs the real line start)
    let mut ls = if from == 0 || b[from - 1] == b'\n' {
        from
    } else {
        match memchr::memchr(b'\n', &b[from..]) {
            Some(p) => from + p + 1,
            None => b.len(),
        }
    };
    while ls < b.len() {
        let le = memchr::memchr(b'\n', &b[ls..])
            .map(|p| p + ls)
            .unwrap_or(b.len());
        if let Some(d) = tg_decl_line(src, ls, le) {
            out.push(d);
        }
        ls = le + 1;
    }
    out
}

fn tg_decl_line(src: &str, ls: usize, le: usize) -> Option<TgDecl<'_>> {
    let line = &src[ls..le];
    let mut c = Cur::at(line, 0);
    let indent = c.ht0();
    if !c.lit("threadgroup ") {
        return None;
    }
    // alternative 1: spvUnsafeArray<(\w+), (\d+)>
    let rest = c;
    let mut alt1: Option<(&str, &str, u64, Cur)> = None;
    {
        let mut a = rest;
        if a.lit("spvUnsafeArray<")
            && let Some(el) = a.word()
            && a.lit(", ")
            && let Some(d) = a.digits()
            && a.lit(">")
        {
            let ty_end = a.i;
            alt1 = Some((
                &line[rest.i..ty_end],
                el,
                d.parse::<u64>().unwrap_or(u64::MAX),
                a,
            ));
        }
    }
    fn try_tail<'a>(mut c: Cur<'a>) -> Option<(&'a str, Option<&'a str>)> {
        if !c.lit(" ") {
            return None;
        }
        let nm = c.word()?;
        let mut suffix = None;
        let mut c2 = c;
        if c2.lit("[")
            && let Some(d) = c2.digits()
            && c2.lit("]")
        {
            suffix = Some(d);
            c = c2;
        }
        if !c.lit(";") {
            return None;
        }
        c.ht0();
        if !c.at_end() {
            return None;
        }
        Some((nm, suffix))
    }
    if let Some((ty, el, n, a)) = alt1
        && let Some((nm, suffix)) = try_tail(a)
    {
        return Some(TgDecl {
            start: ls,
            end: le,
            indent,
            ty,
            elem: el,
            n,
            name: nm,
            suffix,
        });
    }
    // the alternation falls through to `\w+` when the rest fails: `spvUnsafeArray` is a word, but `<` is not a space, so it cannot match
    // alternative 2: \w+
    let mut a = rest;
    let ty = a.word()?;
    let (nm, suffix) = try_tail(a)?;
    let n = suffix
        .map(|s| s.parse::<u64>().unwrap_or(u64::MAX))
        .unwrap_or(1);
    Some(TgDecl {
        start: ls,
        end: le,
        indent,
        ty,
        elem: ty,
        n,
        name: nm,
        suffix,
    })
}

/// the threadgroup bytes dmsh_rewrite.tg_spill counts, with the declarations of main0's scope; Err on an element type it does not know
fn tg_scan<'a>(src: &'a str, mend: usize) -> Result<(Vec<(TgDecl<'a>, u64)>, u64), PostError> {
    let mut decls = Vec::new();
    let mut total: u64 = 0;
    for d in tg_decls(src, mend) {
        let bytes = tg_bytes(d.elem)
            .ok_or_else(|| failed("tg_spill", format!("threadgroup array of {}", d.elem)))?;
        let off = total;
        // Python's sum has no bound: a wrapped u64 made a huge threadgroup look small (no spill); saturated, it stays past TG_MAX
        total = total.saturating_add((bytes.saturating_mul(d.n).saturating_add(15)) & !15);
        decls.push((d, off));
    }
    Ok((decls, total))
}

/// `\bkernel\s+void\s+main0\s*\(` -> (start, end)
fn kernel_main0(src: &str) -> Option<(usize, usize)> {
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"kernel", from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let mut c = Cur::at(src, p + 6);
        if c.ws1() && c.lit("void") && c.ws1() && c.lit("main0") {
            c.ws0();
            if c.lit("(") {
                return Some((p, c.i));
            }
        }
    }
    None
}

/// dmsh_rewrite.tg_spill -> (the shader, the bytes per workgroup, 0 when unchanged)
pub fn tg_spill(src: &str) -> Result<(String, u64), PostError> {
    let Some((mstart, mend)) = kernel_main0(src) else {
        return Ok((src.to_string(), 0));
    };
    let (decls, total) = tg_scan(src, mend)?;
    if total <= t::TG_MAX as u64 {
        return Ok((src.to_string(), 0));
    }
    // the parameter list's end: its attributes hold parentheses ([[buffer(0)]])
    let b = src.as_bytes();
    let mut depth = 1i32;
    let mut i = mend;
    while depth != 0 {
        if i >= b.len() {
            return Err(failed("tg_spill", "string index out of range"));
        }
        match b[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    let params = &src[mend..i - 1];
    // g = re.search(r'uint3 (\w+) \[\[threadgroup_position_in_grid\]\]', params)
    let pb = params.as_bytes();
    let mut gid: Option<&str> = None;
    let mut from = 0;
    while let Some(p) = find_b(pb, b"uint3 ", from) {
        from = p + 1;
        let mut c = Cur::at(params, p + 6);
        if c.word().is_some() {
            let nm = &params[p + 6..c.i];
            if c.lit(" [[threadgroup_position_in_grid]]") {
                gid = Some(nm);
                break;
            }
        }
    }
    let gname = gid.unwrap_or("dm_tgid");
    let add = format!(
        "{}, uint3 dm_ntg [[threadgroups_per_grid]], device uchar* dm_tg [[buffer({})]]",
        if gid.is_some() {
            ""
        } else {
            ", uint3 dm_tgid [[threadgroup_position_in_grid]]"
        },
        t::TG_SLOT
    );
    let mut out = String::with_capacity(src.len() + 512);
    let mut at = 0;
    for (k, (d, off)) in decls.iter().enumerate() {
        out.push_str(&src[at..d.start]);
        let ty = match d.suffix {
            Some(s) => format!("{}[{s}]", d.ty),
            None => d.ty.to_string(),
        };
        if k == 0 {
            out.push_str(&format!("{}ulong dm_tgbase = ((ulong({gname}.z) * dm_ntg.y + {gname}.y) * dm_ntg.x + {gname}.x) * {total}ul;\n", d.indent));
        }
        match d.suffix {
            Some(s) => out.push_str(&format!(
                "{}device {} (&{})[{s}] = *(device {} (*)[{s}])(dm_tg + dm_tgbase + {off});",
                d.indent, d.ty, d.name, d.ty
            )),
            None => out.push_str(&format!(
                "{}device {ty}& {} = *(device {ty}*)(dm_tg + dm_tgbase + {off});",
                d.indent, d.name
            )),
        }
        at = d.end;
    }
    out.push_str(&src[at..]);
    // e = src.index(params, m.start()) + len(params)
    let e = if params.is_empty() {
        mstart
    } else {
        find(&out, params, mstart).ok_or_else(|| failed("tg_spill", "substring not found"))?
    } + params.len();
    out.insert_str(e, &add);
    // the rest of the address space (helper parameters, atomics); the array template's own threadgroup overloads stay
    let mut res = String::with_capacity(out.len() + 256);
    let mut first = true;
    for ln in out.split('\n') {
        if !first {
            res.push('\n');
        }
        first = false;
        if contains(ln, "operator [] (size_t pos)") {
            res.push_str(ln);
            continue;
        }
        spill_line(ln, &mut res);
    }
    Ok((res, total))
}

/// `re.sub(r'\bthreadgroup\b(?=\s)', 'device', ln)`, then `re.sub(r'(?<!mem_device \| )mem_flags::mem_threadgroup\b', 'mem_flags::mem_device | mem_flags::mem_threadgroup', ln)`
fn spill_line(ln: &str, out: &mut String) {
    let b = ln.as_bytes();
    let mut l1 = String::with_capacity(ln.len() + 16);
    let (mut at, mut from) = (0, 0);
    while let Some(p) = find_b(b, b"threadgroup", from) {
        let e = p + 11;
        if !in_word(b, p) && e < b.len() && is_space(b[e]) {
            l1.push_str(&ln[at..p]);
            l1.push_str("device");
            at = e;
            from = e;
        } else {
            from = p + 1;
        }
    }
    l1.push_str(&ln[at..]);
    let b = l1.as_bytes();
    let needle = b"mem_flags::mem_threadgroup";
    let (mut at, mut from) = (0, 0);
    while let Some(p) = find_b(b, needle, from) {
        let e = p + needle.len();
        let after_ok = !(e < b.len() && is_word(b[e]));
        let pre = b"mem_device | ";
        let before_bad = p >= pre.len() && &b[p - pre.len()..p] == pre;
        if after_ok && !before_bad {
            out.push_str(&l1[at..p]);
            out.push_str("mem_flags::mem_device | mem_flags::mem_threadgroup");
            at = e;
            from = e;
        } else {
            from = p + 1;
        }
    }
    out.push_str(&l1[at..]);
}

#[cfg(test)]
mod tg_tests {
    use super::*;

    #[test]
    fn a_threadgroup_total_past_u64_is_still_past_the_limit() {
        let src = "kernel void main0()\n{\n    threadgroup spvUnsafeArray<float4, 18446744073709551615> a;\n    threadgroup spvUnsafeArray<float4, 16> b;\n}\n";
        let (decls, total) = tg_scan(src, 0).unwrap();
        assert_eq!(decls.len(), 2);
        assert!(total > t::TG_MAX as u64, "{total}");
    }
}
