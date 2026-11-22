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

struct Pool<'a> {
    name: &'a str,
    elem: &'a str,
    marked: bool,
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
    // members = re.sub(r'^(\s*)array<(.+), (\d+)> (\w+) \[\[id\(', to_pointer, body, flags=re.M)
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
                    p.elem = elem;
                    p.marked = marked;
                } else {
                    pools.push(Pool { name, elem, marked });
                }
                members.push_str(&format!("{lead}constant spvPool_{name}* {name} [[id("));
                members.push_str(&l[end..]);
                continue;
            }
        }
        members.push_str(l);
    }
    if pools.is_empty() {
        return Ok(Cow::Borrowed(src));
    }
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
                "struct spvPool_{}\n{{\n    uint n;\n    ulong seen;\n    array<{}, 1> e;\n}};\n\n",
                p.name, p.elem
            )
        })
        .collect();
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
    for p in &pools {
        // base.name[index] -> (&base.name->e[0])[min(uint(index), base.name->n)], the index taken with its brackets balanced
        let nm = p.name;
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
