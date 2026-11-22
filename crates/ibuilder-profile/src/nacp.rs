//! `dm/tools/nacp.py`: the game name and display version out of `control.nacp`.
//!
//! The title area holds 0x300 bytes per language (name 0x200 + publisher 0x100), 16 of them, or 32 when a newer
//! SDK compressed it (a u16 size + raw deflate, inflating to 32 x 0x300); the display version is 0x10 bytes at
//! 0x3060. The raw-deflate decoder is a small `puff`-style one below (no extra dependency).
use crate::pyutil::py_isprintable;
use std::path::Path;

/// Inflate a raw deflate stream; `Some` only when the stream ends (final block completed) within `max_out`
/// bytes of output. Mirrors `zlib.decompressobj(-15).decompress(data, max_out)` followed by an `eof` check.
pub fn inflate_raw(input: &[u8], max_out: usize) -> Option<Vec<u8>> {
    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
        bit: u32,
        cnt: u32,
    }
    impl Bits<'_> {
        fn need(&mut self, n: u32) -> Option<u32> {
            while self.cnt < n {
                let b = *self.d.get(self.pos)? as u32;
                self.pos += 1;
                self.bit |= b << self.cnt;
                self.cnt += 8;
            }
            let v = self.bit & ((1u32 << n) - 1);
            self.bit >>= n;
            self.cnt -= n;
            Some(v)
        }
    }
    struct Huff {
        count: [u16; 16],
        symbol: Vec<u16>,
    }
    fn build(lengths: &[u8]) -> Option<Huff> {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        let mut left: i32 = 1;
        for &n in &count[1..16] {
            left <<= 1;
            left -= n as i32;
            if left < 0 {
                return None; // over-subscribed
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Some(Huff { count, symbol })
    }
    fn decode(b: &mut Bits, h: &Huff) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.need(1)? as i32;
            let count = h.count[len] as i32;
            if code - count < first {
                return h.symbol.get((index + (code - first)) as usize).copied();
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        None
    }
    const LBASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEXT: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DBASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DEXT: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];

    fn codes(b: &mut Bits, out: &mut Vec<u8>, max: usize, lit: &Huff, dist: &Huff) -> Option<()> {
        loop {
            let sym = decode(b, lit)?;
            if sym < 256 {
                if out.len() >= max {
                    return None;
                }
                out.push(sym as u8);
            } else if sym == 256 {
                return Some(());
            } else {
                let s = (sym - 257) as usize;
                if s >= 29 {
                    return None;
                }
                let len = LBASE[s] as usize + b.need(LEXT[s] as u32)? as usize;
                let ds = decode(b, dist)? as usize;
                if ds >= 30 {
                    return None;
                }
                let d = DBASE[ds] as usize + b.need(DEXT[ds] as u32)? as usize;
                if d > out.len() || out.len() + len > max {
                    return None;
                }
                for _ in 0..len {
                    let c = out[out.len() - d];
                    out.push(c);
                }
            }
        }
    }

    let mut b = Bits {
        d: input,
        pos: 0,
        bit: 0,
        cnt: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.need(1)?;
        match b.need(2)? {
            0 => {
                b.bit = 0;
                b.cnt = 0;
                let len =
                    u16::from_le_bytes(input.get(b.pos..b.pos + 2)?.try_into().ok()?) as usize;
                let nlen =
                    u16::from_le_bytes(input.get(b.pos + 2..b.pos + 4)?.try_into().ok()?) as usize;
                if len != (!nlen & 0xffff) {
                    return None;
                }
                b.pos += 4;
                if out.len() + len > max_out {
                    return None;
                }
                out.extend_from_slice(input.get(b.pos..b.pos + len)?);
                b.pos += len;
            }
            1 => {
                let mut l = [8u8; 288];
                l[144..256].fill(9);
                l[256..280].fill(7);
                let lit = build(&l)?;
                let dist = build(&[5u8; 30])?;
                codes(&mut b, &mut out, max_out, &lit, &dist)?;
            }
            2 => {
                let nlen = b.need(5)? as usize + 257;
                let ndist = b.need(5)? as usize + 1;
                let ncode = b.need(4)? as usize + 4;
                if nlen > 286 || ndist > 30 {
                    return None;
                }
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut lengths = [0u8; 320];
                for &o in ORDER.iter().take(ncode) {
                    lengths[o] = b.need(3)? as u8;
                }
                let lencode = build(&lengths[..19])?;
                let mut idx = 0;
                let mut ll = [0u8; 320];
                while idx < nlen + ndist {
                    let sym = decode(&mut b, &lencode)?;
                    if sym < 16 {
                        ll[idx] = sym as u8;
                        idx += 1;
                    } else {
                        let (val, rep) = match sym {
                            16 => {
                                if idx == 0 {
                                    return None;
                                }
                                (ll[idx - 1], 3 + b.need(2)? as usize)
                            }
                            17 => (0, 3 + b.need(3)? as usize),
                            _ => (0, 11 + b.need(7)? as usize),
                        };
                        if idx + rep > nlen + ndist {
                            return None;
                        }
                        ll[idx..idx + rep].fill(val);
                        idx += rep;
                    }
                }
                if ll[256] == 0 {
                    return None;
                }
                let lit = build(&ll[..nlen])?;
                let dist = build(&ll[nlen..nlen + ndist])?;
                codes(&mut b, &mut out, max_out, &lit, &dist)?;
            }
            _ => return None,
        }
        if last == 1 {
            return Some(out);
        }
    }
}

