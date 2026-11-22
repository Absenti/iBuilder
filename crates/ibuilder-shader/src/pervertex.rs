//! dmsh_rewrite.per_vertex: the translator's per-vertex attribute reads of a pixel shader (`dm_pv<L>_<v>` flat inputs at user(locn<64 + 3L + v>), Ampere's LDTRAM) as one
//! `vertex_value<float4>` at user(locn<L>) and `.get(vertex_index::...)` on each read.
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;

const BASE: u64 = 64;

struct M<'a> {
    start: usize,
    end: usize,
    l: &'a str,
    v: &'a str,
    loc: &'a str,
}

/// PER_VERTEX_IN `\n    float4 dm_pv(\d+)_([012]) \[\[user\(locn(\d+)\)(?:, flat)?\]\];` at `p`
fn per_vertex_in(src: &str, p: usize) -> Option<M<'_>> {
    let mut c = Cur::at(src, p);
    if !c.lit("\n    float4 dm_pv") {
        return None;
    }
    let l = c.digits()?;
    if !c.lit("_") {
        return None;
    }
    let vpos = c.i;
    if !matches!(c.peek(), Some(b'0' | b'1' | b'2')) {
        return None;
    }
    c.i += 1;
    if !c.lit(" [[user(locn") {
        return None;
    }
    let loc = c.digits()?;
    if !c.lit(")") {
        return None;
    }
    c.lit(", flat");
    if !c.lit("]];") {
        return None;
    }
    Some(M {
        start: p,
        end: c.i,
        l,
        v: &src[vpos..vpos + 1],
        loc,
    })
}

pub fn per_vertex(src: &str) -> Result<Cow<'_, str>, PostError> {
    let b = src.as_bytes();
    let mut found: Vec<M> = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"\n    float4 dm_pv", from) {
        match per_vertex_in(src, p) {
            Some(m) => {
                from = m.end;
                found.push(m);
            }
            None => from = p + 1,
        }
    }
    if found.is_empty() {
        return Ok(Cow::Borrowed(src));
    }
    let num = |s: &str| -> u64 { s.parse::<u64>().unwrap_or(u64::MAX) };
    for m in &found {
        if num(m.loc)
            != BASE
                .saturating_add(3u64.saturating_mul(num(m.l)))
                .saturating_add(num(m.v))
        {
            return Err(failed(
                "per_vertex",
                format!(
                    "per-vertex input dm_pv{}_{} at user(locn{}), not at the translator's location",
                    m.l, m.v, m.loc
                ),
            ));
        }
    }
    let mut locs: Vec<u64> = found.iter().map(|m| num(m.l)).collect();
    locs.sort_unstable();
    locs.dedup();
    // st = re.search(r'\nstruct main0_in\n\{(\n.*?)\n\};', msl, re.S)
    let head = "\nstruct main0_in\n{\n";
    let Some(h) = find(src, head, 0) else {
        return Err(failed("per_vertex", "per-vertex inputs outside main0_in"));
    };
    let g0 = h + head.len() - 1; // group 1 starts at the newline after the brace
    let Some(g1) = find(src, "\n};", g0 + 1) else {
        return Err(failed("per_vertex", "per-vertex inputs outside main0_in"));
    };
    if !found.iter().all(|m| g0 <= m.start && m.start < g1) {
        return Err(failed("per_vertex", "per-vertex inputs outside main0_in"));
    }
    let group = &src[g0..g1];
    if let Some(l) = locs
        .iter()
        .find(|l| contains(group, &format!("[[user(locn{l})")))
    {
        return Err(failed(
            "per_vertex",
            format!("attribute {l} read both per vertex and as an ordinary input"),
        ));
    }
    let mut out = String::with_capacity(src.len());
    out.push_str(&src[..found[0].start]);
    for l in &locs {
        out.push_str(&format!(
            "\n    vertex_value<float4> dm_pv{l} [[user(locn{l})]];"
        ));
    }
    for w in found.windows(2) {
        out.push_str(&src[w[0].end..w[1].start]);
    }
    out.push_str(&src[found[found.len() - 1].end..]);
    // re.sub(r'\.dm_pv(\d+)_([012])\b', ...)
    let ob = out.as_bytes();
    let mut res = String::with_capacity(out.len() + 64);
    let (mut at, mut from) = (0, 0);
    while let Some(p) = find_b(ob, b".dm_pv", from) {
        from = p + 1;
        let mut c = Cur::at(&out, p + 6);
        let Some(l) = c.digits() else { continue };
        if !c.lit("_") {
            continue;
        }
        let Some(&v) = ob.get(c.i) else { continue };
        if !matches!(v, b'0' | b'1' | b'2') {
            continue;
        }
        c.i += 1;
        if c.i < ob.len() && is_word(ob[c.i]) {
            continue;
        }
        res.push_str(&out[at..p]);
        res.push_str(&format!(
            ".dm_pv{l}.get(vertex_index::{})",
            ["first", "second", "third"][(v - b'0') as usize]
        ));
        at = c.i;
        from = at;
    }
    res.push_str(&out[at..]);
    Ok(Cow::Owned(res))
}
