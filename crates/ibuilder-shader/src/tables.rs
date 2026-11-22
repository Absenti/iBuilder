//! `c1_guard.py` and `cb_extent.py`: the two tables stage 3 makes from the finished library (c1_full.tsv, cb_extent.tsv). Serial pure Python under the GIL in the engine
//! (269 s of a 733 s Grizzy build on the Mac, measured); here every shader is one rayon job over the memory-mapped image.
use crate::interval::{Interval, known};
use crate::romfs::{FileRef, Image};
use crate::text::*;
use rayon::prelude::*;
use std::collections::HashMap;

/// `[0-9a-f]{16}_b\w*\.(metal|json)` fullmatch -> the extension
fn output_ext(name: &str) -> Option<&'static str> {
    let b = name.as_bytes();
    if b.len() < 16 + 2
        || !b[..16]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        || !(b[16] == b'_' && b[17] == b'b')
    {
        return None;
    }
    let rest = &name[18..];
    // `\w*\.(metal|json)`: the last dot; the stem is word characters only
    let (stem, ext) = rest.rsplit_once('.')?;
    if !stem.bytes().all(is_word) {
        return None;
    }
    match ext {
        "metal" => Some("metal"),
        "json" => Some("json"),
        _ => None,
    }
}

/// The tables are made from the library folder the build's **Mac side** writes (`<key>_b.metal`, `<key>_b.json` and the geometry variants), which in source mode holds none of the
/// iOS-only files (`<key>_bx` / `_bf` / `_bv` / `_bp`, the placeholder libraries the host compiles from source): the final image holds those too, so they are left out here.
fn mac_side(name: &str) -> bool {
    // `<16 hex>_b<stem>.<ext>`: stem x / f / v / p is iOS-only
    let stem = &name[18..name.rfind('.').unwrap_or(name.len())];
    !matches!(stem, "x" | "f" | "v" | "p")
}

/// text_of: UTF-8, universal newlines
fn text_of(data: &[u8]) -> Option<std::borrow::Cow<'_, str>> {
    let s = std::str::from_utf8(data).ok()?;
    if s.contains('\r') {
        Some(std::borrow::Cow::Owned(
            s.replace("\r\n", "\n").replace('\r', "\n"),
        ))
    } else {
        Some(std::borrow::Cow::Borrowed(s))
    }
}

// ---------------------------------------------------------------------------------------------------------------- definitions_of

