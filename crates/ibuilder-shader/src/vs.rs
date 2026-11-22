//! The vertex-shader steps of dmsh_entry.py (vs_init, vs_depth, vs_discard, vs_layer_entry, vs_writes) and dmsh_geometry.capture.
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::collections::BTreeSet;

const STRUCT: &str = "struct main0_out\n{\n";

/// (i, j) of `src.find('struct main0_out\n{\n')`, `src.find('\n};', i)` as Python gives them (-1 as None)
fn out_struct_span(src: &str) -> (Option<usize>, Option<usize>) {
    let i = find(src, STRUCT, 0);
    let j = match i {
        Some(i) => find(src, "\n};", i),
        None => None,
    };
    (i, j)
}

pub fn vs_init(src: &str) -> String {
    let (i, j) = out_struct_span(src);
    let k = find(src, "main0_out out = {};", 0);
    let (Some(i), Some(j), Some(k)) = (i, j, k) else {
        return src.to_string();
    };
    // `\n    float4 (\w+) \[\[user\(locn\d+\)\]\];` in src[i:j]
    let seg = &src[i..j];
    let sb = seg.as_bytes();
    let pat = b"\n    float4 ";
    let mut outs: Vec<&str> = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(sb, pat, from) {
        from = p + 1;
        let n0 = p + pat.len();
        let n1 = word_end(sb, n0);
        if n1 == n0 || !starts(sb, n1, " [[user(locn") {
            continue;
        }
        let d0 = n1 + 12;
        let d1 = digits_end(sb, d0);
        if d1 == d0 || !starts(sb, d1, ")]];") {
            continue;
        }
        outs.push(&seg[n0..n1]);
    }
    let k = k + "main0_out out = {};".len();
    let mut o = String::with_capacity(src.len() + outs.len() * 48);
    o.push_str(&src[..k]);
    for n in outs {
        o.push_str(&format!("\n    out.{n} = float4(0.0, 0.0, 0.0, 1.0);"));
    }
    o.push_str(&src[k..]);
    o
}

const VP_BASE_INSTANCE: &str = "(*spvDescriptorSet0.vp_c0)._m0[100].y";

/// dmsh_entry.vs_base_instance: the base instance read from Metal (`[[base_instance]]`), not from the driver segment
pub fn vs_base_instance(src: &str) -> String {
    if !contains(src, VP_BASE_INSTANCE) {
        return src.to_string();
    }
    let src = src.replace(VP_BASE_INSTANCE, "as_type<float>(gl_BaseInstance)");
    // re.sub(r'\bvertex\s+\w+\s+main0\s*\((?:[^()]|\([^()]*\))*\)', add, src)
    let b = src.as_bytes();
    let mut o = String::with_capacity(src.len() + 64);
    let (mut at, mut from) = (0, 0);
    while let Some(p) = find_b(b, b"vertex", from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let mut c = Cur::at(&src, p + 6);
        if !(c.ws1() && c.word().is_some() && c.ws1() && c.lit("main0")) {
            continue;
        }
        c.ws0();
        if !c.lit("(") {
            continue;
        }
        // the parameter list: its attributes hold parentheses ([[buffer(0)]]), one level deep
        let mut i = c.i;
        let close = loop {
            if i >= b.len() {
                break None;
            }
            match b[i] {
                b')' => break Some(i),
                b'(' => match b[i + 1..].iter().position(|&x| x == b'(' || x == b')') {
                    Some(r) if b[i + 1 + r] == b')' => i += r + 2,
                    _ => break None,
                },
                _ => i += 1,
            }
        };
        let Some(close) = close else { continue };
        let whole = &src[p..close + 1];
        o.push_str(&src[at..p]);
        if contains(whole, "[[base_instance]]") {
            o.push_str(whole);
        } else {
            o.push_str(&src[p..close]);
            o.push_str(", uint gl_BaseInstance [[base_instance]])");
        }
        at = close + 1;
        from = at;
    }
    o.push_str(&src[at..]);
    o
}

