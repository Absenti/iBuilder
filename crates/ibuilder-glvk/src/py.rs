//! The few Python `str` semantics the ports need to give the same bytes: text-mode reads (`subprocess.run(text=True)`, `open().read()`: universal newlines),
//! `str.isspace` / `strip`, `str.splitlines`, `bytes.decode('latin1')`.

/// `subprocess.run(..., text=True).stdout` / `open(p).read()`: universal newlines, `\r\n` and a lone `\r` become `\n`.
pub fn text_mode(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// `str.isspace()` for one character: the ASCII whitespace, the information separators `\x1c`-`\x1f`, and Unicode's White_Space (`\x85`, `\xa0`, ...)
pub fn isspace(c: char) -> bool {
    matches!(c, '\t'..='\r' | '\x1c'..='\x1f' | ' ') || (c as u32 >= 0x80 && c.is_whitespace())
}

/// `s.strip()`
pub fn strip(s: &str) -> &str {
    s.trim_matches(isspace)
}

/// `s.lstrip()`
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(isspace)
}

/// `s.rstrip()`
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(isspace)
}

/// `bytes.decode('latin1')`
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// `str.encode('latin1')`; `None` when a character is above U+00FF (Python raises `UnicodeEncodeError`)
pub fn to_latin1(s: &str) -> Option<Vec<u8>> {
    s.chars()
        .map(|c| u8::try_from(c as u32).ok())
        .collect::<Option<Vec<u8>>>()
}

fn line_break(c: char) -> bool {
    matches!(
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
    )
}

/// `str.splitlines()` (no `keepends`): every Python line boundary, `\r\n` as one
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let (mut start, mut i) = (0usize, 0usize);
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        let w = c.len_utf8();
        if line_break(c) {
            out.push(&s[start..i]);
            let mut j = i + w;
            if c == '\r' && j < s.len() && b[j] == b'\n' {
                j += 1;
            }
            start = j;
            i = j;
        } else {
            i += w;
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
    fn text_mode_is_universal_newlines() {
        assert_eq!(text_mode("a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(text_mode("a\r"), "a\n");
    }

    #[test]
    fn splitlines_as_python() {
        assert_eq!(
            splitlines("a\nb\r\nc\rd\x0be"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(splitlines("a\n"), vec!["a"]);
        assert_eq!(splitlines("\n\n"), vec!["", ""]);
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn strip_takes_python_whitespace() {
        assert_eq!(strip("\x1c a \x1f"), "a");
        assert_eq!(rstrip("x)\u{a0}"), "x)");
    }
}