/// `definitions_of`: {name: (kind, text)} of the integer temporaries written only where declared
pub fn definitions_of(src: &str) -> HashMap<String, (String, String)> {
    let b = src.as_bytes();
    let mut writes: HashMap<&str, u32> = HashMap::new();
    // ASSIGNMENT: (?<![\w.])(_\d+)\s*(?:<<|>>|[-+*/%&|^])?=(?!=)|(?<![\w.])(_\d+)\s*(?:\+\+|--)|(?:\+\+|--)\s*(_\d+)\b
    let mut i = 0;
    let n = b.len();
    while i < n {
        let c = b[i];
        if c == b'_'
            && i + 1 < n
            && b[i + 1].is_ascii_digit()
            && (i == 0 || !(is_word(b[i - 1]) || b[i - 1] == b'.'))
        {
            let d1 = digits_end(b, i + 1);
            let name = &src[i..d1];
            let k = skip_space(b, d1);
            // alt 1
            let mut m = None;
            if starts(b, k, "<<") || starts(b, k, ">>") {
                let e = k + 2;
                if e < n && b[e] == b'=' && !(e + 1 < n && b[e + 1] == b'=') {
                    m = Some(e + 1);
                }
            }
            if m.is_none() {
                let mut e2 = k;
                if e2 < n && matches!(b[e2], b'-' | b'+' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
                {
                    e2 += 1;
                    if e2 < n && b[e2] == b'=' && !(e2 + 1 < n && b[e2 + 1] == b'=') {
                        m = Some(e2 + 1);
                    }
                }
            }
            if m.is_none() && k < n && b[k] == b'=' && !(k + 1 < n && b[k + 1] == b'=') {
                m = Some(k + 1);
            }
            // alt 2
            if m.is_none() && (starts(b, k, "++") || starts(b, k, "--")) {
                m = Some(k + 2);
            }
            if let Some(end) = m {
                *writes.entry(name).or_insert(0) += 1;
                i = end;
                continue;
            }
            i += 1;
            continue;
        }
        if (c == b'+' || c == b'-') && i + 1 < n && b[i + 1] == c {
            let k = skip_space(b, i + 2);
            if k < n && b[k] == b'_' && k + 1 < n && b[k + 1].is_ascii_digit() {
                let d1 = digits_end(b, k + 1);
                if !(d1 < n && is_word(b[d1])) {
                    // `\b` after the digits
                    *writes.entry(&src[k..d1]).or_insert(0) += 1;
                    i = d1;
                    continue;
                }
            }
        }
        i += 1;
    }
    // DEFINITION: \b(u?int|u?short|u?long|bool)\s+(_\d+)\s*=\s*([^;]+);
    let mut decls: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
    let mut counts: HashMap<&str, u32> = HashMap::new();
    let mut found: Vec<(usize, usize, &str, &str, &str)> = Vec::new();
    for kw in ["int", "short", "long", "bool"] {
        let mut from = 0;
        while let Some(p) = find_b(b, kw.as_bytes(), from) {
            from = p + 1;
            // optional `u` before int / short / long, with \b before it
            let mut st = p;
            if kw != "bool" && p > 0 && b[p - 1] == b'u' {
                st = p - 1;
            }
            // the regex tries `u?int` at a \b position: when `u` is present the start is the `u`; otherwise the keyword itself
            let cand = if st != p { [st, p] } else { [p, p] };
            for (ci, &s0) in cand.iter().enumerate() {
                if ci == 1 && cand[0] == cand[1] {
                    break;
                }
                if in_word(b, s0) {
                    continue;
                }
                let mut q = p + kw.len();
                let q1 = skip_space(b, q);
                if q1 == q {
                    continue;
                }
                q = q1;
                if q >= n || b[q] != b'_' {
                    continue;
                }
                let d1 = digits_end(b, q + 1);
                if d1 == q + 1 {
                    continue;
                }
                let q2 = skip_space(b, d1);
                if q2 >= n || b[q2] != b'=' {
                    continue;
                }
                let t0 = skip_space(b, q2 + 1);
                // `\s*([^;]+);`: the greedy `\s*` may give back whitespace so that `[^;]+` has at least one character
                let Some(semi) = memchr::memchr(b';', &b[q2 + 1..]).map(|x| x + q2 + 1) else {
                    continue;
                };
                if semi == q2 + 1 {
                    continue;
                }
                let t0 = if t0 >= semi { semi - 1 } else { t0 };
                // the kind text as the regex captured it
                let kind = &src[s0..p + kw.len()];
                found.push((s0, semi + 1, kind, &src[q..d1], &src[t0..semi]));
                break;
            }
        }
    }
    found.sort_by_key(|f| f.0);
    let mut last_end = 0;
    for (st, en, kind, name, text) in found {
        if st < last_end {
            continue;
        }
        last_end = en;
        let d = decls.entry(name).or_default();
        let item = (kind, strip(text));
        if !d.contains(&item) {
            d.push(item);
        }
        *counts.entry(name).or_insert(0) += 1;
    }
    let mut out = HashMap::new();
    for (name, d) in decls {
        if d.len() == 1 && writes.get(name).copied().unwrap_or(0) == counts[name] {
            out.insert(name.to_string(), (d[0].0.to_string(), d[0].1.to_string()));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------- c1_guard

/// `ACCESS = \b[a-z]+_c1\)\._m0\[([^\]]+)\]` -> the set of index texts
fn c1_accesses(src: &str) -> Vec<&str> {
    let b = src.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let needle = b"_c1)._m0[";
    let mut from = 0;
    while let Some(p) = find_b(b, needle, from) {
        from = p + 1;
        // `\b[a-z]+` before: lowercase letters, with a non-word character before them
        let mut k = p;
        while k > 0 && b[k - 1].is_ascii_lowercase() {
            k -= 1;
        }
        if k == p {
            continue;
        }
        // `\b` before the letters: the previous character is not a word character (the run of letters may be preceded by digits or `_`, which are word characters)
        if in_word(b, k) {
            // the regex may start inside the run: `\b` needs a boundary at the start of `[a-z]+`; a shorter suffix of the letters is preceded by a letter (a word character): no boundary
            continue;
        }
        let i0 = p + needle.len();
        let Some(rel) = memchr::memchr(b']', &b[i0..]) else {
            continue;
        };
        if rel == 0 {
            continue;
        }
        let idx = &src[i0..i0 + rel];
        if !out.contains(&idx) {
            out.push(idx);
        }
    }
    out
}

fn c1_span_of(reflection: Option<&[u8]>) -> Option<i64> {
    let data = reflection?;
    let txt = text_of(data)?;
    let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let len = v
        .get("c1")
        .and_then(|c| c.as_str())
        .map(|s| s.chars().count())
        .unwrap_or(0) as i64
        / 2;
    Some((len + 255) / 256 * 256 + 256)
}

/// whether one of the shader's c1 reads is not proven bounded
pub fn unproven_in(src: &str, reflection: Option<&[u8]>) -> bool {
    let acc = c1_accesses(src);
    if acc.is_empty() {
        return false;
    }
    let defs = definitions_of(src);
    let span = c1_span_of(reflection);
    !acc.iter().all(|idx| {
        let (low, high) = Interval::new(&defs).expression(idx);
        match span {
            // `16 * (high + 1) <= span` in Python's unbounded ints: a product past i128 is past any span, so an overflow is "not proven" (a wrapped product proved it)
            Some(sp) => {
                known(low, high)
                    && low.unwrap() >= 0
                    && row_end(high.unwrap()).is_some_and(|e| e <= sp as i128)
            }
            None => false,
        }
    })
}

/// c1_guard.main: the keys the table lists, in the order of the library's sorted `.metal` names
pub fn c1_guard(img: &Image) -> Vec<String> {
    let names: Vec<&FileRef> = img
        .files
        .iter()
        .filter(|f| output_ext(&f.name) == Some("metal") && mac_side(&f.name))
        .collect();
    let res: Vec<Option<String>> = names
        .par_iter()
        .map(|f| {
            let key = if let Some(k) = f.name.strip_suffix("_b.metal") {
                k
            } else {
                &f.name[..f.name.len() - 6]
            };
            let metal = img.bytes(f);
            let reflection = img.find(&format!("{key}_b.json")).map(|r| img.bytes(r));
            let src = text_of(metal)?;
            if unproven_in(&src, reflection) {
                Some(key.to_string())
            } else {
                None
            }
        })
        .collect();
    res.into_iter().flatten().collect()
}

// ---------------------------------------------------------------------------------------------------------------- cb_extent

struct Binding {
    kind: String,
    reg: Option<i64>,
    msl_name: String,
}

fn bindings_of(v: &serde_json::Value) -> Vec<Binding> {
    v.get("bindings")
        .and_then(|b| b.as_array())
        .map(|a| {
            a.iter()
                .map(|b| Binding {
                    kind: b
                        .get("kind")
                        .and_then(|k| k.as_str())
                        .unwrap_or("")
                        .to_string(),
                    reg: b.get("reg").and_then(|r| r.as_i64()),
                    msl_name: b
                        .get("msl_name")
                        .and_then(|r| r.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

struct Scan<'a> {
    /// maximal word runs -> count
    uses: HashMap<&'a str, u32>,
    /// `\bNAME\s*\[\[id\(` -> count
    declared: HashMap<&'a str, u32>,
    /// `\(\*spvDescriptorSet0\.NAME\)\._m0\[([^\]]+)\]` -> index texts in order
    reads: HashMap<&'a str, Vec<&'a str>>,
}

fn scan(src: &str) -> Scan<'_> {
    let b = src.as_bytes();
    let mut uses: HashMap<&str, u32> = HashMap::new();
    let mut declared: HashMap<&str, u32> = HashMap::new();
    let mut reads: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut i = 0;
    let n = b.len();
    while i < n {
        if is_word(b[i]) {
            let e = word_end(b, i);
            let w = &src[i..e];
            *uses.entry(w).or_insert(0) += 1;
            let k = skip_space(b, e);
            if starts(b, k, "[[id(") {
                *declared.entry(w).or_insert(0) += 1;
            }
            if w == "spvDescriptorSet0" && i >= 2 && &b[i - 2..i] == b"(*" && e < n && b[e] == b'.'
            {
                let w0 = e + 1;
                let w1 = word_end(b, w0);
                if w1 > w0 && starts(b, w1, ")._m0[") {
                    let i0 = w1 + 6;
                    if let Some(rel) = memchr::memchr(b']', &b[i0..])
                        && rel > 0
                    {
                        reads
                            .entry(&src[w0..w1])
                            .or_default()
                            .push(&src[i0..i0 + rel]);
                    }
                }
            }
            i = e;
        } else {
            i += 1;
        }
    }
    Scan {
        uses,
        declared,
        reads,
    }
}

const MAX: i128 = 65536;

/// `16 * (high + 1)`, the end of the last 16-byte row read: `None` when it does not fit an i128
fn row_end(high: i128) -> Option<i128> {
    high.checked_add(1)?.checked_mul(16)
}

fn extents(src: &str, bindings: &[Binding]) -> Vec<(i64, i128)> {
    let mut out: Vec<(i64, i128)> = Vec::new();
    let mut sc: Option<Scan> = None;
    let mut defs: Option<HashMap<String, (String, String)>> = None;
    for b in bindings {
        if b.kind != "cbuf" || b.reg.is_none() {
            continue;
        }
        let s = sc.get_or_insert_with(|| scan(src));
        let name = b.msl_name.as_str();
        let uses = s.uses.get(name).copied().unwrap_or(0) as usize;
        let reads = s.reads.get(name).map(|v| v.as_slice()).unwrap_or(&[]);
        let declared = s.declared.get(name).copied().unwrap_or(0) as usize;
        if uses != reads.len() + declared {
            continue;
        }
        let mut high: Option<i128> = Some(-1);
        let mut seen: Vec<&str> = Vec::new();
        for idx in reads {
            if seen.contains(idx) {
                continue;
            }
            seen.push(idx);
            let d = defs.get_or_insert_with(|| definitions_of(src));
            let (low, top) = Interval::new(d).expression(idx);
            if !known(low, top) || low.unwrap() < 0 {
                high = None;
                break;
            }
            high = Some(high.unwrap().max(top.unwrap()));
        }
        // `16 * (high + 1) >= MAX`: an overflow (Python's product is past MAX) is left out too
        let Some(ext) = high.and_then(row_end).filter(|&e| e < MAX) else {
            continue;
        };
        let reg = b.reg.unwrap();
        if let Some(e) = out.iter_mut().find(|(r, _)| *r == reg) {
            e.1 = ext;
        } else {
            out.push((reg, ext));
        }
    }
    out.sort();
    out
}

/// WRITE.match(source, j): `\s*(?:=(?!=)|[-+*/%&|^]=|<<=|>>=|\+\+|--)`
fn write_at(b: &[u8], j: usize) -> bool {
    let k = skip_space(b, j);
    let n = b.len();
    if k < n && b[k] == b'=' && !(k + 1 < n && b[k + 1] == b'=') {
        return true;
    }
    if k + 1 < n
        && matches!(b[k], b'-' | b'+' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
        && b[k + 1] == b'='
    {
        return true;
    }
    if starts(b, k, "<<=") || starts(b, k, ">>=") || starts(b, k, "++") || starts(b, k, "--") {
        return true;
    }
    false
}

fn unwritten(src: &str, bindings: &[Binding]) -> Vec<i64> {
    let b = src.as_bytes();
    let mut out: Vec<i64> = Vec::new();
    for bd in bindings {
        if bd.kind != "sbuf" || bd.reg.is_none() {
            continue;
        }
        let name = format!("spvDescriptorSet0.{}", bd.msl_name);
        let nb = name.as_bytes();
        let mut ok = true;
        // `\(\*NAME\b` occurrences
        let mut total_pref = 0usize; // occurrences of `(*NAME\b`
        let mut from = 0;
        let pat = format!("(*{name}");
        while let Some(p) = find_b(b, pat.as_bytes(), from) {
            from = p + 1;
            let end = p + pat.len();
            if end < b.len() && is_word(b[end]) {
                continue;
            } // `\b` after NAME
            total_pref += 1;
            if !ok {
                continue;
            }
            let j = end;
            if (p > 0 && b[p - 1] == b'&') || !starts(b, j, ")._m0[") {
                ok = false;
                continue;
            }
            let mut j = j + 5; // len(")._m0")
            let mut depth = 0i32;
            while j < b.len() {
                let c = b[j];
                if c == b'[' {
                    depth += 1;
                } else if c == b']' {
                    depth -= 1;
                } else if depth == 0 && !(c == b'.' || c.is_ascii_alphanumeric() || c == b'_') {
                    break;
                }
                j += 1;
            }
            if write_at(b, j) {
                ok = false;
            }
        }
        if !ok {
            continue;
        }
        // `NAME\b` occurrences anywhere
        let mut all = 0usize;
        let mut from = 0;
        while let Some(p) = find_b(b, nb, from) {
            from = p + 1;
            let end = p + nb.len();
            if !(end < b.len() && is_word(b[end])) {
                all += 1;
            }
        }
        if all == total_pref {
            out.push(bd.reg.unwrap());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// cb_extent.tokens: the slots of a table line, "" for no line
pub fn cb_tokens(reflection: Option<&[u8]>, metal: Option<&[u8]>) -> String {
    let Some(refl) = reflection else {
        return String::new();
    };
    let Some(txt) = text_of(refl) else {
        return String::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return String::new();
    };
    let bindings = bindings_of(&v);
    if !bindings
        .iter()
        .any(|b| b.kind == "cbuf" || b.kind == "sbuf")
    {
        return String::new();
    }
    let Some(metal) = metal else {
        return String::new();
    };
    let Some(src) = text_of(metal) else {
        return String::new();
    };
    let mut parts: Vec<String> = extents(&src, &bindings)
        .into_iter()
        .map(|(r, n)| format!("{r}:{n}"))
        .collect();
    parts.extend(
        unwritten(&src, &bindings)
            .into_iter()
            .map(|r| format!("s{r}")),
    );
    parts.join(" ")
}

/// cb_extent.main: the table's lines (`key\ttokens`) in order
pub fn cb_extent(img: &Image) -> Vec<String> {
    let names: Vec<&FileRef> = img
        .files
        .iter()
        .filter(|f| f.name.ends_with("_b.json") && output_ext(&f.name) == Some("json"))
        .collect();
    let rows: Vec<Option<String>> = names
        .par_iter()
        .map(|f| {
            let key = &f.name[..f.name.len() - 7];
            let metal = img.find(&format!("{key}_b.metal")).map(|r| img.bytes(r));
            let answer = cb_tokens(Some(img.bytes(f)), metal);
            if answer.is_empty() {
                None
            } else {
                Some(format!("{key}\t{answer}"))
            }
        })
        .collect();
    rows.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUGE: &str = "170141183460469231731687303715884105727"; // i128::MAX: 16 * (high + 1) wraps to 0

    #[test]
    fn a_c1_read_past_i128_is_not_proven() {
        let refl = br#"{"c1": "00"}"#;
        let src = format!("float4 v = (*fp_c1)._m0[{HUGE}];\n");
        assert!(unproven_in(&src, Some(refl)));
        // a small proven index stays proven
        assert!(!unproven_in("float4 v = (*fp_c1)._m0[3];\n", Some(refl)));
    }

    #[test]
    fn a_cbuf_extent_past_i128_is_left_out() {
        let b = |n: &str| Binding {
            kind: "cbuf".into(),
            reg: Some(0),
            msl_name: n.into(),
        };
        let src =
            format!("constant X* cb [[id(0)]];\nfloat4 v = (*spvDescriptorSet0.cb)._m0[{HUGE}];\n");
        assert!(extents(&src, &[b("cb")]).is_empty());
        let src = "constant X* cb [[id(0)]];\nfloat4 v = (*spvDescriptorSet0.cb)._m0[3];\n";
        assert_eq!(extents(src, &[b("cb")]), vec![(0, 64)]);
    }
}
