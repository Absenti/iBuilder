//! The small Ampere / Switch 2 rewrites of dmsh_rewrite.py: `mul_extended`, `float_add_msl`, `gather_offsets`, `shared_store_msl`. Each is a fixed idiom of the translator's text,
//! so each regex is matched with a cursor (the Python's back-references are the captured names compared as text).
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;

// ------------------------------------------------------------------------------------------------------------------------------ mul_extended

/// the helper `template<typename T, typename U, typename V>\s*\[\[clang::optnone\]\] T spvMulExtended\(V l, V r\)\s*\{\s*return T\{U\(l \* r\), U\(mulhi\(l, r\)\)\};\s*\}\n` at `p` -> its end
fn mul_helper_at(src: &str, p: usize) -> Option<usize> {
    let mut c = Cur::at(src, p);
    if !c.lit("template<typename T, typename U, typename V>") {
        return None;
    }
    c.ws0();
    if !c.lit("[[clang::optnone]] T spvMulExtended(V l, V r)") {
        return None;
    }
    c.ws0();
    if !c.lit("{") {
        return None;
    }
    c.ws0();
    if !c.lit("return T{U(l * r), U(mulhi(l, r))};") {
        return None;
    }
    c.ws0();
    if !c.lit("}\n") {
        return None;
    }
    Some(c.i)
}

pub fn mul_extended(src: &str) -> Cow<'_, str> {
    if !contains(src, "spvMulExtended") {
        return Cow::Borrowed(src);
    }
    let b = src.as_bytes();
    let mut found: Vec<usize> = Vec::new(); // the ends of the matches
    let mut from = 0;
    while let Some(p) = find_b(b, b"template<typename T, typename U, typename V>", from) {
        match mul_helper_at(src, p) {
            Some(e) => {
                found.push(e);
                from = e;
            }
            None => from = p + 1,
        }
    }
    if found.len() != 1 || contains(src, "spvMulExtended(uint l, uint r)") {
        return Cow::Borrowed(src);
    }
    let mut scalar = String::new();
    for (ty, w) in [("uint", "ulong"), ("int", "long")] {
        scalar.push_str(&format!("template<typename T, typename U> static inline __attribute__((always_inline)) T spvMulExtended({ty} l, {ty} r) {{ {w} p = {w}(l) * {w}(r); return T{{U(p), U(p >> 32)}}; }}\n"));
    }
    let e = found[0];
    Cow::Owned(format!("{}{scalar}{}", &src[..e], &src[e..]))
}

// ------------------------------------------------------------------------------------------------------------------------------ float_add_msl

struct FloatAdd<'a> {
    start: usize,
    end: usize,
    ind: &'a str,
    addr: &'a str,
    addr2: &'a str,
    val: &'a str,
    /// the retry flag, declared before the loop: kept as `bool AGAIN = false;`, its value on the loop's normal exit, for a later read
    again: &'a str,
}

/// one line `<text>\n` at the cursor: the text up to the newline, the cursor after the newline
fn take_line<'a>(c: &mut Cur<'a>) -> Option<&'a str> {
    let b = c.b();
    let e = memchr::memchr(b'\n', &b[c.i..])? + c.i;
    let l = &c.s[c.i..e];
    c.i = e + 1;
    Some(l)
}

