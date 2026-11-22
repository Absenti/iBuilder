//! The few Python `str` / `int` / `OSError` behaviours `mods.py` leans on, spelled as Python spells them: the port must produce the same
//! text in the logs and take the same branches on odd input. Every function here says which Python it stands for.

use crate::pytables::{NON_PRINTABLE, PY_SPACE};
use std::io;
use std::path::Path;

/// `str.isspace()` of one character (Python's whitespace is Rust's plus U+001C..U+001F).
pub fn is_space(c: char) -> bool {
    let u = c as u32;
    if u < 0x80 {
        return matches!(u, 0x9..=0xD | 0x1C..=0x20);
    }
    PY_SPACE.binary_search(&u).is_ok()
}

/// `s.strip()`
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.strip(chars)`
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `s.split()`
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|p| !p.is_empty()).collect()
}

/// `s.split(None, 1)`: the first word and what follows it with its leading white space removed (trailing white space kept).
pub fn split_ws_once(s: &str) -> Vec<&str> {
    let s = s.trim_start_matches(is_space);
    if s.is_empty() {
        return vec![];
    }
    match s.find(is_space) {
        None => vec![s],
        Some(i) => {
            let rest = s[i..].trim_start_matches(is_space);
            if rest.is_empty() {
                vec![&s[..i]]
            } else {
                vec![&s[..i], rest]
            }
        }
    }
}

/// `s.splitlines()`: line boundaries are \n \r \r\n \v \f \x1c \x1d \x1e \x85    .
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r'
            && let Some(&(j, '\n')) = it.peek()
        {
            it.next();
            end = j + 1;
        }
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// What reading a file in Python's text mode does with line ends (`newline=None`): `\r\n` and a lone `\r` become `\n`.
pub fn universal_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// `re.compile(r'^[0-9A-Fa-f]+$').match(s)`: Python's `$` also matches just before a trailing newline.
pub fn hex_match(s: &str) -> bool {
    let body = s.strip_suffix('\n').unwrap_or(s);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `os.path.splitext(basename)[0]` for a name with no separator in it.
pub fn splitext_stem(base: &str) -> &str {
    if let Some(dot) = base.rfind('.') {
        // leading dots are not an extension separator
        if base[..dot].chars().any(|c| c != '.') {
            return &base[..dot];
        }
    }
    base
}

/// `os.path.basename`
pub fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `repr(s)` of a Python `str`.
pub fn repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let u = c as u32;
        match c {
            _ if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if u < 0x20 || u == 0x7F => out.push_str(&format!("\\x{u:02x}")),
            _ if u < 0x7F => out.push(c),
            _ if !NON_PRINTABLE.iter().any(|&(a, b)| u >= a && u <= b) => out.push(c),
            _ if u <= 0xFF => out.push_str(&format!("\\x{u:02x}")),
            _ if u <= 0xFFFF => out.push_str(&format!("\\u{u:04x}")),
            _ => out.push_str(&format!("\\U{u:08x}")),
        }
    }
    out.push(quote);
    out
}

/// Offsets are Python ints (any size): `i128`, saturated at +-2^100, which is far beyond anything a module holds and keeps every comparison
/// and every "is it a valid `struct` field" test the same as with an unbounded integer.
pub const BIG: i128 = 1 << 100;

fn clamp(v: i128) -> i128 {
    v.clamp(-BIG, BIG)
}

/// `int(s, 16)` (no sign, no underscores: the callers only pass what `HEX` matched after removing `0x`): `None` where Python raises ValueError.
pub fn int_base16(s: &str) -> Option<i128> {
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut v: i128 = 0;
    for b in digits.bytes() {
        v = clamp(
            v.saturating_mul(16)
                .saturating_add((b as char).to_digit(16).unwrap() as i128),
        );
    }
    Some(v)
}

/// `int(s, 0)` for an ASCII token (white space around it is accepted, as Python does): sign, `0x` / `0o` / `0b` prefixes, a decimal number without
/// leading zeros, single underscores between digits. `None` where Python raises ValueError. (Unicode decimal digits are not accepted here: documented.)
pub fn int_base0(s: &str) -> Option<i128> {
    let t = strip(s);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let (radix, body, prefixed) =
        if let Some(r) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
            (16, r, true)
        } else if let Some(r) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
            (8, r, true)
        } else if let Some(r) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
            (2, r, true)
        } else {
            (10, t, false)
        };
    // underscores: only between digits (or right after a base prefix)
    let body = if prefixed {
        body.strip_prefix('_').unwrap_or(body)
    } else {
        body
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let digits: String = body.chars().filter(|&c| c != '_').collect();
    if radix == 10
        && digits.len() > 1
        && digits.starts_with('0')
        && !digits.bytes().all(|b| b == b'0')
    {
        return None; // leading zeros in a decimal literal
    }
    let mut v: i128 = 0;
    for c in digits.chars() {
        let d = c.to_digit(radix)? as i128;
        v = clamp(v.saturating_mul(radix as i128).saturating_add(d));
    }
    Some(if neg { -v } else { v })
}

/// `str(OSError)` as Python spells it: `[Errno 2] No such file or directory: '/path'`.
pub fn os_error(e: &io::Error, path: &Path) -> String {
    let full = e.to_string();
    // Rust: "<strerror> (os error N)"; Python: "[Errno N] <strerror>: 'path'"
    let (text, code) = match (full.rfind(" (os error "), e.raw_os_error()) {
        (Some(i), Some(n)) => (full[..i].to_string(), n),
        (_, n) => (full.clone(), n.unwrap_or(0)),
    };
    format!("[Errno {code}] {text}: {}", repr(&path.to_string_lossy()))
}

/// `'%s' % bytes_hex_upper`
pub fn hex_upper(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// The text of the `UnicodeDecodeError` Python raises for `bytes.decode('utf-8')` (strict), by the rules of CPython's decoder.
pub fn utf8_error_text(data: &[u8], e: &std::str::Utf8Error) -> String {
    let start = e.valid_up_to();
    match e.error_len() {
        None => {
            let end = data.len();
            if end - start == 1 {
                format!(
                    "'utf-8' codec can't decode byte 0x{:02x} in position {start}: unexpected end of data",
                    data[start]
                )
            } else {
                format!(
                    "'utf-8' codec can't decode bytes in position {start}-{}: unexpected end of data",
                    end - 1
                )
            }
        }
        Some(n) => {
            let b = data[start];
            let why = if (0x80..=0xC1).contains(&b) || b >= 0xF5 {
                "invalid start byte"
            } else {
                "invalid continuation byte"
            };
            if n == 1 {
                format!("'utf-8' codec can't decode byte 0x{b:02x} in position {start}: {why}")
            } else {
                format!(
                    "'utf-8' codec can't decode bytes in position {start}-{}: {why}",
                    start + n - 1
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_pythons() {
        for u in 0..0x110000u32 {
            if let Some(c) = char::from_u32(u) {
                assert_eq!(is_space(c), PY_SPACE.contains(&u), "{u:#x}");
            }
        }
    }

    #[test]
    fn splits_like_python() {
        assert_eq!(split_ws_once("a  b c "), vec!["a", "b c "]);
        assert_eq!(split_ws_once("  a  "), vec!["a"]);
        assert!(split_ws_once("   ").is_empty());
        assert_eq!(
            splitlines("a\r\nb\rc\x0bd\u{2028}e\n"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(splitext_stem("a.b.c"), "a.b");
        assert_eq!(splitext_stem(".ips"), ".ips");
        assert_eq!(splitext_stem("...x"), "...x");
        assert_eq!(splitext_stem("x."), "x");
    }

    #[test]
    fn ints_like_python() {
        assert_eq!(int_base0("0x100"), Some(256));
        assert_eq!(int_base0("-0x10"), Some(-16));
        assert_eq!(int_base0("1_000"), Some(1000));
        assert_eq!(int_base0("00"), Some(0));
        assert_eq!(int_base0("010"), None);
        assert_eq!(int_base0("0x"), None);
        assert_eq!(int_base0("1__0"), None);
        assert_eq!(int_base0("0b101"), Some(5));
        assert_eq!(int_base0("0o17"), Some(15));
        assert_eq!(int_base16("0x0x12"), None);
        assert_eq!(int_base16("0X1F"), Some(31));
    }

    #[test]
    fn utf8_error_texts_are_pythons() {
        // the texts CPython 3.14.1 printed for these inputs (bytes.decode('utf-8'))
        let cases: [(&[u8], &str); 7] = [
            (
                b"mod\tM\t1\n\xff",
                "'utf-8' codec can't decode byte 0xff in position 8: invalid start byte",
            ),
            (
                b"ab\xe2\x82",
                "'utf-8' codec can't decode bytes in position 2-3: unexpected end of data",
            ),
            (
                b"ab\xe2\x28\xa1",
                "'utf-8' codec can't decode byte 0xe2 in position 2: invalid continuation byte",
            ),
            (
                b"x\xf0\x9f\x98",
                "'utf-8' codec can't decode bytes in position 1-3: unexpected end of data",
            ),
            (
                b"\xc3",
                "'utf-8' codec can't decode byte 0xc3 in position 0: unexpected end of data",
            ),
            (
                b"\xed\xa0\x80x",
                "'utf-8' codec can't decode byte 0xed in position 0: invalid continuation byte",
            ),
            (
                b"\xf8\x88\x80\x80\x80",
                "'utf-8' codec can't decode byte 0xf8 in position 0: invalid start byte",
            ),
        ];
        for (bytes, want) in cases {
            let e = std::str::from_utf8(bytes).unwrap_err();
            assert_eq!(utf8_error_text(bytes, &e), want);
        }
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(repr("abc"), "'abc'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("it's \"x\""), "'it\\'s \"x\"'");
        assert_eq!(repr("a\\b\n\u{1}"), "'a\\\\b\\n\\x01'");
        assert_eq!(repr("\u{e9}\u{200b}"), "'\u{e9}\\u200b'");
    }
}
