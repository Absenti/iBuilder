//! Regular expressions the finishing builds from run-time names, without compiling one per call (review r4 6.3, optimisations 4.1-4.4).
//!
//! The Python builds these patterns with `re.compile(f'...{re.escape(name)}...')` (and `re` keeps the last 512 in its own cache); a straight port compiled a `regex::Regex` for every attribute,
//! sample, pool member or entry point, 10-30 us each. Two shapes cover every site, each exact by construction (the same pattern text, or the same language):
//!
//! * [`cached`]: a pattern whose run-time part comes from a small set (a struct or entry point name, a stride): compiled once per thread and pattern text.
//! * [`lit_then`]: a pattern `escape(lit) + tail` whose literal holds a per-shader temporary (`float _123 = ...`): the leftmost match of the whole pattern is the first occurrence of the
//!   literal (overlapping occurrences included) after which the tail matches **anchored**, so the literal is found with a substring search and only the tail, a fixed pattern, is a
//!   regex (cached). The tail must not start with a look-behind assertion (`\b`, `^` in multi-line mode): it sees the text after the literal only.
//! * [`is_word`] / [`boundary_at`]: the regex crate's Unicode `\w` and `\b`, for the sites that test a word boundary around a literal.
use regex::{Captures, Regex};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

/// patterns kept per thread before the cache starts again (each is a few KB; the sites have a handful of distinct texts)
const CAP: usize = 256;

thread_local! {
    static CACHE: RefCell<HashMap<String, Rc<Regex>>> = RefCell::new(HashMap::new());
}

/// `Regex::new(pat)`, compiled once per thread and pattern text (an invalid pattern is not kept: it fails again, as `Regex::new` does)
pub fn cached(pat: &str) -> Result<Rc<Regex>, regex::Error> {
    if let Some(r) = CACHE.with(|c| c.borrow().get(pat).cloned()) {
        return Ok(r);
    }
    let r = Rc::new(Regex::new(pat)?);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= CAP {
            c.clear();
        }
        c.insert(pat.to_string(), r.clone());
    });
    Ok(r)
}

