//! `dmsh_fetch.py`: `fold` (a pixel shader's readable-tile version `main0_f`) and `screen_varyings` (the vertex side of a projective sample's tile read).
//! (`select_guards`, the third function of that file, is a rewrite of the chain: `crate::guards`.)
use super::abi;
use super::pyjson::J;
use super::reflect::is_word;
use super::unit;
use regex::Regex;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

const NFB: usize = 8;
const SCALE: &str = "(*spvDescriptorSet0.support_buffer)._m5[0].x";

/// `_once(msl, name)`: `name = ` is assigned exactly once (its declaration), found as text with a look at the character before
fn once(msl: &str, name: &str) -> bool {
    let text = format!("{name} = ");
    let b = msl.as_bytes();
    let (mut n, mut from) = (0, 0);
    while let Some(p) = msl[from..].find(&text) {
        let i = from + p;
        if i == 0 || !(matches!(b[i - 1], b'.' | b'_') || b[i - 1].is_ascii_alphanumeric()) {
            n += 1;
        }
        from = i + text.len();
    }
    n == 1
}

/// the leftmost match of `float <var> = <tail>` (the Python's `re.search(rf'float {re.escape(var)} = {tail}', msl)`), without compiling a pattern per variable
fn decl<'t>(msl: &'t str, var: &str, tail: &str) -> Option<regex::Captures<'t>> {
    super::rx::lit_then(msl, &format!("float {var} = "), tail)
}

/// `_proj(msl, var, comp, locn)` -> the `m_X` name
fn proj(msl: &str, var: &str, comp: &str, locn: &HashMap<String, i64>) -> Option<String> {
    let src = |n: &str, c: &str| -> Option<String> {
        let m = decl(msl, n, &format!(r"in\.(m_\d+)\.{c};"))?;
        if once(msl, n) && locn.contains_key(&m[1]) {
            Some(m[1].to_string())
        } else {
            None
        }
    };
    if !once(msl, var) {
        return None;
    }
    if let Some(m) = decl(msl, var, r"(_\d+) \* (_\d+);") {
        for (x, r) in [
            (m[1].to_string(), m[2].to_string()),
            (m[2].to_string(), m[1].to_string()),
        ] {
            let d = decl(msl, &r, r"1\.0 / (_\d+);");
            if let Some(d) = d
                && once(msl, &r)
                && let (Some(a), Some(b)) = (src(&x, comp), src(&d[1], "w"))
                && a == b
            {
                return Some(a);
            }
        }
        return None;
    }
    let m = decl(msl, var, r"(_\d+) / (_\d+);")?;
    let (a, b) = (src(&m[1], comp), src(&m[2], "w"));
    if a == b { a } else { None }
}

re!(
    RE_UNI,
    r"^\(\*spvDescriptorSet0\.fp_c(\d+)\)\._m0\[(\d+)\]\.([xyzw])$"
);
re!(RE_REGNAME, r"^_\d+$");

/// `_frag_pos(msl, var, comp)` -> [uniform slot, vector, component]
fn frag_pos(msl: &str, var: &str, comp: &str) -> Option<[i64; 3]> {
    let one = |n: &str, pat: &str| -> Option<regex::Captures<'_>> {
        let m = decl(msl, n, &format!("{pat};"))?;
        if once(msl, n) { Some(m) } else { None }
    };
    let pixel = |n: &str| -> bool {
        if one(n, &format!(r"gl_FragCoord\.{comp}")).is_some() {
            return true;
        }
        let Some(q) = one(n, r"(_\d+) / (_\d+)") else {
            return false;
        };
        one(&q[1], &format!(r"gl_FragCoord\.{comp}")).is_some()
            && one(&q[2], &regex::escape(SCALE)).is_some()
    };
    let m = one(var, r"(\S+) \* (\S+)")?;
    for (a, u) in [
        (m[1].to_string(), m[2].to_string()),
        (m[2].to_string(), m[1].to_string()),
    ] {
        if let Some(k) = RE_UNI.captures(&u) {
            let n: i64 = k[1].parse().ok()?;
            if n >= 3 && RE_REGNAME.is_match(&a) && pixel(&a) {
                return Some([
                    n - 3,
                    k[2].parse().ok()?,
                    "xyzw".find(&k[3]).unwrap_or(0) as i64,
                ]);
            }
        }
    }
    None
}

