//! dmsh_wide.wide_loads: the words of one read-only storage buffer the translator split (`LDG.E.64 / .128` as 32-bit loads, each with its own address arithmetic) read whole again
//! as one `packed_uint2/3/4`, within one block of straight-line code. Every regex of the Python is a line pattern; they are matched with cursors on one line at a time.
use crate::text::*;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// An index guard: (the bound's three text pieces).
type Bound<'a> = Option<(&'a str, &'a str, &'a str)>;

/// `_[0-9A-Za-z_]+` (an underscore and at least one more word character)
fn ident<'a>(c: &mut Cur<'a>) -> Option<&'a str> {
    let a = c.i;
    if c.peek() != Some(b'_') {
        return None;
    }
    let e = word_end(c.b(), a + 1);
    if e == a + 1 {
        return None;
    }
    c.i = e;
    Some(&c.s[a..e])
}

/// `[a-z]p_s\d+`
fn buf<'a>(c: &mut Cur<'a>) -> Option<&'a str> {
    let a = c.i;
    let b = c.b();
    if !(a + 3 < b.len()
        && b[a].is_ascii_lowercase()
        && b[a + 1] == b'p'
        && b[a + 2] == b'_'
        && b[a + 3] == b's')
    {
        return None;
    }
    let e = digits_end(b, a + 4);
    if e == a + 4 {
        return None;
    }
    c.i = e;
    Some(&c.s[a..e])
}

fn at_eol(c: &mut Cur) -> bool {
    c.ws0();
    c.at_end()
}

/// `^\s*int (ID) = (ID) \+ (-?\d+);\s*$` -> (name, base, constant)
fn add_line(l: &str) -> Option<(&str, &str, i128)> {
    let mut c = Cur::at(l, 0);
    c.ws0();
    if !c.lit("int ") {
        return None;
    }
    let a = ident(&mut c)?;
    if !c.lit(" = ") {
        return None;
    }
    let b = ident(&mut c)?;
    if !c.lit(" + ") {
        return None;
    }
    let k = c.int()?;
    if !c.lit(";") || !at_eol(&mut c) {
        return None;
    }
    Some((a, b, k.parse().ok()?))
}

/// `^\s*uint (ID) = uint\(int\(uint\((ID)\) >> uint\(2\)\)\);\s*$` -> (name, value)
fn idx_line(l: &str) -> Option<(&str, &str)> {
    let mut c = Cur::at(l, 0);
    c.ws0();
    if !c.lit("uint ") {
        return None;
    }
    let a = ident(&mut c)?;
    if !c.lit(" = uint(int(uint(") {
        return None;
    }
    let b = ident(&mut c)?;
    if !c.lit(") >> uint(2)));") || !at_eol(&mut c) {
        return None;
    }
    Some((a, b))
}

/// `^\s*int (ID) = int\((ID)\);\s*$`
fn signed_line(l: &str) -> Option<(&str, &str)> {
    let mut c = Cur::at(l, 0);
    c.ws0();
    if !c.lit("int ") {
        return None;
    }
    let a = ident(&mut c)?;
    if !c.lit(" = int(") {
        return None;
    }
    let b = ident(&mut c)?;
    if !c.lit(");") || !at_eol(&mut c) {
        return None;
    }
    Some((a, b))
}

/// TABLE `\(\*spvDescriptorSet0\.[a-z]+_c27\)\._m0\[\d+\]\.[xyzw]` -> the text
fn table<'a>(c: &mut Cur<'a>) -> Option<&'a str> {
    let a = c.i;
    if !c.lit("(*spvDescriptorSet0.") {
        return None;
    }
    let l0 = c.i;
    while c.peek().map(|x| x.is_ascii_lowercase()).unwrap_or(false) {
        c.i += 1;
    }
    if c.i == l0 {
        return None;
    }
    if !c.lit(&format!("_c{})._m0[", crate::templates::SBLEN_BANK)) {
        return None;
    }
    c.digits()?;
    if !c.lit("].") {
        return None;
    }
    if !matches!(c.peek(), Some(b'x' | b'y' | b'z' | b'w')) {
        return None;
    }
    c.i += 1;
    Some(&c.s[a..c.i])
}

