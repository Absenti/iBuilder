//! dmsh_reflect.pool_arrays: the arrays of texture / sampler handles Ryujinx sizes to a whole ID range (<stage>_<t|s>_cb0_40000000, or a bindless texture table in one of the game's uniform
//! buffers) become a pointer to a pool the replay keeps, `struct spvPool_<name> { uint n; ulong seen; array<element, 1> e; }`, read as `(&base.name->e[0])[min(index, base.name->n)]`.
use crate::border::{descriptor_struct, parse_member};
use crate::templates as t;
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;

/// `decode_ubo_tex(name)` is not None: `re.match(r'[a-z]+_t\w*?_cb(\d+)_([0-9A-F]+)', name)` and the slot / handle tests behind it
pub fn decode_ubo_tex_ok(name: &str) -> bool {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_lowercase() {
        i += 1;
    }
    if i == 0 || !starts(b, i, "_t") {
        return false;
    }
    // `\w*?` is lazy: the first k (every character before it a word character) where `_cb\d+_[0-9A-F]+` follows
    let mut k = i + 2;
    loop {
        let mut c = Cur::at(name, k);
        if c.lit("_cb")
            && let Some(d) = c.digits()
            && c.lit("_")
        {
            let h0 = c.i;
            while c.i < b.len() && (b[c.i].is_ascii_digit() || (b'A'..=b'F').contains(&b[c.i])) {
                c.i += 1;
            }
            if c.i > h0 {
                return ubo_tex_accepts(d, &name[h0..c.i]);
            }
        }
        if k < b.len() && is_word(b[k]) {
            k += 1;
        } else {
            return false;
        }
    }
}

fn ubo_tex_accepts(slots: &str, hex: &str) -> bool {
    let s = slots.trim_start_matches('0');
    let h = hex.trim_start_matches('0');
    let Ok(slots) = (if s.is_empty() {
        Ok(0u128)
    } else {
        s.parse::<u128>()
    }) else {
        return false;
    };
    let Ok(h) = (if h.is_empty() {
        Ok(0u128)
    } else {
        u128::from_str_radix(h, 16)
    }) else {
        return false;
    };
    let tb = slots & 0xffff;
    let sb: i128 = if slots >> 16 != 0 {
        (slots >> 16) as i128 - 1
    } else {
        tb as i128
    };
    let ty = h >> 28;
    !(!(3..=16).contains(&tb)
        || !matches!(ty, 0 | 1 | 3)
        || (ty == 1 && sb != 2 && !(3..=16).contains(&sb)))
}

/// `array<.+, (\d+)>` fullmatch -> the count
fn array_count(ty: &str) -> Option<u64> {
    let r = ty.strip_prefix("array<")?.strip_suffix('>')?;
    let c = r.rfind(", ")?;
    if c == 0 {
        return None;
    }
    let d = &r[c + 2..];
    if d.is_empty() || !d.bytes().all(|x| x.is_ascii_digit()) {
        return None;
    }
    Some(d.parse().unwrap_or(u64::MAX))
}

/// `^(\s*)array<(.+), (\d+)> (\w+) \[\[id\(` on one line -> (indent, element, count digits, name, end of the match)
fn array_member(line: &str) -> Option<(&str, &str, &str, &str, usize)> {
    let mut c = Cur::at(line, 0);
    let lead = c.ws0();
    if !c.lit("array<") {
        return None;
    }
    let rest0 = c.i;
    let b = line.as_bytes();
    // `(.+)` is greedy: the rightmost `, (\d+)> (\w+) \[\[id\(` wins
    let mut k = line.len();
    while k > rest0 + 1 {
        k -= 1;
        if !starts(b, k, ", ") {
            continue;
        }
        let mut c = Cur::at(line, k + 2);
        let Some(d) = c.digits() else { continue };
        if !c.lit("> ") {
            continue;
        }
        let Some(n) = c.word() else { continue };
        if !c.lit(" [[id(") {
            continue;
        }
        return Some((lead, &line[rest0..k], d, n, c.i));
    }
    None
}

struct Pool {
    name: String,
    elem: String,
    /// the array's length
    len: u64,
    marked: bool,
}

