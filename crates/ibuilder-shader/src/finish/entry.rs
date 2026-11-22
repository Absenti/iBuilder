//! `dmsh_entry.int_inputs`: vertex inputs a shader only uses as integers (the second translation pass declares them `uint`).
//!
//! The Python compiles four patterns per input attribute (`in\.{name}\b`, the `float _v = in.{name}.x;` declaration, `\b{_v}\b`, the copy `x = _v;`). Here none is compiled per call: the
//! attribute and variable names are literals searched as substrings, with the regex crate's own `\b` test around them (`rx::boundary_at`), and the two line patterns are matched as
//! "the line ends with the literal tail, and what is before it matches a fixed pattern" (`$` is the end of the text, so the split is unique). The function and the regex form it
//! replaced are compared over generated vertex shaders by `tests::int_inputs_is_the_regex_form`.
use super::reflect::attributes;
use super::rx::{boundary_at, find_lit_iter};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// `^\s*float\s+(_\w+)\s*=\s*` up to the end: what a declaration `float _v = in.<name>.<c>;` has before `in.`
static DECL_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A\s*float\s+(_\w+)\s*=\s*\z").expect("static regex"));
/// `^\s*[\w.\[\]]+\s*=\s*` up to the end: what a copy `<lvalue> = _v;` has before the variable
static COPY_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A\s*[\w.\[\]]+\s*=\s*\z").expect("static regex"));

/// `re.match(r'^\s*float\s+(_\w+)\s*=\s*in\.{name}\.[xyzw];$', line)` -> the variable
fn decl_of<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_suffix(';')?;
    let c = rest.chars().next_back()?;
    if !matches!(c, 'x' | 'y' | 'z' | 'w') {
        return None;
    }
    let rest = rest[..rest.len() - 1].strip_suffix('.')?;
    let rest = rest.strip_suffix(name)?.strip_suffix("in.")?;
    DECL_HEAD.captures(rest).map(|m| m.get(1).unwrap().as_str())
}

/// `re.match(r'^\s*[\w.\[\]]+\s*=\s*{v};$', line)`
fn is_copy(line: &str, v: &str) -> bool {
    line.strip_suffix(';')
        .and_then(|r| r.strip_suffix(v))
        .is_some_and(|head| COPY_HEAD.is_match(head))
}

/// Python `body[a:b]` where `b` may be -1 (`str.find` found nothing): the slice then ends one character early
fn slice_to(body: &str, a: usize, b: Option<usize>) -> &str {
    let e = match b {
        Some(e) => e,
        None => body.len().saturating_sub(1).max(a),
    };
    body.get(a..e).unwrap_or("")
}

