//! `dmsh_geometry.lean(src, out_stride)`: a passthrough geometry kernel without its copy of the vertex: only the viewport mask is written, one word per output vertex, and only
//! the capture words it still reads are loaded, from a record of `len(reads)` words in that order. The rasterizer then draws the game's own vertex shader through the kernel's
//! indices (nvn_geometry.inc). Input reads and output stores must be the generated literal offsets; anything else is a `ValueError` (the full kernel is kept).
//!
//! The Python works on `src.split('\n')` with regular expressions; this is the same algorithm over the same lines (the dead-code removal works on a stack, as the Python's).
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

/// the capture record's words
pub const WORDS: i64 = 144;
/// the viewport mask's word in an output vertex
pub const MASK_WORD: i64 = 5;

re!(RE_DEF, r"^\s*(?:int|uint|float|bool) (_\d+) = (.+);$");
re!(RE_PURE, r"atomic|Subgroup|simd_|\+\+|--|[^=!<>]=[^=]");
re!(
    RE_STORE,
    r"^\s*\(\*spvDescriptorSet0\.geometry_vb_output\)\._m0\[(_\d+)\] = (_\d+);$"
);
re!(RE_PLUS, r"^(_\d+) \+ (\d+)$");
re!(RE_EQ_TAIL, r"= .+;$");
re!(RE_DECL, r"^\s*spvUnsafeArray<\w+, \d+> (_\d+);$");
re!(RE_TOKEN, r"_\d+(?:\[\d+\])?");
re!(RE_KERNEL, r"^kernel void main0\(");
re!(RE_ARRAY_STORE, r"^\s*(_\d+)\[(\d+)\] = (.+);$");
re!(
    RE_CAPTURE_READ,
    r"= \(\*spvDescriptorSet0\.vertex_output\)\._m0\[(_\d+)\];"
);
re!(RE_PLUS_TAIL, r"\+ \d+;$");
re!(RE_MUL_TAIL, r"\* 144;$");

fn value<T>(m: &str) -> Result<T, String> {
    Err(m.to_string())
}

/// whole-word occurrences of `name` (`\bname\b`): their start offsets
fn word_hits<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
    let b = text.as_bytes();
    text.match_indices(name).map(|(i, _)| i).filter(move |&i| {
        let before = i == 0 || !crate::text::is_word(b[i - 1]);
        let e = i + name.len();
        let after = e >= b.len() || !crate::text::is_word(b[e]);
        before && after
    })
}

/// the base of `var = _X * stride`: `_X`
fn base_of(defs: &HashMap<String, (usize, String)>, var: &str, stride: &Regex) -> Option<String> {
    let e = defs.get(var).map(|d| d.1.as_str()).unwrap_or("");
    stride.captures(e).map(|m| m[1].to_string())
}

/// `offset(var, stride)`: (`_B`, K) for `var = _B + K` with `_B = _X * stride` (or var itself, K = 0)
fn offset(
    defs: &HashMap<String, (usize, String)>,
    var: &str,
    stride: &Regex,
) -> Option<(String, i64)> {
    let e = defs.get(var).map(|d| d.1.as_str()).unwrap_or("");
    if let Some(m) = RE_PLUS.captures(e)
        && base_of(defs, &m[1], stride).is_some()
    {
        return Some((m[1].to_string(), m[2].parse().unwrap_or(i64::MAX)));
    }
    if base_of(defs, var, stride).is_some() {
        return Some((var.to_string(), 0));
    }
    None
}

