//! dmsh_side: fragment entry points (`entry`), main0's body as a shared function (`shared_body`), `calling`.
use crate::templates as t;
use crate::text::*;

#[derive(Debug, Clone)]
pub struct Entry {
    pub attr: String,
    pub ret: String,
    pub name: String,
    pub params: Vec<String>,
    pub decl: String,
    pub args: String,
    pub start: usize,
    pub head: usize,
    pub end: usize,
}

/// `^((?:\[\[[^\]]*\]\]\s*)?)fragment\s+(\w+)\s+(\w+)\s*\(` matches: (match start, attr end (= `fragment` start), ret, name, match end)
type Span = (usize, usize);

fn entry_matches(src: &str) -> Vec<(usize, usize, Span, Span, usize)> {
    let b = src.as_bytes();
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(p) = find_b(b, b"fragment", from) {
        from = p + 1;
        // the match starts at a line start; the attribute (if any) sits between it and `fragment`
        let mut ls = None;
        {
            let mut k = p;
            while k > 0 && is_space(b[k - 1]) {
                k -= 1;
            }
            if k >= 2 && &b[k - 2..k] == b"]]" {
                // `\[\[[^\]]*\]\]`: the opening `[[` is the nearest one before with no `]` between
                let mut j = k - 2;
                while j > 0 && b[j - 1] != b']' && !(j >= 2 && b[j - 1] == b'[' && b[j - 2] == b'[')
                {
                    j -= 1;
                }
                if j >= 2 && b[j - 1] == b'[' && b[j - 2] == b'[' {
                    let st = j - 2;
                    if st == 0 || b[st - 1] == b'\n' {
                        ls = Some(st);
                    }
                }
            }
        }
        if ls.is_none() && (p == 0 || b[p - 1] == b'\n') {
            ls = Some(p);
        }
        let Some(start) = ls else { continue };
        let mut i = p + 8;
        let s = skip_space(b, i);
        if s == i {
            continue;
        }
        i = s;
        let r0 = i;
        let r1 = word_end(b, r0);
        if r1 == r0 {
            continue;
        }
        let s = skip_space(b, r1);
        if s == r1 {
            continue;
        }
        let n0 = s;
        let n1 = word_end(b, n0);
        if n1 == n0 {
            continue;
        }
        let q = skip_space(b, n1);
        if q < b.len() && b[q] == b'(' {
            v.push((start, p, (r0, r1), (n0, n1), q + 1));
            from = q + 1;
        }
    }
    v
}

