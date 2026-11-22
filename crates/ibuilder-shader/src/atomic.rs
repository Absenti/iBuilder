//! dmsh_atomic.wide_atomics: the translator's pairs of 32-bit atomics on an `rg32ui` image (a 64-bit atomic max / min, PRAGMATA's visibility buffer) as Metal's one 64-bit
//! texture atomic, the image declared as `ulong`.
use crate::text::*;
use crate::{PostError, failed};
use std::borrow::Cow;

/// `_split_args`: the top-level comma-separated arguments of a call's argument text
fn split_args(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let (mut out, mut depth, mut cur) = (Vec::new(), 0i32, 0usize);
    for (i, &ch) in b.iter().enumerate() {
        match ch {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            _ => {}
        }
        if ch == b',' && depth == 0 {
            out.push(strip(&text[cur..i]));
            cur = i + 1;
        }
    }
    out.push(strip(&text[cur..]));
    out
}

/// `\b(texture(?:2d|2d_array|3d))<uint, access::read_write>(\s+)(\w+_rg32ui)\b` at `p` (the start of `texture`) -> (kind end, the whitespace, the name)
fn decl_at(src: &str, p: usize) -> Option<(&str, &str, &str, usize)> {
    let b = src.as_bytes();
    if in_word(b, p) {
        return None;
    }
    let mut c = Cur::at(src, p);
    if !c.lit("texture") {
        return None;
    }
    let k0 = c.i;
    // the alternation order is 2d, 2d_array, 3d: `2d` is taken first and `<` must follow, else `2d_array`
    let kinds = ["2d", "2d_array", "3d"];
    for k in kinds {
        let mut x = Cur::at(src, k0);
        if x.lit(k) && x.lit("<uint, access::read_write>") {
            let ws0 = x.i;
            if !x.ws1() {
                continue;
            }
            let ws = &src[ws0..x.i];
            let n0 = x.i;
            let e = word_end(b, n0);
            // `\w+_rg32ui\b`: the word run must end with `_rg32ui`, with at least one character before it
            if e - n0 > 7 && src[n0..e].ends_with("_rg32ui") {
                return Some((&src[p..k0 + k.len()], ws, &src[n0..e], e));
            }
        }
    }
    None
}

/// `\bNAME\.atomic_`
fn has_atomic_use(src: &str, name: &str) -> bool {
    let pat = format!("{name}.atomic_");
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(p) = find(src, &pat, from) {
        if !in_word(b, p) {
            return true;
        }
        from = p + 1;
    }
    false
}

/// `^([ \t]*)(?:uint _\d+ = )?([\w.]*?\bNAME)\.atomic_fetch_(max|min)\((.*)\)(?:\.x)?;[ \t]*$` on a line -> (indent, image expression, max|min, arguments)
fn stmt<'a>(l: &'a str, name: &str) -> Option<(&'a str, &'a str, &'a str, &'a str)> {
    let mut c = Cur::at(l, 0);
    let indent = c.ht0();
    let mut x = c;
    if x.lit("uint ") && x.tmp_nb().is_some() && x.lit(" = ") {
        c = x;
    }
    // the run of `[\w.]` up to the `(` of the call: `<prefix>NAME.atomic_fetch_(max|min)`
    let b = l.as_bytes();
    let r0 = c.i;
    let mut e = r0;
    while e < b.len() && (is_word(b[e]) || b[e] == b'.') {
        e += 1;
    }
    let run = &l[r0..e];
    let (img, op) = if let Some(i) = run
        .strip_suffix("max")
        .and_then(|r| r.strip_suffix(".atomic_fetch_"))
    {
        (i, "max")
    } else {
        let i = run
            .strip_suffix("min")
            .and_then(|r| r.strip_suffix(".atomic_fetch_"))?;
        (i, "min")
    };
    // the expression ends with NAME, which starts the expression or follows a dot
    if !(img == name || img.ends_with(&format!(".{name}"))) {
        return None;
    }
    if e >= b.len() || b[e] != b'(' {
        return None;
    }
    let a0 = e + 1;
    // `(.*)\)(?:\.x)?;[ \t]*$`
    let t = l.trim_end_matches([' ', '\t']);
    let t = t.strip_suffix(';')?;
    let a1 = if let Some(r) = t.strip_suffix(')') {
        r.len()
    } else {
        let r = t.strip_suffix(").x")?;
        r.len()
    };
    if a1 < a0 {
        return None;
    }
    Some((indent, img, op, &l[a0..a1]))
}

