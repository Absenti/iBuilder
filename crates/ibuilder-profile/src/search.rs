//! Byte-pattern scans over a module image (up to ~100 MB): the `bytes.find / count / re.findall` calls of
//! `profile_guess.facts_of`, each reproduced with the semantics of the Python regex it replaces.
//!
//! The literal search anchors on the needle's rarest byte and scans for it in 32-byte blocks that auto-vectorize
//! (std has no fast `&[u8]` substring search and the `memchr` crate is not on the allow-list).

/// Position of the first `x` in `hay[from..]`. 32-byte blocks are tested without an early exit so the loop
/// vectorizes (~10 GB/s when the byte is rare), then the hit block is scanned.
fn find_byte(hay: &[u8], from: usize, x: u8) -> Option<usize> {
    let mut i = from;
    while i + 32 <= hay.len() {
        let hit = hay[i..i + 32].iter().fold(false, |a, &b| a | (b == x));
        if hit {
            return hay[i..i + 32].iter().position(|&b| b == x).map(|p| p + i);
        }
        i += 32;
    }
    hay[i.min(hay.len())..]
        .iter()
        .position(|&b| b == x)
        .map(|p| p + i)
}

/// How common a byte is in code/data images (lower = rarer): the anchor of a literal search is its rarest byte.
fn commonness(b: u8) -> u32 {
    match b {
        0 => 1000,
        0xff => 900,
        b' ' => 300,
        b'e' | b'a' | b'o' | b'i' | b't' | b'n' | b's' | b'r' => 260,
        b'a'..=b'z' => 200,
        0..=0x1f | 0x80..=0xfe => 500,
        b'0'..=b'9' => 150,
        b'A'..=b'Z' => 100,
        _ => 80,
    }
}

fn anchor_of(needle: &[u8]) -> usize {
    (0..needle.len())
        .min_by_key(|&i| commonness(needle[i]))
        .unwrap_or(0)
}

/// First occurrence of `needle` at or after `from`.
pub fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let n = needle.len();
    if n == 0 {
        return (from <= hay.len()).then_some(from);
    }
    if hay.len() < n || from > hay.len() - n {
        return None;
    }
    let a = anchor_of(needle);
    let mut pos = from + a; // position of the anchor byte for a match starting at `from`
    let last_anchor = hay.len() - n + a;
    while pos <= last_anchor {
        let p = find_byte(hay, pos, needle[a])?;
        if p > last_anchor {
            return None;
        }
        let start = p - a;
        if hay[start..start + n] == *needle {
            return Some(start);
        }
        pos = p + 1;
    }
    None
}

/// ASCII case-insensitive `find` (the needle must be given in lower case and hold a non-letter byte to anchor on).
pub fn find_ci(hay: &[u8], needle_lower: &[u8], from: usize) -> Option<usize> {
    let n = needle_lower.len();
    if hay.len() < n || from > hay.len() - n {
        return None;
    }
    let a = needle_lower
        .iter()
        .position(|b| !b.is_ascii_alphabetic())
        .expect("a non-letter anchor");
    let mut pos = from + a;
    let last_anchor = hay.len() - n + a;
    while pos <= last_anchor {
        let p = find_byte(hay, pos, needle_lower[a])?;
        if p > last_anchor {
            return None;
        }
        let start = p - a;
        if hay[start..start + n]
            .iter()
            .zip(needle_lower)
            .all(|(x, y)| x.to_ascii_lowercase() == *y)
        {
            return Some(start);
        }
        pos = p + 1;
    }
    None
}

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle, 0).is_some()
}

/// `bytes.count(needle)`: non-overlapping occurrences.
pub fn count(hay: &[u8], needle: &[u8]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while let Some(p) = find(hay, needle, i) {
        n += 1;
        i = p + needle.len();
    }
    n
}

