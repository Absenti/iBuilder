//! dmsh_rewrite.uniform_tests: a game uniform word tested against constants more than once becomes a volatile load (`dm_uword`).
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;
use std::collections::HashMap;

struct Read {
    end: usize,
    ty: (usize, usize),
    r: (usize, usize),
    c: u8,
}

/// `as_type<(u?int)>\((\(\*spvDescriptorSet0\.[a-z]+_c(?:[3-9]|\d\d)\)\._m0\[\d+\])\.([xyzw])\)` at `p`
fn parse_read(s: &[u8], p: usize) -> Option<Read> {
    if !starts(s, p, "as_type<") {
        return None;
    }
    let t0 = p + 8;
    let mut i = t0;
    if i < s.len() && s[i] == b'u' {
        i += 1;
    }
    if !starts(s, i, "int") {
        return None;
    }
    let t1 = i + 3;
    if !starts(s, t1, ">(") {
        return None;
    }
    let r0 = t1 + 2;
    if !starts(s, r0, "(*spvDescriptorSet0.") {
        return None;
    }
    let mut e = r0 + 20;
    let l0 = e;
    while e < s.len() && s[e].is_ascii_lowercase() {
        e += 1;
    }
    if e == l0 || !starts(s, e, "_c") {
        return None;
    }
    let n = e + 2;
    let ok = (n + 1 < s.len() && (b'3'..=b'9').contains(&s[n]) && s[n + 1] == b')')
        || (n + 2 < s.len()
            && s[n].is_ascii_digit()
            && s[n + 1].is_ascii_digit()
            && s[n + 2] == b')');
    if !ok {
        return None;
    }
    let close = if (b'3'..=b'9').contains(&s[n]) && s[n + 1] == b')' {
        n + 1
    } else {
        n + 2
    };
    if !starts(s, close, ")._m0[") {
        return None;
    }
    let d0 = close + 6;
    let d1 = digits_end(s, d0);
    if d1 == d0 || d1 >= s.len() || s[d1] != b']' {
        return None;
    }
    let r1 = d1 + 1;
    if !(r1 + 2 < s.len()
        && s[r1] == b'.'
        && matches!(s[r1 + 1], b'x' | b'y' | b'z' | b'w')
        && s[r1 + 2] == b')')
    {
        return None;
    }
    Some(Read {
        end: r1 + 3,
        ty: (t0, t1),
        r: (r0, r1),
        c: s[r1 + 1],
    })
}

/// ` [!=]= -?\d+u?(?![\w.])` at `e`
fn lookahead_const(s: &[u8], e: usize) -> bool {
    if !(e + 3 < s.len()
        && s[e] == b' '
        && (s[e + 1] == b'!' || s[e + 1] == b'=')
        && s[e + 2] == b'='
        && s[e + 3] == b' ')
    {
        return false;
    }
    let mut i = e + 4;
    if i < s.len() && s[i] == b'-' {
        i += 1;
    }
    let d1 = digits_end(s, i);
    if d1 == i {
        return false;
    }
    let mut j = d1;
    if j < s.len() && s[j] == b'u' {
        j += 1;
    }
    !(j < s.len() && (is_word(s[j]) || s[j] == b'.'))
}

/// `_UTEST_LEFT.search(msl, max(0, start - 16), start)`: `(?<![\w.])-?\d+u? [!=]= $` ending exactly at `start`
fn utest_left(s: &[u8], start: usize) -> bool {
    let ws = start.saturating_sub(16);
    if start < 5 {
        return false;
    }
    // the tail ` X= ` (4 chars)
    if !(s[start - 1] == b' '
        && s[start - 2] == b'='
        && (s[start - 3] == b'!' || s[start - 3] == b'=')
        && s[start - 4] == b' ')
    {
        return false;
    }
    let mut k = start - 4;
    if k > ws && s[k - 1] == b'u' {
        k -= 1;
    }
    let d_end = k;
    let mut d = d_end;
    while d > ws && s[d - 1].is_ascii_digit() {
        d -= 1;
    }
    if d == d_end {
        return false;
    }
    // a match may start at the first digit (lookbehind sees the real previous character) or at a `-` before it
    let prev = if d > 0 { Some(s[d - 1]) } else { None };
    match prev {
        None => true,
        Some(b'-') => true,
        Some(p) => !(is_word(p) || p == b'.'),
    }
}

pub fn uniform_tests(src: &str) -> Result<Cow<'_, str>, PostError> {
    if !contains(src, "spvDescriptorSet0") || !contains(src, "as_type<") {
        return Ok(Cow::Borrowed(src));
    }
    let s = src.as_bytes();
    struct F {
        start: usize,
        end: usize,
        ty: (usize, usize),
        r: (usize, usize),
        c: u8,
    }
    let mut found: Vec<F> = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(s, b"as_type<", from) {
        from = p + 1;
        let Some(rd) = parse_read(s, p) else { continue };
        // alternative A
        let a_ok =
            (p == 0 || !(is_word(s[p - 1]) || s[p - 1] == b'.')) && lookahead_const(s, rd.end);
        let take = if a_ok {
            true
        } else if p >= 3
            && (s[p - 3] == b'!' || s[p - 3] == b'=')
            && s[p - 2] == b'='
            && s[p - 1] == b' '
        {
            utest_left(s, p)
        } else {
            false
        };
        if take {
            found.push(F {
                start: p,
                end: rd.end,
                ty: rd.ty,
                r: rd.r,
                c: rd.c,
            });
            from = rd.end;
        } else if !a_ok
            && p >= 3
            && (s[p - 3] == b'!' || s[p - 3] == b'=')
            && s[p - 2] == b'='
            && s[p - 1] == b' '
        {
            // alternative B matched the regex but `parts` returned None: the match is still consumed
            from = rd.end;
        }
    }
    let mut tested: HashMap<(&str, u8), u32> = HashMap::new();
    for f in &found {
        *tested.entry((&src[f.r.0..f.r.1], f.c)).or_insert(0) += 1;
    }
    if !tested.values().any(|&n| n > 1) {
        return Ok(Cow::Borrowed(src));
    }
    let mut out = String::with_capacity(src.len() + 512);
    let mut at = 0;
    for f in &found {
        if tested[&(&src[f.r.0..f.r.1], f.c)] <= 1 {
            continue;
        }
        out.push_str(&src[at..f.start]);
        let idx = b"xyzw".iter().position(|&x| x == f.c).unwrap();
        out.push_str(&format!(
            "as_type<{}>(dm_uword({}, {}))",
            &src[f.ty.0..f.ty.1],
            &src[f.r.0..f.r.1],
            idx
        ));
        at = f.end;
    }
    out.push_str(&src[at..]);
    // `^using namespace metal;\n` (re.M)
    let needle = b"using namespace metal;\n";
    let ob = out.as_bytes();
    let mut from = 0;
    let mut pos = None;
    while let Some(p) = find_b(ob, needle, from) {
        if p == 0 || ob[p - 1] == b'\n' {
            pos = Some(p + needle.len());
            break;
        }
        from = p + 1;
    }
    let Some(h) = pos else {
        return Err(failed(
            "uniform_tests",
            "uniform tests: no \"using namespace metal;\" to put dm_uword after",
        ));
    };
    out.insert_str(h, t::UWORD);
    Ok(Cow::Owned(out))
}
