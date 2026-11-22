//! Small Python-semantics helpers the tessellation / mesh ports need so a failure message (which the reflection JSON or the build log carries) is the Python's own text:
//! `str.splitlines`, `str.strip`, `repr(str)` and `int(str)` with its `ValueError` message.

/// Python's `str.isspace` for one character (Unicode White_Space plus the ASCII separators U+001C..U+001F)
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `str.splitlines()` (keepends=False): the line boundaries are `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`, `\x1d`, `\x1e`, `\x85`, ` ` and ` `; a trailing boundary yields no empty last line
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut it = s.char_indices().peekable();
    while let Some((p, c)) = it.next() {
        let boundary = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if boundary {
            out.push(&s[start..p]);
            let mut end = p + c.len_utf8();
            if c == '\r'
                && let Some(&(q, '\n')) = it.peek()
            {
                it.next();
                end = q + 1;
            }
            start = end;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `repr(s)` of a Python `str` (the quote choice and the escapes; non-ASCII printable characters are kept)
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut o = String::with_capacity(s.len() + 2);
    o.push(quote);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c if c == quote => {
                o.push('\\');
                o.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                o.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if (c as u32) < 0x7f => o.push(c),
            c if is_py_printable(c) => o.push(c),
            c if (c as u32) <= 0xff => o.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xffff => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    o.push(quote);
    o
}

/// a coarse `str.isprintable` for non-ASCII characters (control characters and the Unicode separators are not printable)
fn is_py_printable(c: char) -> bool {
    !c.is_control()
        && !c.is_whitespace()
        && c != '\u{ad}'
        && !matches!(c as u32, 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x206f | 0xfeff | 0xe000..=0xf8ff)
}

/// `int(s)` (base 10) as an `i128`: surrounding whitespace, an optional sign, digits with single underscores between them. Err = the `ValueError` message of CPython
pub fn py_int(s: &str) -> Result<i128, String> {
    let bad = || format!("invalid literal for int() with base 10: {}", py_repr(s));
    let t = strip(s);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return Err(bad());
    }
    let mut v: i128 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let Some(d) = c.to_digit(10) else {
            return Err(bad());
        };
        v = v
            .checked_mul(10)
            .and_then(|x| x.checked_add(d as i128))
            .ok_or_else(bad)?;
    }
    Ok(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(
            splitlines("a\nb\r\nc\rd\x0be\x0cf\x1cg\u{85}h\u{2028}i\u{2029}j"),
            vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]
        );
        assert_eq!(splitlines("a\n\nb\n"), vec!["a", "", "b"]);
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("\n"), vec![""]);
    }

    #[test]
    fn repr_and_int() {
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(py_repr("x\ty\x01"), "'x\\ty\\x01'");
        assert_eq!(py_int(" 12 "), Ok(12));
        assert_eq!(py_int("1_0"), Ok(10));
        assert_eq!(py_int("-5"), Ok(-5));
        assert_eq!(
            py_int("1__0").unwrap_err(),
            "invalid literal for int() with base 10: '1__0'"
        );
        assert_eq!(
            py_int("0), foo").unwrap_err(),
            "invalid literal for int() with base 10: '0), foo'"
        );
        assert_eq!(
            py_int("").unwrap_err(),
            "invalid literal for int() with base 10: ''"
        );
    }
}
