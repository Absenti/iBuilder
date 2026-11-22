//! dmsh_rewrite.hmma_msl: Ampere's HMMA.16816 with an f16 accumulator (the translator's `dmsh_hmma16816_f16(lane, block, A0..A3, B0, B1, C0, C1, flags, A's LDSM address, A0..A3's
//! offsets, A's row stride, out D1) -> D0` helper over the subgroup's block of scratch shared memory) as simdgroup_matrix code, and `hmma_bslots`, the choice of which calls keep their
//! B matrices in the SIMD-group. The Python's regular expressions are matched with cursors; `\b`, lazy quantifiers and alternation order are kept as the Python has them.
use crate::ampere::split_params;
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;
use std::collections::HashMap;

/// the order of the helper's plain parameters (HMMA_ARGS)
const ARGS: [&str; 18] = t::HMMA_ARGS;

/// `\n(\w+) dmsh_hmma16816_f16\(([^)]*)\)\n\{` (the first one) -> (start of the match, return type, parameters, end of the match)
fn find_definition(src: &str) -> Option<(usize, &str, &str, usize)> {
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b" dmsh_hmma16816_f16(", from) {
        from = p + 1;
        let mut k = p;
        while k > 0 && is_word(b[k - 1]) {
            k -= 1;
        }
        if k == p || k == 0 || b[k - 1] != b'\n' {
            continue;
        }
        let g0 = p + 20;
        let Some(r) = memchr::memchr(b')', &b[g0..]) else {
            continue;
        };
        if starts(b, g0 + r, ")\n{") {
            return Some((k - 1, &src[k..p], &src[g0..g0 + r], g0 + r + 3));
        }
    }
    None
}

/// `(\w+)$` of a stripped parameter: its last word
fn last_word(x: &str) -> Option<&str> {
    let b = x.as_bytes();
    let mut k = b.len();
    while k > 0 && is_word(b[k - 1]) {
        k -= 1;
    }
    if k == b.len() { None } else { Some(&x[k..]) }
}

struct Call<'a> {
    start: usize,
    end: usize,
    args: &'a str,
    open_at: usize,
}

/// the `\bdmsh_hmma16816_f16\(([^()]*)\)` matches of the text
fn calls_of(src: &str) -> Vec<(usize, usize, &str)> {
    let b = src.as_bytes();
    let mut v = Vec::new();
    let mut from = 0;
    let pat = b"dmsh_hmma16816_f16(";
    while let Some(p) = find_b(b, pat, from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let a0 = p + pat.len();
        match b[a0..].iter().position(|&x| x == b'(' || x == b')') {
            Some(r) if b[a0 + r] == b')' => {
                v.push((p, a0 + r + 1, &src[a0..a0 + r]));
                from = a0 + r + 1;
            }
            _ => {}
        }
    }
    v
}

/// `^[ \t]+threadgroup (spvUnsafeArray<uint, \d+>) (\w+);[ \t]*$` over msl[pos:endpos] (re.M) -> (type, name)
fn tg_arrays(src: &str, pos: usize, endpos: usize) -> Vec<(&str, &str)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut ls = if pos == 0 || b[pos - 1] == b'\n' {
        pos
    } else {
        match memchr::memchr(b'\n', &b[pos..endpos]) {
            Some(p) => pos + p + 1,
            None => return out,
        }
    };
    while ls < endpos {
        let le = memchr::memchr(b'\n', &b[ls..endpos])
            .map(|p| p + ls)
            .unwrap_or(endpos);
        let line = &src[ls..le];
        let mut c = Cur::at(line, 0);
        if !c.ht0().is_empty() && c.lit("threadgroup ") {
            let t0 = c.i;
            if c.lit("spvUnsafeArray<uint, ") && c.digits().is_some() && c.lit(">") {
                let t1 = c.i;
                if c.lit(" ")
                    && let Some(n) = c.word()
                    && c.lit(";")
                {
                    c.ht0();
                    if c.at_end() {
                        out.push((&line[t0..t1], n));
                    }
                }
            }
        }
        ls = le + 1;
    }
    out
}