/// `members = re.sub(r'^(\s*)array<(.+), (\d+)> (\w+) \[\[id\(', to_pointer, body, flags=re.M)`: the pool members as pointers, and the pools (in first-seen order, a later
/// member of the same name taking its place's values, as the Python's dict)
fn to_pointers(body: &str, pools: &mut Vec<Pool>) -> String {
    let mut members = String::with_capacity(body.len() + 64);
    let mut first = true;
    for l in body.split('\n') {
        if !first {
            members.push('\n');
        }
        first = false;
        if let Some((lead, elem, d, name, end)) = array_member(l) {
            let n: u64 = d.parse().unwrap_or(u64::MAX);
            if n >= t::POOL_MIN as u64
                && (name.ends_with("_cb0_40000000")
                    || decode_ubo_tex_ok(name.strip_suffix("Smplr").unwrap_or(name)))
            {
                let marked = elem != "sampler" && name.ends_with("_cb0_40000000");
                if let Some(p) = pools.iter_mut().find(|p| p.name == name) {
                    p.elem = elem.to_string();
                    p.len = n;
                    p.marked = marked;
                } else {
                    pools.push(Pool {
                        name: name.to_string(),
                        elem: elem.to_string(),
                        len: n,
                        marked,
                    });
                }
                members.push_str(&format!("{lead}constant spvPool_{name}* {name} [[id("));
                members.push_str(&l[end..]);
                continue;
            }
        }
        members.push_str(l);
    }
    members
}

/// `\batomic_\w+_explicit\b`
fn has_atomic_explicit(s: &str) -> bool {
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"atomic_", from) {
        from = p + 1;
        if in_word(b, p) {
            continue;
        }
        let e = word_end(b, p + 7);
        let w = &s[p + 7..e];
        if w.len() > 9 && w.ends_with("_explicit") {
            return true;
        }
    }
    false
}

/// the text between two `[[` ... `]]`-style tokens: `\[\[\s*<word><tail>`
fn bracket_word(s: &str, word: &str, close_ws: bool, close: &str) -> bool {
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"[[", from) {
        from = p + 1;
        let mut c = Cur::at(s, p + 2);
        c.ws0();
        if !c.lit(word) {
            continue;
        }
        if close_ws {
            c.ws0();
        }
        if c.lit(close) || close.is_empty() {
            return true;
        }
    }
    false
}

/// `^\s*device\s` (re.M): a `device` after a whitespace run that holds a newline or starts the text
fn device_at_line_start(s: &str) -> bool {
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, b"device", from) {
        from = p + 1;
        if !(p + 6 < b.len() && is_space(b[p + 6])) {
            continue;
        }
        let mut j = p;
        while j > 0 && is_space(b[j - 1]) {
            j -= 1;
        }
        if j == 0 || b[j..p].contains(&b'\n') {
            return true;
        }
    }
    false
}

/// whether the shader does something itself that keeps its depth test where the game's own shader has it (the pool mark's early-test rule)
fn own_side_effects(own: &str) -> bool {
    count_word(own, "discard_fragment") > 0
        || bracket_word(own, "depth(", false, "")
        || bracket_word(own, "sample_mask", true, "]]")
        || bracket_word(own, "stencil", true, "]]")
        || contains(own, ".write(")
        || has_atomic_explicit(own)
        || device_at_line_start(own)
}

