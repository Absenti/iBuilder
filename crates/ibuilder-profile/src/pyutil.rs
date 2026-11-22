//! Python string / number semantics shared by the ports.

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

pub fn py_split_ws(s: &str) -> Vec<&str> {
    s.split(is_py_space).filter(|p| !p.is_empty()).collect()
}

/// `s.splitlines()`.
pub fn py_splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut start = 0;
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        if matches!(
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
        ) {
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

/// Python's `str.isprintable()`, approximated: control characters, whitespace other than the ASCII space, the
/// common format characters (Cf), private use (Co) and noncharacters are not printable. Unassigned code points
/// (Cn) cannot be told from assigned ones without the Unicode tables and count as printable.
pub fn py_isprintable(s: &str) -> bool {
    s.chars().all(|c| {
        if c == ' ' {
            return true;
        }
        let u = c as u32;
        !(c.is_control()
            || c.is_whitespace()
            || is_py_space(c)
            || matches!(u, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB)
            || matches!(u, 0xD800..=0xDFFF | 0xE000..=0xF8FF | 0xFDD0..=0xFDEF | 0xF0000..=0x10FFFF)
            || (u & 0xFFFE) == 0xFFFE)
    })
}

/// Python's `repr()` of a str.
pub fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut o = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c)
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                o.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

/// `int(s, 0)`: optional sign, `0x`/`0o`/`0b` prefixes, decimal without leading zeros, `_` between digits.
pub fn py_int0(s: &str) -> Option<i128> {
    let t = py_strip(s);
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let lower = t.to_ascii_lowercase();
    let (radix, digits) = if let Some(r) = lower.strip_prefix("0x") {
        (16, r.to_string())
    } else if let Some(r) = lower.strip_prefix("0o") {
        (8, r.to_string())
    } else if let Some(r) = lower.strip_prefix("0b") {
        (2, r.to_string())
    } else {
        (10, lower.clone())
    };
    let digits = if radix == 10 {
        digits
    } else {
        digits.strip_prefix('_').unwrap_or(&digits).to_string() // 0x_ff is legal
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if radix == 10 && clean.len() > 1 && clean.starts_with('0') && clean.chars().any(|c| c != '0') {
        return None; // "010" is not a valid base-0 literal
    }
    let v = i128::from_str_radix(&clean, radix).ok()?;
    Some(if neg { -v } else { v })
}

/// `f'{v:#x}'`
pub fn hex(v: i128) -> String {
    if v < 0 {
        format!("-{:#x}", -v)
    } else {
        format!("{v:#x}")
    }
}

/// Floor division like Python's `//`.
pub fn floordiv(a: i128, b: i128) -> i128 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn int0() {
        assert_eq!(py_int0("0x140000000"), Some(0x140000000));
        assert_eq!(py_int0(" 12 "), Some(12));
        assert_eq!(py_int0("0"), Some(0));
        assert_eq!(py_int0("00"), Some(0));
        assert_eq!(py_int0("010"), None);
        assert_eq!(py_int0("1_000"), Some(1000));
        assert_eq!(py_int0("0o17"), Some(15));
        assert_eq!(py_int0("-0x10"), Some(-16));
        assert_eq!(py_int0(""), None);
        assert_eq!(py_int0("0x"), None);
    }
    #[test]
    fn floor() {
        assert_eq!(floordiv(-7, 2), -4);
        assert_eq!(floordiv(7, 2), 3);
        assert_eq!(-floordiv(-(5i128), 2), 3); // ceil(5 / 2)
    }
}