/// Regex `\w` on bytes: `[a-zA-Z0-9_]`.
fn is_w(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn run(h: &[u8], from: usize, f: impl Fn(u8) -> bool) -> usize {
    let mut j = from;
    while j < h.len() && f(h[j]) {
        j += 1;
    }
    j
}

/// `re.findall(rb'NintendoSDK_(?!nnSdk)\w+-[0-9_]+', img)` reduced to `rsplit('-', 1)[0]`: the component name
/// (`NintendoSDK_<word>`, everything before the single `-`). Non-overlapping, leftmost.
pub fn nintendo_sdk_components(img: &[u8]) -> Vec<Vec<u8>> {
    const LIT: &[u8] = b"NintendoSDK_";
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find(img, LIT, pos) {
        let a = s + LIT.len();
        if img[a..].starts_with(b"nnSdk") {
            pos = s + 1;
            continue;
        }
        let w_end = run(img, a, is_w);
        if w_end > a && img.get(w_end) == Some(&b'-') {
            let d_end = run(img, w_end + 1, |b| b.is_ascii_digit() || b == b'_');
            if d_end > w_end + 1 {
                out.push(img[s..w_end].to_vec());
                pos = d_end;
                continue;
            }
        }
        pos = s + 1;
    }
    out
}

/// `re.findall(rb'SDK MW\+Nintendo\+([\w]+-[0-9_]+)', img)`: the captured `<word>-<digits>`.
pub fn sdk_mw_components(img: &[u8]) -> Vec<Vec<u8>> {
    const LIT: &[u8] = b"SDK MW+Nintendo+";
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find(img, LIT, pos) {
        let a = s + LIT.len();
        let w_end = run(img, a, is_w);
        if w_end > a && img.get(w_end) == Some(&b'-') {
            let d_end = run(img, w_end + 1, |b| b.is_ascii_digit() || b == b'_');
            if d_end > w_end + 1 {
                out.push(img[a..d_end].to_vec());
                pos = d_end;
                continue;
            }
        }
        pos = s + 1;
    }
    out
}

/// `re.findall(rb'(?i)NintendoSDK_nnSdk-(?:[a-z]+_)?([0-9_]+)', img)`: the captured digit groups.
pub fn sdk_versions(img: &[u8]) -> Vec<Vec<u8>> {
    const LIT: &[u8] = b"nintendosdk_nnsdk-";
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find_ci(img, LIT, pos) {
        let a = s + LIT.len();
        let isdig_ = |b: u8| b.is_ascii_digit() || b == b'_';
        // first try the optional `[a-z]+_` group (case-insensitive), then the bare digits
        let l_end = run(img, a, |b| b.is_ascii_alphabetic());
        let mut found = None;
        if l_end > a && img.get(l_end) == Some(&b'_') {
            let d_end = run(img, l_end + 1, isdig_);
            if d_end > l_end + 1 {
                found = Some((l_end + 1, d_end));
            }
        }
        if found.is_none() {
            let d_end = run(img, a, isdig_);
            if d_end > a {
                found = Some((a, d_end));
            }
        }
        match found {
            Some((x, y)) => {
                out.push(img[x..y].to_vec());
                pos = y;
            }
            None => pos = s + 1,
        }
    }
    out
}

/// `re.findall(rb'20[12][0-9]\.[0-9]+\.[0-9]+f[0-9]', img)`: Unity version strings (`2022.3.12f1`).
pub fn unity_versions(img: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(s) = find(img, b"20", pos) {
        let m = (|| {
            let b = img.get(s + 2..s + 4)?;
            if !(matches!(b[0], b'1' | b'2') && b[1].is_ascii_digit()) {
                return None;
            }
            let mut j = s + 4;
            if *img.get(j)? != b'.' {
                return None;
            }
            j += 1;
            let e = run(img, j, |c| c.is_ascii_digit());
            if e == j || img.get(e) != Some(&b'.') {
                return None;
            }
            let e2 = run(img, e + 1, |c| c.is_ascii_digit());
            if e2 == e + 1 || img.get(e2) != Some(&b'f') || !img.get(e2 + 1)?.is_ascii_digit() {
                return None;
            }
            Some(e2 + 2)
        })();
        match m {
            Some(end) => {
                out.push(img[s..end].to_vec());
                pos = end;
            }
            None => pos = s + 1,
        }
    }
    out
}

/// `len(re.findall(rb'\0#version [0-9]', img))`: non-overlapping.
pub fn count_gl_src(img: &[u8]) -> usize {
    const LIT: &[u8] = b"\0#version ";
    let mut n = 0;
    let mut pos = 0;
    while let Some(s) = find(img, LIT, pos) {
        let a = s + LIT.len();
        if img.get(a).is_some_and(u8::is_ascii_digit) {
            n += 1;
            pos = a + 1;
        } else {
            pos = s + 1;
        }
    }
    n
}

fn is_s(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x0b)
}

