//! Python string semantics the port has to share: `str.strip()`, `str.split()`, `str.splitlines()`.

/// `str.isspace()` for one char (Unicode White_Space plus the C0 separators Python also counts).
pub fn is_py_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

pub fn py_lstrip(s: &str) -> &str {
    s.trim_start_matches(is_py_space)
}

/// `s.split()`: runs of whitespace separate, empty fields dropped.
pub fn py_split_ws(s: &str) -> Vec<&str> {
    s.split(is_py_space).filter(|p| !p.is_empty()).collect()
}

/// `s.splitlines()` (no line ends kept).
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if brk {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' && b.get(next) == Some(&b'\n') {
                it.next();
                next += 1;
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splitlines_like_python() {
        assert_eq!(
            py_splitlines("a\nb\r\nc\rd\u{2028}e"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(py_splitlines("a\n"), vec!["a"]);
        assert_eq!(py_splitlines("\n\nx"), vec!["", "", "x"]);
        assert!(py_splitlines("").is_empty());
    }
    #[test]
    fn strip_split() {
        assert_eq!(py_strip(" \t\u{a0}x y\u{1f} "), "x y");
        assert_eq!(
            py_split_ws(" main  sdk\tsubsdk0 "),
            vec!["main", "sdk", "subsdk0"]
        );
    }
}