/// FLOAT_ADD at the line start `p` (re.M `^`)
fn float_add_at(src: &str, p: usize) -> Option<FloatAdd<'_>> {
    let mut c = Cur::at(src, p);
    let ind = c.spaces();
    if !c.lit("bool ") {
        return None;
    }
    let again = c.tmp_nb()?;
    if !c.lit(";\n") {
        return None;
    }
    let pre = |c: &mut Cur, rest: &str| -> bool { c.lit(ind) && c.lit(rest) };
    if !pre(&mut c, "do\n") || !pre(&mut c, "{\n") {
        return None;
    }
    // float OLD = as_type<float>(ADDR);\n
    if !pre(&mut c, "    float ") {
        return None;
    }
    let old = c.tmp_nb()?;
    if !c.lit(" = as_type<float>(") {
        return None;
    }
    let l = take_line(&mut c)?;
    let addr = l.strip_suffix(");")?;
    // float SUM = (?:OLD + VAL|spvFAdd(OLD, VALP));\n
    if !pre(&mut c, "    float ") {
        return None;
    }
    let sum = c.tmp_nb()?;
    if !c.lit(" = ") {
        return None;
    }
    let l = take_line(&mut c)?;
    let val: &str = {
        let pf = format!("{old} + ");
        if let Some(r) = l.strip_prefix(pf.as_str()) {
            // `[^;\n]+` then `;` at the end of the line
            let v = r.strip_suffix(';')?;
            if v.is_empty() || v.contains(';') {
                return None;
            }
            v
        } else {
            let pf = format!("spvFAdd({old}, ");
            let r = l.strip_prefix(pf.as_str())?;
            let r = r.strip_suffix(");")?;
            if r.is_empty() || r.contains(';') {
                return None;
            }
            r
        }
    };
    // uint WANT = as_type<uint>(OLD);\n
    if !pre(&mut c, "    uint ") {
        return None;
    }
    let want = c.tmp_nb()?;
    if !c.lit(" = as_type<uint>(") || !c.lit(old) || !c.lit(");\n") {
        return None;
    }
    // uint SEEN;\n
    if !pre(&mut c, "    uint ") {
        return None;
    }
    let seen = c.tmp_nb()?;
    if !c.lit(";\n") {
        return None;
    }
    if !pre(&mut c, "    do\n") || !pre(&mut c, "    {\n") {
        return None;
    }
    if !pre(&mut c, "        ") || !c.lit(seen) || !c.lit(" = ") || !c.lit(want) || !c.lit(";\n") {
        return None;
    }
    // } while (!atomic_compare_exchange_weak_explicit((device atomic_uint*)&ADDR2, &SEEN, as_type<uint>(SUM), memory_order_relaxed, memory_order_relaxed) && SEEN == WANT);\n
    if !pre(
        &mut c,
        "    } while (!atomic_compare_exchange_weak_explicit((device atomic_uint*)&",
    ) {
        return None;
    }
    let l = take_line(&mut c)?;
    let tail = format!(
        ", &{seen}, as_type<uint>({sum}), memory_order_relaxed, memory_order_relaxed) && {seen} == {want});"
    );
    let addr2 = l.strip_suffix(tail.as_str())?;
    // uint FOUND = SEEN;\n
    if !pre(&mut c, "    uint ") {
        return None;
    }
    let found = c.tmp_nb()?;
    if !c.lit(" = ") || !c.lit(seen) || !c.lit(";\n") {
        return None;
    }
    // AGAIN = int(FOUND) != as_type<int>(OLD);\n
    if !pre(&mut c, "    ")
        || !c.lit(again)
        || !c.lit(" = int(")
        || !c.lit(found)
        || !c.lit(") != as_type<int>(")
        || !c.lit(old)
        || !c.lit(");\n")
    {
        return None;
    }
    // } while (AGAIN);\n
    if !pre(&mut c, "} while (") || !c.lit(again) || !c.lit(");\n") {
        return None;
    }
    Some(FloatAdd {
        start: p,
        end: c.i,
        ind,
        addr,
        addr2,
        val,
        again,
    })
}

