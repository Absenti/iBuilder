//! dmsh_rewrite.frag_z_snap: gl_FragCoord.z against a depth the shader read (`dm_zsnap`).
use crate::side;
use crate::templates as t;
use crate::text::*;
use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

/// `\bfloat4 gl_FragCoord( \[\[position\]\])?\)?$` on a stripped parameter
fn is_fragcoord_param(p: &str) -> bool {
    let p = strip(p);
    let p = p.strip_suffix(')').unwrap_or(p);
    let p = p.strip_suffix(" [[position]]").unwrap_or(p);
    match p.strip_suffix("float4 gl_FragCoord") {
        Some(pre) => !pre.bytes().next_back().map(is_word).unwrap_or(false),
        None => false,
    }
}

/// the `_\d+` words of `s` (`\b_\d+\b`)
fn refs(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'_' && !in_word(b, i) {
            let d = digits_end(b, i + 1);
            if d > i + 1 && !(d < b.len() && is_word(b[d])) {
                out.push(&s[i..d]);
                i = d;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `\s*(?:const )?(float|bool|int|uint)\s+(_\d+) = (.+);$` on a line -> (type, name, expression)
fn decl(l: &str) -> Option<(&str, &str, &str)> {
    let b = l.as_bytes();
    let mut i = skip_space(b, 0);
    if starts(b, i, "const ") {
        i += 6;
    }
    let ty = ["float", "bool", "uint", "int"]
        .into_iter()
        .find(|t| starts(b, i, t))?;
    // `(float|bool|int|uint)\s+`: for "int" vs "uint" the alternation order float, bool, int, uint: `int` cannot match the start of "uint"
    let mut j = i + ty.len();
    let s = skip_space(b, j);
    if s == j {
        return None;
    }
    j = s;
    if j >= b.len() || b[j] != b'_' {
        return None;
    }
    let d = digits_end(b, j + 1);
    if d == j + 1 || !starts(b, d, " = ") {
        return None;
    }
    let e0 = d + 3;
    if b.len() <= e0 + 1 || b[b.len() - 1] != b';' {
        return None;
    }
    Some((ty, &l[j..d], &l[e0..b.len() - 1]))
}

/// `(_\d+) (?:<=|>=|<|>|==|!=) (_\d+)` fullmatch -> (a, b)
fn compare(ex: &str) -> Option<(&str, &str)> {
    let b = ex.as_bytes();
    if b.is_empty() || b[0] != b'_' {
        return None;
    }
    let d = digits_end(b, 1);
    if d == 1 || b[d..].first() != Some(&b' ') {
        return None;
    }
    let mut k = d + 1;
    for op in ["<=", ">=", "==", "!=", "<", ">"] {
        if starts(b, k, op) {
            k += op.len();
            break;
        }
    }
    if k == d + 1 || !starts(b, k, " ") {
        return None;
    }
    k += 1;
    if k >= b.len() || b[k] != b'_' {
        return None;
    }
    let e = digits_end(b, k + 1);
    if e == k + 1 || e != b.len() {
        return None;
    }
    Some((&ex[..d], &ex[k..e]))
}

/// dmsh_rewrite.frag_z_snap -> (msl with each paired `gl_FragCoord.z` read in main0 as `dm_zsnap(gl_FragCoord.z, <texture value>, dm_zw)` and `dm_zw = fwidth(gl_FragCoord.z)` first in main0's body, the number of reads)
pub fn frag_z_snap(msl: &str) -> (Cow<'_, str>, u32) {
    let same = || (Cow::Borrowed(msl), 0u32);
    // a z origin needs `_N = gl_FragCoord.z;` and a pair needs a scalar read from a texture: without both, `pairs` stays empty and the Python returns the text as it is
    if !contains(msl, "= gl_FragCoord.z;")
        || !(contains(msl, ".sample(") || contains(msl, ".read("))
    {
        return same();
    }
    let (head, end, params): (usize, usize, Vec<String>) =
        if let Some((st, head, end)) = side::body(msl) {
            // re.findall(r'[^,]+', msl[st:head]): the pieces between commas
            (
                head,
                end,
                msl[st..head]
                    .split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| p.to_string())
                    .collect(),
            )
        } else {
            match side::entry(msl, "main0") {
                Ok(Some(e)) => (e.head, e.end, e.params.clone()),
                _ => return same(),
            }
        };
    if !params.iter().any(|p| is_fragcoord_param(p)) {
        return same();
    }
    // first = msl.find('{', head) + 1 (-1 + 1 = 0 without one)
    let first = find(msl, "{", head).map(|o| o + 1).unwrap_or(0);
    if end == 0 || end - 1 < first {
        return same();
    }
    let body = &msl[first..end - 1];
    let lines: Vec<&str> = body.split('\n').collect();
    let mut zr: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    let mut xr: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    let mut scope: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut line_of: HashMap<&str, usize> = HashMap::new();
    let mut stack: Vec<u32> = vec![0];
    let mut blocks = 0u32;
    let mut pairs: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for (i, ln) in lines.iter().enumerate() {
        if let Some((ty, v, ex)) = decl(ln) {
            let rs = refs(ex);
            let mut z: BTreeSet<&str> = BTreeSet::new();
            let mut x: BTreeSet<&str> = BTreeSet::new();
            for r in &rs {
                if let Some(s) = zr.get(r) {
                    z.extend(s.iter().copied());
                }
                if let Some(s) = xr.get(r) {
                    x.extend(s.iter().copied());
                }
            }
            if ex == "gl_FragCoord.z" {
                z = BTreeSet::from([v]);
            } else if ty == "float"
                && (contains(ex, ".sample(") || contains(ex, ".read("))
                && z.is_empty()
            {
                x = BTreeSet::from([v]);
            }
            zr.insert(v, z);
            xr.insert(v, x);
            scope.insert(v, stack.clone());
            line_of.insert(v, i);
            if ty == "bool"
                && let Some((a, b)) = compare(ex)
            {
                for (zs, xs) in [(a, b), (b, a)] {
                    let (zz, xx) = (zr.get(zs), xr.get(xs));
                    if zz.map(|s| !s.is_empty()).unwrap_or(false)
                        && xx.map(|s| !s.is_empty()).unwrap_or(false)
                        && xr.get(zs).map(|s| s.is_empty()).unwrap_or(true)
                        && zr.get(xs).map(|s| s.is_empty()).unwrap_or(true)
                    {
                        for zv in zr[zs].clone() {
                            pairs.entry(zv).or_default().extend(xr[xs].iter().copied());
                        }
                    }
                }
            }
        }
        for ch in ln.bytes() {
            if ch == b'{' {
                blocks += 1;
                stack.push(blocks);
            } else if ch == b'}' && stack.len() > 1 {
                stack.pop();
            }
        }
    }
    let mut zvs: Vec<&str> = pairs.keys().copied().collect();
    zvs.sort_by_key(|v| line_of[v]);
    let mut edited: HashMap<usize, String> = HashMap::new();
    let mut n = 0u32;
    for zv in zvs {
        let lz = line_of[zv];
        let sz = &scope[zv];
        let mut xs: Vec<&str> = pairs[zv]
            .iter()
            .copied()
            .filter(|x| {
                let sx = &scope[x];
                line_of[x] < lz && sz.len() >= sx.len() && sz[..sx.len()] == sx[..]
            })
            .collect();
        if xs.is_empty() {
            continue;
        }
        xs.sort_by_key(|v| line_of[v]);
        let mut expr = String::from("gl_FragCoord.z");
        for x in xs {
            expr = format!("dm_zsnap({expr}, {x}, dm_zw)");
        }
        let cur = edited.get(&lz).map(|s| s.as_str()).unwrap_or(lines[lz]);
        let new = cur.replace(
            &format!("{zv} = gl_FragCoord.z;"),
            &format!("{zv} = {expr};"),
        );
        edited.insert(lz, new);
        n += 1;
    }
    if n == 0 {
        return same();
    }
    let mut o = String::with_capacity(msl.len() + 512);
    o.push_str(&msl[..first]);
    o.push_str("\n    float dm_zw = fwidth(gl_FragCoord.z);");
    for (i, ln) in lines.iter().enumerate() {
        if i > 0 {
            o.push('\n');
        }
        match edited.get(&i) {
            Some(e) => o.push_str(e),
            None => o.push_str(ln),
        }
    }
    o.push_str(&msl[end - 1..]);
    // msl.replace('using namespace metal;\n', 'using namespace metal;\n\n' + ZSNAP, 1)
    if let Some(p) = find(&o, "using namespace metal;\n", 0) {
        let at = p + "using namespace metal;\n".len();
        o.insert_str(at, &format!("\n{}", t::ZSNAP));
    }
    (Cow::Owned(o), n)
}