pub fn pool_arrays(src: &str) -> Result<Cow<'_, str>, PostError> {
    let Some((st, g0, g1)) = descriptor_struct(src) else {
        return Ok(Cow::Borrowed(src));
    };
    let body = &src[g0..g1];
    // nearly every shader has no pool: look for a qualifying member before copying anything
    if !contains(body, "array<") || !body.split('\n').any(|l| matches!(array_member(l), Some((_, _, d, n, _)) if d.parse::<u64>().unwrap_or(u64::MAX) >= t::POOL_MIN as u64 && (n.ends_with("_cb0_40000000") || decode_ubo_tex_ok(n.strip_suffix("Smplr").unwrap_or(n))))) { return Ok(Cow::Borrowed(src)); }
    let mut pools: Vec<Pool> = Vec::new();
    let mut members = to_pointers(body, &mut pools);
    if pools.is_empty() {
        return Ok(Cow::Borrowed(src));
    }
    // a pool handed to a function as a `constant array<...>&` parameter: the parameters, the forwarding calls and the uses are rewritten together
    let callable = pools.iter().any(|p| callable_parameter(src, &p.name));
    let mut text: Cow<str> = Cow::Borrowed(src);
    let (mut st, mut g0, mut g1) = (st, g0, g1);
    if callable {
        text = Cow::Owned(callable_pool_arrays(src, &pools)?);
        let Some(d) = descriptor_struct(&text) else {
            return Err(failed(
                "pool_arrays",
                "pool callable: descriptor definition was lost",
            ));
        };
        (st, g0, g1) = d;
        members = to_pointers(&text[g0..g1], &mut pools);
    }
    let src: &str = &text;
    // the argument IDs are renumbered in member order, a pointer taking one and any other array its length
    let mut aid: u64 = 0;
    let mut renum = String::with_capacity(members.len() + 64);
    let mut first = true;
    for l in members.split('\n') {
        if !first {
            renum.push('\n');
        }
        first = false;
        match parse_member(l) {
            Some(m) => {
                renum.push_str(&format!(
                    "{}{} {} [[id({aid}){}]];",
                    m.lead, m.ty, m.name, m.attrs
                ));
                aid = aid.saturating_add(array_count(strip(m.ty)).unwrap_or(1));
                renum.push_str(&l[m.end..]);
            }
            None => renum.push_str(l),
        }
    }
    let mut structs: String = pools
        .iter()
        .map(|p| {
            format!(
                "struct spvPool_{}\n{{\n    uint n;\n{}    ulong seen;\n    array<{}, 1> e;\n}};\n\n",
                p.name,
                if p.elem.contains("texture_buffer<") {
                    "    uint bias;\n"
                } else {
                    ""
                },
                p.elem
            )
        })
        .collect();
    if pools.iter().any(|p| p.elem.contains("texture_buffer<")) {
        structs = format!("{}{structs}", t::BUFFER_POOL_READ);
    }
    let marked = pools.iter().any(|p| p.marked);
    if marked {
        structs = format!("{}{structs}", t::POOL_MARK);
    }
    let mut cur = String::with_capacity(src.len() + structs.len() + 512);
    cur.push_str(&src[..st]);
    cur.push_str(&structs);
    cur.push_str(&src[st..g0]);
    cur.push_str(&renum);
    cur.push_str(&src[g1..]);
    for p in pools.iter().filter(|_| !callable) {
        // base.name[index] -> (&base.name->e[0])[min(uint(index), base.name->n)], the index taken with its brackets balanced
        let nm = p.name.as_str();
        let needle = format!(".{nm}[");
        let b = cur.as_bytes();
        let mut out = String::with_capacity(cur.len() + 256);
        let mut pos = 0usize;
        let mut from = 0usize;
        while let Some(d) = find_b(b, needle.as_bytes(), from) {
            from = d + 1;
            // `([\w.]+)` before the dot: the maximal run
            let mut s0 = d;
            while s0 > 0 && (is_word(b[s0 - 1]) || b[s0 - 1] == b'.') {
                s0 -= 1;
            }
            if s0 == d {
                continue;
            }
            if s0 < pos {
                continue;
            }
            let m_end = d + needle.len();
            let mut depth = 1i32;
            let mut j = m_end;
            while depth != 0 {
                if j >= b.len() {
                    return Err(failed("pool_arrays", format!("{nm}: unbalanced index")));
                }
                match b[j] {
                    b'[' => depth += 1,
                    b']' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let base = format!("{}.{nm}", &cur[s0..d]);
            let index = format!("min(uint({}), {base}->n)", &cur[m_end..j - 1]);
            out.push_str(&cur[pos..s0]);
            if p.marked {
                out.push_str(&format!("(&{base}->e[0])[dm_pool_mark({base}, {index})]"));
            } else {
                out.push_str(&format!("(&{base}->e[0])[{index}]"));
            }
            pos = j;
            from = m_end.max(from);
        }
        out.push_str(&cur[pos..]);
        cur = out;
        let names: i64 = if p.marked { 3 } else { 2 };
        let uses = count(&cur, &format!(".{nm}->e[0])["));
        let left = count_word(&cur, nm) as i64 - names * uses as i64 - 1;
        if left != 0 {
            return Err(failed(
                "pool_arrays",
                format!("{nm}: {left} uses of the pool array the rewrite does not cover"),
            ));
        }
    }
    // a buffer texture pool's element: `.read(c)` / `.get_width()` through its row bias (dm_buffer_read / dm_buffer_width), from the last access back, so that a nested
    // access is already rewritten inside the argument of the one around it
    if pools.iter().any(|p| p.elem.contains("texture_buffer<")) {
        cur = buffer_pool_operations(cur, &pools)?;
    }
    // the mark stores to memory, so Metal would test this fragment shader against depth after running it: early, unless the shader itself discards / writes depth / stores
    if marked {
        let own = cur.replace(t::POOL_MARK, "");
        if !own_side_effects(&own) {
            // re.sub(r'^(?<!\]\] )fragment\s', '[[early_fragment_tests]] fragment ', src, flags=re.M)
            let b = cur.as_bytes();
            let mut o = String::with_capacity(cur.len() + 64);
            let mut at = 0;
            let mut from = 0;
            while let Some(p) = find_b(b, b"fragment", from) {
                from = p + 1;
                if (p == 0 || b[p - 1] == b'\n') && p + 8 < b.len() && is_space(b[p + 8]) {
                    o.push_str(&cur[at..p]);
                    o.push_str("[[early_fragment_tests]] fragment ");
                    at = p + 9;
                    from = at;
                }
            }
            o.push_str(&cur[at..]);
            cur = o;
        }
    }
    Ok(Cow::Owned(cur))
}

// ------------------------------------------------------------------------------------------------------------------------------ pools handed to functions

/// Python `str.strip()`: `str.isspace` characters at both ends
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// `re.sub(r'\s', '', s)`
fn no_space(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !('\x1c'..='\x1f').contains(c))
        .collect()
}