/// `^\s*bool (ID) = \(uint\((ID)\) \+ TABLE\) < TABLE;\s*$` -> (bool name, signed index, words before, words in all)
fn inside_line(l: &str) -> Option<(&str, &str, &str, &str)> {
    let mut c = Cur::at(l, 0);
    c.ws0();
    if !c.lit("bool ") {
        return None;
    }
    let a = ident(&mut c)?;
    if !c.lit(" = (uint(") {
        return None;
    }
    let b = ident(&mut c)?;
    if !c.lit(") + ") {
        return None;
    }
    let t1 = table(&mut c)?;
    if !c.lit(") < ") {
        return None;
    }
    let t2 = table(&mut c)?;
    if !c.lit(";") || !at_eol(&mut c) {
        return None;
    }
    Some((a, b, t1, t2))
}

/// every `\(\*spvDescriptorSet0\.(BUF)\)\._m0\[int\((ID)\)\]` of a line: (start, end, buffer, index)
fn loads(l: &str) -> Vec<(usize, usize, &str, &str)> {
    let b = l.as_bytes();
    let mut v = Vec::new();
    let mut from = 0;
    let pre = "(*spvDescriptorSet0.";
    while let Some(p) = find_b(b, pre.as_bytes(), from) {
        from = p + 1;
        let mut c = Cur::at(l, p + pre.len());
        let Some(bf) = buf(&mut c) else { continue };
        if !c.lit(")._m0[int(") {
            continue;
        }
        let Some(id) = ident(&mut c) else { continue };
        if !c.lit(")]") {
            continue;
        }
        v.push((p, c.i, bf, id));
        from = c.i;
    }
    v
}

/// every `select\(0u, \(\*spvDescriptorSet0\.(BUF)\)\._m0\[(ID) \? (ID) : 0\], \2\)` of a line: (start, end, buffer, condition, index)
fn bloads(l: &str) -> Vec<(usize, usize, &str, &str, &str)> {
    let b = l.as_bytes();
    let mut v = Vec::new();
    let mut from = 0;
    let pre = "select(0u, (*spvDescriptorSet0.";
    while let Some(p) = find_b(b, pre.as_bytes(), from) {
        from = p + 1;
        let mut c = Cur::at(l, p + pre.len());
        let Some(bf) = buf(&mut c) else { continue };
        if !c.lit(")._m0[") {
            continue;
        }
        let Some(cond) = ident(&mut c) else { continue };
        if !c.lit(" ? ") {
            continue;
        }
        let Some(ix) = ident(&mut c) else { continue };
        if !c.lit(" : 0], ") || !c.lit(cond) || !c.lit(")") {
            continue;
        }
        v.push((p, c.i, bf, cond, ix));
        from = c.i;
    }
    v
}

/// `(?:[-+*/%|&^]|<<|>>)?=(?!=)` at `c`
fn op_eq(c: &mut Cur) -> bool {
    let mut x = *c;
    if !(x.lit("<<") || x.lit(">>"))
        && matches!(
            x.peek(),
            Some(b'-' | b'+' | b'*' | b'/' | b'%' | b'|' | b'&' | b'^')
        )
    {
        x.i += 1;
    }
    if x.lit("=") && x.peek() != Some(b'=') {
        *c = x;
        return true;
    }
    // the optional operator absent
    let mut y = *c;
    if y.lit("=") && y.peek() != Some(b'=') {
        *c = y;
        return true;
    }
    false
}

/// `_WRITE`: `atomic_|_barrier\(|\.write\(|\]\s*OP|^\s*\*|^\s*ID\s*OP` found anywhere in the line: the lines a run does not reach across
fn write_line(l: &str) -> bool {
    if contains(l, "atomic_") || contains(l, "_barrier(") || contains(l, ".write(") {
        return true;
    }
    let b = l.as_bytes();
    let mut from = 0;
    while let Some(p) = memchr::memchr(b']', &b[from..]).map(|x| x + from) {
        from = p + 1;
        let mut c = Cur::at(l, p + 1);
        c.ws0();
        if op_eq(&mut c) {
            return true;
        }
    }
    let mut c = Cur::at(l, 0);
    c.ws0();
    if c.peek() == Some(b'*') {
        return true;
    }
    if ident(&mut c).is_some() {
        c.ws0();
        if op_eq(&mut c) {
            return true;
        }
    }
    false
}