/// `titles(path)`: the title area's names, one per entry in its order: `Some("")` for an empty entry, `None` for
/// one that is not printable UTF-8. A missing file returns an empty list.
pub fn titles(path: &Path) -> std::io::Result<Vec<Option<String>>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let b = std::fs::read(path)?;
    let mut t: Vec<u8> = b[..b.len().min(0x3000)].to_vec();
    if b.len() >= 2 {
        // newer SDKs compress the title area: a u16 size + raw deflate, inflating to 32 languages x 0x300
        let size = u16::from_le_bytes([b[0], b[1]]) as usize;
        if let Some(z) = inflate_raw(&b[2..(2 + size).min(b.len())], 0x6000 + 1)
            && z.len() == 0x6000
        {
            t = z;
        }
    }
    let mut out = Vec::new();
    for i in 0..t.len() / 0x300 {
        let n = &t[i * 0x300..i * 0x300 + 0x200];
        let n = n.split(|&c| c == 0).next().unwrap_or(&[]);
        if n.is_empty() {
            out.push(Some(String::new()));
            continue;
        }
        out.push(match std::str::from_utf8(n) {
            Ok(s) if py_isprintable(s) => Some(s.to_string()),
            _ => None,
        });
    }
    Ok(out)
}

/// `name(path)`: the first non-empty title (0 = American English); a missing file or no sensible name -> "".
pub fn name(path: &Path) -> std::io::Result<String> {
    Ok(titles(path)?
        .into_iter()
        .find(|n| n.as_deref() != Some(""))
        .flatten()
        .unwrap_or_default())
}

/// `version(path)`: the display version (0x10 bytes at 0x3060), cut to what iOS accepts as a bundle version (up to
/// three dot-separated integers); a missing file or no leading number -> "".
pub fn version(path: &Path) -> std::io::Result<String> {
    if !path.is_file() {
        return Ok(String::new());
    }
    let b = std::fs::read(path)?;
    let s = b.get(0x3060..0x3070.min(b.len())).unwrap_or(&[]);
    // re.match(rb'\d+(\.\d+){0,2}', s)
    let digits = |from: usize| s[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut end = digits(0);
    if end == 0 {
        return Ok(String::new());
    }
    for _ in 0..2 {
        if s.get(end) == Some(&b'.') {
            let d = digits(end + 1);
            if d > 0 {
                end += 1 + d;
                continue;
            }
        }
        break;
    }
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflate_fixed_and_stored() {
        // "hello hello hello" deflated with fixed Huffman (zlib -9 raw)
        let z = [203, 72, 205, 201, 201, 87, 200, 64, 144, 0];
        assert_eq!(inflate_raw(&z, 100).unwrap(), b"hello hello hello");
        assert!(inflate_raw(&z, 5).is_none());
        // stored block
        let s = [1, 3, 0, 0xfc, 0xff, b'a', b'b', b'c'];
        assert_eq!(inflate_raw(&s, 10).unwrap(), b"abc");
        assert!(inflate_raw(&s[..6], 10).is_none());
    }

    #[test]
    fn inflate_dynamic_huffman() {
        // zlib.compressobj(9, DEFLATED, -15) of this text (a dynamic-Huffman block with back-references)
        const Z: &str = "0bc94855282ccd4cce56482aca2fcf5348cbaf50c82acd2d2856c82f4b2d5228014ae72456552aa4e4a7eb29848c2a1e553caa7854f1a8e251c5c34b31032313330b2b1b3b072717370f2f1fbf80a090b088a898b884a494b48cac9cbc82a292b28aaa9aba86a696b68eae9ebe81a191b189a999b985a595b58dad9dbd83a393b38bab9bbb87a797b78faf9f7f4060507048685878446454744c6c5c7c426252724a6a5a7a466656764e6e5e7e4161517149695979456555754d6d5d7d436353734b6b5b7b476757774f6f5fff848993264f993a6dfa8c99b366cf993b6ffe82858b162f59ba6cf98a95ab56af59bb6efd868d9b366fd9ba6dfb8e9dbb76efd9bb6fff8183870e1f397aecf88993a74e9f397beefc858b972e5fb97aedfa8d9bb76edfb97beffe83878f1e3f79faecf98b97af5ebf79fbeefd878f9f3e7ff9faedfb8f9fbf7efff9fbefff48f73f00";
        let z: Vec<u8> = (0..Z.len() / 2)
            .map(|i| u8::from_str_radix(&Z[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let mut text = b"The quick brown fox jumps over the lazy dog. ".repeat(40);
        for _ in 0..2 {
            text.extend(0..=255u8);
        }
        assert_eq!(inflate_raw(&z, text.len() + 1).unwrap(), text);
        assert!(inflate_raw(&z, text.len() - 1).is_none());
        assert!(inflate_raw(&z[..z.len() / 2], 1 << 20).is_none());
    }
}