/// `\bNAME\b` finditer (NAME a word): the (start, end) of each match
fn word_matches(h: &str, name: &str) -> Vec<(usize, usize)> {
    let b = h.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = find(h, name, from) {
        let e = p + name.len();
        if !in_word(b, p) && !(e < b.len() && is_word(b[e])) {
            out.push((p, e));
            from = e;
        } else {
            from = p + 1;
        }
    }
    out
}

/// `[\w.]+\.NAME` fullmatch
fn dotted(text: &str, name: &str) -> bool {
    text.len() > name.len() + 1
        && text.ends_with(name)
        && text.as_bytes()[text.len() - name.len() - 1] == b'.'
        && text.as_bytes()[..text.len() - name.len() - 1]
            .iter()
            .all(|&c| is_word(c) || c == b'.')
}

/// `re.search(r'array<[^;\n]+>\s*&\s*' + re.escape(name) + r'\b', src)`
fn callable_parameter(src: &str, name: &str) -> bool {
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(p) = find(src, name, from) {
        from = p + 1;
        let e = p + name.len();
        if e < b.len() && is_word(b[e]) {
            continue;
        }
        let mut k = p;
        while k > 0 && is_space(b[k - 1]) {
            k -= 1;
        }
        if k == 0 || b[k - 1] != b'&' {
            continue;
        }
        k -= 1;
        while k > 0 && is_space(b[k - 1]) {
            k -= 1;
        }
        if k == 0 || b[k - 1] != b'>' {
            continue;
        }
        let g = k - 1;
        // `array<` then at least one character, none of them `;` or a newline, up to this `>`
        let sep = b[..g]
            .iter()
            .rposition(|&c| c == b';' || c == b'\n')
            .map_or(0, |x| x + 1);
        if g >= sep + 7 && find_b(&b[..g - 1], b"array<", sep).is_some() {
            return true;
        }
    }
    false
}

/// `//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*"` (re.S) blanked: every character of a comment or a string a space (its newlines kept), byte for byte, so positions stay those of the text
pub fn mask_comments(src: &str) -> String {
    static COMMENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?s)//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*""#).expect("static regex")
    });
    let mut out = src.as_bytes().to_vec();
    for m in COMMENT.find_iter(src) {
        for c in &mut out[m.start()..m.end()] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    }
    // only whole characters were blanked (a match starts and ends on character boundaries), so the bytes are UTF-8
    String::from_utf8(out).unwrap_or_default()
}

struct Func {
    args: Vec<(usize, usize)>,
    body: usize,
    body_end: usize,
    /// argument index -> pool name
    pools: Vec<(usize, String)>,
}