/// The leftmost match of the regex `regex::escape(lit) + tail` in `text`, as the captures of its tail (group 0 is the tail's match, groups 1.. are the pattern's). `None` also when
/// `tail` is not a valid pattern (every caller's tail is a fixed text).
pub fn lit_then<'t>(text: &'t str, lit: &str, tail: &str) -> Option<Captures<'t>> {
    if lit.is_empty() {
        return cached(tail).ok()?.captures(text);
    }
    let re = cached(&format!("^(?:{tail})")).ok()?;
    let mut from = 0;
    while let Some(p) = text[from..].find(lit).map(|i| i + from) {
        if let Some(c) = re.captures(&text[p + lit.len()..]) {
            return Some(c);
        }
        // the next occurrence may overlap this one: search again from the next character
        from = p + text[p..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A\w\z").expect("static regex"));

/// the regex crate's `\w` (Unicode word character)
pub fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    let mut b = [0u8; 4];
    WORD.is_match(c.encode_utf8(&mut b))
}

/// the regex crate's `\b` at byte `i` of `s` (a character boundary): a word character on exactly one side
pub fn boundary_at(s: &str, i: usize) -> bool {
    let before = s[..i].chars().next_back().is_some_and(is_word);
    let after = s[i..].chars().next().is_some_and(is_word);
    before != after
}

/// The non-overlapping matches of `\b{lit}\b`-like patterns: occurrences of `lit` (leftmost first, the search resumed after a match and one character after a refused occurrence)
/// that `ok(start, end)` accepts. With `ok` testing the boundaries this is `Regex::find_iter` of `\b` + escaped literal + `\b` (the literal begins and ends with word characters).
pub fn find_lit_iter<'t>(
    text: &'t str,
    lit: &'t str,
    ok: impl Fn(usize, usize) -> bool + 't,
) -> impl Iterator<Item = (usize, usize)> + 't {
    let mut from = 0;
    std::iter::from_fn(move || {
        if lit.is_empty() {
            return None;
        }
        while let Some(p) = text.get(from..)?.find(lit).map(|i| i + from) {
            let e = p + lit.len();
            if ok(p, e) {
                from = e;
                return Some((p, e));
            }
            from = p + text[p..].chars().next().map_or(1, char::len_utf8);
        }
        from = text.len() + 1;
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// a deterministic stream of texts made of the pieces the finishing's patterns look for, separators, digits and a few non-ASCII characters
    fn texts(n: usize) -> Vec<String> {
        let pieces = [
            "float ",
            "float2 ",
            "_1",
            "_12",
            "_123",
            " = ",
            "in.",
            "m_0",
            "m_10",
            ".x",
            ".y",
            ".w",
            ";",
            "\n",
            " ",
            "(",
            ")",
            ", ",
            "1.0 / ",
            " * ",
            " / ",
            "_",
            "é",
            "٣",
            "²",
            "x",
            "float _1 = ",
            "float _12 = in.m_1",
            "float2 _1 = float2(",
            "_7, _8);",
            "\t",
            "a",
            "Z",
            "struct spvPool_a",
            " {\n uint n;\n ulong seen;\n array<int, 4> e;",
            "float _1 = _7 * _8;",
            "float _12 = 1.0 / _3;",
            "float _1 = in.m_0.x;",
            "float _12 = in.m_10.w;",
            "float2 _1 = float2(_7, _8);",
            "float _1 = _2 / _3;",
            "struct spvPool_a {\n uint n;\n ulong seen;\n array<int, 4> e;",
            "float _1 = a * b;",
            "_1_1",
        ];
        let mut x = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                let mut s = String::new();
                for _ in 0..(x % 24) as usize + 1 {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    s.push_str(pieces[(x % pieces.len() as u64) as usize]);
                }
                x = x.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
                s
            })
            .collect()
    }

    fn groups(c: Option<Captures<'_>>) -> Option<Vec<Option<String>>> {
        c.map(|c| {
            (1..c.len())
                .map(|i| c.get(i).map(|m| m.as_str().to_string()))
                .collect()
        })
    }

    #[test]
    fn lit_then_is_the_leftmost_match_of_the_whole_pattern() {
        let cases: [(&str, &str); 9] = [
            ("float _1 = ", r"(_\d+) \* (_\d+);"),
            ("float _12 = ", r"1\.0 / (_\d+);"),
            ("float _1 = in.", r"(m_\d+)\.x;"),
            ("float _12 = in.", r"(m_\d+)\.w;"),
            ("float2 _1 = float2(", r"(_\d+), (_\d+)\);"),
            ("float _1 = ", r"(\S+) \* (\S+);"),
            ("float _1 = ", r"(_\d+) / (_\d+);"),
            (
                "struct spvPool_a",
                r"\s*\{\s*uint n;\s*ulong seen;\s*array<(.+), \d+> e;",
            ),
            ("_1_1", r"(_\d*)"),
        ];
        let mut found = 0;
        let wholes: Vec<Regex> = cases
            .iter()
            .map(|(lit, tail)| Regex::new(&format!("{}{tail}", regex::escape(lit))).unwrap())
            .collect();
        for t in texts(4000) {
            for ((lit, tail), whole) in cases.iter().zip(&wholes) {
                let want = groups(whole.captures(&t));
                found += want.is_some() as usize;
                assert_eq!(
                    groups(lit_then(&t, lit, tail)),
                    want,
                    "{lit:?} {tail:?} in {t:?}"
                );
            }
        }
        assert!(found > 200, "the generator must make matches: {found}");
        // overlapping occurrences of the literal: the second one matches
        assert_eq!(groups(lit_then("_1_1_1x", "_1_1", "_1x")), Some(vec![]));
        assert!(
            lit_then("abc", "b", "(").is_none(),
            "an invalid tail is no match"
        );
    }

    #[test]
    fn word_and_boundary_are_the_regex_crates() {
        for c in [
            'a', 'Z', '0', '_', 'é', '٣', '²', 'ⅷ', '\u{301}', '-', '.', ' ', '\u{200d}', '中',
            '🙂',
        ] {
            assert_eq!(is_word(c), WORD.is_match(&c.to_string()), "{c:?}");
        }
        let b = Regex::new(r"\b").unwrap();
        for t in texts(500) {
            let want: Vec<usize> = b.find_iter(&t).map(|m| m.start()).collect();
            let got: Vec<usize> = t
                .char_indices()
                .map(|(i, _)| i)
                .chain([t.len()])
                .filter(|&i| boundary_at(&t, i))
                .collect();
            assert_eq!(got, want, "{t:?}");
        }
    }

    #[test]
    fn find_lit_iter_is_find_iter_of_the_bounded_literal() {
        let lits = ["_1", "_12", "in.m_0", "m_1"];
        let both: Vec<Regex> = lits
            .iter()
            .map(|l| Regex::new(&format!(r"\b{}\b", regex::escape(l))).unwrap())
            .collect();
        let end: Vec<Regex> = lits
            .iter()
            .map(|l| Regex::new(&format!(r"in\.{}\b", regex::escape(l))).unwrap())
            .collect();
        for t in texts(3000) {
            for ((lit, re), re2) in lits.iter().zip(&both).zip(&end) {
                let want: Vec<(usize, usize)> =
                    re.find_iter(&t).map(|m| (m.start(), m.end())).collect();
                let got: Vec<(usize, usize)> =
                    find_lit_iter(&t, lit, |s, e| boundary_at(&t, s) && boundary_at(&t, e))
                        .collect();
                assert_eq!(got, want, "{lit:?} in {t:?}");
                // only the end tested: `in\.{name}\b`
                let re = re2;
                let l2 = format!("in.{lit}");
                let want: Vec<(usize, usize)> =
                    re.find_iter(&t).map(|m| (m.start(), m.end())).collect();
                let got: Vec<(usize, usize)> =
                    find_lit_iter(&t, &l2, |_, e| boundary_at(&t, e)).collect();
                assert_eq!(got, want, "{l2:?} in {t:?}");
            }
        }
    }

    #[test]
    fn the_cache_returns_the_same_regex_and_starts_again_when_full() {
        let a = cached("ab+c").unwrap();
        let b = cached("ab+c").unwrap();
        assert!(Rc::ptr_eq(&a, &b));
        for i in 0..CAP + 3 {
            cached(&format!("x{i}")).unwrap();
        }
        assert!(CACHE.with(|c| c.borrow().len()) <= CAP);
        assert!(cached("(").is_err());
    }
}
