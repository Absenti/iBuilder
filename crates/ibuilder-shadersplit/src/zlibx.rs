//! zlib's `inflate()` as Python's `zlib.decompressobj().decompress(data[, max_length])` runs it, byte for byte: the output, whether the stream ended (`.eof`), how many input bytes it took
//! (`len(data) - len(.unused_data)`), and the error text of a damaged stream (`Error -3 while decompressing data: invalid block type`).
//!
//! `inflate.rs` (flate2 over `miniz_oxide`) is the fast general reader. Two things in the Snowdrop bare-stream reader (`archive::snowdrop_zlib`, `paches/engine/0104`) depend on zlib's own behaviour, which
//! `miniz_oxide` does not copy: (1) `decompress(head, 20)` stops when 20 bytes are out, so a damaged symbol *after* them, inside the 256 bytes of input, is no error there, while a decoder
//! that works ahead of the output reports one (and the stream is skipped instead of read); (2) the text of the error, which the reader puts into its `[RED]` line.
//! This is the slow path of zlib (no `inflate_fast`: the two give the same results) translated function by function; it is used for those two cases only, on at most 64 KB + 0x500 of input at a time.
//!
//! Limits of the model, all taken from zlib 1.2.12 - 1.3 (`inflate.c`, `inftrees.c`): the order of the checks (a stored block's length check, a dynamic block's table checks, "invalid distance too far back" after
//! the `left == 0` test, the data check after the last block), the code sets zlib refuses (over-subscribed, incomplete unless a lone 1-bit code, the code-length code must be complete),
//! the all-zero code-length code (every bit is a length 0), symbols 286 / 287 and distance symbols 30 / 31, a preset dictionary (`Error 2`), and `Adler-32`.

/// What a call of `decompress` left behind
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Z {
    /// the decoded bytes (at most `max_length`)
    pub out: Vec<u8>,
    /// the stream's last byte (its Adler-32) was read: `decompressobj.eof`
    pub eof: bool,
    /// input bytes the stream took when `eof` (the rest of `data` is `unused_data`); otherwise the bytes read
    pub used: usize,
}

enum Stop {
    /// `strm->msg` of a damaged stream (Z_DATA_ERROR)
    Data(&'static str),
    /// Z_NEED_DICT: the header names a preset dictionary
    NeedDict,
}

enum Flow {
    Done,
    /// the input ended inside a symbol or a header: not an error, `eof` stays false
    Trunc,
    /// `max_length` bytes are out
    Limit,
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    hold: u64,
    n: u32,
}

impl Bits<'_> {
    /// `NEEDBITS(n)` + `BITS(n)` + `DROPBITS(n)` (n <= 32); None when the input ends first
    fn bits(&mut self, n: u32) -> Option<u32> {
        while self.n < n {
            let b = *self.d.get(self.pos)?;
            self.hold |= (b as u64) << self.n;
            self.n += 8;
            self.pos += 1;
        }
        let v = (self.hold & ((1u64 << n) - 1)) as u32;
        self.hold >>= n;
        self.n -= n;
        Some(v)
    }
    /// `BYTEBITS()`: drop the bits of the byte in progress
    fn align(&mut self) {
        let k = self.n & 7;
        self.hold >>= k;
        self.n -= k;
    }
}

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// A canonical Huffman code as `inflate_table` accepts it
struct Huff {
    count: [u16; 16],
    sym: Vec<u16>,
    max: usize,
}

enum Dec {
    Sym(usize),
    Trunc,
    /// a code no symbol has (an incomplete set), or the table of no symbols at all
    Invalid,
}

impl Huff {
    /// `inflate_table`: None where it returns an error (over-subscribed; incomplete, except a lone 1-bit code of a literal / distance set; a code-length set must be complete)
    fn build(lens: &[u8], code_lengths: bool) -> Option<Huff> {
        let mut count = [0u16; 16];
        for &l in lens {
            count[l as usize] += 1;
        }
        let mut max = 15;
        while max >= 1 && count[max] == 0 {
            max -= 1;
        }
        if max == 0 {
            return Some(Huff {
                count,
                sym: Vec::new(),
                max: 0,
            });
        }
        let mut left: i32 = 1;
        for &c in &count[1..16] {
            left <<= 1;
            left -= c as i32;
            if left < 0 {
                return None;
            }
        }
        if left > 0 && (code_lengths || max != 1) {
            return None;
        }
        let mut offs = [0u16; 16];
        for l in 1..15 {
            offs[l + 1] = offs[l] + count[l];
        }
        let mut sym = vec![0u16; lens.len()];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Some(Huff { count, sym, max })
    }