struct Call {
    name: String,
    args: Vec<(usize, usize)>,
}

fn pool_err(msg: impl Into<String>) -> PostError {
    failed("pool_arrays", msg)
}

/// `closing(start, left, right)`: the index of the bracket closing the one at `start`
fn closing(code: &[u8], start: usize, left: u8, right: u8) -> Result<usize, PostError> {
    let mut depth = 1i64;
    for (i, &c) in code.iter().enumerate().skip(start + 1) {
        depth += (c == left) as i64 - (c == right) as i64;
        if depth == 0 {
            return Ok(i);
        }
    }
    Err(pool_err("pool callable: unbalanced delimiters"))
}

/// `arguments(start, end, types)`: the top-level comma-separated pieces of `code[start..end]` (`<>` nest too in a definition's parameters)
fn arguments(
    code: &str,
    start: usize,
    end: usize,
    types: bool,
) -> Result<Vec<(usize, usize)>, PostError> {
    let b = code.as_bytes();
    let (mut result, mut first, mut stack) = (Vec::new(), start, Vec::new());
    for (i, &c) in b.iter().enumerate().take(end).skip(start) {
        let open = matches!(c, b'(' | b'[' | b'{') || (types && c == b'<');
        let pair = match c {
            b')' => Some(b'('),
            b']' => Some(b'['),
            b'}' => Some(b'{'),
            b'>' if types => Some(b'<'),
            _ => None,
        };
        if open {
            stack.push(c);
        } else if let Some(o) = pair {
            if stack.pop() != Some(o) {
                return Err(pool_err("pool callable: unbalanced argument"));
            }
        } else if c == b',' && stack.is_empty() {
            result.push((first, i));
            first = i + 1;
        }
    }
    if !stack.is_empty() {
        return Err(pool_err("pool callable: unbalanced argument"));
    }
    if !py_strip(&code[first..end]).is_empty() {
        result.push((first, end));
    }
    Ok(result)
}

/// `constant\s+array<(.+),\s*(\d+)>\s*&\s*NAME\s*$` fullmatch of a stripped parameter -> (element, length digits)
fn pool_parameter<'a>(text: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let b = text.as_bytes();
    if !text.starts_with("constant") {
        return None;
    }
    let a = skip_space(b, 8);
    if a == 8 || !starts(b, a, "array<") {
        return None;
    }
    let mut k = text.strip_suffix(name)?.len();
    while k > 0 && is_space(b[k - 1]) {
        k -= 1;
    }
    if k == 0 || b[k - 1] != b'&' {
        return None;
    }
    k -= 1;
    while k > 0 && is_space(b[k - 1]) {
        k -= 1;
    }
    if k == 0 || b[k - 1] != b'>' {
        return None;
    }
    let d1 = k - 1;
    let mut d0 = d1;
    while d0 > 0 && b[d0 - 1].is_ascii_digit() {
        d0 -= 1;
    }
    if d0 == d1 {
        return None;
    }
    let mut c = d0;
    while c > 0 && is_space(b[c - 1]) {
        c -= 1;
    }
    if c == 0 || b[c - 1] != b',' {
        return None;
    }
    let e0 = a + 6;
    if c - 1 <= e0 {
        return None;
    }
    let elem = &text[e0..c - 1];
    if elem.contains('\n') {
        return None;
    }
    Some((elem, &text[d0..d1]))
}