/// vs_layer: a vertex shader that writes the layer loses the point size output
pub fn vs_layer(src: String) -> Result<String, PostError> {
    if !layer_out(&src) {
        return Ok(src);
    }
    let (Some(i), Some(j)) = out_struct_span(&src) else {
        return Ok(src);
    };
    if !contains(&src[i..j], "[[point_size]]") {
        return Ok(src);
    }
    let seg = src[i..j].replacen("\n    float gl_PointSize [[point_size]];", "", 1);
    let s = format!("{}{}{}", &src[..i], seg, &src[j..]);
    drop_point_size(&s)
}

/// layer_out: main0's output struct has `[[render_target_array_index]]`
pub fn layer_out(src: &str) -> bool {
    let (i, j) = out_struct_span(src);
    match (i, j) {
        (Some(i), Some(j)) => contains(&src[i..j], "[[render_target_array_index]]"),
        _ => false,
    }
}

/// vs_depth -> (single: only main0 with the GL depth rewrite, full: single + main0_z)
pub fn vs_depth(src: &str) -> Result<(String, String), PostError> {
    let b = src.as_bytes();
    let m = main_entry(b, "main0", 0).ok_or_else(|| {
        failed(
            "vs_depth",
            "[RED] vertex shader without a main0 main function, cannot add the depth entry points",
        )
    })?;
    let body = &src[m..];
    let gl = body.replace("return out;", "out.gl_Position.z = fma(out.gl_Position.z, 0.5, out.gl_Position.w * 0.5);\n    return out;");
    let mut single = String::with_capacity(src.len() + gl.len() - body.len() + 8);
    single.push_str(&src[..m]);
    single.push_str(&gl);
    // main0_z: the original body with its first `main0\s*\(` renamed (`\bmain0\s*\(`)
    let bb = body.as_bytes();
    let mut from = 0;
    let mut z = body.to_string();
    while let Some(p) = find_b(bb, b"main0", from) {
        from = p + 1;
        if in_word(bb, p) {
            continue;
        }
        let q = skip_space(bb, p + 5);
        if q < bb.len() && bb[q] == b'(' {
            z = format!("{}main0_z({}", &body[..p], &body[q + 1..]);
            break;
        }
    }
    let full = format!("{single}\n{z}");
    Ok((single, full))
}

/// the start of `\bvertex\s+main0_out\s+<name>\s*\(` at or after `from`, with the index after `(` via `main_entry_end`
pub fn main_entry(b: &[u8], name: &str, from: usize) -> Option<usize> {
    main_entry_m(b, name, from).map(|(s, _)| s)
}
pub fn main_entry_m(b: &[u8], name: &str, from: usize) -> Option<(usize, usize)> {
    let mut f = from;
    while let Some(p) = find_b(b, b"vertex", f) {
        f = p + 1;
        if in_word(b, p) {
            continue;
        }
        let i = skip_space(b, p + 6);
        if i == p + 6 || !starts(b, i, "main0_out") {
            continue;
        }
        let j = skip_space(b, i + 9);
        if j == i + 9 || !starts(b, j, name) {
            continue;
        }
        let q = skip_space(b, j + name.len());
        if q < b.len() && b[q] == b'(' {
            return Some((p, q + 1));
        }
    }
    None
}

/// vs_discard (the text appended to `full`); `src` is the single (main0 only) source
pub fn vs_discard(src: &str) -> Result<String, PostError> {
    let b = src.as_bytes();
    let (_, pe) = main_entry_m(b, "main0", 0).ok_or_else(|| {
        failed(
            "vs_discard",
            "vertex shader has no main0 entry for rasterizer discard",
        )
    })?;
    // `(.*?)\)\s*\{` : the first `)` followed by optional whitespace and `{`
    let mut from = pe;
    loop {
        let r = memchr::memchr(b')', &b[from..])
            .map(|x| x + from)
            .ok_or_else(|| {
                failed(
                    "vs_discard",
                    "vertex shader has no main0 entry for rasterizer discard",
                )
            })?;
        let q = skip_space(b, r + 1);
        if q < b.len() && b[q] == b'{' {
            let params = &src[pe..r];
            return Ok(format!(
                "\nvertex void main0_discard({})\n{{{}",
                params,
                src[q + 1..].replace("return out;", "return;")
            ));
        }
        from = r + 1;
    }
}