fn params_split(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let (mut out, mut at, mut depth) = (Vec::new(), 0usize, 0i32);
    for (i, &ch) in b.iter().enumerate() {
        if matches!(ch, b'<' | b'(' | b'[') {
            depth += 1;
        }
        if matches!(ch, b'>' | b')' | b']') {
            depth -= 1;
        }
        if ch == b',' && depth == 0 {
            out.push(strip(&s[at..i]).to_string());
            at = i + 1;
        }
    }
    let tail = strip(&s[at..]);
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// `re.sub(r'\s*\[\[.*\]\]$', '', p)`
fn strip_attr(p: &str) -> String {
    let b = p.as_bytes();
    let mut from = 0;
    while let Some(q) = find_b(b, b"[[", from) {
        let rest = &b[q + 2..];
        if rest.ends_with(b"]]") && rest.len() >= 2 && !rest.contains(&b'\n') {
            let mut k = q;
            while k > 0 && is_space(b[k - 1]) {
                k -= 1;
            }
            return p[..k].to_string();
        }
        from = q + 1;
    }
    p.to_string()
}

fn build(
    src: &str,
    m: &(usize, usize, (usize, usize), (usize, usize), usize),
) -> Result<Entry, String> {
    let b = src.as_bytes();
    let (start, fpos, r, n, mend) = *m;
    let name = &src[n.0..n.1];
    let head = close(b, mend, b'(', b')');
    let body = head.and_then(|h| find(src, "{", h));
    let end = body.and_then(|bo| close(b, bo + 1, b'{', b'}'));
    let (Some(head), Some(end)) = (head, end) else {
        return Err(format!("no body for {name}"));
    };
    let params = params_split(&src[mend..head - 1]);
    let (mut decl, mut args) = (Vec::new(), Vec::new());
    for p in &params {
        let d = strip_attr(p);
        let pb = d.as_bytes();
        let mut k = pb.len();
        while k > 0 && is_word(pb[k - 1]) {
            k -= 1;
        }
        if k == pb.len() || p.contains("function_constant") {
            // `p[:80]`: 80 characters (a byte cut could split a UTF-8 character)
            return Err(format!(
                "a parameter of {name}: {}",
                p.chars().take(80).collect::<String>()
            ));
        }
        args.push(d[k..].to_string());
        decl.push(d);
    }
    Ok(Entry {
        attr: src[start..fpos].to_string(),
        ret: src[r.0..r.1].to_string(),
        name: name.to_string(),
        params,
        decl: decl.join(", "),
        args: args.join(", "),
        start,
        head,
        end,
    })
}

/// dmsh_side.entry: the entry point `name`, None when there is none; Err on a shape it does not know
pub fn entry(src: &str, name: &str) -> Result<Option<Entry>, String> {
    for m in entry_matches(src) {
        if &src[m.3.0..m.3.1] == name {
            return build(src, &m).map(Some);
        }
    }
    Ok(None)
}

/// `_BODY.search(msl)`: (start, group(1) present, end of the match)
fn body_match(src: &str) -> Option<(usize, bool, usize)> {
    let tail = "main0_out dm_body_main0(";
    let b = src.as_bytes();
    let mut from = 0;
    while let Some(p) = find_b(b, tail.as_bytes(), from) {
        from = p + 1;
        let pre = "static ";
        let attr = "__attribute__((always_inline)) ";
        if p >= pre.len() + attr.len()
            && &src[p - attr.len()..p] == attr
            && &src[p - attr.len() - pre.len()..p - attr.len()] == pre
        {
            return Some((p - attr.len() - pre.len(), true, p + tail.len()));
        }
        if p >= pre.len() && &src[p - pre.len()..p] == pre {
            return Some((p - pre.len(), false, p + tail.len()));
        }
    }
    None
}

/// dmsh_side.body: (its start, the end of its parameter list, the end of its body), None when there is none
pub fn body(src: &str) -> Option<(usize, usize, usize)> {
    let (st, _, end_m) = body_match(src)?;
    let b = src.as_bytes();
    let head = close(b, end_m, b'(', b')')?;
    let o = find(src, "{", head)?;
    let end = close(b, o + 1, b'{', b'}')?;
    Some((st, head, end))
}

pub fn calling(e: &Entry, name: &str, ret: &str, tail: &str) -> String {
    format!(
        "{}fragment {} {}({})\n{{\n    main0_out out = dm_body_main0({});\n    {}\n}}\n",
        e.attr,
        ret,
        name,
        e.params.join(", "),
        e.args,
        tail
    )
}

/// dmsh_side.shared_body
pub fn shared_body(src: &str) -> Option<String> {
    if let Some((st, has_attr, end_m)) = body_match(src) {
        return Some(if has_attr {
            src.to_string()
        } else {
            format!("{}{}{}", &src[..st], t::BODY, &src[end_m..])
        });
    }
    let e = entry(src, "main0").ok()??;
    if e.ret != "main0_out" {
        return None;
    }
    Some(
        format!("{}{}{})", &src[..e.start], t::BODY, e.decl)
            + &src[e.head..e.end]
            + "\n"
            + &calling(&e, "main0", "main0_out", "return out;")
            + &src[e.end..],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameter_in_the_message_is_cut_at_80_characters_not_bytes() {
        // byte 80 falls inside an `é`: the byte cut panicked; Python's p[:80] counts characters
        let p = format!("float4 {}x [[function_constant(0)]]", "é".repeat(50));
        let src = format!("fragment float4 main0({p})\n{{\n}}\n");
        let e = entry(&src, "main0").unwrap_err();
        assert_eq!(
            e,
            format!(
                "a parameter of main0: float4 {}x [[function_constant(0",
                "é".repeat(50)
            )
        );
    }
}