pub fn float_add_msl(src: &str) -> Cow<'_, str> {
    if !contains(src, "atomic_compare_exchange_weak_explicit") {
        return Cow::Borrowed(src);
    }
    let b = src.as_bytes();
    let mut out: Option<String> = None;
    let (mut at, mut p) = (0usize, 0usize);
    while p < b.len() {
        // the line start `^` positions only
        if (p == 0 || b[p - 1] == b'\n')
            && let Some(m) = float_add_at(src, p)
        {
            if m.addr == m.addr2 {
                let o = out.get_or_insert_with(|| String::with_capacity(src.len()));
                o.push_str(&src[at..m.start]);
                o.push_str(&format!("{}atomic_fetch_add_explicit((device atomic_float*)&{}, {}, memory_order_relaxed);\n", m.ind, m.addr, m.val));
                o.push_str(&format!("{}bool {} = false;\n", m.ind, m.again));
                at = m.end;
            }
            p = m.end;
            continue;
        }
        match memchr::memchr(b'\n', &b[p..]) {
            Some(r) => p += r + 1,
            None => break,
        }
    }
    match out {
        Some(mut o) => {
            o.push_str(&src[at..]);
            Cow::Owned(o)
        }
        None => Cow::Borrowed(src),
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ gather_offsets

fn gather_quad_char(rel: (i64, i64)) -> Option<char> {
    match rel {
        (0, 0) => Some('w'),
        (1, 0) => Some('z'),
        (0, 1) => Some('x'),
        (1, 1) => Some('y'),
        _ => None,
    }
}

/// `int2\(\s*(-?\d+)\s*(?:,\s*(-?\d+)\s*)?\)` fullmatch -> (x, y)
fn int2_args(o: &str) -> Option<(i64, i64)> {
    let mut c = Cur::at(o, 0);
    if !c.lit("int2(") {
        return None;
    }
    c.ws0();
    let x: i64 = c.int()?.parse().ok()?;
    c.ws0();
    let mut y = x;
    if c.peek() == Some(b',') {
        c.i += 1;
        c.ws0();
        y = c.int()?.parse().ok()?;
        c.ws0();
    }
    if !c.lit(")") || !c.at_end() {
        return None;
    }
    Some((x, y))
}

/// gather_quad(off) -> (the square's corner as MSL, the swizzle)
fn gather_quad(off: &[&str]) -> Option<(String, String)> {
    let mut xy = Vec::new();
    for o in off {
        xy.push(int2_args(o)?);
    }
    let bx = xy.iter().map(|p| p.0).min()?;
    let by = xy.iter().map(|p| p.1).min()?;
    let rel: Vec<(i64, i64)> = xy.iter().map(|&(x, y)| (x - bx, y - by)).collect();
    let mut sorted = rel.clone();
    sorted.sort_unstable();
    if sorted != [(0, 0), (0, 1), (1, 0), (1, 1)] {
        return None;
    }
    let sw: String = rel.iter().map(|&r| gather_quad_char(r).unwrap()).collect();
    Some((format!("int2({bx}, {by})"), sw))
}

/// `\bspvGather(Compare)?ConstOffsets\(` at `p` -> (is compare, end of the match)
fn gather_call_at(src: &str, p: usize) -> Option<(bool, usize)> {
    let mut c = Cur::at(src, p);
    if !c.lit("spvGather") {
        return None;
    }
    let cmp = c.lit("Compare");
    if !c.lit("ConstOffsets(") {
        return None;
    }
    Some((cmp, c.i))
}

pub fn gather_offsets(src: &str) -> Result<Cow<'_, str>, PostError> {
    let b = src.as_bytes();
    // re.search(r'\bspvGather(Compare)?ConstOffsets\(spvDesc|= spvGather(Compare)?ConstOffsets\(', msl)
    let mut hit = false;
    let mut from = 0;
    while let Some(p) = find_b(b, b"spvGather", from) {
        from = p + 1;
        if let Some((_, e)) = gather_call_at(src, p)
            && ((!in_word(b, p) && starts(b, e, "spvDesc")) || (p >= 2 && &b[p - 2..p] == b"= "))
        {
            hit = true;
            break;
        }
    }
    if !hit {
        return Ok(Cow::Borrowed(src));
    }
    // arrays: `\bconstant spvUnsafeArray<int2, 4> (\w+) = spvUnsafeArray<int2, 4>\(\{ (.*?) \}\);`
    let mut arrays: Vec<(&str, Vec<&str>)> = Vec::new();
    let pre = "constant spvUnsafeArray<int2, 4> ";
    let mut from = 0;
    while let Some(p) = find_b(b, pre.as_bytes(), from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let mut c = Cur::at(src, p + pre.len());
        let Some(name) = c.word() else { continue };
        if !c.lit(" = spvUnsafeArray<int2, 4>({ ") {
            continue;
        }
        let g0 = c.i;
        let Some(rel) = find_b(b, b" });", g0) else {
            continue;
        };
        if memchr::memchr(b'\n', &b[g0..rel]).is_some() {
            continue;
        }
        let group = &src[g0..rel];
        // re.findall(r'int2\([^()]*\)', group)
        let gb = group.as_bytes();
        let mut off: Vec<&str> = Vec::new();
        let mut f = 0;
        while let Some(q) = find_b(gb, b"int2(", f) {
            match gb[q + 5..].iter().position(|&x| x == b'(' || x == b')') {
                Some(r) if gb[q + 5 + r] == b')' => {
                    off.push(&group[q..q + 5 + r + 1]);
                    f = q + 5 + r + 1;
                }
                _ => f = q + 1,
            }
        }
        if off.len() != 4 {
            return Err(failed(
                "gather_offsets",
                format!(
                    "gather offsets: unexpected array {}",
                    cut(&src[p..rel + 4], 120)
                ),
            ));
        }
        if let Some(e) = arrays.iter_mut().find(|(n, _)| *n == name) {
            e.1 = off;
        } else {
            arrays.push((name, off));
        }
        from = rel + 4;
    }
    let mut out = String::with_capacity(src.len() + 512);
    let mut i = 0usize;
    let mut from = 0;
    while let Some(p) = find_b(b, b"spvGather", from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let Some((cmp, mend)) = gather_call_at(src, p) else {
            continue;
        };
        // the lookahead `(?=[^)]*?\w)`: a word character before the next `)`
        let mut k = mend;
        let look = loop {
            if k >= b.len() {
                break false;
            }
            if b[k] == b')' {
                break false;
            }
            if is_word(b[k]) {
                break true;
            }
            k += 1;
        };
        if !look {
            continue;
        }
        from = mend;
        if p < i || starts(b, mend, "const ") {
            continue;
        }
        // the arguments, split at the commas of depth 1
        let (mut depth, mut args, mut cur, mut j) = (1i32, Vec::<&str>::new(), mend, mend);
        while depth != 0 {
            if j >= b.len() {
                return Err(failed("gather_offsets", "string index out of range"));
            }
            let ch = b[j];
            j += 1;
            if ch == b'(' {
                depth += 1;
            } else if ch == b')' {
                depth -= 1;
            }
            if depth == 0 || (ch == b',' && depth == 1) {
                args.push(strip(&src[cur..j - 1]));
                cur = j;
            }
        }
        let base = if cmp { 3 } else { 4 };
        if args.len() <= base || !arrays.iter().any(|(n, _)| *n == args[2]) {
            return Err(failed(
                "gather_offsets",
                format!("gather offsets: unexpected call {}", cut(&src[p..j], 160)),
            ));
        }
        let off = &arrays.iter().rev().find(|(n, _)| *n == args[2]).unwrap().1;
        let (tex, smp) = (args[0], args[1]);
        let rest = args[base..].join(", ");
        let tail = if cmp {
            String::new()
        } else {
            format!(", {}", args[3])
        };
        let f = if cmp { "gather_compare" } else { "gather" };
        out.push_str(&src[i..p]);
        match gather_quad(off) {
            Some((corner, sw)) => {
                out.push_str(&format!("({tex}.{f}({smp}, {rest}, {corner}{tail}).{sw})"))
            }
            None => {
                let parts: Vec<String> = off
                    .iter()
                    .map(|o| format!("{tex}.{f}({smp}, {rest}, {o}{tail}).w"))
                    .collect();
                out.push_str(&format!("dm_gather4({})", parts.join(", ")));
            }
        }
        i = j;
    }
    out.push_str(&src[i..]);
    if !contains(&out, "dm_gather4(") {
        return Ok(Cow::Owned(out));
    }
    let u = find(&out, "using namespace metal;", 0)
        .ok_or_else(|| failed("gather_offsets", "substring not found"))?;
    let k = find(&out, "\n", u).ok_or_else(|| failed("gather_offsets", "substring not found"))? + 1;
    out.insert_str(k, t::GATHER4);
    Ok(Cow::Owned(out))
}