/// vs_writes with the reflection's `rw` flag: storage buffer writes, or image stores (`access::(read_)?write\b`)
pub fn vs_writes(src: &str, rw_buffers: bool) -> bool {
    if rw_buffers {
        return true;
    }
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"access::", from) {
        from = p + 1;
        let i = p + 8;
        for w in ["read_write", "write"] {
            if starts(b, i, w) && !(i + w.len() < b.len() && is_word(b[i + w.len()])) {
                return true;
            }
        }
    }
    false
}

/// the `rw` of dmsh_reflect.reflect for a vertex shader: any storage-buffer binding (`<stage>_s<n>` member of the argument buffer) that the translator says is written; and the
/// ValueErrors of `reflect` that depend on the members (an `[[id(` the member pattern cannot read, a bounded storage buffer that is not a uniform-buffer / push binding, bounded
/// storage buffers without their length table `<stage>_c27`). Exceptions the full reflection raises for names it cannot decode are not reproduced (F2's `finish` runs it).
pub fn reflect_rw(src: &str, res: &serde_json::Value) -> Result<bool, PostError> {
    let Some((_, g0, g1)) = crate::border::descriptor_struct(src) else {
        return Ok(false);
    };
    let body = &src[g0..g1];
    let mem = crate::border::members(body);
    if mem.len() != count(body, "[[id(") {
        return Err(failed(
            "reflect",
            "an argument buffer member the reflection cannot read",
        ));
    }
    let mut sb: Vec<&serde_json::Value> = res
        .get("sbufs")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    sb.sort_by_key(|b| b.get("binding").and_then(|v| v.as_i64()).unwrap_or(0)); // stable: equal bindings keep their order
    let (mut rw, mut sbx, mut sblen) = (false, false, false);
    for m in &mem {
        // `[a-z]+_c(\d+)` fullmatch: the bank n of a uniform slot member
        let nb = m.name.as_bytes();
        let mut k = 0;
        while k < nb.len() && nb[k].is_ascii_lowercase() {
            k += 1;
        }
        if k == 0 {
            continue;
        }
        if m.name[k..].starts_with("_c") {
            let d = &m.name[k + 2..];
            if !d.is_empty()
                && d.bytes().all(|c| c.is_ascii_digit())
                && d.parse::<u64>().ok() == Some(27)
            {
                sblen = true;
            }
            continue;
        }
        // `[a-z]+_s(\d+)` fullmatch
        if !m.name[k..].starts_with("_s") {
            continue;
        }
        let d = &m.name[k + 2..];
        if d.is_empty() || !d.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let index: usize = d.parse().unwrap_or(usize::MAX);
        let b = sb.get(index);
        let slot = b.and_then(|b| b.get("cb_slot")).and_then(|v| v.as_i64());
        let off = b.and_then(|b| b.get("cb_offset")).and_then(|v| v.as_i64());
        if slot == Some(0xFF) {
            // not a game buffer: the host's pool table or the write log; rw by the offset, or by the translator's notes
            if off == Some(1) {
                rw = true;
                continue;
            }
            let w = res
                .get("notes")
                .and_then(|v| v.as_array())
                .map(|ns| {
                    ns.iter()
                        .any(|n| n.as_str().map(note_writes).unwrap_or(false))
                })
                .unwrap_or(false);
            if w {
                rw = true;
            }
            continue;
        }
        let flags = b
            .and_then(|b| b.get("flags"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if b.is_some() && flags & 1 != 0 {
            rw = true;
        }
        if b.is_some() && flags & 2 != 0 {
            // bounded: only a binding whose address is in the game's own uniform buffer (c[n >= 3]) or the pushed one (c[24]) can be
            let cb = slot.unwrap_or(i64::MIN);
            let sl = b
                .and_then(|b| b.get("slot"))
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            if cb < 3 || !(0..32).contains(&sl) {
                return Err(failed(
                    "reflect",
                    format!(
                        "storage buffer {} is bounded but is not a uniform-buffer binding, slot {sl}",
                        m.name
                    ),
                ));
            }
            sbx = true;
        }
    }
    if sbx && !sblen {
        return Err(failed(
            "reflect",
            "bounded storage buffers without their length table",
        ));
    }
    Ok(rw)
}

/// `operation "(Store|Atomic\w*)" traced to no storage buffer`
fn note_writes(s: &str) -> bool {
    let mut from = 0;
    while let Some(p) = s[from..].find("operation \"") {
        let a = from + p + 11;
        let rest = &s[a..];
        if let Some(q) = rest.find('"') {
            let w = &rest[..q];
            let ok = w == "Store" || (w.starts_with("Atomic") && w.bytes().all(is_word));
            if ok && rest[q + 1..].starts_with(" traced to no storage buffer") {
                return true;
            }
        }
        from = a;
    }
    false
}

/// vs_pad (dmsh_build.vs_pad / shader_post.vs_pad): -> (source, padded locations)
pub fn vs_pad(src: &str, partner: &BTreeSet<u32>) -> (String, Vec<u32>) {
    // the user(locnN) locations the shader's own outputs hold: `\[\[user\(locn(\d+)\)`
    let b = src.as_bytes();
    let mut have: BTreeSet<u32> = BTreeSet::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"[[user(locn", from) {
        from = p + 1;
        let d0 = p + 11;
        let d1 = digits_end(b, d0);
        if d1 > d0
            && let Ok(v) = src[d0..d1].parse::<u32>()
        {
            have.insert(v);
        }
    }
    let miss: Vec<u32> = partner.difference(&have).copied().collect();
    let (i, j) = out_struct_span(src);
    let k = find(src, "main0_out out = {};", 0);
    let (Some(_), Some(j), Some(_)) = (i, j, k) else {
        return (src.to_string(), vec![]);
    };
    if miss.is_empty() {
        return (src.to_string(), vec![]);
    }
    let mut s = String::with_capacity(src.len() + miss.len() * 120);
    s.push_str(&src[..j]);
    for l in &miss {
        s.push_str(&format!("\n    float4 dm_pad{l} [[user(locn{l})]];"));
    }
    s.push_str(&src[j..]);
    let k = find(&s, "main0_out out = {};", 0).unwrap() + "main0_out out = {};".len();
    let mut o = String::with_capacity(s.len() + miss.len() * 60);
    o.push_str(&s[..k]);
    for l in &miss {
        o.push_str(&format!(
            "\n    out.dm_pad{l} = float4(0.0, 0.0, 0.0, 1.0);"
        ));
    }
    o.push_str(&s[k..]);
    (o, miss)
}

/// the `[[user(locnN)` numbers a (pixel) MSL text holds: `\[\[user\(locn(\d+)\)`
pub fn user_locations(msl: &str) -> BTreeSet<u32> {
    let b = msl.as_bytes();
    let mut have = BTreeSet::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"[[user(locn", from) {
        from = p + 1;
        let d0 = p + 11;
        let d1 = digits_end(b, d0);
        if d1 > d0
            && let Ok(v) = msl[d0..d1].parse::<u32>()
        {
            have.insert(v);
        }
    }
    have
}