/// `int_inputs(src)` -> {location (decimal string): "uint"}
pub fn int_inputs(src: &str) -> BTreeMap<String, String> {
    let mut ins: Vec<(String, i64)> = Vec::new();
    for (_, n, l) in attributes(src) {
        if let Some(e) = ins.iter_mut().find(|(k, _)| *k == n) {
            e.1 = l;
        } else {
            ins.push((n, l));
        }
    }
    let Some(i) = src.find("vertex main0_out main0(") else {
        return BTreeMap::new();
    };
    let body = &src[i..];
    let mut out = BTreeMap::new();
    let cast = |end: usize| {
        body[..end].ends_with("as_type<int>(") || body[..end].ends_with("as_type<uint>(")
    };
    for (name, loc) in &ins {
        let (mut nint, mut bad) = (0, false);
        // `re.finditer(rf'in\.{name}\b', body)`
        let lit = format!("in.{name}");
        for (us, ue) in find_lit_iter(body, &lit, |_, e| boundary_at(body, e)) {
            let ls = body[..us].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let le = body[ue..].find('\n').map(|p| p + ue);
            let line = slice_to(body, ls, le);
            let Some(v) = decl_of(line, name) else {
                if cast(us) {
                    nint += 1;
                    continue;
                }
                bad = true;
                break;
            };
            // `re.finditer(rf'\b{v}\b', body)` after its first match (`v` is `_\w+`: word characters only)
            for (os, oe) in
                find_lit_iter(body, v, |s, e| boundary_at(body, s) && boundary_at(body, e)).skip(1)
            {
                if cast(os) {
                    nint += 1;
                    continue;
                }
                let l2s = body[..os].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let l2e = body[oe..].find('\n').map(|p| p + oe);
                if is_copy(slice_to(body, l2s, l2e), v) {
                    continue;
                }
                bad = true;
                break;
            }
            if bad {
                break;
            }
        }
        if !bad && nint > 0 {
            out.insert(loc.to_string(), "uint".to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the form this module had before: four `Regex::new` per input attribute, as the Python writes it (the oracle of the rewrite above)
    #[allow(clippy::regex_creation_in_loops)]
    fn int_inputs_regex(src: &str) -> BTreeMap<String, String> {
        let mut ins: Vec<(String, i64)> = Vec::new();
        for (_, n, l) in attributes(src) {
            if let Some(e) = ins.iter_mut().find(|(k, _)| *k == n) {
                e.1 = l;
            } else {
                ins.push((n, l));
            }
        }
        let Some(i) = src.find("vertex main0_out main0(") else {
            return BTreeMap::new();
        };
        let body = &src[i..];
        let mut out = BTreeMap::new();
        let cast = |end: usize| {
            body[..end].ends_with("as_type<int>(") || body[..end].ends_with("as_type<uint>(")
        };
        for (name, loc) in &ins {
            let esc = regex::escape(name);
            let Ok(uses_re) = Regex::new(&format!(r"in\.{esc}\b")) else {
                continue;
            };
            let Ok(decl_re) = Regex::new(&format!(r"^\s*float\s+(_\w+)\s*=\s*in\.{esc}\.[xyzw];$"))
            else {
                continue;
            };
            let (mut nint, mut bad) = (0, false);
            for u in uses_re.find_iter(body) {
                let ls = body[..u.start()].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let le = body[u.end()..].find('\n').map(|p| p + u.end());
                let line = slice_to(body, ls, le);
                let Some(m) = decl_re.captures(line) else {
                    if cast(u.start()) {
                        nint += 1;
                        continue;
                    }
                    bad = true;
                    break;
                };
                let v = m[1].to_string();
                let Ok(vre) = Regex::new(&format!(r"\b{}\b", regex::escape(&v))) else {
                    bad = true;
                    break;
                };
                let copy_re =
                    Regex::new(&format!(r"^\s*[\w.\[\]]+\s*=\s*{};$", regex::escape(&v))).ok();
                for o in vre.find_iter(body).skip(1) {
                    if cast(o.start()) {
                        nint += 1;
                        continue;
                    }
                    let l2s = body[..o.start()].rfind('\n').map(|p| p + 1).unwrap_or(0);
                    let l2e = body[o.end()..].find('\n').map(|p| p + o.end());
                    if copy_re
                        .as_ref()
                        .map(|r| r.is_match(slice_to(body, l2s, l2e)))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    bad = true;
                    break;
                }
                if bad {
                    break;
                }
            }
            if !bad && nint > 0 {
                out.insert(loc.to_string(), "uint".to_string());
            }
        }
        out
    }

    #[test]
    fn decl_and_copy_lines_are_the_regex_forms() {
        let lines = [
            "    float _100 = in.m_20.x;",
            "float _1=in.m_20.w;",
            "  float\t_1x =  in.m_20.y;",
            "float _1 = in.m_20.q;",
            "float _1 = in.m_20.x; ",
            "float _1 = in.m_20.x",
            "float 1 = in.m_20.x;",
            "float _é = in.m_20.z;",
            "float _1 = in.m_2.x;",
            "xfloat _1 = in.m_20.x;",
            "float _1 = in.m_20.xx;",
            "float _ = in.m_20.x;",
            " float  _1 = in.m_20.x;",
            "out.a = _1;",
            "out.a[2].b = _1;",
            "a=_1;",
            "out.a = _1 ;",
            "out.a = _12;",
            " = _1;",
            "x.é = _1;",
            "out.a = (_1);",
            "_1;",
            "\tout.m_30.x = _1;",
        ];
        let d = Regex::new(r"^\s*float\s+(_\w+)\s*=\s*in\.m_20\.[xyzw];$").unwrap();
        let c = Regex::new(r"^\s*[\w.\[\]]+\s*=\s*_1;$").unwrap();
        for l in lines {
            assert_eq!(
                decl_of(l, "m_20"),
                d.captures(l).map(|m| m.get(1).unwrap().as_str()),
                "{l:?}"
            );
            assert_eq!(is_copy(l, "_1"), c.is_match(l), "{l:?}");
        }
    }

    #[test]
    fn int_inputs_is_the_regex_form() {
        // the 260 vertex shaders of the finish corpus (105 of them have integer inputs), each also with small mutations around the inputs' uses (10 in a release build, 2 in a debug one: the
        // regex form takes ~4 ms a shader in release)
        const MUTANTS: usize = if cfg!(debug_assertions) { 2 } else { 10 };
        let p =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/finish_cases.json");
        let d: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        let inserts = [
            " ",
            "é",
            "_",
            "x",
            ";",
            "\n",
            "as_type<int>(",
            "as_type<uint>(",
            "out.a = ",
            "1",
            "in.",
            ".x",
            "float ",
            "٣",
        ];
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n.max(1) as u64) as usize
        };
        let (mut cases, mut with_ints) = (0, 0);
        let (mut t_regex, mut t_now) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for c in d["vlib"].as_array().unwrap() {
            let msl = c["msl"].as_str().unwrap();
            let uses: Vec<usize> = msl.match_indices("in.").map(|(i, _)| i).collect();
            let mut variants = vec![msl.to_string()];
            for _ in 0..MUTANTS {
                let mut m = msl.to_string();
                for _ in 0..1 + rnd(3) {
                    let at = uses.get(rnd(uses.len())).copied().unwrap_or(0) + rnd(40);
                    let mut at = at.min(m.len());
                    while !m.is_char_boundary(at) {
                        at -= 1;
                    }
                    match rnd(3) {
                        0 => m.insert_str(at, inserts[rnd(inserts.len())]),
                        1 => {
                            let mut e = (at + 1 + rnd(6)).min(m.len());
                            while !m.is_char_boundary(e) {
                                e += 1;
                            }
                            m.replace_range(at..e, "");
                        }
                        _ => {
                            // a copy of the current line after it (a second use of the same variable)
                            let ls = m[..at].rfind('\n').map_or(0, |p| p + 1);
                            let le = m[at..].find('\n').map_or(m.len(), |p| p + at);
                            let line = m[ls..le].to_string();
                            m.insert_str(le, &format!("\n{line}"));
                        }
                    }
                }
                variants.push(m);
            }
            for v in &variants {
                let t = std::time::Instant::now();
                let want = int_inputs_regex(v);
                t_regex += t.elapsed();
                let t = std::time::Instant::now();
                let got = int_inputs(v);
                t_now += t.elapsed();
                with_ints += !want.is_empty() as usize;
                assert_eq!(got, want, "{v}");
                cases += 1;
            }
        }
        eprintln!(
            "int_inputs over {cases} shaders: regex form {:.1} ms, this form {:.1} ms",
            t_regex.as_secs_f64() * 1e3,
            t_now.as_secs_f64() * 1e3
        );
        assert_eq!(cases, 260 * (1 + MUTANTS));
        assert!(with_ints > 105 + 10 * MUTANTS, "{with_ints}");
    }
}
