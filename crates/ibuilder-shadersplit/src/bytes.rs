//! Byte access with the Python's semantics: `struct.unpack_from` fails when the range is not inside the data (here `Err`), a slice `d[a:b]` silently clamps.
//! Nothing in a reader indexes a slice directly: a file that lies about its own lengths is an `Err`, never a panic (the release profile aborts on panic).

use crate::error::{Failure, Res};

pub fn short(what: &str) -> Failure {
    Failure::Red(format!(
        "struct.error: unpack_from requires a buffer of more bytes ({what})"
    ))
}

macro_rules! rd {
    ($name:ident, $t:ty, $n:expr, $from:ident) => {
        pub fn $name(d: &[u8], o: usize) -> Res<$t> {
            match o.checked_add($n).and_then(|e| d.get(o..e)) {
                Some(b) => Ok(<$t>::$from(b.try_into().unwrap())),
                None => Err(short(stringify!($name))),
            }
        }
    };
}
rd!(u16le, u16, 2, from_le_bytes);
rd!(u32le, u32, 4, from_le_bytes);
rd!(u64le, u64, 8, from_le_bytes);
rd!(i32le, i32, 4, from_le_bytes);
rd!(i64le, i64, 8, from_le_bytes);
rd!(u16be, u16, 2, from_be_bytes);
rd!(u32be, u32, 4, from_be_bytes);
rd!(u64be, u64, 8, from_be_bytes);
rd!(i32be, i32, 4, from_be_bytes);
rd!(i64be, i64, 8, from_be_bytes);

pub fn u8at(d: &[u8], o: usize) -> Res<u8> {
    d.get(o).copied().ok_or_else(|| short("u8"))
}

/// `d[a:b]` as Python slices it (both ends non-negative): clamped, never an error
pub fn sl(d: &[u8], a: usize, b: usize) -> &[u8] {
    let b = b.min(d.len());
    let a = a.min(b);
    &d[a..b]
}

/// `d[a:a+n]`
pub fn at(d: &[u8], a: usize, n: usize) -> &[u8] {
    sl(d, a, a.saturating_add(n))
}

/// `span(d, at, n, where)`: the range or `red()` naming where when it is not inside d
pub fn span<'a>(d: &'a [u8], a: usize, n: usize, wh: &str) -> Res<&'a [u8]> {
    if a <= d.len() && n <= d.len() - a {
        Ok(&d[a..a + n])
    } else {
        Err(Failure::Red(format!(
            "{wh}: {n:#x} bytes at {a:#x} lie outside the {:#x} bytes there are",
            d.len()
        )))
    }
}

/// `field(d, fmt, at, where)` for the sizes the readers ask: check that `n` bytes at `a` are inside d (the unpack follows with the `rd!` readers)
pub fn field_ok(d: &[u8], a: usize, n: usize, wh: &str) -> Res<()> {
    if a <= d.len() && n <= d.len() - a {
        Ok(())
    } else {
        Err(Failure::Red(format!(
            "{wh}: {n:#x} bytes at {a:#x} lie outside the {:#x} bytes there are",
            d.len()
        )))
    }
}

/// `d.find(sub, start)`: the first offset >= start, or None
pub fn find(d: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if start > d.len() {
        return None;
    }
    memchr::memmem::find(&d[start..], needle).map(|p| p + start)
}

/// `d.find(sub, start, end)`: the match must lie inside d[start:end]
pub fn find_in(d: &[u8], needle: &[u8], start: usize, end: usize) -> Option<usize> {
    let end = end.min(d.len());
    if start > end {
        return None;
    }
    memchr::memmem::find(&d[start..end], needle).map(|p| p + start)
}

pub fn contains(d: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(d, needle).is_some()
}

/// `re.finditer(re.escape(needle), d)` offsets, in order (the needles used here cannot overlap themselves)
pub fn find_all<'a>(d: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    memchr::memmem::find_iter(d, needle)
}

/// a bytes literal's prefix test `d[:n] == lit`
pub fn starts(d: &[u8], lit: &[u8]) -> bool {
    d.len() >= lit.len() && &d[..lit.len()] == lit
}

/// `d[a:b] == lit` (the slice may be shorter than lit near the end: then False, as in Python)
pub fn eq_at(d: &[u8], a: usize, lit: &[u8]) -> bool {
    at(d, a, lit.len()) == lit
}

/// Python's `bytes.count(sub)`: non-overlapping, left to right
pub fn count_nonoverlapping(d: &[u8], needle: &[u8]) -> usize {
    let (mut n, mut p) = (0, 0);
    while let Some(i) = find(d, needle, p) {
        n += 1;
        p = i + needle.len();
    }
    n
}

pub fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(char::from_digit((x >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((x & 15) as u32, 16).unwrap());
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|c| Some((char::from(c[0]).to_digit(16)? * 16 + char::from(c[1]).to_digit(16)?) as u8))
        .collect()
}