/// the `[{}]` characters of `body` with their depth after each, positions absolute
struct Depth {
    at: Vec<usize>,
    after: Vec<i32>,
}
impl Depth {
    fn new(src: &str, open_at: usize, end: usize) -> Depth {
        let b = &src.as_bytes()[open_at..end];
        let (mut at, mut after, mut d) = (Vec::new(), Vec::new(), 0i32);
        for (i, &c) in b.iter().enumerate() {
            if c == b'{' {
                d += 1;
                at.push(open_at + i);
                after.push(d);
            } else if c == b'}' {
                d -= 1;
                at.push(open_at + i);
                after.push(d);
            }
        }
        Depth { at, after }
    }
    /// the depth in force at `pos` (1: the function's top level)
    fn at(&self, pos: usize) -> i32 {
        let k = self.at.partition_point(|&p| p < pos);
        if k == 0 { 0 } else { self.after[k - 1] }
    }
}

/// HMMA_WRITE over `body`: the names counted as written: assignments, `++` / `--`, address-of / binary `&`
fn count_writes<'a>(body: &'a str, writes: &mut HashMap<&'a str, i64>) {
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let word_start = is_word(b[i]) && !in_word(b, i);
        let mut hit: Option<(&str, usize)> = None;
        if word_start {
            let we = word_end(b, i);
            let w = &body[i..we];
            // alt 1: `[ \t]*(?:<<|>>|[-+*/%&|^])?=(?!=)`
            let mut c = Cur::at(body, we);
            c.ht0();
            let mut x = c;
            if !(x.lit("<<") || x.lit(">>"))
                && matches!(
                    x.peek(),
                    Some(b'-' | b'+' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
                )
            {
                x.i += 1;
            }
            if x.lit("=") && x.peek() != Some(b'=') {
                hit = Some((w, x.i));
            } else {
                let mut y = c;
                if y.lit("=") && y.peek() != Some(b'=') {
                    hit = Some((w, y.i));
                } else {
                    // alt 3: `\b(\w+)[ \t]*\+\+`, alt 5: `\b(\w+)[ \t]*--`
                    let mut z = c;
                    if z.lit("++") || z.lit("--") {
                        hit = Some((w, z.i));
                    }
                }
            }
        }
        if hit.is_none() {
            // alt 2: `\+\+[ \t]*(\w+)`, alt 4: `--[ \t]*(\w+)`, alt 6: `&[ \t]*(\w+)`
            let mut c = Cur::at(body, i);
            if c.lit("++") || c.lit("--") || c.lit("&") {
                c.ht0();
                let a = c.i;
                let e = word_end(b, a);
                if e > a {
                    hit = Some((&body[a..e], e));
                }
            }
        }
        match hit {
            Some((w, e)) => {
                *writes.entry(w).or_insert(0) += 1;
                i = e.max(i + 1);
            }
            None => i += 1,
        }
    }
}

