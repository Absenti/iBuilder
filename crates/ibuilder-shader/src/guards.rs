//! dmsh_rewrite.select_guards: bounded uniform / storage reads written as `select(0, read, cond)` (the hottest rewrite of the chain: 2.4 ms a shader in Python).
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;
use std::collections::HashSet;

/// `(*spvDescriptorSet0.` followed by `[a-z]+` and the literal `suffix_prefix` (`_c` / `_s`), returns the index after the letters
fn set0_letters(s: &[u8], i: usize) -> Option<usize> {
    if !starts(s, i, "(*spvDescriptorSet0.") {
        return None;
    }
    let a = i + 20;
    let mut e = a;
    while e < s.len() && s[e].is_ascii_lowercase() {
        e += 1;
    }
    if e == a { None } else { Some(e) }
}

/// the start of the GSIZE that ends exactly at `e` (just after its last `)`): `((*spvDescriptorSet0.[a-z]+_c25)._m0[\d].[xyzw] >> (?:2|4)u)` (a size in words or in vec4 rows)
fn gsize_start(s: &[u8], e: usize) -> Option<usize> {
    let tail = b" >> 2u)";
    if e < tail.len()
        || &s[e - tail.len()..e - 3] != b" >> "
        || !matches!(s[e - 3], b'2' | b'4')
        || &s[e - 2..e] != b"u)"
    {
        return None;
    }
    let mut k = e - tail.len();
    if k == 0 || !matches!(s[k - 1], b'x' | b'y' | b'z' | b'w') {
        return None;
    }
    k -= 1;
    if k < 2 || &s[k - 2..k] != b"]." {
        return None;
    }
    k -= 2;
    if k == 0 || !s[k - 1].is_ascii_digit() {
        return None;
    }
    k -= 1;
    let lit = format!("_c{})._m0[", t::CBSIZE_BANK);
    let lb = lit.as_bytes();
    if k < lb.len() || &s[k - lb.len()..k] != lb {
        return None;
    }
    k -= lb.len();
    let mut l = k;
    while l > 0 && s[l - 1].is_ascii_lowercase() {
        l -= 1;
    }
    if l == k {
        return None;
    }
    let pre = b"((*spvDescriptorSet0.";
    if l >= pre.len() && &s[l - pre.len()..l] == pre {
        Some(l - pre.len())
    } else {
        None
    }
}

/// STABLE at `i`: `\(\*spvDescriptorSet0\.[a-z]+_c27\)\._m0\[\d+\]\.[xyzw]` -> end
fn parse_stable(s: &[u8], i: usize) -> Option<usize> {
    let e = set0_letters(s, i)?;
    let lit = format!("_c{})._m0[", t::SBLEN_BANK);
    if !starts(s, e, &lit) {
        return None;
    }
    let d0 = e + lit.len();
    let d1 = digits_end(s, d0);
    if d1 == d0 || !starts(s, d1, "].") {
        return None;
    }
    let c = d1 + 2;
    if c < s.len() && matches!(s[c], b'x' | b'y' | b'z' | b'w') {
        Some(c + 1)
    } else {
        None
    }
}

/// `\bbool (_\d+) = [^;\n]* < GSIZE;` names
fn gbools(src: &str) -> HashSet<&str> {
    let s = src.as_bytes();
    let mut out = HashSet::new();
    let mut from = 0;
    while let Some(p) = find_b(s, b"bool _", from) {
        from = p + 5;
        if in_word(s, p) {
            continue;
        }
        let n0 = p + 5; // at `_`
        let n1 = digits_end(s, n0 + 1);
        if n1 == n0 + 1 || !starts(s, n1, " = ") {
            continue;
        }
        let seg0 = n1 + 3;
        let Some(rel) = memchr::memchr2(b';', b'\n', &s[seg0..]) else {
            continue;
        };
        let semi = seg0 + rel;
        if s[semi] != b';' {
            continue;
        }
        if let Some(gs) = gsize_start(s, semi)
            && gs >= seg0 + 3
            && &s[gs - 3..gs] == b" < "
        {
            out.insert(&src[n0..n1]);
        }
    }
    out
}