pub fn wide_atomics(source: &str) -> Result<Cow<'_, str>, PostError> {
    if !contains(source, "_rg32ui") {
        return Ok(Cow::Borrowed(source));
    }
    // the declarations of the original text: every `texture<kind><uint, access::read_write> NAME_rg32ui`
    let mut names: Vec<String> = Vec::new();
    {
        let b = source.as_bytes();
        let mut from = 0;
        while let Some(p) = find_b(b, b"texture", from) {
            from = p + 1;
            if let Some((_, _, name, e)) = decl_at(source, p) {
                names.push(name.to_string());
                from = e;
            }
        }
    }
    let mut cur: Cow<str> = Cow::Borrowed(source);
    for name in names {
        if !has_atomic_use(&cur, &name) {
            continue;
        }
        let lines: Vec<&str> = cur.split('\n').collect();
        let mut out: Vec<String> = Vec::with_capacity(lines.len());
        let mut i = 0;
        while i < lines.len() {
            let Some(a) = stmt(lines[i], &name) else {
                out.push(lines[i].to_string());
                i += 1;
                continue;
            };
            let b = if i + 1 < lines.len() {
                stmt(lines[i + 1], &name)
            } else {
                None
            };
            let Some(b) = b.filter(|b| b.1 == a.1 && b.2 == a.2) else {
                return Err(failed(
                    "check_msl",
                    format!(
                        "{name}: a 32-bit atomic on a 64-bit image that is not the translator's low / high pair"
                    ),
                ));
            };
            let (x, y) = (split_args(a.3), split_args(b.3));
            if x.len() < 2 || x[..x.len() - 1] != y[..y.len() - 1] {
                return Err(failed(
                    "check_msl",
                    format!("{name}: the two halves of a 64-bit atomic name different texels"),
                ));
            }
            out.push(format!(
                "{}{}.atomic_{}({}, ulong4(as_type<ulong>(uint2({}, {}))));",
                a.0,
                a.1,
                a.2,
                x[..x.len() - 1].join(", "),
                x[x.len() - 1],
                y[y.len() - 1]
            ));
            i += 2;
        }
        let source2 = out.join("\n");
        // left = re.sub(r'\bNAME(?:\.get_(?:width|height|depth|array_size)\(\)|\.atomic_(?:max|min)\()', '', source)
        let mut left = String::with_capacity(source2.len());
        let sb = source2.as_bytes();
        let (mut at, mut from) = (0, 0);
        while let Some(p) = find(&source2, &name, from) {
            from = p + 1;
            if in_word(sb, p) {
                continue;
            }
            let e = p + name.len();
            let rest = &source2[e..];
            let skip = ["get_width", "get_height", "get_depth", "get_array_size"]
                .iter()
                .find_map(|q| {
                    let t = format!(".{q}()");
                    if rest.starts_with(&t) {
                        Some(t.len())
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    ["max", "min"].iter().find_map(|q| {
                        let t = format!(".atomic_{q}(");
                        if rest.starts_with(&t) {
                            Some(t.len())
                        } else {
                            None
                        }
                    })
                });
            if let Some(k) = skip {
                left.push_str(&source2[at..p]);
                at = e + k;
                from = at;
            }
        }
        left.push_str(&source2[at..]);
        let uses = count_word(&left, &name);
        if uses != 1 {
            return Err(failed(
                "check_msl",
                format!(
                    "{name}: {} accesses of a 64-bit atomic image besides its atomics and size queries",
                    uses as i64 - 1
                ),
            ));
        }
        // re.sub(r'\b(texture(?:2d|2d_array|3d))<uint, access::read_write>(\s+NAME)\b', r'\1<ulong, access::read_write>\2', source)
        let mut res = String::with_capacity(source2.len() + 16);
        let sb = source2.as_bytes();
        let (mut at, mut from) = (0, 0);
        while let Some(p) = find_b(sb, b"texture", from) {
            from = p + 1;
            if let Some((kind, ws, n, e)) = decl_at(&source2, p)
                && n == name
            {
                res.push_str(&source2[at..p]);
                res.push_str(&format!("{kind}<ulong, access::read_write>{ws}{n}"));
                at = e;
                from = e;
            }
        }
        res.push_str(&source2[at..]);
        cur = Cow::Owned(res);
    }
    Ok(cur)
}
