//! Raw LZMA1 as Python's `lzma.LZMADecompressor(format=FORMAT_RAW, filters=[{'id': FILTER_LZMA1, ...}]).decompress(data, max_length)` runs it (liblzma): the bytes it hands out for the
//! input it is given, its end-of-payload marker, its errors. Written for GIANTS' SHARC members (`sharc.rs`), whose streams omit the end marker: liblzma then hands out every symbol the input
//! holds whole and waits for more, which `lzma-rs`'s streaming decoder does not (it keeps the last 20 input bytes back).
//!
//! The model: the range decoder normalises before each bit (liblzma's `rc_normalize` at the start of `rc_bit`), so a symbol whose bits the input holds is handed out without a byte after it;
//! a symbol cut by the end of the input is not. Errors (`LZMAError`): a first range-coder byte other than 0, a distance past the bytes decoded so far (or the dictionary), a repeated match
//! before any byte, an end marker whose range coder does not end at 0 (`Corrupt input data`); `lc + lp > 4` or `pb > 4` (`Internal error`).

/// What the decoder ended with: the bytes (at most `max_length`) and whether the end marker was read
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub out: Vec<u8>,
    pub eof: bool,
}

const TOP: u32 = 1 << 24;

struct Rc<'a> {
    d: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

/// the input ended: stop where the last whole symbol ended
struct Starved;

impl Rc<'_> {
    #[inline]
    fn normalize(&mut self) -> Result<(), Starved> {
        if self.range < TOP {
            let Some(&b) = self.d.get(self.pos) else {
                return Err(Starved);
            };
            self.pos += 1;
            self.range <<= 8;
            self.code = (self.code << 8) | b as u32;
        }
        Ok(())
    }

    #[inline]
    fn bit(&mut self, p: &mut u16) -> Result<u32, Starved> {
        self.normalize()?;
        let bound = (self.range >> 11) * *p as u32;
        if self.code < bound {
            self.range = bound;
            *p += (2048 - *p) >> 5;
            Ok(0)
        } else {
            self.range -= bound;
            self.code -= bound;
            *p -= *p >> 5;
            Ok(1)
        }
    }

    fn tree(&mut self, probs: &mut [u16], bits: u32) -> Result<u32, Starved> {
        let mut m = 1usize;
        for _ in 0..bits {
            m = (m << 1) | self.bit(&mut probs[m])? as usize;
        }
        Ok(m as u32 - (1 << bits))
    }

    /// a reverse bit tree whose node `m` (from 1) is `probs[m - shift]`
    fn reverse(&mut self, probs: &mut [u16], bits: u32, shift: usize) -> Result<u32, Starved> {
        let (mut m, mut sym) = (1usize, 0u32);
        for i in 0..bits {
            let b = self.bit(&mut probs[m - shift])?;
            m = (m << 1) | b as usize;
            sym |= b << i;
        }
        Ok(sym)
    }

    fn direct(&mut self, bits: u32) -> Result<u32, Starved> {
        let mut r = 0u32;
        for _ in 0..bits {
            self.normalize()?;
            self.range >>= 1;
            let t = if self.code >= self.range {
                self.code -= self.range;
                1
            } else {
                0
            };
            r = (r << 1) | t;
        }
        Ok(r)
    }
}

struct LenDec {
    choice: u16,
    choice2: u16,
    low: [[u16; 8]; 16],
    mid: [[u16; 8]; 16],
    high: [u16; 256],
}

impl LenDec {
    fn new() -> LenDec {
        LenDec {
            choice: 1024,
            choice2: 1024,
            low: [[1024; 8]; 16],
            mid: [[1024; 8]; 16],
            high: [1024; 256],
        }
    }
    fn decode(&mut self, rc: &mut Rc<'_>, ps: usize) -> Result<u32, Starved> {
        if rc.bit(&mut self.choice)? == 0 {
            return Ok(rc.tree(&mut self.low[ps], 3)? + 2);
        }
        if rc.bit(&mut self.choice2)? == 0 {
            return Ok(rc.tree(&mut self.mid[ps], 3)? + 10);
        }
        Ok(rc.tree(&mut self.high, 8)? + 18)
    }
}