/// `\bbool (_\d+) = \(uint\(_\d+\) \+ STABLE\) < STABLE;` names
fn sbools(src: &str) -> HashSet<&str> {
    let s = src.as_bytes();
    let mut out = HashSet::new();
    let mut from = 0;
    while let Some(p) = find_b(s, b"bool _", from) {
        from = p + 5;
        if in_word(s, p) {
            continue;
        }
        let n0 = p + 5;
        let n1 = digits_end(s, n0 + 1);
        if n1 == n0 + 1 || !starts(s, n1, " = (uint(_") {
            continue;
        }
        let d0 = n1 + 10;
        let d1 = digits_end(s, d0);
        if d1 == d0 || !starts(s, d1, ") + ") {
            continue;
        }
        let Some(e1) = parse_stable(s, d1 + 4) else {
            continue;
        };
        if !starts(s, e1, ") < ") {
            continue;
        }
        let Some(e2) = parse_stable(s, e1 + 4) else {
            continue;
        };
        if e2 < s.len() && s[e2] == b';' {
            out.insert(&src[n0..n1]);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Row,
    S,
}

struct M {
    start: usize,
    end: usize,
    form1: bool,
    q: (usize, usize),
    r: (usize, usize),
    z: (usize, usize),
    i: (usize, usize),
}

/// the read pattern at `i`: `\(\*spvDescriptorSet0\.[a-z]+_(c|s)\d+\)\._m0\[(?P<i>_\d+) \? [^\]\n]+ : 0\]` and, for Kind::Row, `(?:\[[^\]\n]+\]|\.[xyzw])` -> (end, index var range)
fn parse_read(s: &[u8], i: usize, kind: Kind) -> Option<(usize, (usize, usize))> {
    let e = set0_letters(s, i)?;
    let want = if kind == Kind::Row { b'c' } else { b's' };
    if !(starts(s, e, "_") && e + 1 < s.len() && s[e + 1] == want) {
        return None;
    }
    let d0 = e + 2;
    let d1 = digits_end(s, d0);
    if d1 == d0 || !starts(s, d1, ")._m0[") {
        return None;
    }
    let v0 = d1 + 6;
    if v0 >= s.len() || s[v0] != b'_' {
        return None;
    }
    let v1 = digits_end(s, v0 + 1);
    if v1 == v0 + 1 || !starts(s, v1, " ? ") {
        return None;
    }
    let x0 = v1 + 3;
    // `[^\]\n]+ : 0\]`: up to the first ']' or '\n'; the region must end with " : 0" and hold at least one character before it
    let rel = memchr::memchr2(b']', b'\n', &s[x0..])?;
    let y = x0 + rel;
    if s[y] != b']' {
        return None;
    }
    if y < x0 + 5 || &s[y - 4..y] != b" : 0" {
        return None;
    }
    let mut end = y + 1;
    if kind == Kind::Row {
        if end < s.len() && s[end] == b'[' {
            let rel = memchr::memchr2(b']', b'\n', &s[end + 1..]);
            match rel {
                Some(r) if s[end + 1 + r] == b']' && r >= 1 => {
                    end = end + 1 + r + 1;
                }
                _ => return trailer_dot(s, end).map(|e| (e, (v0, v1))),
            }
        } else {
            return trailer_dot(s, end).map(|e| (e, (v0, v1)));
        }
    }
    Some((end, (v0, v1)))
}
fn trailer_dot(s: &[u8], at: usize) -> Option<usize> {
    if at + 1 < s.len() && s[at] == b'.' && matches!(s[at + 1], b'x' | b'y' | b'z' | b'w') {
        Some(at + 2)
    } else {
        None
    }
}

/// `0(?:\.0|u)?` followed by `follow` -> end
fn parse_zero(s: &[u8], i: usize, form1: bool) -> Option<(usize, usize)> {
    if i >= s.len() || s[i] != b'0' {
        return None;
    }
    let ok = |e: usize| -> bool {
        if form1 {
            e < s.len() && s[e] == b')'
        } else {
            !(e < s.len() && (is_word(s[e]) || s[e] == b'.'))
        }
    };
    let a = i + 1;
    if starts(s, a, ".0") && ok(a + 2) {
        return Some((a + 2, a + 2));
    }
    if starts(s, a, "u") && ok(a + 1) {
        return Some((a + 1, a + 1));
    }
    if ok(a) {
        return Some((a, a));
    }
    None
}

fn try_form(s: &[u8], q0: usize, form1: bool, kind: Kind) -> Option<M> {
    // q = `_\d+` at q0
    if q0 >= s.len() || s[q0] != b'_' {
        return None;
    }
    let q1 = digits_end(s, q0 + 1);
    if q1 == q0 + 1 || !starts(s, q1, " ? ") {
        return None;
    }
    let r0 = q1 + 3;
    let (r1, i) = parse_read(s, r0, kind)?;
    if !starts(s, r1, " : ") {
        return None;
    }
    let z0 = r1 + 3;
    let (z1, endz) = parse_zero(s, z0, form1)?;
    let (start, end) = if form1 {
        (q0 - 1, endz + 1)
    } else {
        (q0, endz)
    };
    Some(M {
        start,
        end,
        form1,
        q: (q0, q1),
        r: (r0, r1),
        z: (z0, z1),
        i,
    })
}

fn grow_sub(src: &str, conds: &HashSet<&str>, kind: Kind) -> Option<String> {
    let s = src.as_bytes();
    let needle = b" ? (*spvDescriptorSet0.";
    let mut out: Option<String> = None;
    let mut at = 0usize; // copied up to
    let mut last_end = 0usize;
    let mut from = 0usize;
    while let Some(c) = find_b(s, needle, from) {
        from = c + 1;
        // q: digits back from c, then `_`
        let mut d = c;
        while d > 0 && s[d - 1].is_ascii_digit() {
            d -= 1;
        }
        if d == c || d == 0 || s[d - 1] != b'_' {
            continue;
        }
        let q0 = d - 1;
        let mut m = None;
        if q0 >= 1 && s[q0 - 1] == b'(' && q0 > last_end {
            let pb = if q0 >= 2 { Some(s[q0 - 2]) } else { None };
            let look_ok = match pb {
                Some(p) => !(is_word(p) || p == b'>'),
                None => true,
            };
            if look_ok {
                m = try_form(s, q0, true, kind);
            }
        }
        if m.is_none() && q0 >= last_end {
            m = try_form(s, q0, false, kind);
        }
        let Some(m) = m else { continue };
        last_end = m.end;
        from = m.end;
        let qn = &src[m.q.0..m.q.1];
        let iv = &src[m.i.0..m.i.1];
        if conds.contains(qn) && iv == qn {
            let o = out.get_or_insert_with(|| String::with_capacity(src.len()));
            o.push_str(&src[at..m.start]);
            o.push_str("select(");
            o.push_str(&src[m.z.0..m.z.1]);
            o.push_str(", ");
            o.push_str(&src[m.r.0..m.r.1]);
            o.push_str(", ");
            o.push_str(qn);
            o.push(')');
            at = m.end;
        }
        let _ = m.form1;
    }
    out.map(|mut o| {
        o.push_str(&src[at..]);
        o
    })
}

/// `GSIZE\) \?` anywhere: the leftmost (the position of its ` >> `)
fn gleft(s: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(q) = find_b(s, b"u)) ?", from) {
        from = q + 1;
        if q >= 5 && gsize_start(s, q + 2).is_some() {
            return Some(q - 5);
        }
    }
    None
}

pub fn select_guards(src: &str) -> Result<Cow<'_, str>, PostError> {
    if !contains(src, "spvDescriptorSet0") {
        return Ok(Cow::Borrowed(src));
    }
    let bools: HashSet<&str> = gbools(src);
    let sb: HashSet<&str> = sbools(src);
    let mut cur: Cow<str> = Cow::Borrowed(src);
    // the sets borrow from `src`; names are copied before the text changes
    let bools: HashSet<String> = bools.into_iter().map(String::from).collect();
    let sb: HashSet<String> = sb.into_iter().map(String::from).collect();
    if !bools.is_empty() {
        let set: HashSet<&str> = bools.iter().map(|x| x.as_str()).collect();
        if let Some(n) = grow_sub(&cur, &set, Kind::Row) {
            cur = Cow::Owned(n);
        }
    }
    if !sb.is_empty() {
        let set: HashSet<&str> = sb.iter().map(|x| x.as_str()).collect();
        if let Some(n) = grow_sub(&cur, &set, Kind::S) {
            cur = Cow::Owned(n);
        }
    }
    let s = cur.as_bytes();
    // (where the match starts, where it ends): the message is cut 100 characters after the END of the match, as the Python's `left.end() + 100`
    let mut left: Option<(usize, usize)> = gleft(s).map(|p| (p, p + b" >> 2u)) ?".len()));
    if left.is_none() && (!bools.is_empty() || !sb.is_empty()) {
        // (?<!\[)\b(_\d+) \?  with the name in bools | sbools
        let mut from = 0;
        while let Some(p) = find_b(s, b" ?", from) {
            from = p + 1;
            let mut d = p;
            while d > 0 && s[d - 1].is_ascii_digit() {
                d -= 1;
            }
            if d == p || d == 0 || s[d - 1] != b'_' {
                continue;
            }
            let q0 = d - 1;
            if in_word(s, q0) || (q0 > 0 && s[q0 - 1] == b'[') {
                continue;
            }
            let name = &cur[q0..p];
            if bools.contains(name) || sb.contains(name) {
                left = Some((q0, p + b" ?".len()));
                break;
            }
        }
    }
    if let Some((at, end)) = left {
        let ls = rfind_before(s, b"\n", at).map(|x| x + 1).unwrap_or(0);
        // `msl[...:left.end() + 100]` counts characters: a byte cut could split a UTF-8 character (a panic)
        let le = cur[end..]
            .char_indices()
            .nth(100)
            .map_or(s.len(), |(i, _)| end + i);
        let line = cur[ls..le].split('\n').next().unwrap_or("").trim();
        let line: String = line.chars().take(160).collect();
        return Err(failed(
            "select_guards",
            format!("a bounded uniform or storage read is not written as a select: {line}"),
        ));
    }
    Ok(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_message_is_cut_by_characters_after_the_match_end() {
        // Python: msl[line start:left.end() + 100].split('\n')[0].strip()[:160]; the old cut was 100 BYTES after the match START (inside an `é`: a panic)
        let head = "float a = (i < ((*spvDescriptorSet0.fp_c25)._m0[0].x >> 2u)) ? 1 : 0; // ";
        let src = format!("spvDescriptorSet0\n{head}{}\nnext", "é".repeat(120));
        let Err(PostError::Failed { step, msg }) = select_guards(&src) else {
            panic!("select_guards accepted an unguarded read");
        };
        assert_eq!(step, "select_guards");
        assert_eq!(
            msg,
            format!(
                "a bounded uniform or storage read is not written as a select: {head}{}",
                "é".repeat(87)
            )
        );
    }
}
