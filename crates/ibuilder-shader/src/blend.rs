//! dmsh_rewrite.blend_zero: NVIDIA's "zero times anything is zero" blend rule as a call before each `return out;` of main0.
use crate::templates as t;
use crate::text::*;
use crate::{PostError, side};
use std::borrow::Cow;

/// `^\s*(\w+)\s+(\w+)\s*\[\[color\((\d+)\)\]\];` per line -> (type, name, index)
pub fn color_members(body: &str) -> Vec<(&str, &str, u32)> {
    let mut out = Vec::new();
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
        if !starts(b, k, "[[color(") {
            continue;
        }
        let d0 = k + 8;
        let d1 = digits_end(b, d0);
        if d1 == d0 || !starts(b, d1, ")]];") {
            continue;
        }
        out.push((&l[i..t1], &l[j..n1], l[d0..d1].parse().unwrap()));
    }
    out
}

/// `struct main0_out\s*\{(.*?)\};` -> the group
pub fn out_struct(src: &str) -> Option<&str> {
    let b = src.as_bytes();
    let needle = b"struct main0_out";
    let mut from = 0;
    while let Some(p) = find_b(b, needle, from) {
        from = p + 1;
        let q = skip_space(b, p + needle.len());
        if q < b.len() && b[q] == b'{' {
            let e = find_b(b, b"};", q + 1)?;
            return Some(&src[q + 1..e]);
        }
    }
    None
}

/// -> (msl, number of returns guarded); Unchanged (0) as in the Python
pub fn blend_zero(src: &str) -> Result<(Cow<'_, str>, u32), PostError> {
    let cols: Vec<(&str, u32)> = match out_struct(src) {
        Some(g) => color_members(g)
            .into_iter()
            .filter(|(ty, _, _)| *ty == "float4")
            .map(|(_, n, c)| (n, c))
            .collect(),
        None => Vec::new(),
    };
    if cols.is_empty()
        || contains(src, "dm_blend0(")
        || contains(src, "[[buffer(28)]]")
        || side::body(src).is_some()
    {
        return Ok((Cow::Borrowed(src), 0));
    }
    let e = match side::entry(src, "main0") {
        Ok(Some(e)) => e,
        _ => return Ok((Cow::Borrowed(src), 0)),
    };
    if e.ret != "main0_out" {
        return Ok((Cow::Borrowed(src), 0));
    }
    let body = &src[e.head..e.end];
    let bb = body.as_bytes();
    // n = len(re.findall(r'\breturn\b', body))
    let mut n = 0u32;
    let mut from = 0;
    while let Some(p) = find_b(bb, b"return", from) {
        from = p + 1;
        if !in_word(bb, p) && !(p + 6 < bb.len() && is_word(bb[p + 6])) {
            n += 1;
        }
    }
    // re.subn(r'^([ \t]*)return out;$', ..., flags=re.M)
    let mut nb = String::with_capacity(body.len() + 64);
    let mut k = 0u32;
    let mut at = 0;
    for (a, e2) in lines(body) {
        let l = &body[a..e2];
        let ind_len = l.bytes().take_while(|&c| c == b' ' || c == b'\t').count();
        if &l[ind_len..] == "return out;" {
            nb.push_str(&body[at..a]);
            let ind = &l[..ind_len];
            nb.push_str(ind);
            nb.push_str("dm_blend0(out, dm_blend);\n");
            nb.push_str(ind);
            nb.push_str("return out;");
            at = e2;
            k += 1;
        }
    }
    nb.push_str(&body[at..]);
    if k == 0 || k != n {
        return Ok((Cow::Borrowed(src), 0));
    }
    let mut cols_txt = String::new();
    for (nm, c) in &cols {
        cols_txt.push_str(&format!(
            "    dm_o.{nm} = dm_rop(dm_bz(dm_o.{nm}, m >> {}u), r >> {}u);\n",
            t::BLEND_ZERO_BITS * c,
            t::BLEND_ROUND_BITS * c
        ));
    }
    let mut out = String::with_capacity(src.len() + 1500);
    out.push_str(&src[..e.start]);
    out.push_str(t::BLEND_ZERO_PRE);
    out.push_str(&cols_txt);
    out.push_str(t::BLEND_ZERO_POST);
    out.push('\n');
    out.push_str(&src[e.start..e.head - 1]);
    if !e.params.is_empty() {
        out.push_str(", ");
    }
    out.push_str(t::BLEND_ZERO_PARAM);
    out.push(')');
    out.push_str(&nb);
    out.push_str(&src[e.end..]);
    Ok((Cow::Owned(out), k))
}