/// at most `n` characters (the Python's `[:n]`)
fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ------------------------------------------------------------------------------------------------------------------------------ shared_store_msl

/// dmsh_rewrite.split_params: commas inside template arguments do not separate parameters
pub fn split_params(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let (mut params, mut depth, mut cur) = (Vec::new(), 0i32, 0usize);
    for i in 0..=b.len() {
        let ch = if i < b.len() { b[i] } else { b',' };
        if ch == b'<' {
            depth += 1;
        } else if ch == b'>' {
            depth -= 1;
        }
        if ch == b',' && depth == 0 {
            params.push(strip(&text[cur..i]));
            cur = i + 1;
        }
    }
    params
}

/// `thread const u?int& (\w+)` fullmatch -> the name
fn scalar_param(p: &str) -> Option<&str> {
    let mut c = Cur::at(p, 0);
    if !c.lit("thread const ") {
        return None;
    }
    c.lit("u");
    if !c.lit("int& ") {
        return None;
    }
    let n = c.word()?;
    if c.at_end() { Some(n) } else { None }
}

/// `(threadgroup|device) spvUnsafeArray<uint, \d+>& (\w+)` fullmatch -> (space, name)
fn array_param(p: &str) -> Option<(&str, &str)> {
    let mut c = Cur::at(p, 0);
    let space = if c.lit("threadgroup") {
        "threadgroup"
    } else if c.lit("device") {
        "device"
    } else {
        return None;
    };
    if !c.lit(" spvUnsafeArray<uint, ") || c.digits().is_none() || !c.lit(">& ") {
        return None;
    }
    let n = c.word()?;
    if c.at_end() { Some((space, n)) } else { None }
}