// ------------------------------------------------------------------------------------------ screen varyings
re!(
    RE_TOK,
    r"^\s*(?:(out\.gl_Position\.[xyzw])|(_\d+)|(\d+\.\d*(?:e[+-]?\d+)?|\d+)f?|(fma)|([-+*/(),]))"
);

type Form = std::collections::BTreeMap<String, f64>;

fn fadd(a: &Form, b: &Form, s: f64) -> Form {
    let mut r = a.clone();
    for (k, v) in b {
        let e = r.entry(k.clone()).or_insert(0.0);
        *e += s * v;
    }
    r.retain(|_, v| *v != 0.0);
    r
}
fn fconst(a: &Form) -> bool {
    a.keys().all(|k| k == "1")
}
fn fscale(a: &Form, c: f64) -> Form {
    a.iter()
        .filter_map(|(k, v)| {
            let x = v * c;
            if x != 0.0 { Some((k.clone(), x)) } else { None }
        })
        .collect()
}
fn fmul(a: &Form, b: &Form) -> Option<Form> {
    if fconst(a) {
        return Some(fscale(b, a.get("1").copied().unwrap_or(0.0)));
    }
    if fconst(b) {
        return Some(fscale(a, b.get("1").copied().unwrap_or(0.0)));
    }
    None
}

struct Lin<'a> {
    toks: Vec<String>,
    i: usize,
    env: &'a dyn Fn(&str) -> Option<Form>,
}
impl Lin<'_> {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.i).map(|s| s.as_str())
    }
    fn take(&mut self, t: Option<&str>) -> Option<String> {
        let x = self.peek()?.to_string();
        if let Some(t) = t
            && x != t
        {
            return None;
        }
        self.i += 1;
        Some(x)
    }
    fn atom(&mut self) -> Option<Form> {
        let t = self.take(None)?;
        match t.as_str() {
            "-" => {
                let u = self.atom()?;
                Some(fscale(&u, -1.0))
            }
            "(" => {
                let v = self.add()?;
                self.take(Some(")"))?;
                Some(v)
            }
            "fma" => {
                self.take(Some("("))?;
                let a = self.add()?;
                self.take(Some(","))?;
                let b = self.add()?;
                self.take(Some(","))?;
                let c = self.add()?;
                self.take(Some(")"))?;
                Some(fadd(&fmul(&a, &b)?, &c, 1.0))
            }
            s if s.starts_with('#') => {
                let v: f64 = s[1..].parse().ok()?;
                let mut f = Form::new();
                if v != 0.0 {
                    f.insert("1".into(), v);
                }
                Some(f)
            }
            s => (self.env)(s),
        }
    }
    fn mul(&mut self) -> Option<Form> {
        let mut v = self.atom()?;
        while matches!(self.peek(), Some("*") | Some("/")) {
            let op = self.take(None)?;
            let w = self.atom()?;
            if op == "*" {
                v = fmul(&v, &w)?;
            } else if fconst(&w) && w.get("1").copied().unwrap_or(0.0) != 0.0 {
                v = fscale(&v, 1.0 / w["1"]);
            } else {
                return None;
            }
        }
        Some(v)
    }
    fn add(&mut self) -> Option<Form> {
        let mut v = self.mul()?;
        while matches!(self.peek(), Some("+") | Some("-")) {
            let op = self.take(None)?;
            let w = self.mul()?;
            v = fadd(&v, &w, if op == "+" { 1.0 } else { -1.0 });
        }
        Some(v)
    }
}