/// dmsh_reflect._callable_pool_arrays: a pool passed to a function as a `constant array<element, length>& name` parameter becomes a `constant spvPool_<name>* name` parameter; a
/// call may only forward the whole pool unchanged to such a parameter of the one function of that name, and every other use is an element read, rewritten as the member's are.
/// Any other alias, cast, address or address space is a ValueError.
fn callable_pool_arrays(src: &str, pools: &[Pool]) -> Result<String, PostError> {
    const KEYWORDS: [&str; 6] = ["if", "for", "while", "switch", "sizeof", "alignof"];
    let code = mask_comments(src);
    let cb = code.as_bytes();
    let mut calls: Vec<Call> = Vec::new();
    // definitions by name, in the order of their first definition
    let mut defs: Vec<(String, Vec<Func>)> = Vec::new();
    // `\b(\w+)\s*\(` finditer: every `(` after a word and optional whitespace, in order
    let mut from = 0;
    while let Some(r) = memchr::memchr(b'(', &cb[from..]) {
        let open = from + r;
        from = open + 1;
        let mut w1 = open;
        while w1 > 0 && is_space(cb[w1 - 1]) {
            w1 -= 1;
        }
        let mut w0 = w1;
        while w0 > 0 && is_word(cb[w0 - 1]) {
            w0 -= 1;
        }
        if w0 == w1 {
            continue;
        }
        let name = &code[w0..w1];
        if KEYWORDS.contains(&name) {
            continue;
        }
        let end = closing(cb, open, b'(', b')')?;
        let next = skip_space(cb, end + 1);
        let definition = next < cb.len() && cb[next] == b'{';
        let args = arguments(&code, open + 1, end, definition)?;
        if definition {
            let body_end = closing(cb, next, b'{', b'}')?;
            let f = Func {
                args,
                body: next,
                body_end,
                pools: Vec::new(),
            };
            match defs.iter_mut().find(|(n, _)| n == name) {
                Some((_, v)) => v.push(f),
                None => defs.push((name.to_string(), vec![f])),
            }
        } else {
            calls.push(Call {
                name: name.to_string(),
                args,
            });
        }
    }
    let mut allowed: Vec<(usize, usize)> = Vec::new();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let Some((m0, _, me)) = descriptor_struct(&code) else {
        return Err(pool_err("pool callable: missing descriptor definition"));
    };
    let member_end = me + 2;
    for p in pools {
        let name = p.name.as_str();
        for (_, fs) in defs.iter_mut() {
            for f in fs.iter_mut() {
                for (index, &(start, end)) in f.args.iter().enumerate() {
                    let text = py_strip(&code[start..end]);
                    if word_matches(text, name).is_empty() {
                        continue;
                    }
                    let ok = pool_parameter(text, name).is_some_and(|(el, n)| {
                        no_space(el) == no_space(&p.elem) && n.parse::<u64>().ok() == Some(p.len)
                    });
                    if !ok {
                        return Err(pool_err(format!(
                            "{name}: unsupported callable pool parameter"
                        )));
                    }
                    if start < member_end {
                        return Err(pool_err(format!(
                            "{name}: pool callable precedes its descriptor definition"
                        )));
                    }
                    match f.pools.iter_mut().find(|(i, _)| *i == index) {
                        Some(x) => x.1 = name.to_string(),
                        None => f.pools.push((index, name.to_string())),
                    }
                    allowed.push((start, end));
                    edits.push((start, end, format!("constant spvPool_{name}* {name}")));
                }
            }
        }
        for (a, e) in word_matches(&code[m0..member_end], name) {
            allowed.push((m0 + a, m0 + e));
        }
    }
    let source_pool =
        |defs: &Vec<(String, Vec<Func>)>, start: usize, end: usize, name: &str| -> bool {
            let text = py_strip(&code[start..end]);
            if text != name {
                return dotted(text, name);
            }
            defs.iter().flat_map(|(_, fs)| fs.iter()).any(|f| {
                f.body < start && start < f.body_end && f.pools.iter().any(|(_, n)| n == name)
            })
        };
    for call in &calls {
        for (index, &(start, end)) in call.args.iter().enumerate() {
            for p in pools {
                let name = p.name.as_str();
                let text = py_strip(&code[start..end]);
                if !(text == name || dotted(text, name)) {
                    continue;
                }
                let targets: &[Func] = defs
                    .iter()
                    .find(|(n, _)| *n == call.name)
                    .map(|(_, v)| v.as_slice())
                    .unwrap_or(&[]);
                let forwarded = targets.len() == 1
                    && targets[0]
                        .pools
                        .iter()
                        .find(|(i, _)| *i == index)
                        .map(|(_, n)| n.as_str())
                        == Some(name)
                    && call.args.len() == targets[0].args.len()
                    && source_pool(&defs, start, end, name);
                if !forwarded {
                    return Err(pool_err(format!(
                        "{name}: unsupported callable pool forwarding"
                    )));
                }
                allowed.push((start, end));
            }
        }
    }
    for p in pools {
        let name = p.name.as_str();
        for (ms, me) in word_matches(&code, name) {
            if allowed.iter().any(|&(a, b)| a <= ms && ms < b) {
                continue;
            }
            // `re.search(r'([\w.]+)\.$', code[:ms])`: a run of word characters and dots that ends with a dot, at the end of the text or before a last newline
            let mut t = ms;
            if t > 0 && cb[t - 1] == b'\n' {
                t -= 1;
            }
            let mut start = ms;
            if t >= 2 && cb[t - 1] == b'.' {
                let mut r = t - 1;
                while r > 0 && (is_word(cb[r - 1]) || cb[r - 1] == b'.') {
                    r -= 1;
                }
                if r < t - 1 {
                    start = r;
                }
            }
            let end = me;
            let q = skip_space(cb, end);
            let bracket = q < cb.len() && cb[q] == b'[';
            if !bracket || !source_pool(&defs, start, end, name) {
                return Err(pool_err(format!(
                    "{name}: unsupported callable pool use or alias"
                )));
            }
            if code[..start]
                .trim_end_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
                .ends_with('&')
            {
                return Err(pool_err(format!(
                    "{name}: taking a pool element address is unsupported"
                )));
            }
            let left = q;
            let right = closing(cb, left, b'[', b']')?;
            let base = &src[start..end];
            let index = format!("min(uint({}), {base}->n)", &src[left + 1..right]);
            let used = if p.marked {
                format!("dm_pool_mark({base}, {index})")
            } else {
                index
            };
            edits.push((start, right + 1, format!("(&{base}->e[0])[{used}]")));
            allowed.push((start, right + 1));
        }
    }
    edits.sort();
    if edits.windows(2).any(|w| w[0].1 > w[1].0) {
        return Err(pool_err("pool callable: overlapping uses are unsupported"));
    }
    let mut out = src.to_string();
    for (start, end, rep) in edits.iter().rev() {
        out.replace_range(*start..*end, rep);
    }
    Ok(out)
}