    fn decode(&self, b: &mut Bits<'_>) -> Dec {
        if self.max == 0 {
            return if b.bits(1).is_none() {
                Dec::Trunc
            } else {
                Dec::Invalid
            };
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for l in 1..=self.max {
            match b.bits(1) {
                Some(bit) => code |= bit as i32,
                None => return Dec::Trunc,
            }
            let c = self.count[l] as i32;
            if code - c < first {
                return Dec::Sym(self.sym[(index + (code - first)) as usize] as usize);
            }
            index += c;
            first = (first + c) << 1;
            code <<= 1;
        }
        Dec::Invalid
    }
}

/// Adler-32 (the zlib trailer, big-endian)
pub fn adler32(d: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for ch in d.chunks(5552) {
        for &x in ch {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    b << 16 | a
}

fn fixed() -> (Huff, Huff) {
    let mut l = [8u8; 288];
    l[144..256].fill(9);
    l[256..280].fill(7);
    (
        Huff::build(&l, false).unwrap(),
        Huff::build(&[5u8; 32], false).unwrap(),
    )
}

/// A dynamic block's two code sets (the TABLE state of `inflate()`); `Ok(None)` when the input ends first
fn tables(b: &mut Bits<'_>) -> Result<Option<(Huff, Huff)>, Stop> {
    macro_rules! get {
        ($n:expr) => {
            match b.bits($n) {
                Some(v) => v,
                None => return Ok(None),
            }
        };
    }
    let nlen = get!(5) as usize + 257;
    let ndist = get!(5) as usize + 1;
    let ncode = get!(4) as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(Stop::Data("too many length or distance symbols"));
    }
    let mut lens = [0u8; 320];
    for &o in ORDER.iter().take(ncode) {
        lens[o] = get!(3) as u8;
    }
    let cl = Huff::build(&lens[..19], true).ok_or(Stop::Data("invalid code lengths set"))?;
    lens = [0u8; 320];
    let mut have = 0usize;
    while have < nlen + ndist {
        // a table of no symbols still answers every 1-bit code with the value 0 here (`here.val < 16`: the entry's value is not looked at when its op is 64)
        let s = if cl.max == 0 {
            get!(1);
            0
        } else {
            match cl.decode(b) {
                Dec::Sym(s) => s,
                Dec::Trunc => return Ok(None),
                Dec::Invalid => unreachable!("a complete code has no invalid entry"),
            }
        };
        if s < 16 {
            lens[have] = s as u8;
            have += 1;
            continue;
        }
        let (len, copy) = match s {
            16 => {
                let e = get!(2) as usize;
                if have == 0 {
                    return Err(Stop::Data("invalid bit length repeat"));
                }
                (lens[have - 1], 3 + e)
            }
            17 => (0, 3 + get!(3) as usize),
            _ => (0, 11 + get!(7) as usize),
        };
        if have + copy > nlen + ndist {
            return Err(Stop::Data("invalid bit length repeat"));
        }
        for _ in 0..copy {
            lens[have] = len;
            have += 1;
        }
    }
    if lens[256] == 0 {
        return Err(Stop::Data("invalid code -- missing end-of-block"));
    }
    let lit = Huff::build(&lens[..nlen], false).ok_or(Stop::Data("invalid literal/lengths set"))?;
    let dist =
        Huff::build(&lens[nlen..nlen + ndist], false).ok_or(Stop::Data("invalid distances set"))?;
    Ok(Some((lit, dist)))
}

/// The symbols of one Huffman block (the LEN, LENEXT, DIST, DISTEXT, MATCH and LIT states): `Done` at the end-of-block code
fn symbols(
    b: &mut Bits<'_>,
    out: &mut Vec<u8>,
    lim: usize,
    lit: &Huff,
    dist: &Huff,
) -> Result<Flow, Stop> {
    loop {
        let sym = match lit.decode(b) {
            Dec::Sym(s) => s,
            Dec::Trunc => return Ok(Flow::Trunc),
            Dec::Invalid => return Err(Stop::Data("invalid literal/length code")),
        };
        if sym < 256 {
            if out.len() >= lim {
                return Ok(Flow::Limit); // LIT: `left == 0`, after the code was looked up
            }
            out.push(sym as u8);
            continue;
        }
        if sym == 256 {
            return Ok(Flow::Done);
        }
        if sym > 285 {
            return Err(Stop::Data("invalid literal/length code"));
        }
        let i = sym - 257;
        let mut len = LBASE[i] as usize;
        if LEXT[i] != 0 {
            match b.bits(LEXT[i] as u32) {
                Some(v) => len += v as usize,
                None => return Ok(Flow::Trunc),
            }
        }
        let ds = match dist.decode(b) {
            Dec::Sym(s) => s,
            Dec::Trunc => return Ok(Flow::Trunc),
            Dec::Invalid => return Err(Stop::Data("invalid distance code")),
        };
        if ds >= 30 {
            return Err(Stop::Data("invalid distance code"));
        }
        let mut off = DBASE[ds] as usize;
        if DEXT[ds] != 0 {
            match b.bits(DEXT[ds] as u32) {
                Some(v) => off += v as usize,
                None => return Ok(Flow::Trunc),
            }
        }
        if out.len() >= lim {
            return Ok(Flow::Limit); // MATCH: `left == 0`, before the distance is tested
        }
        if off > out.len() {
            return Err(Stop::Data("invalid distance too far back"));
        }
        for _ in 0..len {
            if out.len() >= lim {
                return Ok(Flow::Limit);
            }
            let c = out[out.len() - off];
            out.push(c);
        }
    }
}

fn run(b: &mut Bits<'_>, out: &mut Vec<u8>, lim: usize) -> Result<Flow, Stop> {
    // HEAD
    let h = match b.bits(16) {
        Some(h) => h,
        None => return Ok(Flow::Trunc),
    };
    let (cmf, flg) = (h & 0xff, h >> 8);
    if (cmf << 8 | flg) % 31 != 0 {
        return Err(Stop::Data("incorrect header check"));
    }
    if cmf & 15 != 8 {
        return Err(Stop::Data("unknown compression method"));
    }
    if (cmf >> 4) + 8 > 15 {
        return Err(Stop::Data("invalid window size"));
    }
    if flg & 0x20 != 0 {
        return match b.bits(32) {
            Some(_) => Err(Stop::NeedDict),
            None => Ok(Flow::Trunc),
        };
    }
    let mut last = false;
    loop {
        // TYPE
        if last {
            b.align();
            let a = match b.bits(32) {
                Some(a) => a.swap_bytes(),
                None => return Ok(Flow::Trunc),
            };
            if a != adler32(out) {
                return Err(Stop::Data("incorrect data check"));
            }
            return Ok(Flow::Done);
        }
        let t = match b.bits(3) {
            Some(t) => t,
            None => return Ok(Flow::Trunc),
        };
        last = t & 1 != 0;
        match t >> 1 {
            0 => {
                b.align();
                let v = match b.bits(32) {
                    Some(v) => v,
                    None => return Ok(Flow::Trunc),
                };
                let (len, nlen) = (v & 0xffff, v >> 16);
                if len != nlen ^ 0xffff {
                    return Err(Stop::Data("invalid stored block lengths"));
                }
                let mut rest = len as usize;
                while rest > 0 {
                    let copy = rest
                        .min(b.d.len() - b.pos)
                        .min(lim.saturating_sub(out.len()));
                    if copy == 0 {
                        return Ok(if out.len() >= lim {
                            Flow::Limit
                        } else {
                            Flow::Trunc
                        });
                    }
                    out.extend_from_slice(&b.d[b.pos..b.pos + copy]);
                    b.pos += copy;
                    rest -= copy;
                }
            }
            1 => {
                let (lit, dist) = fixed();
                match symbols(b, out, lim, &lit, &dist)? {
                    Flow::Done => {}
                    f => return Ok(f),
                }
            }
            2 => {
                let (lit, dist) = match tables(b)? {
                    Some(t) => t,
                    None => return Ok(Flow::Trunc),
                };
                match symbols(b, out, lim, &lit, &dist)? {
                    Flow::Done => {}
                    f => return Ok(f),
                }
            }
            _ => return Err(Stop::Data("invalid block type")),
        }
    }
}

/// `zlib.decompressobj().decompress(data)` / `.decompress(data, max_length)`, one call: the output, `eof` and the bytes used, or the error text Python raises
pub fn decompress(data: &[u8], max_length: Option<usize>) -> Result<Z, String> {
    let mut b = Bits {
        d: data,
        pos: 0,
        hold: 0,
        n: 0,
    };
    let mut out = Vec::new();
    match run(&mut b, &mut out, max_length.unwrap_or(usize::MAX)) {
        Ok(Flow::Done) => Ok(Z {
            out,
            eof: true,
            used: b.pos,
        }),
        Ok(_) => Ok(Z {
            out,
            eof: false,
            used: b.pos,
        }),
        Err(Stop::Data(m)) => Err(format!("Error -3 while decompressing data: {m}")),
        Err(Stop::NeedDict) => Err("Error 2 while decompressing data".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    fn z(data: &[u8], level: u32) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::new(level));
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn round_trips_stored_fixed_and_dynamic_blocks() {
        let mut x = 7u32;
        let rnd: Vec<u8> = (0..70000)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                (x >> 24) as u8
            })
            .collect();
        let text: Vec<u8> = b"the quick brown fox jumps over the lazy dog "
            .iter()
            .cycle()
            .take(50000)
            .copied()
            .collect();
        for (name, data) in [
            ("empty", Vec::new()),
            ("tiny", b"abc".to_vec()),
            ("random", rnd),
            ("text", text),
        ] {
            for level in [0, 1, 6, 9] {
                let c = z(&data, level);
                let mut tail = c.clone();
                tail.extend_from_slice(b"trailing");
                let r = decompress(&tail, None).unwrap();
                assert!(r.eof, "{name} level {level}");
                assert_eq!(r.out, data, "{name} level {level}");
                assert_eq!(r.used, c.len(), "{name} level {level}: bytes used");
                // cut short: not an error, not eof
                let r = decompress(&c[..c.len() - 1], None).unwrap();
                assert!(
                    !r.eof && r.out.len() <= data.len(),
                    "{name} level {level}: truncated"
                );
                // `max_length`
                let r = decompress(&c, Some(20)).unwrap();
                assert_eq!(
                    r.out,
                    data[..data.len().min(20)],
                    "{name} level {level}: 20 bytes"
                );
            }
        }
    }

    #[test]
    fn error_texts_are_zlibs() {
        assert_eq!(
            decompress(&[0x78, 0x9d], None).unwrap_err(),
            "Error -3 while decompressing data: incorrect header check"
        );
        assert_eq!(
            decompress(&[0x79, 0x18], None).unwrap_err(),
            "Error -3 while decompressing data: unknown compression method"
        );
        assert_eq!(
            decompress(&[0x88, 0x1c], None).unwrap_err(),
            "Error -3 while decompressing data: invalid window size"
        );
        assert_eq!(
            decompress(&[0x78, 0xbb, 0, 0, 0, 0], None).unwrap_err(),
            "Error 2 while decompressing data"
        );
        assert!(
            decompress(&[0x78, 0xbb, 0, 0], None)
                .unwrap()
                .out
                .is_empty()
        ); // the dictionary id is not all there yet
        assert_eq!(
            decompress(&[0x78, 0x9c, 0x07], None).unwrap_err(),
            "Error -3 while decompressing data: invalid block type"
        );
        assert_eq!(
            decompress(&[0x78, 0x9c, 0x01, 0x01, 0x00, 0x00, 0x00], None).unwrap_err(),
            "Error -3 while decompressing data: invalid stored block lengths"
        );
        let mut c = z(b"hello hello hello", 9);
        let n = c.len();
        c[n - 1] ^= 1;
        assert_eq!(
            decompress(&c, None).unwrap_err(),
            "Error -3 while decompressing data: incorrect data check"
        );
    }
}