/// `_lin(expr, env)`: expr as {symbol: coefficient}, or None
fn lin(expr: &str, env: &dyn Fn(&str) -> Option<Form>) -> Option<Form> {
    let (mut toks, mut pos) = (Vec::new(), 0usize);
    while pos < expr.len() {
        let m = RE_TOK.captures(&expr[pos..])?;
        let whole = m.get(0).unwrap();
        if whole.end() == 0 {
            return None;
        }
        let t = if let Some(g) = m.get(1) {
            g.as_str().to_string()
        } else if let Some(g) = m.get(2) {
            g.as_str().to_string()
        } else if let Some(g) = m.get(3) {
            format!("#{}", g.as_str())
        } else if let Some(g) = m.get(4) {
            g.as_str().to_string()
        } else {
            m.get(5)?.as_str().to_string()
        };
        toks.push(t);
        pos += whole.end();
    }
    let mut p = Lin { toks, i: 0, env };
    let v = p.add()?;
    if p.peek().is_none() { Some(v) } else { None }
}

struct Stmts<'a> {
    decl: HashMap<&'a str, (usize, usize, &'a str)>,
    last: HashMap<(String, char), (usize, usize, Option<String>)>,
    nassign: HashMap<&'a str, usize>,
    pos: HashMap<char, (usize, usize, String)>,
    sym: HashMap<String, String>,
}

re!(RE_DECL, r"^(?:float|int|uint|bool|half) (_\d+) = (.+);$");
re!(
    RE_OUT_COMPONENT,
    r"^out\.(gl_Position|m_\d+)\.([xyzw]) = (.+);$"
);
re!(RE_OUT_VEC4, r"^out\.(gl_Position|m_\d+) = float4\((.+)\);$");
re!(RE_CTRL_START, r"^(?:if|else|for|while|do|switch)\b");
re!(RE_ASSIGN, r"(_\d+) = ");
re!(RE_RETURN, r"\breturn\b");

fn env_at(st: &Stmts, name: &str, at: usize) -> Option<Form> {
    if let Some(rest) = name.strip_prefix("out.gl_Position.") {
        let c = rest.chars().next()?;
        let p = st.pos.get(&c)?;
        return if p.0 < at {
            let mut f = Form::new();
            f.insert(format!("P{c}"), 1.0);
            Some(f)
        } else {
            None
        };
    }
    if let Some(s) = st.sym.get(name) {
        let mut f = Form::new();
        f.insert(s.clone(), 1.0);
        return Some(f);
    }
    let d = st.decl.get(name)?;
    if d.0 >= at || st.nassign.get(name).copied().unwrap_or(0) != 1 {
        return None;
    }
    let at2 = d.0;
    lin(d.2, &|n| env_at(st, n, at2))
}

