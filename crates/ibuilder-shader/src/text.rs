//! Scanning helpers that stand in for the regular expressions of the Python: ASCII word characters, balanced brackets, substring search.
use memchr::memmem;

#[inline]
pub fn is_word(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}
/// Python `\s` on ASCII
#[inline]
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}
/// whether a word character comes just before `s[i]` (dmsh_reflect.in_word)
#[inline]
pub fn in_word(s: &[u8], i: usize) -> bool {
    i > 0 && is_word(s[i - 1])
}

pub fn skip_space(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_space(s[i]) {
        i += 1;
    }
    i
}
/// end of the run of word characters starting at `i` (== i when none)
pub fn word_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && is_word(s[i]) {
        i += 1;
    }
    i
}
pub fn digits_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    i
}

/// Python `str.find(needle, from)`
pub fn find(h: &str, n: &str, from: usize) -> Option<usize> {
    if from > h.len() {
        return None;
    }
    memmem::find(&h.as_bytes()[from..], n.as_bytes()).map(|p| p + from)
}
pub fn find_b(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    if from > h.len() {
        return None;
    }
    memmem::find(&h[from..], n).map(|p| p + from)
}
pub fn contains(h: &str, n: &str) -> bool {
    memmem::find(h.as_bytes(), n.as_bytes()).is_some()
}
/// Python `str.rfind(needle, 0, end)`
pub fn rfind_before(h: &[u8], n: &[u8], end: usize) -> Option<usize> {
    memmem::rfind(&h[..end.min(h.len())], n)
}

/// `_close(src, i, o, c)` of dmsh_side: the index after the bracket closing the one opened just before `i`
pub fn close(s: &[u8], i: usize, o: u8, c: u8) -> Option<usize> {
    let mut depth = 1i32;
    let mut k = i;
    while k < s.len() {
        let p = memchr::memchr2(o, c, &s[k..])? + k;
        if s[p] == o {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(p + 1);
            }
        }
        k = p + 1;
    }
    None
}

/// `block_end(src, start)` of dmsh_rewrite: the index of the brace closing the block opened at `s[start]`; `start` itself when it is no brace
pub fn block_end(s: &[u8], start: usize) -> Option<usize> {
    if start < s.len() && s[start] != b'{' && s[start] != b'}' {
        return Some(start);
    }
    let mut depth = 0i32;
    let mut k = start;
    while k < s.len() {
        let p = memchr::memchr2(b'{', b'}', &s[k..])? + k;
        if s[p] == b'{' {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(p);
            }
        }
        k = p + 1;
    }
    None
}

/// Python `str.strip()` on ASCII whitespace
pub fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii() && is_space(c as u8))
}

/// whether `s[i..]` starts with `p`
#[inline]
pub fn starts(s: &[u8], i: usize, p: &str) -> bool {
    s.len() >= i + p.len() && &s[i..i + p.len()] == p.as_bytes()
}

/// The lines of `s` as (start, end) ranges, `end` excluding the newline (Python `splitlines` for `\n` text; a trailing newline yields no empty last line)
pub fn lines(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut v = Vec::new();
    let mut st = 0;
    while st < b.len() {
        let e = memchr::memchr(b'\n', &b[st..])
            .map(|p| p + st)
            .unwrap_or(b.len());
        v.push((st, e));
        st = e + 1;
    }
    v
}