/// HMMA_DECL `^[ \t]*(?:const[ \t]+)?[A-Za-z_]\w*(?:<[^<>;]*>)?[ \t]+(\w+)[ \t]*=[ \t]*([^;\n]+);` on a line -> (name, its offset in the line, initializer stripped)
fn decl_line(l: &str) -> Option<(&str, usize, &str)> {
    let b = l.as_bytes();
    let attempt = |with_const: bool| -> Option<(usize, usize, usize, usize)> {
        let mut c = Cur::at(l, 0);
        c.ht0();
        if with_const && (!c.lit("const") || c.ht0().is_empty()) {
            return None;
        }
        if !(c.i < b.len() && (b[c.i].is_ascii_alphabetic() || b[c.i] == b'_')) {
            return None;
        }
        c.i = word_end(b, c.i);
        if c.peek() == Some(b'<') {
            let mut k = c.i + 1;
            while k < b.len() && !matches!(b[k], b'<' | b'>' | b';') {
                k += 1;
            }
            if k < b.len() && b[k] == b'>' {
                c.i = k + 1;
            } else {
                return None;
            }
        }
        if c.ht0().is_empty() {
            return None;
        }
        let n0 = c.i;
        c.word()?;
        let n1 = c.i;
        c.ht0();
        if !c.lit("=") {
            return None;
        }
        let v0 = c.i;
        let mut k = v0;
        while k < b.len() && b[k] != b';' && b[k] != b'\n' {
            k += 1;
        }
        if k >= b.len() || b[k] != b';' || k == v0 {
            return None;
        }
        Some((n0, n1, v0, k))
    };
    let (n0, n1, v0, k) = attempt(true).or_else(|| attempt(false))?;
    Some((&l[n0..n1], n0, strip(&l[v0..k])))
}

/// HMMA_COPY fullmatch: `(?:as_type<\w+>|int|uint|float|half|short|ushort)?\((\w+)\)|(\w+)` -> the inner name
fn copy_inner(init: &str) -> Option<&str> {
    let b = init.as_bytes();
    // alternative 1: an optional cast, then `(name)`; the ways the optional prefix can match, the empty one last
    let mut prefixes: Vec<usize> = Vec::new();
    if let Some(r) = init.strip_prefix("as_type<") {
        let w = word_end(r.as_bytes(), 0);
        if w > 0 && r[w..].starts_with('>') {
            prefixes.push(8 + w + 1);
        }
    }
    for p in ["int", "uint", "float", "half", "short", "ushort"] {
        if init.starts_with(p) {
            prefixes.push(p.len());
        }
    }
    prefixes.push(0);
    for s in prefixes {
        let mut c = Cur::at(init, s);
        if c.lit("(")
            && let Some(n) = c.word()
            && c.lit(")")
            && c.at_end()
        {
            return Some(n);
        }
    }
    // alternative 2: `\w+`
    let e = word_end(b, 0);
    if e > 0 && e == b.len() {
        Some(init)
    } else {
        None
    }
}