/// `lean(src, out_stride)` -> (the kernel, the capture words it reads)
pub fn lean(src: &str, out_stride: i64) -> Result<(String, Vec<i64>), String> {
    let mut lines: Vec<String> = src.split('\n').map(|s| s.to_string()).collect();
    let mut defs: HashMap<String, (usize, String)> = HashMap::new();
    for (i, ln) in lines.iter().enumerate() {
        if let Some(m) = RE_DEF.captures(ln) {
            defs.entry(m[1].to_string())
                .or_insert((i, m[2].to_string()));
        }
    }
    let out_re =
        super::rx::cached(&format!(r"^(_\d+) \* {out_stride}$")).map_err(|e| e.to_string())?;
    let words_re =
        super::rx::cached(&format!(r"^(_\d+) \* {WORDS}$")).map_err(|e| e.to_string())?;
    let mut kept = 0usize;
    for i in 0..lines.len() {
        let ln = lines[i].clone();
        if !ln.contains("geometry_vb_output") || super::pycompat::strip(&ln).starts_with("device ")
        {
            continue;
        }
        let Some(m) = RE_STORE.captures(&ln) else {
            return value("lean geometry: unknown output store");
        };
        let Some((b, k)) = offset(&defs, &m[1], &out_re) else {
            return value("lean geometry: unknown output store");
        };
        if k != MASK_WORD {
            lines[i] = String::new();
            continue;
        }
        let (j, _) = defs[&m[1]].clone();
        let base = base_of(&defs, &b, &out_re).unwrap_or_default();
        lines[j] = RE_EQ_TAIL
            .replace(&lines[j], format!("= {base};").as_str())
            .into_owned();
        kept += 1;
    }
    if kept == 0 {
        return value("lean geometry: no viewport mask store");
    }
    // Remove what only fed the removed stores: unused pure definitions, and local array elements written at a literal index and never read.
    let mut arrays: BTreeSet<String> = lines
        .iter()
        .filter_map(|l| RE_DECL.captures(l).map(|m| m[1].to_string()))
        .collect();
    let rest: String = lines
        .iter()
        .filter(|l| !RE_DECL.is_match(l))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    for a in arrays.clone() {
        // `\b<a>\b(?!\[\d+\])`
        let bytes = rest.as_bytes();
        let used = word_hits(&rest, &a).any(|i| {
            let e = i + a.len();
            // followed by `[digits]`?
            if bytes.get(e) == Some(&b'[') {
                let mut j = e + 1;
                let ds = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > ds && bytes.get(j) == Some(&b']') {
                    return false;
                }
            }
            true
        });
        if used {
            arrays.remove(&a);
        }
    }
    let n = lines.len();
    let mut target: Vec<Option<String>> = vec![None; n];
    let mut rvalues: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut uses: HashMap<String, i64> = HashMap::new();
    let mut writers: HashMap<String, Vec<usize>> = HashMap::new();
    let Some(body) = lines.iter().position(|l| RE_KERNEL.is_match(l)) else {
        return value("lean geometry: no kernel entry");
    };
    for i in 0..n {
        if i <= body {
            continue;
        }
        let ln = lines[i].clone();
        let m = RE_DEF.captures(&ln);
        let a = if m.is_some() {
            None
        } else {
            RE_ARRAY_STORE.captures(&ln)
        };
        let expr: String;
        if let Some(m) = m.as_ref().filter(|m| !RE_PURE.is_match(&m[2])) {
            target[i] = Some(m[1].to_string());
            expr = m[2].to_string();
        } else if let Some(a) = a
            .as_ref()
            .filter(|a| arrays.contains(&a[1]) && !RE_PURE.is_match(&a[3]))
        {
            target[i] = Some(format!("{}[{}]", &a[1], &a[2]));
            expr = a[3].to_string();
        } else {
            expr = ln.clone();
        }
        rvalues[i] = RE_TOKEN
            .find_iter(&expr)
            .map(|t| t.as_str().to_string())
            .filter(|t| !(arrays.contains(t.split('[').next().unwrap_or("")) && !t.contains('[')))
            .collect();
        for t in &rvalues[i] {
            *uses.entry(t.clone()).or_insert(0) += 1;
        }
        if let Some(t) = &target[i] {
            writers.entry(t.clone()).or_default().push(i);
        }
    }
    let mut work: Vec<usize> = (0..n)
        .filter(|i| {
            target[*i]
                .as_ref()
                .map(|t| uses.get(t).copied().unwrap_or(0) == 0)
                .unwrap_or(false)
        })
        .collect();
    while let Some(i) = work.pop() {
        if lines[i].is_empty() {
            continue;
        }
        lines[i] = String::new();
        for t in rvalues[i].clone() {
            let u = uses.entry(t.clone()).or_insert(0);
            *u -= 1;
            if *u == 0
                && let Some(w) = writers.get(&t)
            {
                work.extend(w.iter().copied());
            }
        }
    }
    // The remaining capture reads: literal words of the 144-word record, renumbered.
    let text = lines.join("\n");
    let reads_found = RE_CAPTURE_READ.captures_iter(&text).count();
    if text.matches("vertex_output").count() != reads_found + 1 {
        return value("lean geometry: unknown capture read");
    }
    let mut found: Vec<(String, String, i64)> = Vec::new();
    for m in RE_CAPTURE_READ.captures_iter(&text) {
        let Some((b, k)) = offset(&defs, &m[1], &words_re) else {
            return value("lean geometry: computed capture read");
        };
        found.push((m[1].to_string(), b, k));
    }
    let reads: Vec<i64> = found
        .iter()
        .map(|f| f.2)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let slot: BTreeMap<i64, usize> = reads.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let bases: BTreeSet<String> = found.iter().map(|f| f.1.clone()).collect();
    for b in &bases {
        let total = word_hits(&text, b).count() as i64;
        let mine = found.iter().filter(|f| &f.1 == b).count() as i64;
        if total - 1 > mine {
            return value("lean geometry: record base used elsewhere");
        }
    }
    for (n, b, k) in &found {
        if n != b {
            let (j, _) = defs[n].clone();
            lines[j] = RE_PLUS_TAIL
                .replace(&lines[j], format!("+ {};", slot[k]).as_str())
                .into_owned();
        }
    }
    for b in &bases {
        let (j, _) = defs[b].clone();
        lines[j] = RE_MUL_TAIL
            .replace(&lines[j], format!("* {};", reads.len().max(1)).as_str())
            .into_owned();
    }
    Ok((lines.join("\n"), reads))
}