// ---------------------------------------------------------------------------------------------------------------- vs_layer_entry

/// dmsh_entry.drop_point_size: the point size output taken out of vertex functions whose output struct no longer has it
pub fn drop_point_size(src: &str) -> Result<String, PostError> {
    // re.sub(r'\n    out\.gl_PointSize = [^;\n]*;', '', src)
    let b = src.as_bytes();
    let pat = b"\n    out.gl_PointSize = ";
    let mut o = String::with_capacity(src.len());
    let mut at = 0;
    let mut from = 0;
    while let Some(p) = find_b(b, pat, from) {
        let s0 = p + pat.len();
        match memchr::memchr2(b';', b'\n', &b[s0..]) {
            Some(r) if b[s0 + r] == b';' => {
                o.push_str(&src[at..p]);
                at = s0 + r + 1;
                from = at;
            }
            _ => {
                from = p + 1;
            }
        }
    }
    o.push_str(&src[at..]);
    // while 'out.gl_PointSize' in src: the write is wherever the body takes the output by reference; the enclosing vertex function gets a local float for it
    let mut rounds = 0u32;
    while let Some(use_at) = find(&o, "out.gl_PointSize", 0) {
        rounds += 1;
        if rounds > 4096 {
            return Err(failed(
                "drop_point_size",
                "no progress (the Python loops here without end)",
            ));
        }
        // the last `\bvertex\s+\w+\s+\w+\s*\(` that starts before the use
        let ob = o.as_bytes();
        let mut last: Option<usize> = None; // the end of the match
        let mut f = 0;
        while let Some(p) = find_b(ob, b"vertex", f) {
            if p >= use_at {
                break;
            }
            f = p + 1;
            if in_word(ob, p) {
                continue;
            }
            let mut c = Cur::at(&o, p + 6);
            if c.ws1() && c.word().is_some() && c.ws1() && c.word().is_some() {
                c.ws0();
                if c.lit("(") {
                    last = Some(c.i);
                }
            }
        }
        let Some(a_end) = last else {
            return Err(failed(
                "drop_point_size",
                "[RED] a point size output used outside a vertex function",
            ));
        };
        // b = src.find('{', a[-1].end()); for e in range(b, len(src)): depth += src[e]=='{'; depth -= src[e]=='}'; if not depth: break
        // (a missing `{` makes b = -1: Python then walks from index -1, the last character, and its slices are relative to that)
        let n = ob.len() as i64;
        let bpos: i64 = match find(&o, "{", a_end) {
            Some(x) => x as i64,
            None => -1,
        };
        let mut depth = 0i32;
        let mut e = bpos;
        let mut idx = bpos;
        loop {
            if idx >= n {
                break;
            }
            let c = ob[if idx < 0 {
                (n + idx) as usize
            } else {
                idx as usize
            }];
            if c == b'{' {
                depth += 1;
            }
            if c == b'}' {
                depth -= 1;
            }
            e = idx;
            if depth == 0 {
                break;
            }
            idx += 1;
        }
        // src[:b+1] + decl + src[b+1:e].replace(...) + src[e:]   (negative indices count from the end, as Python's slices do)
        let norm = |i: i64| -> usize {
            if i < 0 {
                (n + i).max(0) as usize
            } else {
                (i as usize).min(ob.len())
            }
        };
        let (s1, s3) = (norm(bpos + 1), norm(e));
        let mid = if s1 <= s3 { &o[s1..s3] } else { "" };
        let mut t = String::with_capacity(o.len() + 64);
        t.push_str(&o[..s1]);
        t.push_str("\n    float dmsh_point_size = 1.0;");
        t.push_str(&mid.replace("out.gl_PointSize", "dmsh_point_size"));
        t.push_str(&o[s3.min(o.len())..]);
        o = t;
    }
    Ok(o)
}