/// `_RO` over the whole text (re.M): `^\s*const device ID\* (BUF) \[\[id\(\d+\)\]\];` -> the buffer names
pub fn ro_buffers(src: &str) -> HashSet<&str> {
    let b = src.as_bytes();
    let mut s = HashSet::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"const device _", from) {
        from = p + 1;
        // `^\s*`: only whitespace between a line start and the text (the whitespace run may itself span lines)
        let mut j = p;
        while j > 0 && is_space(b[j - 1]) {
            j -= 1;
        }
        if !(j == 0 || b[j - 1] == b'\n' || b[j..p].contains(&b'\n')) {
            continue;
        }
        let mut c = Cur::at(src, p + 13);
        if ident(&mut c).is_none() || !c.lit("* ") {
            continue;
        }
        let Some(bf) = buf(&mut c) else { continue };
        if !c.lit(" [[id(") || c.digits().is_none() || !c.lit(")]];") {
            continue;
        }
        s.insert(bf);
    }
    s
}

fn root<'a>(mut v: &'a str, adds: &HashMap<&'a str, (&'a str, i128)>) -> (&'a str, i128) {
    let mut off = 0;
    for _ in 0..64 {
        let Some(&(nv, k)) = adds.get(v) else { break };
        v = nv;
        off += k;
    }
    (v, off)
}

struct Word {
    line: usize,
    start: usize,
    end: usize,
    index: String,
    bound: Option<(String, String, String)>,
}