/// `_entry_screen(body, locn)`
fn entry_screen(body: &str, locn: &[(String, i64)]) -> BTreeSet<i64> {
    let none = BTreeSet::new;
    if RE_RETURN.find_iter(body).count() != 1 {
        return none();
    }
    let tail = format!("{body}\n");
    if !(tail.ends_with("\n    return out;\n") || tail.ends_with("\n    return out;\n\n")) {
        return none();
    }
    let lines: Vec<&str> = body.split('\n').map(|l| l.trim()).collect();
    for (n, s) in lines.iter().enumerate() {
        if RE_CTRL_START.is_match(s) && (n + 1 >= lines.len() || lines[n + 1] != "{") {
            return none();
        }
    }
    let (mut depth, mut stmts) = (0usize, Vec::new());
    for s in &lines {
        if *s == "{" {
            depth += 1;
            continue;
        }
        if *s == "}" {
            depth = depth.saturating_sub(1);
            continue;
        }
        if !s.is_empty() {
            stmts.push((depth, *s));
        }
    }
    let mut st = Stmts {
        decl: HashMap::new(),
        last: HashMap::new(),
        nassign: HashMap::new(),
        pos: HashMap::new(),
        sym: HashMap::new(),
    };
    // `(?<![\w.])(_\d+) = `
    for m in RE_ASSIGN.captures_iter(body) {
        let g = m.get(1).unwrap();
        let prev = if g.start() == 0 {
            None
        } else {
            Some(body.as_bytes()[g.start() - 1])
        };
        if prev.map(|c| is_word(c) || c == b'.').unwrap_or(false) {
            continue;
        }
        *st.nassign.entry(g.as_str()).or_insert(0) += 1;
    }
    for (n, (dp, s)) in stmts.iter().enumerate() {
        if let Some(m) = RE_DECL.captures(s) {
            st.decl.insert(
                m.get(1).unwrap().as_str(),
                (n, *dp, m.get(2).unwrap().as_str()),
            );
            continue;
        }
        if let Some(m) = RE_OUT_COMPONENT.captures(s) {
            st.last.insert(
                (m[1].to_string(), m[2].chars().next().unwrap()),
                (n, *dp, Some(m[3].to_string())),
            );
            continue;
        }
        if let Some(m) = RE_OUT_VEC4.captures(s) {
            let parts: Vec<String> = m[2].split(',').map(|p| p.trim().to_string()).collect();
            for (i, c) in "xyzw".chars().enumerate() {
                st.last.insert(
                    (m[1].to_string(), c),
                    (
                        n,
                        *dp,
                        if parts.len() == 4 {
                            Some(parts[i].clone())
                        } else {
                            None
                        },
                    ),
                );
            }
        }
    }
    for c in ['x', 'y', 'w'] {
        match st.last.get(&("gl_Position".to_string(), c)) {
            Some((n, dp, Some(e))) if *dp == 0 => {
                st.pos.insert(c, (*n, *dp, e.clone()));
            }
            _ => return none(),
        }
    }
    for (c, p) in st.pos.clone() {
        if RE_REGNAME.is_match(&p.2) {
            st.sym.insert(p.2.clone(), format!("P{c}"));
        }
    }
    let want = |c: char| -> Form {
        let mut f = Form::new();
        match c {
            'x' => {
                f.insert("Px".into(), 0.5);
                f.insert("Pw".into(), 0.5);
            }
            'y' => {
                f.insert("Py".into(), -0.5);
                f.insert("Pw".into(), 0.5);
            }
            _ => {
                f.insert("Pw".into(), 1.0);
            }
        }
        f
    };
    let mut got = BTreeSet::new();
    for (mem, k) in locn {
        let mut ok = true;
        for c in ['x', 'y', 'w'] {
            match st.last.get(&(mem.clone(), c)) {
                Some((n, dp, Some(e))) if *dp == 0 => {
                    let at = *n;
                    if lin(e, &|nm| env_at(&st, nm, at)) != Some(want(c)) {
                        ok = false;
                        break;
                    }
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            got.insert(*k);
        }
    }
    got
}

re!(RE_VS_OUT_STRUCT, r"(?s)struct main0_out\s*\{(.*?)\};");
re!(RE_LOCN4, r"float4 (m_\d+) \[\[user\(locn(\d+)\)\]\]");
re!(
    RE_VS_ENTRY,
    r"(?s)\nvertex main0_out \w+\(.*?\)\n\{\n(.*?)\n\}\n"
);

/// `screen_varyings(msl)`: the interpolated outputs (locn numbers, sorted) of a vertex shader that are its clip -> screen varying in every entry point that returns the output struct
pub fn screen_varyings(msl: &str) -> Vec<i64> {
    let ost = RE_VS_OUT_STRUCT
        .captures(msl)
        .map(|c| c.get(1).unwrap().as_str())
        .unwrap_or("");
    let mut locn: Vec<(String, i64)> = Vec::new();
    for m in RE_LOCN4.captures_iter(ost) {
        let (n, k) = (m[1].to_string(), m[2].parse().unwrap_or(0));
        if let Some(e) = locn.iter_mut().find(|(x, _)| *x == n) {
            e.1 = k;
        } else {
            locn.push((n, k));
        }
    }
    let entries: Vec<&str> = RE_VS_ENTRY
        .captures_iter(msl)
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    if locn.is_empty() || entries.is_empty() {
        return vec![];
    }
    let mut got: BTreeSet<i64> = locn.iter().map(|(_, k)| *k).collect();
    for e in entries {
        let s = entry_screen(e, &locn);
        got = got.intersection(&s).copied().collect();
    }
    got.into_iter().collect()
}

// ------------------------------------------------------------------------------------------ fold
re!(RE_TEX2D, r"texture2d<float> (\w+) \[\[id\(\d+\)\]\];");
re!(RE_IN_LOCN, r"float4 (m_\d+) \[\[user\(locn(\d+)\)\]\];");
re!(
    RE_PLAIN_SAMPLE,
    r"^spvDescriptorSet0\.(\w+)\.sample\(spvDescriptorSet0\.(\w+)Smplr, select\((_\d+), float2\(0\.0\), isnan\((_\d+)\)\)\)"
);
re!(RE_COLOR_OUT, r"(\w+) (m_\d+) \[\[color\((\d+)\)\]\];");
re!(RE_BODY_CALL, r"\bdm_body_main0\(");

struct SampleMatch<'a> {
    start: usize,
    end: usize,
    whole: &'a str,
    member: &'a str,
    smp: &'a str,
    var: &'a str,
}

/// `pat.finditer(text)` of `fold`, with the conditional group and the back-references done by hand
fn sample_matches(text: &str) -> Vec<SampleMatch<'_>> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut p = 0usize;
    let cont_pre = format!(", spvDescriptorSet0.{}, ", abi::BORDER_MEMBER);
    while p < b.len() {
        let rel = {
            let a = text[p..].find("dm_bs(");
            let c = text[p..].find("spvDescriptorSet0.");
            match (a, c) {
                (Some(x), Some(y)) => Some(x.min(y)),
                (Some(x), None) => Some(x),
                (None, Some(y)) => Some(y),
                _ => None,
            }
        };
        let Some(rel) = rel else { break };
        let s = p + rel;
        let (wrapped, q) = if text[s..].starts_with("dm_bs(") {
            (true, s + 6)
        } else {
            (false, s)
        };
        let Some(m) = RE_PLAIN_SAMPLE.captures(&text[q..]) else {
            p = s + 1;
            continue;
        };
        if m[3] != m[4] {
            p = s + 1;
            continue;
        }
        let inner_end = q + m.get(0).unwrap().end();
        if !wrapped {
            out.push(SampleMatch {
                start: s,
                end: inner_end,
                whole: &text[s..inner_end],
                member: m.get(1).unwrap().as_str(),
                smp: m.get(2).unwrap().as_str(),
                var: m.get(3).unwrap().as_str(),
            });
            p = inner_end;
            continue;
        }
        // the border form: , spvDescriptorSet0.dm_border, \d+u, spvDescriptorSet0.<member>, spvDescriptorSet0.<smp>Smplr_w, select(<var>, float2(0.0), isnan(<var>)))
        let rest = &text[inner_end..];
        let mut ok = false;
        let mut end = inner_end;
        if let Some(r) = rest.strip_prefix(cont_pre.as_str()) {
            let digits = r.bytes().take_while(|c| c.is_ascii_digit()).count();
            if digits > 0 && r[digits..].starts_with('u') {
                let tail = format!(
                    ", spvDescriptorSet0.{}, spvDescriptorSet0.{}Smplr{}, select({}, float2(0.0), isnan({})))",
                    &m[1],
                    &m[2],
                    abi::BORDER_TWIN,
                    &m[3],
                    &m[3]
                );
                let after = &r[digits + 1..];
                if after.starts_with(&tail) {
                    ok = true;
                    end = inner_end + cont_pre.len() + digits + 1 + tail.len();
                }
            }
        }
        if ok {
            out.push(SampleMatch {
                start: s,
                end,
                whole: &text[s..end],
                member: m.get(1).unwrap().as_str(),
                smp: m.get(2).unwrap().as_str(),
                var: m.get(3).unwrap().as_str(),
            });
            p = end;
        } else {
            // group 1 cannot match at `s` any more: the scan goes on, and finds the inner plain sample as a match of its own
            p = s + 1;
        }
    }
    out
}