/// A cursor for hand-written patterns (the Python regexes as straight-line code): every method either matches and moves, or leaves the position alone and says no.
/// Only deterministic pieces (a maximal `\w+`, `\s*`, a literal) are offered: where a regex would backtrack, the caller says why it cannot.
#[derive(Clone, Copy)]
pub struct Cur<'a> {
    pub s: &'a str,
    pub i: usize,
}
impl<'a> Cur<'a> {
    pub fn at(s: &'a str, i: usize) -> Self {
        Cur { s, i }
    }
    #[inline]
    pub fn b(&self) -> &'a [u8] {
        self.s.as_bytes()
    }
    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }
    #[inline]
    pub fn at_end(&self) -> bool {
        self.i >= self.s.len()
    }
    pub fn ahead(&self, p: &str) -> bool {
        starts(self.b(), self.i, p)
    }
    /// a literal
    pub fn lit(&mut self, p: &str) -> bool {
        if starts(self.b(), self.i, p) {
            self.i += p.len();
            true
        } else {
            false
        }
    }
    /// ` *`
    pub fn spaces(&mut self) -> &'a str {
        let a = self.i;
        while self.peek() == Some(b' ') {
            self.i += 1;
        }
        &self.s[a..self.i]
    }
    /// `[ \t]*`
    pub fn ht0(&mut self) -> &'a str {
        let a = self.i;
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.i += 1;
        }
        &self.s[a..self.i]
    }
    /// `\s*`
    pub fn ws0(&mut self) -> &'a str {
        let a = self.i;
        self.i = skip_space(self.b(), self.i);
        &self.s[a..self.i]
    }
    /// `\s+`
    pub fn ws1(&mut self) -> bool {
        let a = self.i;
        self.i = skip_space(self.b(), self.i);
        self.i > a
    }
    /// `\w+` (maximal)
    pub fn word(&mut self) -> Option<&'a str> {
        let a = self.i;
        let e = word_end(self.b(), a);
        if e == a {
            None
        } else {
            self.i = e;
            Some(&self.s[a..e])
        }
    }
    /// `\d+` (maximal)
    pub fn digits(&mut self) -> Option<&'a str> {
        let a = self.i;
        let e = digits_end(self.b(), a);
        if e == a {
            None
        } else {
            self.i = e;
            Some(&self.s[a..e])
        }
    }
    /// `-?\d+`
    pub fn int(&mut self) -> Option<&'a str> {
        let a = self.i;
        let mut k = a;
        if self.b().get(k) == Some(&b'-') {
            k += 1;
        }
        let e = digits_end(self.b(), k);
        if e == k {
            None
        } else {
            self.i = e;
            Some(&self.s[a..e])
        }
    }
    /// `_\d+` followed by a non-word character (the `\b` that the Python patterns put after it); the `_` is part of the result
    pub fn tmp(&mut self) -> Option<&'a str> {
        let a = self.i;
        if self.b().get(a) != Some(&b'_') {
            return None;
        }
        let e = digits_end(self.b(), a + 1);
        if e == a + 1 || (e < self.s.len() && is_word(self.b()[e])) {
            return None;
        }
        self.i = e;
        Some(&self.s[a..e])
    }
    /// `_\d+` without a boundary test (the pattern continues with a literal)
    pub fn tmp_nb(&mut self) -> Option<&'a str> {
        let a = self.i;
        if self.b().get(a) != Some(&b'_') {
            return None;
        }
        let e = digits_end(self.b(), a + 1);
        if e == a + 1 {
            return None;
        }
        self.i = e;
        Some(&self.s[a..e])
    }
}

/// Python `re.escape`-free substring count (non-overlapping), as `str.count`
pub fn count(h: &str, n: &str) -> usize {
    if n.is_empty() {
        return h.len() + 1;
    }
    let mut c = 0;
    let mut from = 0;
    while let Some(p) = find(h, n, from) {
        c += 1;
        from = p + n.len();
    }
    c
}

/// the number of `\bNAME\b` matches (NAME starts and ends with a word character)
pub fn count_word(h: &str, name: &str) -> usize {
    let b = h.as_bytes();
    let mut c = 0;
    let mut from = 0;
    while let Some(p) = find(h, name, from) {
        let e = p + name.len();
        if !in_word(b, p) && !(e < b.len() && is_word(b[e])) {
            c += 1;
            from = e;
        } else {
            from = p + 1;
        }
    }
    c
}