/// One `decompress(data, max_length)` call on a fresh decoder. `Err` is the `LZMAError` text.
pub fn decompress_raw(
    data: &[u8],
    lc: u32,
    lp: u32,
    pb: u32,
    dict_size: u32,
    max_length: usize,
) -> Result<Decoded, String> {
    if lc > 4 || lp > 4 || lc + lp > 4 || pb > 4 {
        return Err("Internal error".into());
    }
    let corrupt = || Err("Corrupt input data".to_string());
    let mut out: Vec<u8> = Vec::new();
    // liblzma's dictionary: at least 4 KiB, a multiple of 16
    let dict = ((dict_size.max(4096) as u64 + 15) & !15) as usize;
    if data.is_empty() {
        return Ok(Decoded { out, eof: false });
    }
    if data[0] != 0 {
        return corrupt();
    }
    if data.len() < 5 {
        return Ok(Decoded { out, eof: false });
    }
    let mut rc = Rc {
        d: data,
        pos: 5,
        range: 0xFFFF_FFFF,
        code: u32::from_be_bytes([data[1], data[2], data[3], data[4]]),
    };
    let (pb_mask, lp_mask) = ((1usize << pb) - 1, (1usize << lp) - 1);
    let mut lit = vec![1024u16; 0x300 << (lc + lp)];
    let mut is_match = [[1024u16; 16]; 12];
    let mut is_rep = [1024u16; 12];
    let mut is_rep0 = [1024u16; 12];
    let mut is_rep1 = [1024u16; 12];
    let mut is_rep2 = [1024u16; 12];
    let mut is_rep0_long = [[1024u16; 16]; 12];
    let mut slot_p = [[1024u16; 64]; 4];
    let mut special = [1024u16; 115];
    let mut align = [1024u16; 16];
    let mut mlen = LenDec::new();
    let mut rlen = LenDec::new();
    let mut state = 0usize;
    let mut reps = [0u32; 4];
    let valid = |out: &Vec<u8>, dist: u32| (dist as usize) < out.len().min(dict);
    while out.len() < max_length {
        // one symbol; `Starved` drops it (and ends the call) as liblzma waits for more input
        let r: Result<Option<bool>, Starved> = (|| {
            let pos = out.len();
            let ps = pos & pb_mask;
            if rc.bit(&mut is_match[state][ps])? == 0 {
                let prev = out.last().copied().unwrap_or(0) as usize;
                let ls = ((pos & lp_mask) << lc) + (prev >> (8 - lc));
                let probs = &mut lit[0x300 * ls..0x300 * (ls + 1)];
                let mut sym = 1usize;
                if state >= 7 {
                    let mut mb = out[pos - reps[0] as usize - 1] as usize;
                    while sym < 0x100 {
                        let mbit = (mb >> 7) & 1;
                        mb <<= 1;
                        let b = rc.bit(&mut probs[0x100 + (mbit << 8) + sym])? as usize;
                        sym = (sym << 1) | b;
                        if mbit != b {
                            break;
                        }
                    }
                }
                while sym < 0x100 {
                    sym = (sym << 1) | rc.bit(&mut probs[sym])? as usize;
                }
                out.push(sym as u8);
                state = if state < 4 {
                    0
                } else if state < 10 {
                    state - 3
                } else {
                    state - 6
                };
                return Ok(Some(true));
            }
            let len;
            if rc.bit(&mut is_rep[state])? == 0 {
                len = mlen.decode(&mut rc, ps)?;
                state = if state < 7 { 7 } else { 10 };
                let ls = (len as usize - 2).min(3);
                let slot = rc.tree(&mut slot_p[ls], 6)?;
                let dist = if slot < 4 {
                    slot
                } else {
                    let n = (slot >> 1) - 1;
                    let base = (2 | (slot & 1)) << n;
                    if slot < 14 {
                        let off = (base - slot) as usize;
                        // liblzma's `pos_special + dist - slot - 1`, indexed from 1
                        base + rc.reverse(&mut special[off..], n, 1)?
                    } else {
                        base + (rc.direct(n - 4)? << 4) + rc.reverse(&mut align[..], 4, 0)?
                    }
                };
                if dist == 0xFFFF_FFFF {
                    // the end-of-payload marker: the range coder must end at 0
                    rc.normalize()?;
                    return Ok(None);
                }
                reps = [dist, reps[0], reps[1], reps[2]];
                if !valid(&out, dist) {
                    return Ok(Some(false));
                }
            } else {
                if !valid(&out, 0) {
                    return Ok(Some(false));
                }
                if rc.bit(&mut is_rep0[state])? == 0 {
                    if rc.bit(&mut is_rep0_long[state][ps])? == 0 {
                        state = if state < 7 { 9 } else { 11 };
                        let b = out[pos - reps[0] as usize - 1];
                        out.push(b);
                        return Ok(Some(true));
                    }
                } else {
                    let dist;
                    if rc.bit(&mut is_rep1[state])? == 0 {
                        dist = reps[1];
                    } else if rc.bit(&mut is_rep2[state])? == 0 {
                        dist = reps[2];
                        reps[2] = reps[1];
                    } else {
                        dist = reps[3];
                        reps[3] = reps[2];
                        reps[2] = reps[1];
                    }
                    reps[1] = reps[0];
                    reps[0] = dist;
                }
                len = rlen.decode(&mut rc, ps)?;
                state = if state < 7 { 8 } else { 11 };
                if !valid(&out, reps[0]) {
                    return Ok(Some(false));
                }
            }
            let from = out.len() - reps[0] as usize - 1;
            for i in 0..len as usize {
                if out.len() >= max_length {
                    break;
                }
                let b = out[from + i];
                out.push(b);
            }
            Ok(Some(true))
        })();
        match r {
            Err(Starved) => return Ok(Decoded { out, eof: false }),
            Ok(Some(true)) => {}
            Ok(Some(false)) => return corrupt(),
            Ok(None) => {
                if rc.code != 0 {
                    return corrupt();
                }
                return Ok(Decoded { out, eof: true });
            }
        }
    }
    Ok(Decoded { out, eof: false })
}