/// `fold(msl, bindings, om_only_ok)` -> Ok((the source with main0_f, meta)) or Err(reason)
pub fn fold(msl: &str, bindings: &[J], om_only_ok: bool) -> Result<(String, J), String> {
    let src = unit::shared_body(msl)
        .ok_or_else(|| "main function signature not recognized".to_string())?;
    let (where_, e) = match (unit::body(&src), unit::entry(&src, "main0")) {
        (b, Ok(e)) => (b, e),
        (_, Err(x)) => return Err(x),
    };
    let (Some((start, head, end)), Some(e)) = (where_, e) else {
        return Err("main function signature not recognized".into());
    };
    let text = &src[head..end];
    let tex: BTreeSet<String> = RE_TEX2D
        .captures_iter(&src)
        .map(|m| m[1].to_string())
        .collect();
    let in_part = src
        .split_once("struct main0_in")
        .map(|x| x.1)
        .unwrap_or(&src[..]);
    let in_part = in_part.split_once("};").map(|x| x.0).unwrap_or(in_part);
    let mut locn: HashMap<String, i64> = HashMap::new();
    for m in RE_IN_LOCN.captures_iter(in_part) {
        locn.insert(m[1].to_string(), m[2].parse().unwrap_or(0));
    }
    let mut reg: Vec<(String, i64)> = Vec::new();
    for b in bindings {
        if b.get("kind").and_then(|k| k.as_str()) == Some(abi::KIND_TEX)
            && let (Some(n), Some(r)) = (
                b.get("msl_name").and_then(|n| n.as_str()),
                b.get("reg").and_then(|r| r.as_i64()),
            )
            && !n.is_empty()
        {
            if let Some(x) = reg.iter_mut().find(|(k, _)| k == n) {
                x.1 = r;
            } else {
                reg.push((n.to_string(), r));
            }
        }
    }
    let reg_of = |n: &str| reg.iter().find(|(k, _)| k == n).map(|(_, r)| *r);
    let (mut calls, mut out_parts, mut pos): (Vec<J>, Vec<String>, usize) =
        (Vec::new(), Vec::new(), 0);
    let mut call_regs: Vec<i64> = Vec::new();
    let mut sels: Vec<String> = Vec::new();
    for m in sample_matches(text) {
        let (member, var) = (m.member, m.var);
        if !tex.contains(member) || member != m.smp || reg_of(member).is_none() {
            continue;
        }
        let d = super::rx::lit_then(
            text,
            &format!("float2 {var} = float2("),
            r"(_\d+), (_\d+)\);",
        );
        let (mut a, mut b) = (None, None);
        if let Some(d) = &d {
            a = decl(text, &d[1], r"in\.(m_\d+)\.x;").map(|c| c[1].to_string());
            b = decl(text, &d[2], r"in\.(m_\d+)\.y;").map(|c| c[1].to_string());
        }
        let how: J;
        if let (Some(a), Some(b)) = (&a, &b)
            && a == b
            && locn.contains_key(a)
        {
            how = J::obj().with("varying_locn", locn[a]);
            calls_push(
                &mut calls,
                &mut call_regs,
                &mut sels,
                reg_of(member).unwrap(),
                how,
                &mut out_parts,
                text,
                &mut pos,
                &m,
            );
            if calls.len() >= 32 {
                break;
            }
            continue;
        }
        let (mut pa, mut pb, mut fx, mut fy) = (None, None, None, None);
        if let Some(d) = &d {
            pa = proj(text, &d[1], "x", &locn);
            pb = proj(text, &d[2], "y", &locn);
            fx = frag_pos(text, &d[1], "x");
            fy = frag_pos(text, &d[2], "y");
        }
        if let Some(pa) = pa.as_ref().filter(|_| pa == pb) {
            how = J::obj().with("varying_locn", locn[pa]).with("proj", 1);
        } else if let (Some(x), Some(y)) = (fx, fy) {
            let arr = |a: [i64; 3]| J::Arr(a.iter().map(|v| J::Int(*v)).collect());
            how = J::obj().with("frag_pos", J::Arr(vec![arr(x), arr(y)]));
        } else {
            continue;
        }
        calls_push(
            &mut calls,
            &mut call_regs,
            &mut sels,
            reg_of(member).unwrap(),
            how,
            &mut out_parts,
            text,
            &mut pos,
            &m,
        );
        if calls.len() >= 32 {
            break;
        }
    }
    if calls.is_empty() && !om_only_ok {
        return Err("no rewritable sample".into());
    }
    // mixed-use slots
    let mut conv: HashMap<i64, usize> = HashMap::new();
    for r in &call_regs {
        *conv.entry(*r).or_insert(0) += 1;
    }
    let mut mixed: BTreeSet<i64> = BTreeSet::new();
    for (mb, r) in &reg {
        if tex.contains(mb)
            && text.matches(&format!("spvDescriptorSet0.{mb}.")).count()
                > conv.get(r).copied().unwrap_or(0)
        {
            mixed.insert(*r);
        }
    }
    let om = RE_VS_OUT_STRUCT
        .captures(&src)
        .ok_or_else(|| "no output struct".to_string())?;
    let omb = om.get(1).unwrap().as_str();
    let outs: Vec<(String, String, i64)> = RE_COLOR_OUT
        .captures_iter(omb)
        .map(|m| {
            (
                m[1].to_string(),
                m[2].to_string(),
                m[3].parse().unwrap_or(0),
            )
        })
        .collect();
    if outs.is_empty() || outs.len() > 4 || !RE_COLOR_OUT.replace_all(omb, "").trim().is_empty() {
        return Err("the output struct is not plain color outputs (or has more than 4)".into());
    }
    let mut rest = src[end..].to_string();
    let sig;
    if !calls.is_empty() {
        let mut params: Vec<String> = (0..calls.len()).map(|i| format!("int fx{i}")).collect();
        params.extend((0..NFB).map(|k| format!("float4 fb{k}")));
        let mut offv: Vec<String> = vec!["-1".to_string(); calls.len()];
        offv.extend(vec!["float4(0.0)".to_string(); NFB]);
        let off = offv.join(", ");
        let mut s = src[start..head - 1].to_string();
        if !s.ends_with('(') {
            s.push_str(", ");
        }
        s.push_str(&params.join(", "));
        s.push(')');
        sig = s;
        let (mut parts, mut at) = (String::new(), 0usize);
        for m in RE_BODY_CALL.find_iter(&rest.clone()) {
            let close = unit::close(&rest, m.end(), b'(', b')')
                .map(|c| c as isize - 1)
                .unwrap_or(-2);
            if close < 0 {
                return Err("unbalanced dm_body_main0 call".into());
            }
            let close = close as usize;
            parts.push_str(&rest[at..close]);
            if !rest[m.end()..close].trim().is_empty() {
                parts.push_str(", ");
            }
            parts.push_str(&off);
            at = close;
        }
        parts.push_str(&rest[at..]);
        rest = parts;
    } else {
        sig = src[start..head].to_string();
    }
    let mut consts = String::new();
    for c in 0..calls.len() {
        consts.push_str(&format!(
            "constant int FX_{c} [[function_constant({c})]];\n"
        ));
    }
    for k in 0..NFB {
        consts.push_str(&format!(
            "constant bool FB_{k} [[function_constant({})]];\n",
            32 + k
        ));
    }
    for j in 0..outs.len() {
        for k in 0..NFB {
            consts.push_str(&format!(
                "constant bool OM_{j}_{k} [[function_constant({})]];\n",
                64 + 8 * j + k
            ));
        }
    }
    let lead = &src[..start];
    if !lead.contains("using namespace metal;\n") {
        return Err("no using namespace metal".into());
    }
    let lead = lead.replacen(
        "using namespace metal;\n",
        &format!("using namespace metal;\n\n{consts}"),
        1,
    );
    let mut members = String::new();
    for (j, (t, n, _)) in outs.iter().enumerate() {
        for k in 0..NFB {
            members.push_str(&format!(
                "    {t} {n}_at{k} [[color({k}), function_constant(OM_{j}_{k})]];\n"
            ));
        }
    }
    let fbs: Vec<String> = (0..NFB)
        .map(|k| format!("float4 fb{k} [[color({k}), function_constant(FB_{k})]]"))
        .collect();
    let mut arg_list: Vec<String> = Vec::new();
    if !e.args.is_empty() {
        arg_list.push(e.args.clone());
    }
    if !calls.is_empty() {
        arg_list.extend((0..calls.len()).map(|i| format!("FX_{i}")));
        arg_list.extend((0..NFB).map(|k| format!("_fb{k}")));
    }
    let args = arg_list.join(", ");
    let mut load = String::new();
    if !calls.is_empty() {
        for k in 0..NFB {
            load.push_str(&format!(
                "    float4 _fb{k} = float4(0.0);\n    if (FB_{k}) _fb{k} = fb{k};\n"
            ));
        }
    }
    let mut put = String::new();
    for (j, (_, n, _)) in outs.iter().enumerate() {
        for k in 0..NFB {
            put.push_str(&format!("    if (OM_{j}_{k}) out.{n}_at{k} = r.{n};\n"));
        }
    }
    let mut all_params = e.params.clone();
    all_params.extend(fbs);
    let main0_f = format!(
        "\nstruct main0_out_f\n{{\n{members}}};\n\n{}fragment main0_out_f main0_f({})\n{{\n{load}    main0_out r = dm_body_main0({args});\n    main0_out_f out = {{}};\n{put}    return out;\n}}\n",
        e.attr,
        all_params.join(", ")
    );
    let mut res = String::new();
    res.push_str(&lead);
    res.push_str(&sig);
    for p in &out_parts {
        res.push_str(p);
    }
    res.push_str(&text[pos..]);
    res.push_str(&rest);
    res.push_str(&main0_f);
    let meta = J::obj()
        .with("calls", J::Arr(calls))
        .with(
            "mixed_regs",
            J::Arr(mixed.into_iter().map(J::Int).collect()),
        )
        .with(
            "outputs",
            J::Arr(
                outs.iter()
                    .map(|(t, _, c)| J::obj().with("color", *c).with("type", t.as_str()))
                    .collect(),
            ),
        );
    Ok((res, meta))
}

#[allow(clippy::too_many_arguments)]
fn calls_push(
    calls: &mut Vec<J>,
    call_regs: &mut Vec<i64>,
    sels: &mut Vec<String>,
    tex_reg: i64,
    how: J,
    out_parts: &mut Vec<String>,
    text: &str,
    pos: &mut usize,
    m: &SampleMatch<'_>,
) {
    let i = calls.len();
    let sel: String = (0..NFB)
        .map(|k| format!("fx{i} == {k} ? fb{k} : "))
        .collect();
    sels.push(sel.clone());
    // out_parts.append(text[pos:m.start()]); out_parts.append(f'({sel}{m.group(0)})'); pos = m.end()
    out_parts.push(text[*pos..m.start].to_string());
    out_parts.push(format!("({sel}{})", m.whole));
    *pos = m.end;
    let mut c = J::obj().with("fc", i).with("tex_reg", tex_reg);
    c.extend(&how);
    calls.push(c);
    call_regs.push(tex_reg);
}