/// vs_layer_entry: -> (source with the layer entry points, whether it added them)
pub fn vs_layer_entry(full: &str) -> Result<(String, Option<(u32, char)>), PostError> {
    let (i, j) = out_struct_span(full);
    let (Some(i), Some(j)) = (i, j) else {
        return Ok((full.to_string(), None));
    };
    if !contains(full, "gl_InstanceIndex") || contains(&full[i..j], "[[render_target_array_index]]")
    {
        return Ok((full.to_string(), None));
    }
    // locs: `\n    float4 (\w+) \[\[user\(locn(\d+)\)\]\];` in full[i:j]
    let seg = &full[i..j];
    let sb = seg.as_bytes();
    let pat = b"\n    float4 ";
    let mut locs: Vec<(&str, u32)> = Vec::new(); // a later declaration of the same name wins (a dict)
    let mut from = 0;
    while let Some(p) = find_b(sb, pat, from) {
        from = p + 1;
        let n0 = p + pat.len();
        let n1 = word_end(sb, n0);
        if n1 == n0 || !starts(sb, n1, " [[user(locn") {
            continue;
        }
        let d0 = n1 + 12;
        let d1 = digits_end(sb, d0);
        if d1 == d0 || !starts(sb, d1, ")]];") {
            continue;
        }
        let v: u32 = seg[d0..d1].parse().unwrap_or(u32::MAX);
        if let Some(e) = locs.iter_mut().find(|(n, _)| *n == &seg[n0..n1]) {
            e.1 = v;
        } else {
            locs.push((&seg[n0..n1], v));
        }
    }
    // first `\n    out\.(\w+)\.([xyzw]) = as_type<float>\(` over the whole text with the name in locs
    let fb = full.as_bytes();
    let pat2 = b"\n    out.";
    let mut hit: Option<(&str, u8)> = None;
    let mut from = 0;
    while let Some(p) = find_b(fb, pat2, from) {
        from = p + 1;
        let n0 = p + pat2.len();
        let n1 = word_end(fb, n0);
        if n1 == n0
            || n1 + 1 >= fb.len()
            || fb[n1] != b'.'
            || !matches!(fb[n1 + 1], b'x' | b'y' | b'z' | b'w')
        {
            continue;
        }
        if !starts(fb, n1 + 2, " = as_type<float>(") {
            continue;
        }
        if locs.iter().any(|(n, _)| *n == &full[n0..n1]) {
            hit = Some((&full[n0..n1], fb[n1 + 1]));
            break;
        }
    }
    let Some((name, comp)) = hit else {
        return Ok((full.to_string(), None));
    };
    let loc = locs
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
        .unwrap();
    let comp = comp as char;
    // st = re.sub(r'\n    float gl_PointSize \[\[point_size\]\];', '', full[i:j+3]).replace('struct main0_out', 'struct main0l_out', 1)
    let st = full[i..j + 3].replace("\n    float gl_PointSize [[point_size]];", "");
    let st = st.replacen("struct main0_out", "struct main0l_out", 1);
    let mut st = st[..st.len() - 3].to_string();
    st.push_str("\n    uint gl_Layer [[render_target_array_index]];\n};");
    let mut add = format!("\n{st}\n");
    for (ep, new) in [("main0", "main0_l"), ("main0_z", "main0_lz")] {
        let Some((a, ae)) = main_entry_m(fb, ep, 0) else {
            return Ok((full.to_string(), None));
        };
        let bpos = match find(full, "{", ae) {
            Some(x) => x,
            None => return Ok((full.to_string(), None)),
        };
        let mut depth = 0i32;
        let mut e = bpos;
        for (k, &c) in fb.iter().enumerate().skip(bpos) {
            if c == b'{' {
                depth += 1;
            }
            if c == b'}' {
                depth -= 1;
            }
            e = k;
            if depth == 0 {
                break;
            }
        }
        let fn0 = full[a..e + 1].replace("main0_out", "main0l_out").replacen(
            &format!(" {ep}("),
            &format!(" {new}("),
            1,
        );
        let fn1 = drop_point_size(&fn0)?;
        add.push('\n');
        add.push_str(&fn1.replace(
            "return out;",
            &format!("out.gl_Layer = as_type<uint>(out.{name}.{comp});\n    return out;"),
        ));
        add.push('\n');
    }
    Ok((format!("{full}{add}"), Some((loc, comp))))
}