pub fn shared_store_msl(src: &str) -> Result<Cow<'_, str>, PostError> {
    if !contains(src, "dmsh_shared_store") {
        return Ok(Cow::Borrowed(src));
    }
    let b = src.as_bytes();
    // SHARED_STORE `\nvoid (dmsh_shared_store(8|16)_\d+)\(([^)]*)\)\n\{`: (bits, params, end)
    let mut found: Vec<(bool, &str, usize)> = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"\nvoid dmsh_shared_store", from) {
        from = p + 1;
        let mut c = Cur::at(src, p + 23);
        let wide = if c.lit("16") {
            true
        } else if c.lit("8") {
            false
        } else {
            continue;
        };
        if !c.lit("_") || c.digits().is_none() || !c.lit("(") {
            continue;
        }
        let g0 = c.i;
        let Some(r) = memchr::memchr(b')', &b[g0..]) else {
            continue;
        };
        let g1 = g0 + r;
        if !starts(b, g1, ")\n{") {
            continue;
        }
        found.push((wide, &src[g0..g1], g1 + 3));
        from = g1 + 3;
    }
    if found.is_empty() {
        return Err(failed(
            "shared_store_msl",
            "dmsh_shared_store: no definition of the expected form",
        ));
    }
    let mut out = String::with_capacity(src.len() + 256);
    let mut at = 0usize;
    for &(wide, ptext, mend) in &found {
        let params = split_params(ptext);
        let bad = || {
            failed(
                "shared_store_msl",
                format!(
                    "dmsh_shared_store: unexpected parameters {}",
                    cut(ptext, 160)
                ),
            )
        };
        if params.len() != 3 {
            return Err(bad());
        }
        let (Some(off), Some(val), Some((space, mem))) = (
            scalar_param(params[0]),
            scalar_param(params[1]),
            array_param(params[2]),
        ) else {
            return Err(bad());
        };
        let store = if !wide {
            format!("(({space} uchar*)&{mem}[0])[uint({off})] = uchar(uint({val}));")
        } else {
            format!(
                "(({space} dm_ushort_alias*)&{mem}[0])[uint({off}) >> 1u] = ushort(uint({val}));"
            )
        };
        if at <= mend {
            out.push_str(&src[at..mend]);
        }
        out.push_str("\n    ");
        out.push_str(&store);
        out.push('\n');
        at = block_end(b, mend - 1)
            .ok_or_else(|| failed("shared_store_msl", "dmsh_hmma16816_f16: block not closed"))?;
    }
    out.push_str(&src[at.min(src.len())..]);
    if found.iter().any(|f| f.0) {
        let key = "using namespace metal;\n";
        let u = find(&out, key, 0).ok_or_else(|| {
            failed(
                "shared_store_msl",
                "dmsh_shared_store: no using namespace metal",
            )
        })? + key.len();
        out.insert_str(u, t::SHARED_ALIAS);
    }
    Ok(Cow::Owned(out))
}