/// the buffer texture pools' accesses `(&base->e[0])[index].read(c)` / `.get_width()` -> `dm_buffer_read(base, index, c)` / `dm_buffer_width(base, index)`
/// (`_buffer_pool_operation`), from the last access back
fn buffer_pool_operations(mut src: String, pools: &[Pool]) -> Result<String, PostError> {
    // `\(&([\w.]+)->e\[0\]\)\[` finditer over the text as it is before the first rewrite
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    {
        let b = src.as_bytes();
        let mut from = 0;
        while let Some(p) = find_b(b, b"(&", from) {
            from = p + 1;
            let mut e = p + 2;
            while e < b.len() && (is_word(b[e]) || b[e] == b'.') {
                e += 1;
            }
            if e == p + 2 || !starts(b, e, "->e[0])[") {
                continue;
            }
            found.push((p, e + 8, src[p + 2..e].to_string()));
            from = e + 8;
        }
    }
    for (start, first, base) in found.into_iter().rev() {
        let name = base.rsplit('.').next().unwrap_or(&base);
        if !pools
            .iter()
            .any(|p| p.name == name && p.elem.contains("texture_buffer<"))
        {
            continue;
        }
        let b = src.as_bytes();
        let (mut end, mut depth) = (first, 1i64);
        while depth != 0 {
            if end >= b.len() {
                return Err(pool_err(format!("{name}: unbalanced BUFFER index")));
            }
            depth += match b[end] {
                b'[' => 1,
                b']' => -1,
                _ => 0,
            };
            end += 1;
        }
        let index = src[first..end - 1].to_string();
        // `\.(read|get_width)\(` at `end`, then the call's argument up to its balanced `)`
        let read = starts(b, end, ".read(");
        if !read && !starts(b, end, ".get_width(") {
            return Err(pool_err(format!(
                "{base}: unsupported BUFFER table operation"
            )));
        }
        let a0 = end + if read { 6 } else { 11 };
        let (mut e, mut depth) = (a0, 1i64);
        while depth != 0 {
            if e >= b.len() {
                return Err(pool_err(format!("{base}: unbalanced BUFFER operation")));
            }
            depth += match b[e] {
                b'(' => 1,
                b')' => -1,
                _ => 0,
            };
            e += 1;
        }
        let rep = if read {
            format!("dm_buffer_read({base}, {index}, {})", &src[a0..e - 1])
        } else {
            format!("dm_buffer_width({base}, {index})")
        };
        src.replace_range(start..e, &rep);
    }
    Ok(src)
}