// ---------------------------------------------------------------------------------------------------------------- dmsh_geometry.capture

/// `output_words`: {word: expression over out}
pub fn output_words(src: &str) -> Result<Vec<(usize, String)>, PostError> {
    let st = crate::blend::out_struct(src)
        .ok_or_else(|| failed("capture", "geometry capture: missing output struct"))?;
    let mut stores: Vec<(usize, String)> = Vec::new();
    for l in st.lines() {
        let line = strip(l);
        if line.is_empty() {
            continue;
        }
        if let Some((ty, name, rest)) = split_decl(line) {
            // (float4|uint4|int4) (\w+) \[\[user\(locn(\d+)\)\]\];
            if matches!(ty, "float4" | "uint4" | "int4")
                && let Some(d) = rest
                    .strip_prefix("[[user(locn")
                    .and_then(|r| r.strip_suffix(")]];"))
                && !d.is_empty()
                && d.bytes().all(|c| c.is_ascii_digit())
            {
                let n: usize = d.parse().unwrap();
                if n >= 32 {
                    return Err(failed(
                        "capture",
                        "geometry capture: attribute outside Maxwell map",
                    ));
                }
                for (c, s) in "xyzw".chars().enumerate() {
                    stores.push((16 + n * 4 + c, format!("out.{name}.{s}")));
                }
                continue;
            }
            if ty == "float4" && (rest == "[[position]];" || rest == "[[position, invariant]];") {
                for (c, s) in "xyzw".chars().enumerate() {
                    stores.push((c, format!("out.{name}.{s}")));
                }
                continue;
            }
            if matches!(ty, "float" | "uint" | "int") {
                let off = match rest {
                    "[[point_size]];" => Some(4),
                    "[[render_target_array_index]];" => Some(13),
                    "[[viewport_array_index]];" => Some(14),
                    _ => None,
                };
                if let Some(o) = off {
                    stores.push((o, format!("out.{name}")));
                    continue;
                }
            }
        }
        // float (\w+)(?:\[(\d+)\])? \[\[clip_distance\]\](?: \[(\d+)\])?;
        if let Some(r) = line.strip_prefix("float ") {
            let nb = r.as_bytes();
            let ne = word_end(nb, 0);
            if ne > 0 {
                let name = &r[..ne];
                let mut rest = &r[ne..];
                let mut n2: Option<usize> = None;
                if let Some(x) = rest.strip_prefix('[')
                    && let Some(q) = x.find(']')
                    && !x[..q].is_empty()
                    && x[..q].bytes().all(|c| c.is_ascii_digit())
                {
                    n2 = x[..q].parse().ok();
                    rest = &x[q + 1..];
                }
                if let Some(x) = rest.strip_prefix(" [[clip_distance]]") {
                    let mut n3: Option<usize> = None;
                    let mut tail = x;
                    if let Some(y) = x.strip_prefix(" [")
                        && let Some(q) = y.find(']')
                        && !y[..q].is_empty()
                        && y[..q].bytes().all(|c| c.is_ascii_digit())
                    {
                        n3 = y[..q].parse().ok();
                        tail = &y[q + 1..];
                    }
                    if tail == ";"
                        && let Some(n) = n2.or(n3)
                        && n <= 8
                    {
                        for c in 0..n {
                            stores.push((5 + c, format!("out.{name}[{c}]")));
                        }
                        continue;
                    }
                }
                // float (\w+) \[\[user\(clip(\d)\)\]\];
                if let Some(x) = r[ne..].strip_prefix(" [[user(clip")
                    && x.len() == 5
                    && x.as_bytes()[0].is_ascii_digit()
                    && &x[1..] == ")]];"
                {
                    stores.push((5 + (x.as_bytes()[0] - b'0') as usize, format!("out.{name}")));
                    continue;
                }
            }
        }
        return Err(failed(
            "capture",
            format!("geometry capture: unknown output {line}"),
        ));
    }
    // dict(stores): the last value of a key, in first-insertion order -- the epilogue is ordered by word anyway
    let mut m: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    for (k, v) in stores {
        m.insert(k, v);
    }
    Ok(m.into_iter().collect())
}