pub fn wide_loads(src: &str) -> Cow<'_, str> {
    if !contains(src, " >> uint(2)") {
        return Cow::Borrowed(src);
    }
    let ro = ro_buffers(src);
    if ro.is_empty() {
        return Cow::Borrowed(src);
    }
    let lines: Vec<&str> = src.split('\n').collect();
    let mut adds: HashMap<&str, (&str, i128)> = HashMap::new();
    let mut idx: HashMap<&str, &str> = HashMap::new();
    let mut signed: HashMap<&str, &str> = HashMap::new();
    let mut inside: HashMap<&str, (&str, &str, &str)> = HashMap::new();
    for ln in &lines {
        if let Some((a, b, k)) = add_line(ln) {
            adds.insert(a, (b, k));
            continue;
        }
        if let Some((a, b)) = idx_line(ln) {
            idx.insert(a, b);
            continue;
        }
        if let Some((a, b)) = signed_line(ln) {
            signed.insert(a, b);
            continue;
        }
        if let Some((a, i, before, total)) = inside_line(ln) {
            inside.insert(a, (i, before, total));
        }
    }
    // (block, buffer, value, bound) -> {byte offset: word}; insertion-ordered
    type Key<'a> = (usize, &'a str, &'a str, Option<(&'a str, &'a str)>);
    let mut order: Vec<Key> = Vec::new();
    let mut by_key: HashMap<Key, Vec<(i128, Word)>> = HashMap::new();
    let mut block = 0usize;
    for (i, ln) in lines.iter().enumerate() {
        if contains(ln, "{") || contains(ln, "}") || write_line(ln) {
            block += 1;
            continue;
        }
        let ms = loads(ln);
        let bs = bloads(ln);
        if ms.len() + bs.len() != 1 {
            continue;
        }
        let (start, end, bf, index, at, bound): (usize, usize, &str, &str, &str, Bound);
        if let Some(&(s, e, b, ix)) = ms.first() {
            if !ro.contains(b) {
                continue;
            }
            let Some(&a) = idx.get(ix) else { continue };
            (start, end, bf, index, at, bound) = (s, e, b, ix, a, None);
        } else {
            let (s, e, b, cond, ix) = bs[0];
            let bd = inside.get(cond).copied();
            let Some(bd) = bd else { continue };
            if !ro.contains(b) || bd.0 != ix {
                continue;
            }
            let Some(sg) = signed.get(ix) else { continue };
            let Some(&a) = idx.get(sg) else { continue };
            (start, end, bf, index, at, bound) = (s, e, b, ix, a, Some(bd));
        }
        let (v, off) = root(at, &adds);
        if off.rem_euclid(4) == 0 {
            let key: Key = (block, bf, v, bound.map(|(_, b, t)| (b, t)));
            if let std::collections::hash_map::Entry::Vacant(e) = by_key.entry(key) {
                order.push(key);
                e.insert(Vec::new());
            }
            let list = by_key.get_mut(&key).unwrap();
            if !list.iter().any(|(o, _)| *o == off) {
                list.push((
                    off,
                    Word {
                        line: i,
                        start,
                        end,
                        index: index.to_string(),
                        bound: bound.map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())),
                    },
                ));
            }
        }
    }
    let mut edits: HashMap<usize, (usize, usize, String, Option<String>)> = HashMap::new();
    let mut n = 0usize;
    for key in &order {
        let buf_name = key.1;
        let by_off = &by_key[key];
        let get = |o: i128| by_off.iter().find(|(x, _)| *x == o).map(|(_, w)| w);
        let mut bases: Vec<i128> = by_off.iter().map(|(o, _)| *o - o.rem_euclid(16)).collect();
        bases.sort_unstable();
        bases.dedup();
        for base in bases {
            let mut run: Vec<&Word> = Vec::new();
            while run.len() < 4 {
                match get(base + 4 * run.len() as i128) {
                    Some(w) => run.push(w),
                    None => break,
                }
            }
            if run.len() < 2 || run.iter().any(|w| w.line < run[0].line) {
                continue;
            }
            let name = format!("_dmw{n}");
            n += 1;
            let vec = match run.len() {
                2 => "packed_uint2",
                3 => "packed_uint3",
                _ => "packed_uint4",
            };
            let src0 = format!("(*spvDescriptorSet0.{buf_name})._m0");
            let decl = match &run[0].bound {
                None => format!(
                    "{vec} {name} = *(const device {vec}*)&{src0}[int({})];",
                    run[0].index_load()
                ),
                Some((first, before, total)) => {
                    let ins: Vec<String> = (0..run.len())
                        .map(|k| format!("(uint({first} + {k}) + {before}) < {total}"))
                        .collect();
                    let lanes: Vec<String> = (0..run.len())
                        .map(|k| {
                            format!(
                                "select(0u, {src0}[{} ? {first} + {k} : 0], {})",
                                ins[k], ins[k]
                            )
                        })
                        .collect();
                    format!(
                        "bool {name}_in = {} && {};\n{vec} {name} = {name}_in ? *(const device {vec}*)&{src0}[{first}] : {vec}({});",
                        ins[0],
                        ins[ins.len() - 1],
                        lanes.join(", ")
                    )
                }
            };
            for (w, word) in run.iter().enumerate() {
                edits.insert(
                    word.line,
                    (
                        word.start,
                        word.end,
                        format!("{name}.{}", ["x", "y", "z", "w"][w]),
                        if w == 0 { Some(decl.clone()) } else { None },
                    ),
                );
            }
        }
    }
    if edits.is_empty() {
        return Cow::Borrowed(src);
    }
    let mut o = String::with_capacity(src.len() + 512);
    for (i, ln) in lines.iter().enumerate() {
        if i > 0 {
            o.push('\n');
        }
        match edits.get(&i) {
            None => o.push_str(ln),
            Some((s, e, rep, decl)) => {
                let indent = &ln[..ln.len()
                    - ln.trim_start_matches(|c: char| c.is_ascii() && is_space(c as u8))
                        .len()];
                if let Some(d) = decl {
                    o.push_str(indent);
                    o.push_str(&d.replace('\n', &format!("\n{indent}")));
                    o.push('\n');
                }
                o.push_str(&ln[..*s]);
                o.push_str(rep);
                o.push_str(&ln[*e..]);
            }
        }
    }
    Cow::Owned(o)
}

impl Word {
    /// the `ID` of `_m0[int(ID)]` of an unbounded load (what `match.group(2)` is for it)
    fn index_load(&self) -> &str {
        &self.index
    }
}