/// hmma_bslots: the calls of one function -> {call start: slot * 2 (keeps its B) or slot * 2 + 1 (takes it)}
fn bslots(
    src: &str,
    open_at: usize,
    end: usize,
    fcalls: &[(&Call, Vec<&str>)],
    writers: &[&str],
) -> Result<Vec<(usize, i64)>, PostError> {
    let body = &src[open_at..end];
    let mut writes: HashMap<&str, i64> = HashMap::new();
    count_writes(body, &mut writes);
    let d1 = ARGS.iter().position(|a| *a == "d1").unwrap();
    let bad = || failed("hmma_msl", "list index out of range");
    for (_, args) in fcalls {
        *writes.entry(args.get(d1).ok_or_else(bad)?).or_insert(0) += 1;
    }
    let bb = body.as_bytes();
    for name in writers {
        let pat = format!("{name}(");
        let mut from = 0;
        while let Some(p) = find_b(bb, pat.as_bytes(), from) {
            from = p + 1;
            if in_word(bb, p) {
                continue;
            }
            let wend = p + pat.len();
            let (mut depth, mut j) = (1i32, wend);
            while depth != 0 && j < bb.len() {
                if bb[j] == b'(' {
                    depth += 1;
                } else if bb[j] == b')' {
                    depth -= 1;
                }
                j += 1;
            }
            // re.findall(r'\b[A-Za-z_]\w*\b', body[wend:j])
            let seg = &body[wend..j];
            let sb = seg.as_bytes();
            let mut k = 0;
            while k < sb.len() {
                if is_word(sb[k]) {
                    let e = word_end(sb, k);
                    if (sb[k].is_ascii_alphabetic() || sb[k] == b'_') && !in_word(sb, k) {
                        *writes.entry(&seg[k..e]).or_insert(0) += 1;
                    }
                    k = e;
                } else {
                    k += 1;
                }
            }
        }
    }
    let depth = Depth::new(src, open_at, end);
    let mut decls: HashMap<&str, (&str, usize)> = HashMap::new();
    {
        let mut ls = 0;
        while ls < body.len() {
            let le = memchr::memchr(b'\n', &bb[ls..])
                .map(|p| p + ls)
                .unwrap_or(body.len());
            if let Some((n, off, init)) = decl_line(&body[ls..le]) {
                decls.entry(n).or_insert((init, open_at + ls + off));
            }
            ls = le + 1;
        }
    }
    let canon = |name: &'_ str| -> Option<String> {
        let mut name: &str = name;
        for _ in 0..64 {
            if writes.get(name).copied().unwrap_or(0) != 1 {
                return None;
            }
            let &(init, pos) = decls.get(name)?;
            let inner = copy_inner(init);
            match inner {
                Some(i) if !i.as_bytes()[0].is_ascii_digit() => name = i,
                _ => {
                    return if depth.at(pos) == 1 {
                        Some(name.to_string())
                    } else {
                        None
                    };
                }
            }
        }
        None
    };
    let b0 = ARGS.iter().position(|a| *a == "b0").unwrap();
    let b1 = ARGS.iter().position(|a| *a == "b1").unwrap();
    let mut by_b: Vec<((String, String), Vec<usize>)> = Vec::new();
    for (c, args) in fcalls {
        let k = (
            canon(args.get(b0).ok_or_else(bad)?),
            canon(args.get(b1).ok_or_else(bad)?),
        );
        if let (Some(x), Some(y)) = k {
            match by_b.iter_mut().find(|(kk, _)| kk.0 == x && kk.1 == y) {
                Some((_, v)) => v.push(c.start),
                None => by_b.push(((x, y), vec![c.start])),
            }
        }
    }
    let mut modes: Vec<(usize, i64)> = Vec::new();
    let mut slot = 0i64;
    for (_, starts) in &by_b {
        let first = starts.iter().position(|&s| depth.at(s) == 1);
        let Some(first) = first else { continue };
        if first == starts.len() - 1 {
            continue;
        }
        modes.push((starts[first], slot * 2));
        for &s in &starts[first + 1..] {
            modes.push((s, slot * 2 + 1));
        }
        slot += 1;
    }
    Ok(modes)
}

/// `\b(\w+)\(([^()]*)\)\s*\{` functions whose parameters have a non-const `thread X&`: their names (the helper's own excluded)
fn writer_names(src: &str) -> Vec<&str> {
    let b = src.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if is_word(b[i]) && !in_word(b, i) {
            let we = word_end(b, i);
            if we < b.len()
                && b[we] == b'('
                && let Some(r) = b[we + 1..].iter().position(|&x| x == b'(' || x == b')')
                && b[we + 1 + r] == b')'
            {
                let mut c = Cur::at(src, we + 2 + r);
                c.ws0();
                if c.peek() == Some(b'{') {
                    let params = &src[we + 1..we + 1 + r];
                    if has_thread_ref(params) && &src[i..we] != "dmsh_hmma16816_f16" {
                        out.push(&src[i..we]);
                    }
                    i = c.i + 1;
                    continue;
                }
            }
            i = we;
            continue;
        }
        i += 1;
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// `\bthread (?!const\b)[\w<>, ]+&`
fn has_thread_ref(params: &str) -> bool {
    let b = params.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"thread ", from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let a = p + 7;
        if starts(b, a, "const") && !(a + 5 < b.len() && is_word(b[a + 5])) {
            continue;
        }
        let mut e = a;
        while e < b.len() && (is_word(b[e]) || matches!(b[e], b'<' | b'>' | b',' | b' ')) {
            e += 1;
        }
        // the run is greedy and then needs `&`: some prefix of it of length >= 1 followed by `&` (only the run's end can be followed by `&`, `&` not being in the class)
        if e > a && e < b.len() && b[e] == b'&' {
            return true;
        }
    }
    false
}

