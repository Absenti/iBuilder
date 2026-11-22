//! dmsh_rewrite.bit0_mask: the 0 / -1 mask of a tested bit 0 written as `-(x & 1)` instead of from its compare.
//!
//! The Python works on every function body (`BIT0_BODY = ^\{\n.*?^\}$`, re.M | re.S: a line `{` at column 0 up to the first line `}` at column 0) and, in it, on four line
//! shapes (`int _a = X & 1;`, `bool _b = _a != 0;`, `... = _b ? (-1) : 0;`) plus the writes / address-takes of the temporaries. Each regex is re-implemented as a scan.
use crate::text::*;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

const MASK_TAIL: &str = " ? (-1) : 0;";

pub fn bit0_mask(src: &str) -> Cow<'_, str> {
    if !contains(src, MASK_TAIL) {
        return Cow::Borrowed(src);
    }
    let b = src.as_bytes();
    let mut out: Option<String> = None;
    let mut at = 0usize;
    let mut from = 0usize;
    // BIT0_BODY.sub: matches left to right, non-overlapping
    while let Some(p) = body_start(b, from) {
        let Some(e) = body_end(b, p) else { break }; // no closing line after this start: none after any later start either
        if let Some(n) = body(&src[p..e]) {
            let o = out.get_or_insert_with(|| String::with_capacity(src.len() + 256));
            o.push_str(&src[at..p]);
            o.push_str(&n);
            at = e;
        }
        from = e;
    }
    match out {
        Some(mut o) => {
            o.push_str(&src[at..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(src),
    }
}

/// the next `^\{\n` at or after `from`
fn body_start(b: &[u8], from: usize) -> Option<usize> {
    let mut f = from;
    while let Some(p) = find_b(b, b"{\n", f) {
        if p == 0 || b[p - 1] == b'\n' {
            return Some(p);
        }
        f = p + 1;
    }
    None
}

/// the end (just after the `}`) of the lazy `.*?^\}$` that follows `{\n` at `p`
fn body_end(b: &[u8], p: usize) -> Option<usize> {
    let mut f = p + 1; // the newline of `{\n` is a candidate `^` for a `}` right after it
    while let Some(n) = find_b(b, b"\n}", f) {
        let q = n + 1;
        if q + 1 == b.len() || b[q + 1] == b'\n' {
            return Some(q + 1);
        }
        f = n + 1;
    }
    None
}

/// `^[ \t]*(u?int) (_\d+) = ([^;\n]+) & 1u?;$` on a line -> (type, name, left operand)
fn and_line(l: &str) -> Option<(&str, &str, &str)> {
    let mut c = Cur::at(l, 0);
    c.ht0();
    let ty = if c.lit("uint ") {
        "uint"
    } else if c.lit("int ") {
        "int"
    } else {
        return None;
    };
    let name = c.tmp_nb()?;
    if !c.lit(" = ") {
        return None;
    }
    let rest = &l[c.i..];
    let x = rest
        .strip_suffix(" & 1u;")
        .or_else(|| rest.strip_suffix(" & 1;"))?;
    if x.is_empty() || x.contains(';') {
        return None;
    }
    Some((ty, name, x))
}

/// `^[ \t]*bool (_\d+) = (_\d+) != 0u?;$` on a line -> (b, a)
fn test_line(l: &str) -> Option<(&str, &str)> {
    let mut c = Cur::at(l, 0);
    c.ht0();
    if !c.lit("bool ") {
        return None;
    }
    let bn = c.tmp_nb()?;
    if !c.lit(" = ") {
        return None;
    }
    let an = c.tmp_nb()?;
    if !c.lit(" != 0") {
        return None;
    }
    c.lit("u");
    if !c.lit(";") || !c.at_end() {
        return None;
    }
    Some((bn, an))
}

/// `re.sub(r'\([^()]*\)', '', e)`
fn strip_parens(e: &str) -> String {
    let b = e.as_bytes();
    let mut o = String::with_capacity(e.len());
    let mut i = 0;
    let mut at = 0;
    while i < b.len() {
        if b[i] == b'('
            && let Some(r) = b[i + 1..].iter().position(|&c| c == b'(' || c == b')')
            && b[i + 1 + r] == b')'
        {
            o.push_str(&e[at..i]);
            at = i + r + 2;
            i = at;
            continue;
        }
        i += 1;
    }
    o.push_str(&e[at..]);
    o
}

/// bit0_operand: the left operand outside its parentheses binds at least as tightly as `&`
fn operand_ok(e: &str) -> bool {
    let mut cur = e.to_string();
    loop {
        let f = strip_parens(&cur);
        if f == cur {
            // BIT0_LOOSER = `\|\||&&|[|^?,]`
            let b = cur.as_bytes();
            return !(b.iter().any(|&c| matches!(c, b'|' | b'^' | b'?' | b','))
                || contains(&cur, "&&"));
        }
        cur = f;
    }
}

/// BIT0_WRITE `\b(_\d+)\b[ \t]*(?:<<|>>|[-+*/%&|^])?=(?!=)`: the temporaries assigned (a declaration's `=` too), counted
fn count_writes(f: &str) -> HashMap<&str, u32> {
    let b = f.as_bytes();
    let mut m: HashMap<&str, u32> = HashMap::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'_' && !in_word(b, i) {
            let mut c = Cur::at(f, i);
            if let Some(name) = c.tmp() {
                c.ht0();
                if c.lit("<<") || c.lit(">>") {
                } else if matches!(
                    c.peek(),
                    Some(b'-' | b'+' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
                ) {
                    c.i += 1;
                }
                // the optional operator is greedy but backtracks: with no operator the `=` must be right here (an operator char is never `=`)
                if c.lit("=") && c.peek() != Some(b'=') {
                    *m.entry(name).or_insert(0) += 1;
                    i = c.i;
                    continue;
                }
                // `<<` / `>>` matched but no `=` after: the operator alternatives `[-+*/%&|^]` cannot match `<` / `>`, so no match here
            }
        }
        i += 1;
    }
    m
}

/// BIT0_OTHER `(?:\+\+|--)[ \t]*(_\d+)\b|\b(_\d+)[ \t]*(?:\+\+|--)|&(_\d+)\b`: temporaries that are incremented or have their address taken
fn others(f: &str) -> HashSet<&str> {
    let b = f.as_bytes();
    let mut s = HashSet::new();
    let mut i = 0;
    while i < b.len() {
        let mut c = Cur::at(f, i);
        let mut hit: Option<&str> = None;
        if c.lit("++") || c.lit("--") {
            c.ht0();
            hit = c.tmp();
        } else if b[i] == b'_' && !in_word(b, i) {
            if let Some(n) = c.tmp() {
                c.ht0();
                if c.lit("++") || c.lit("--") {
                    hit = Some(n);
                }
            }
        } else if b[i] == b'&' {
            c.i += 1;
            hit = c.tmp();
        }
        match hit {
            Some(n) => {
                s.insert(n);
                i = c.i;
            }
            None => i += 1,
        }
    }
    s
}

/// one function body (the text from `{\n` to its closing `}`) -> the rewritten body, None when unchanged
fn body(f: &str) -> Option<String> {
    if !contains(f, MASK_TAIL) {
        return None;
    }
    let mut bits: HashMap<&str, &str> = HashMap::new(); // name -> type (the last declaration wins)
    for l in f.split('\n') {
        if let Some((ty, n, x)) = and_line(l)
            && operand_ok(x)
        {
            bits.insert(n, ty);
        }
    }
    let mut tests: HashMap<&str, &str> = HashMap::new(); // b -> a
    for l in f.split('\n') {
        if let Some((bn, an)) = test_line(l)
            && bits.contains_key(an)
        {
            tests.insert(bn, an);
        }
    }
    if tests.is_empty() {
        return None;
    }
    let writes = count_writes(f);
    let other = others(f);
    tests.retain(|b, a| {
        writes.get(*a) == Some(&1)
            && writes.get(*b) == Some(&1)
            && !other.contains(*a)
            && !other.contains(*b)
    });
    if tests.is_empty() {
        return None;
    }
    // BIT0_MASK `= (_\d+) \? \(-1\) : 0;`
    let b = f.as_bytes();
    let mut o = String::with_capacity(f.len() + 64);
    let (mut at, mut from, mut changed) = (0, 0, false);
    while let Some(p) = find_b(b, MASK_TAIL.as_bytes(), from) {
        from = p + 1;
        // the `_\d+` before the tail, then `= `
        let mut d = p;
        while d > 0 && b[d - 1].is_ascii_digit() {
            d -= 1;
        }
        if d == p || d < 3 || b[d - 1] != b'_' || &b[d - 3..d - 1] != b"= " {
            continue;
        }
        let q = &f[d - 1..p];
        let Some(a) = tests.get(q) else { continue };
        o.push_str(&f[at..d - 3]);
        if bits[a] == "int" {
            o.push_str(&format!("= -{a};   // bit 0 as a mask (bit0_mask)"));
        } else {
            o.push_str(&format!("= -int({a});   // bit 0 as a mask (bit0_mask)"));
        }
        at = p + MASK_TAIL.len();
        from = at;
        changed = true;
    }
    if !changed {
        return None;
    }
    o.push_str(&f[at..]);
    Some(o)
}