/// `(\w+) (\w+) <rest>` of a stripped struct member line: (type, name, rest after the name and its space)
fn split_decl(line: &str) -> Option<(&str, &str, &str)> {
    let b = line.as_bytes();
    let t1 = word_end(b, 0);
    if t1 == 0 || !starts(b, t1, " ") {
        return None;
    }
    let n0 = t1 + 1;
    let n1 = word_end(b, n0);
    if n1 == n0 || !starts(b, n1, " ") {
        return None;
    }
    Some((&line[..t1], &line[n0..n1], &line[n1 + 1..]))
}

/// dmsh_geometry.capture: the rasterizer-discard capture entry point appended to the unmodified-depth source
pub fn capture(src: &str) -> Result<String, PostError> {
    let b = src.as_bytes();
    // `\bvertex main0_out main0\((.*?)\)\s*\{`
    let needle = b"vertex main0_out main0(";
    let mut from = 0;
    let (_, pe) = loop {
        let p = find_b(b, needle, from)
            .ok_or_else(|| failed("capture", "geometry capture: missing main0 vertex entry"))?;
        from = p + 1;
        if !in_word(b, p) {
            break (p, p + needle.len());
        }
    };
    let mut f = pe;
    let (params_end, brace) = loop {
        let r = memchr::memchr(b')', &b[f..])
            .map(|x| x + f)
            .ok_or_else(|| failed("capture", "geometry capture: missing main0 vertex entry"))?;
        let q = skip_space(b, r + 1);
        if q < b.len() && b[q] == b'{' {
            break (r, q);
        }
        f = r + 1;
    };
    let mut params = src[pe..params_end].to_string();
    let body = &src[brace + 1..];
    if !body
        .trim_end_matches(|c: char| c.is_ascii() && is_space(c as u8))
        .ends_with('}')
    {
        return Err(failed(
            "capture",
            "geometry capture: unexpected function ending",
        ));
    }
    let mut pro = String::new();
    for (semantic, name) in [("vertex_id", "dm_vid"), ("instance_id", "dm_iid")] {
        // `uint (\w+) \[\[<semantic>\]\]` in params
        let pat = format!(" [[{semantic}]]");
        let pb = params.as_bytes();
        let mut found: Option<String> = None;
        let mut f = 0;
        while let Some(p) = find_b(pb, pat.as_bytes(), f) {
            f = p + 1;
            let mut k = p;
            while k > 0 && is_word(pb[k - 1]) {
                k -= 1;
            }
            if k < p && k >= 5 && &pb[k - 5..k] == b"uint " {
                found = Some(params[k..p].to_string());
                break;
            }
        }
        match found {
            Some(n) => {
                if n != name {
                    pro.push_str(&format!("    uint {name} = {n};\n"));
                }
            }
            None => {
                if !params.is_empty() {
                    params.push_str(", ");
                }
                params.push_str(&format!("uint {name} [[{semantic}]]"));
            }
        }
    }
    let stores: Vec<(usize, String)> = output_words(src)?
        .into_iter()
        .map(|(k, e)| (k, format!("as_type<uint>({e})")))
        .collect();
    params.push_str(&format!(
        ", constant dm_geometry_capture& dm_gs [[buffer({})]]",
        t::CAPTURE_SLOT
    ));
    pro.push_str("    uint4 dm_ci = dm_gs.info; device uint* dm_capture = dm_gs.dst;\n    if (dm_vid - dm_ci.x >= dm_ci.y || dm_iid - dm_ci.z >= dm_ci.w) return;\n    device uint* dm_p = dm_capture + ((ulong(dm_iid - dm_ci.z) * dm_ci.y + dm_vid - dm_ci.x) * dm_gs.stride);\n");
    let mut words: Vec<String> = (0..t::CAPTURE_WORDS)
        .map(|k| {
            if k == 3 || k == 4 || (k >= 19 && k % 4 == 3) {
                "0x3f800000u".to_string()
            } else {
                "0u".to_string()
            }
        })
        .collect();
    for (k, v) in stores {
        words[k] = v;
    }
    let mut epi = String::from("{ ");
    for (k, v) in words.iter().enumerate() {
        if k > 0 {
            epi.push(' ');
        }
        epi.push_str(&format!("dm_cw(dm_gs, dm_p, {k}, {v});"));
    }
    epi.push_str(" return; }");
    if !contains(body, "return out;") {
        return Err(failed("capture", "geometry capture: missing return"));
    }
    Ok(format!(
        "\nstruct dm_geometry_capture {{ device uint* dst; uint4 info; uint stride; short slot[{}]; }};\nstatic inline void dm_cw(constant dm_geometry_capture& g, device uint* p, uint k, uint v) {{ short s = g.slot[k]; if (s >= 0) p[s] = v; }}\nvertex void main0_gs({})\n{{\n{}{}",
        t::CAPTURE_WORDS,
        params,
        pro,
        body.replace("return out;", &epi)
    ))
}