pub fn hmma_msl(src: &str) -> Result<Cow<'_, str>, PostError> {
    let Some((mstart, rt, ptext, mend)) = find_definition(src) else {
        return Ok(Cow::Borrowed(src));
    };
    let params = split_params(ptext);
    let scr: Vec<&str> = params
        .iter()
        .copied()
        .filter(|x| x.starts_with("threadgroup "))
        .collect();
    let plain: Vec<&str> = params
        .iter()
        .copied()
        .filter(|x| !x.starts_with("threadgroup "))
        .collect();
    if scr.len() != 1 || plain.len() != ARGS.len() {
        return Err(failed(
            "hmma_msl",
            format!(
                "dmsh_hmma16816_f16: unexpected parameters {}",
                cut(ptext, 200)
            ),
        ));
    }
    let unexpected = |x: &str| {
        failed(
            "hmma_msl",
            format!("dmsh_hmma16816_f16: unexpected parameter {}", cut(x, 100)),
        )
    };
    let mut v: HashMap<&str, String> = HashMap::new();
    for (k, x) in ARGS.iter().zip(plain.iter()) {
        v.insert(k, last_word(x).ok_or_else(|| unexpected(x))?.to_string());
    }
    v.insert(
        "scr",
        last_word(scr[0])
            .ok_or_else(|| unexpected(scr[0]))?
            .to_string(),
    );
    {
        // re.match(r'thread (\w+)&', plain[-1])
        let last = plain[plain.len() - 1];
        let mut c = Cur::at(last, 0);
        let dt = if c.lit("thread ") {
            c.word().filter(|_| c.lit("&"))
        } else {
            None
        };
        v.insert("dt", dt.ok_or_else(|| unexpected(last))?.to_string());
    }
    v.insert("rt", rt.to_string());
    let b = src.as_bytes();
    let body_end = block_end(b, mend - 1)
        .ok_or_else(|| failed("hmma_msl", "dmsh_hmma16816_f16: block not closed"))?;
    // where each function opens its body (a line holding only a brace): every `\n{\n`, overlapping ones included
    let mut opens: Vec<usize> = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"\n{\n", from) {
        opens.push(p);
        from = p + 1;
    }
    let mut calls: Vec<Call> = Vec::new();
    let mut callers: Vec<(usize, Option<(String, String)>)> = Vec::new();
    let mut ends: HashMap<usize, usize> = HashMap::new();
    for (cs, ce, cargs) in calls_of(src) {
        if mstart <= cs && cs <= body_end {
            continue;
        }
        let lim = cs as i64 - 3;
        let k = opens.partition_point(|&o| (o as i64) <= lim) as i64 - 1;
        let open_at = if k >= 0 { opens[k as usize] + 1 } else { 0 };
        if open_at > 0 && !ends.contains_key(&open_at) {
            ends.insert(
                open_at,
                block_end(b, open_at)
                    .ok_or_else(|| failed("hmma_msl", "dmsh_hmma16816_f16: block not closed"))?,
            );
        }
        if open_at == 0 || ends[&open_at] < cs {
            return Err(failed(
                "hmma_msl",
                "dmsh_hmma16816_f16: a call outside a function body",
            ));
        }
        if !callers.iter().any(|(o, _)| *o == open_at) {
            let scratch = *split_params(cargs).last().unwrap();
            let arrays: Vec<(&str, &str)> = tg_arrays(src, open_at, ends[&open_at])
                .into_iter()
                .filter(|(_, n)| *n != scratch)
                .collect();
            callers.push((
                open_at,
                if arrays.len() == 1 {
                    Some((arrays[0].0.to_string(), arrays[0].1.to_string()))
                } else {
                    None
                },
            ));
        }
        calls.push(Call {
            start: cs,
            end: ce,
            args: cargs,
            open_at,
        });
    }
    if calls.is_empty() {
        return Ok(Cow::Borrowed(src));
    }
    let mut shared: Vec<&Option<(String, String)>> = Vec::new();
    for (_, a) in &callers {
        if !shared.contains(&a) {
            shared.push(a);
        }
    }
    let direct = shared.len() == 1 && shared[0].is_some();
    let writers = writer_names(src);
    let mut modes: HashMap<usize, i64> = HashMap::new();
    for (open_at, _) in &callers {
        let fcalls: Vec<(&Call, Vec<&str>)> = calls
            .iter()
            .filter(|c| c.open_at == *open_at)
            .map(|c| (c, split_params(c.args)))
            .collect();
        for (s, m) in bslots(src, *open_at, ends[open_at], &fcalls, &writers)? {
            modes.insert(s, m);
        }
    }
    let nb = modes
        .values()
        .map(|&v| (v / 2 + 1) * 2)
        .max()
        .unwrap_or(2)
        .max(2);
    let mut extra = format!(
        ", thread simdgroup_half8x8 (&acc)[16], thread simdgroup_half8x8 (&am)[4], thread simdgroup_half8x8 (&bm)[{nb}], const int bmode"
    );
    if direct {
        extra.push_str(&format!(
            ", threadgroup {}& dm_shm",
            shared[0].as_ref().unwrap().0
        ));
    }
    v.insert(
        "adirect",
        if direct {
            "(f & 16) != 0".to_string()
        } else {
            "false".to_string()
        },
    );
    v.insert(
        "shm_base",
        if direct {
            "(threadgroup half*)&dm_shm[0]".to_string()
        } else {
            "(threadgroup half*)nullptr".to_string()
        },
    );
    let body = format_body(&v);
    let mut edits: Vec<(usize, usize, String)> = vec![(
        mstart,
        body_end + 1,
        format!("\n{rt} dmsh_hmma16816_f16({ptext}{extra})\n{body}"),
    )];
    for c in &calls {
        let mode = modes.get(&c.start).copied().unwrap_or(-1);
        let tail = if direct {
            let o = callers.iter().find(|(o, _)| *o == c.open_at).unwrap();
            format!(", {}", o.1.as_ref().unwrap().1)
        } else {
            String::new()
        };
        edits.push((
            c.end - 1,
            c.end - 1,
            format!(", dm_acc, dm_am, dm_bm, {mode}{tail}"),
        ));
    }
    for (open_at, _) in &callers {
        edits.push((
            open_at + 2,
            open_at + 2,
            format!("    simdgroup_half8x8 dm_acc[16], dm_am[4], dm_bm[{nb}];\n"),
        ));
    }
    edits.sort_by_key(|a| std::cmp::Reverse(a.0)); // stable: equal starts keep their order, as Python's sorted(reverse=True)
    let mut out = src.to_string();
    for (a, e, text) in edits {
        out.replace_range(a..e, &text);
    }
    Ok(Cow::Owned(out))
}

/// `HMMA_BODY % v`
fn format_body(v: &HashMap<&str, String>) -> String {
    let tpl = t::HMMA_BODY;
    let mut out = String::with_capacity(tpl.len() + 512);
    let b = tpl.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 1 < b.len() && b[i + 1] == b'%' {
                out.push('%');
                i += 2;
                continue;
            }
            if i + 1 < b.len() && b[i + 1] == b'(' {
                let close = tpl[i..]
                    .find(")s")
                    .map(|x| x + i)
                    .expect("a %(name)s placeholder");
                out.push_str(&v[&tpl[i + 2..close]]);
                i = close + 2;
                continue;
            }
        }
        out.push(tpl[i..].chars().next().unwrap());
        i += tpl[i..].chars().next().unwrap().len_utf8();
    }
    out
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