/// `re.search(rb'\bvoid\s+main\s*\(', s)`
fn has_void_main(s: &[u8]) -> bool {
    let mut pos = 0;
    while let Some(p) = find(s, b"void", pos) {
        pos = p + 1;
        if p > 0 && is_w(s[p - 1]) {
            continue;
        }
        let a = run(s, p + 4, is_s);
        if a == p + 4 || !s[a..].starts_with(b"main") {
            continue;
        }
        let b = run(s, a + 4, is_s);
        if s.get(b) == Some(&b'(') {
            return true;
        }
    }
    false
}

/// `sum(bool(re.search(rb'\bvoid\s+main\s*\(', s)) for s in re.findall(rb'#version [^\0]*', img))`
pub fn glsl_sources(img: &[u8]) -> usize {
    const LIT: &[u8] = b"#version ";
    let mut n = 0;
    let mut pos = 0;
    while let Some(s) = find(img, LIT, pos) {
        let e = img[s..]
            .iter()
            .position(|&b| b == 0)
            .map_or(img.len(), |p| s + p);
        if has_void_main(&img[s..e]) {
            n += 1;
        }
        pos = e.max(s + 1);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_search() {
        let h = b"xxabcabcabdabc";
        assert_eq!(find(h, b"abd", 0), Some(8));
        assert_eq!(find(h, b"abc", 3), Some(5));
        assert_eq!(count(h, b"abc"), 3);
        assert_eq!(count(b"aaaa", b"aa"), 2);
        assert_eq!(find(h, b"zz", 0), None);
        assert_eq!(
            find_ci(b"..NINTENDOSDK_NNSDK-x", b"nintendosdk_nnsdk-", 0),
            Some(2)
        );
    }

    #[test]
    fn components_and_versions() {
        let img = b"..NintendoSDK_Vulkan-1_2_3 NintendoSDK_nnSdk-12_0_1 NintendoSDK_OpenGL- NintendoSDK_X_y-9\0SDK MW+Nintendo+movie-4_0 NintendoSdk_nnSdk-Generic_22_2_0 NINTENDOSDK_NNSDK-_5";
        let c: Vec<String> = nintendo_sdk_components(img)
            .into_iter()
            .map(|v| String::from_utf8(v).unwrap())
            .collect();
        assert_eq!(c, vec!["NintendoSDK_Vulkan", "NintendoSDK_X_y"]);
        let m: Vec<String> = sdk_mw_components(img)
            .into_iter()
            .map(|v| String::from_utf8(v).unwrap())
            .collect();
        assert_eq!(m, vec!["movie-4_0"]);
        let v: Vec<String> = sdk_versions(img)
            .into_iter()
            .map(|v| String::from_utf8(v).unwrap())
            .collect();
        assert_eq!(v, vec!["12_0_1", "22_2_0", "_5"]);
    }

    #[test]
    fn glsl() {
        let img = b"\0#version 450\nvoid  main (){}\0#version 300 es\nint x;\0#version 100 void main()\0x#version 1";
        assert_eq!(glsl_sources(img), 2);
        assert_eq!(count_gl_src(img), 3);
        assert!(!has_void_main(b"avoid main()"));
        assert_eq!(
            unity_versions(b"zz2022.3.12f1 2019.4.0f1x 2030.1.1f1 20.1"),
            vec![b"2022.3.12f1".to_vec(), b"2019.4.0f1".to_vec()]
        );
    }
}
