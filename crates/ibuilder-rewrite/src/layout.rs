//! `mod_layout.relayout_module` (nxnative 17454fc3 + 65dfeb33): give a module's writable data a 16 KB page of its own.
//!
//! Horizon maps modules at 4 KB, the Mac and iOS hosts at 16 KB: the loader seals `[0, text_end rounded up to 16 KB)` read + execute (and the iOS executable has that range in
//! `__TEXT`), so a module whose data starts below that boundary has read-only data and the first `R_AARCH64_RELATIVE` into it stops the game with SIGBUS in `relocate()`
//! (FINAL FANTASY VIII Remastered's Math.nro: text, ro and data of 4 KB each, data at 0x2000; its Platform.nro has data at 0x3000). Data and bss move up by whole 4 KB pages (D);
//! text and read-only data stay where they are, so the linker's relative references between them (signed jump-table entries among them) keep working. What crosses into or
//! out of the moved part changes by D: the NRO header (module size, ro size, data offset), the MOD0 offset at +4 and MOD0's offsets, the dynamic section's address tags, symbol
//! values (not `STT_TLS`), RELA / JMPREL offsets and RELATIVE (and symbol-less ABS64) addends, DT_RELR offsets and slot contents, the text's adrp / adr / literal loads into the
//! moved part (not the MOD0 offset at +4, MOD0 itself in text, the data blocks in text and the literal pools in-text literal loads read), and the pointers of
//! `.eh_frame_hdr` / `.eh_frame` (and an LSDA's explicit lpstart). A module whose data already sits on its own 16 KB page (every module the rewriter saw before) is returned
//! unchanged: the rewrite of such a module is byte-identical to the one without this step.
//!
//! The rewriter calls [`relayout_module`] right after loading the module (`patch_nso.py`, after `nso.load()`); the words it changes in text keep their pre-move value as the
//! original (`ORIG_TEXT` is taken before the move), so they are listed in `changed.tsv`.

use ibuilder_nso::Module;

const PAGE: i128 = 0x1000;
const HOST_PAGE: i128 = 0x4000;
/// PLTGOT HASH STRTAB SYMTAB RELA INIT FINI REL JMPREL INIT/FINI/PREINIT_ARRAY RELR GNU_HASH VERSYM VERDEF VERNEED
const PTR_TAGS: [i128; 17] = [
    3, 4, 5, 6, 7, 12, 13, 17, 23, 25, 26, 32, 36, 0x6ffffef5, 0x6ffffff0, 0x6ffffffc, 0x6ffffffe,
];
const R_ABS64: u64 = 257;
const R_GLOB_DAT: u64 = 1025;
const R_JUMP_SLOT: u64 = 1026;
const R_RELATIVE: u64 = 1027;

/// `D` (bytes, a multiple of 4 KB) that moves data to a 16 KB page past the code, 0 when it already is
pub fn shift_needed(text_end: u64, data_off: u64) -> u64 {
    let rx = (text_end + HOST_PAGE as u64 - 1) & !(HOST_PAGE as u64 - 1);
    rx.saturating_sub(data_off)
}

/// Python's `f'{v:#x}'` (a negative value is `-0x...`)
fn px(v: i128) -> String {
    if v < 0 {
        format!("-{:#x}", -v)
    } else {
        format!("{v:#x}")
    }
}

/// Python's `repr()` of a bytes object
fn py_bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    let mut s = format!("b{quote}");
    for &c in b {
        match c {
            b'\\' => s.push_str("\\\\"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            c if c as char == quote => {
                s.push('\\');
                s.push(c as char);
            }
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push(quote);
    s
}

/// a struct format of the move: unsigned / signed, size in bytes
#[derive(Clone, Copy)]
enum F {
    U32,
    I32,
    U64,
    I64,
    U16,
}

impl F {
    fn size(self) -> usize {
        match self {
            F::U16 => 2,
            F::U32 | F::I32 => 4,
            F::U64 | F::I64 => 8,
        }
    }
    fn range(self) -> (i128, i128) {
        match self {
            F::U16 => (0, u16::MAX as i128),
            F::U32 => (0, u32::MAX as i128),
            F::I32 => (i32::MIN as i128, i32::MAX as i128),
            F::U64 => (0, u64::MAX as i128),
            F::I64 => (i64::MIN as i128, i64::MAX as i128),
        }
    }
}

fn get(b: &[u8], o: i128, f: F) -> Result<i128, String> {
    let n = f.size();
    let s = usize::try_from(o)
        .ok()
        .and_then(|o| b.get(o..o.checked_add(n)?))
        .ok_or_else(|| format!("a {n}-byte read at {} is outside the module", px(o)))?;
    Ok(match f {
        F::U16 => u16::from_le_bytes(s.try_into().unwrap()) as i128,
        F::U32 => u32::from_le_bytes(s.try_into().unwrap()) as i128,
        F::I32 => i32::from_le_bytes(s.try_into().unwrap()) as i128,
        F::U64 => u64::from_le_bytes(s.try_into().unwrap()) as i128,
        F::I64 => i64::from_le_bytes(s.try_into().unwrap()) as i128,
    })
}

fn set(b: &mut [u8], o: i128, f: F, v: i128) -> Result<(), String> {
    let n = f.size();
    let (lo, hi) = f.range();
    if v < lo || v > hi {
        return Err(format!(
            "the value {} does not fit the field at {}",
            px(v),
            px(o)
        ));
    }
    let s = usize::try_from(o)
        .ok()
        .and_then(|o| {
            let e = o.checked_add(n)?;
            b.get_mut(o..e)
        })
        .ok_or_else(|| format!("a {n}-byte write at {} is outside the module", px(o)))?;
    let bytes = (v as u128).to_le_bytes();
    s.copy_from_slice(&bytes[..n]);
    Ok(())
}

fn byte(b: &[u8], o: i128) -> Result<u8, String> {
    usize::try_from(o)
        .ok()
        .and_then(|o| b.get(o).copied())
        .ok_or_else(|| format!("a read at {} is outside the module", px(o)))
}

fn uleb(b: &[u8], mut o: i128) -> Result<(i128, i128), String> {
    let (mut r, mut sh) = (0i128, 0u32);
    loop {
        let c = byte(b, o)?;
        o += 1;
        if sh < 120 {
            r |= ((c & 0x7f) as i128) << sh;
        }
        sh += 7;
        if c & 0x80 == 0 {
            return Ok((r, o));
        }
    }
}

/// `FMT_INFO` of a pointer encoding's format: (struct format), None for one the move cannot rewrite
fn fmt_info(fmt: u8) -> Option<F> {
    match fmt {
        0x3 => Some(F::U32),
        0xB => Some(F::I32),
        0x0 | 0x4 => Some(F::U64),
        0xC => Some(F::I64),
        _ => None,
    }
}

fn sx(v: i128, bits: u32) -> i128 {
    if (v >> (bits - 1)) & 1 != 0 {
        v - (1i128 << bits)
    } else {
        v
    }
}

/// `relayout_module(mod)`: `(module, D, notes)`. `D == 0` (and no notes) when data already sits on its own 16 KB page: the module is returned as it was. `Err` is the
/// `mod_layout.Error` message (the rewrite stops with `[RED] <name>: <message>`).
pub fn relayout_module(m: Module) -> Result<(Module, u64, Vec<String>), String> {
    let [(to, tsz), (ro, rsz), (dof, dsz)] = m.segments;
    let d = shift_needed(tsz as u64, dof as u64);
    if d == 0 {
        return Ok((m, 0, Vec::new()));
    }
    let (to, tsz, ro, mut rsz, dof, dsz) = (
        to as i128,
        tsz as i128,
        ro as i128,
        rsz as i128,
        dof as i128,
        dsz as i128,
    );
    let dd = d as i128;
    let len = m.image.len() as i128;
    if to != 0
        || tsz > ro
        || ro + rsz > dof
        || dof + dsz != len
        || ro % PAGE != 0
        || dof % PAGE != 0
        || dd % PAGE != 0
    {
        return Err(format!(
            "unexpected segments text {}+{} ro {}+{} data {}+{} image {}",
            px(to),
            px(tsz),
            px(ro),
            px(rsz),
            px(dof),
            px(dsz),
            px(len)
        ));
    }
    let mv = |a: i128| if a >= dof { a + dd } else { a };
    let old = &m.image[..];
    let mut new: Vec<u8> = Vec::with_capacity(old.len() + d as usize);
    new.extend_from_slice(&old[..dof as usize]);
    new.resize(dof as usize + d as usize, 0);
    new.extend_from_slice(&old[dof as usize..]);
    // adrp, adr, literal, reloc, relr, sym, eh, dyn
    let mut n = [0usize; 8];
    let (n_adrp, n_adr, n_lit, n_reloc, n_relr, n_sym, n_eh, n_dyn) = (0, 1, 2, 3, 4, 5, 6, 7);
    macro_rules! put {
        ($f:expr, $o_old:expr, $v:expr) => {
            set(&mut new, mv($o_old), $f, $v)?
        };
    }

    // An NRO's segments are contiguous, so its read-only segment takes the padding
    if old.len() >= 0x40 && &old[0x10..0x14] == b"NRO0" {
        let size = get(old, 0x18, F::U32)?;
        if size != len {
            return Err(format!("NRO header size {} != image {}", px(size), px(len)));
        }
        rsz += dd;
        set(&mut new, 0x18, F::U32, size + dd)?;
        set(&mut new, 0x2c, F::U32, rsz)?;
        set(&mut new, 0x30, F::U32, dof + dd)?;
    }

    // MOD0: its offset at +4, and six offsets relative to MOD0 itself
    let m0 = get(old, 4, F::U32)?;
    if m0 + 28 > len || &old[m0 as usize..m0 as usize + 4] != b"MOD0" {
        return Err(format!("no MOD0 at {}", px(m0)));
    }
    set(&mut new, 4, F::U32, mv(m0))?;
    let mut mod0 = [0i128; 6];
    for (i, r) in mod0.iter_mut().enumerate() {
        *r = get(old, m0 + 4 + 4 * i as i128, F::I32)?;
    }
    for (i, r) in mod0.iter().enumerate() {
        put!(F::I32, m0 + 4 + 4 * i as i128, mv(m0 + r) - mv(m0));
    }

    // the dynamic section (a later entry of a tag wins, as in a Python dict)
    let dynp = m0 + mod0[0];
    let mut o = dynp;
    let mut tags: std::collections::HashMap<i128, i128> = std::collections::HashMap::new();
    loop {
        let t = get(old, o, F::I64)?;
        let v = get(old, o + 8, F::U64)?;
        if t == 0 {
            break;
        }
        tags.insert(t, v);
        if PTR_TAGS.contains(&t) && mv(v) != v {
            put!(F::U64, o + 8, mv(v));
            n[n_dyn] += 1;
        }
        o += 16;
    }
    if tags.contains_key(&17) {
        return Err("DT_REL relocations (only RELA is expected on AArch64)".into());
    }
    let tag = |t: i128| -> Result<i128, String> {
        tags.get(&t)
            .copied()
            .ok_or_else(|| format!("the dynamic section has no tag {t}"))
    };

    // symbols: nchain of DT_HASH, else up to DT_STRTAB (STT_TLS values are offsets in the TLS block, not module addresses)
    if let Some(&symtab) = tags.get(&6) {
        let nsym = if let Some(&h) = tags.get(&4) {
            get(old, h + 4, F::U32)?
        } else if let Some(&strtab) = tags.get(&5) {
            (strtab - symtab).div_euclid(24)
        } else {
            return Err("DT_SYMTAB without DT_HASH or DT_STRTAB".into());
        };
        for i in 0..nsym.max(0) {
            let s = symtab + 24 * i;
            let st_type = byte(old, s + 4)? & 0xf;
            let shndx = get(old, s + 6, F::U16)?;
            let val = get(old, s + 8, F::U64)?;
            if shndx != 0 && shndx < 0xff00 && st_type != 6 && mv(val) != val {
                put!(F::U64, s + 8, mv(val));
                n[n_sym] += 1;
            }
        }
    }

    // RELA and JMPREL
    for (tab, sz) in [(7, 8), (23, 2)] {
        let Some(&base) = tags.get(&tab) else {
            continue;
        };
        for k in 0..tag(sz)?.div_euclid(24).max(0) {
            let r = base + 24 * k;
            let off = get(old, r, F::U64)?;
            let info = get(old, r + 8, F::U64)? as u64;
            let mut add = get(old, r + 16, F::I64)?;
            let (typ, si) = (info & 0xffff_ffff, info >> 32);
            if ![R_ABS64, R_GLOB_DAT, R_JUMP_SLOT, R_RELATIVE].contains(&typ) {
                return Err(format!("relocation type {typ} at {}", px(r)));
            }
            if typ == R_RELATIVE || (typ == R_ABS64 && si == 0) {
                add = mv(add);
            }
            set(&mut new, mv(r), F::U64, mv(off))?;
            set(&mut new, mv(r) + 8, F::U64, info as i128)?;
            set(&mut new, mv(r) + 16, F::I64, add)?;
            n[n_reloc] += 1;
        }
    }

    // DT_RELR: even entries are addresses, odd ones bitmaps; the addend is the slot's content
    if let Some(&relr) = tags.get(&36) {
        let mut wher: Option<i128> = None;
        for k in 0..tag(35)?.div_euclid(8).max(0) {
            let e = relr + 8 * k;
            let v = get(old, e, F::U64)?;
            let slots: Vec<i128> = if v & 1 == 0 {
                put!(F::U64, e, mv(v));
                wher = Some(v + 8);
                vec![v]
            } else {
                let Some(w) = wher else {
                    return Err(format!("DT_RELR bitmap before start at {}", px(e)));
                };
                wher = Some(w + 63 * 8);
                (0..63)
                    .filter(|b| (v >> (b + 1)) & 1 != 0)
                    .map(|b| w + 8 * b as i128)
                    .collect()
            };
            for a in slots {
                put!(F::U64, a, mv(get(old, a, F::U64)?));
                n[n_relr] += 1;
            }
        }
    }

    // text: adrp / adr / literal loads into the moved part. The MOD0 offset at +4 is never code; the data blocks in text are skipped, and so are the words of the
    // small literal pools that in-text literal loads read
    let mut blocks = crate::fde::data_blocks(old, tsz as i64)?;
    blocks.push((4, 8));
    if m0 < tsz {
        blocks.push((m0 as i64, tsz.min(m0 + 28) as i64)); // MOD0 in text: b"MOD0" (0x30444f4d) matches the adr bit pattern
    }
    let code = |a: i128| !blocks.iter().any(|&(s, e)| s as i128 <= a && a < e as i128);
    let word = |a: i128| get(old, a, F::U32).map(|w| w as u32);
    let mut lit_words: std::collections::HashSet<i128> = std::collections::HashSet::new();
    let mut a = 0i128;
    while a < tsz {
        if code(a) {
            let w = word(a)?;
            if w & 0x3B00_0000 == 0x1800_0000 && (w >> 30) != 3 {
                // ldr / ldrsw (literal), not prfm or the unallocated opc=3 V=1
                let t = a + sx(((w >> 5) & 0x7ffff) as i128, 19) * 4;
                if 0 <= t && t < tsz {
                    let (opc, vec) = (w >> 30, (w >> 26) & 1);
                    let width = match (opc, vec) {
                        (0, _) => 4,
                        (1, _) => 8,
                        (_, 1) => 16,
                        _ => 4,
                    };
                    let mut p = t & !3;
                    while p < tsz.min(t + width) {
                        lit_words.insert(p);
                        p += 4;
                    }
                }
            }
        }
        a += 4;
    }
    let mut a = 0i128;
    while a < tsz {
        if !code(a) || lit_words.contains(&a) {
            a += 4;
            continue;
        }
        let w = word(a)?;
        if w & 0x1F00_0000 == 0x1000_0000 {
            // adr / adrp
            let mut imm = sx(((((w >> 5) & 0x7ffff) << 2) | ((w >> 29) & 3)) as i128, 21);
            if w >> 31 != 0 {
                let t = (a & !0xfff) + (imm << 12);
                if t < dof {
                    a += 4;
                    continue;
                }
                imm += dd >> 12;
                n[n_adrp] += 1;
            } else {
                let t = a + imm;
                if t < dof {
                    a += 4;
                    continue;
                }
                imm += dd;
                n[n_adr] += 1;
            }
            if !(-(1i128 << 20)..1 << 20).contains(&imm) {
                return Err(format!("adr(p) at {} out of range after the move", px(a)));
            }
            let imm = (imm & 0x1f_ffff) as u32;
            let w = (w & 0x9F00_001F) | (imm & 3) << 29 | (imm >> 2) << 5;
            set(&mut new, a, F::U32, w as i128)?;
        } else if w & 0x3B00_0000 == 0x1800_0000 && (w & 0xFF00_0000) != 0xDC00_0000 {
            // ldr / ldrsw / prfm (literal), not the unallocated opc=3 V=1
            let mut imm = sx(((w >> 5) & 0x7ffff) as i128, 19);
            let t = a + imm * 4;
            if t < dof {
                a += 4;
                continue;
            }
            imm += dd >> 2;
            if !(-(1i128 << 18)..1 << 18).contains(&imm) {
                return Err(format!(
                    "literal load at {} out of range after the move",
                    px(a)
                ));
            }
            let w = (w & 0xFF00_001F) | ((imm & 0x7ffff) as u32) << 5;
            set(&mut new, a, F::U32, w as i128)?;
            n[n_lit] += 1;
        }
        a += 4;
    }

    // .eh_frame_hdr and .eh_frame (old offsets; a pointer read at o from base b to target t becomes mv(t) - mv(b))
    let (ehs, ehe) = (m0 + mod0[3], m0 + mod0[4]);
    if ehe - ehs >= 8 && byte(old, ehs)? == 1 {
        // rewrites the encoded pointer at old offset o: (old target, size)
        let fix = |new: &mut Vec<u8>,
                   n: &mut [usize; 8],
                   o: i128,
                   enc: u8,
                   optional: bool|
         -> Result<(i128, i128), String> {
            let enc = enc & 0x7f; // without DW_EH_PE_indirect: the stored pointer itself changes, not what it points at
            let (fmt, app) = (enc & 0xf, enc & 0x70);
            let f = fmt_info(fmt)
                .ok_or_else(|| format!("pointer encoding {} at {}", px(enc as i128), px(o)))?;
            let v = get(old, o, f)?;
            let sz = f.size() as i128;
            if optional && v == 0 {
                return Ok((0, sz));
            }
            let base = match app {
                0x10 => o,
                0x30 => ehs,
                0 => 0,
                _ => {
                    return Err(format!(
                        "pointer application {} at {}",
                        px(app as i128),
                        px(o)
                    ));
                }
            };
            let t = base + v;
            let nv = mv(t) - mv(base);
            let (lo, hi) = f.range();
            if !(lo <= nv && nv <= hi) {
                return Err(format!(
                    "encoded pointer {} at {} overflows after the move ({})",
                    px(enc as i128),
                    px(o),
                    px(nv)
                ));
            }
            if nv != v {
                set(new, mv(o), f, nv)?;
                n[n_eh] += 1;
            }
            Ok((t, sz))
        };
        let (penc, cenc, tenc) = (
            byte(old, ehs + 1)?,
            byte(old, ehs + 2)?,
            byte(old, ehs + 3)?,
        );
        let mut o = ehs + 4;
        let mut ehf: Option<i128> = None;
        if penc != 0xff {
            let (t, s) = fix(&mut new, &mut n, o, penc, false)?;
            ehf = Some(t);
            o += s;
        }
        let cnt = if cenc == 0xff {
            0
        } else if cenc & 0xf == 0x3 || cenc & 0xf == 0xb {
            let c = get(old, o, if cenc & 0xf == 3 { F::U32 } else { F::I32 })?;
            o += 4;
            c
        } else {
            return Err(format!("eh_frame_hdr count encoding {}", px(cenc as i128)));
        };
        if tenc != 0xff && tenc != 0 {
            for _ in 0..cnt.max(0) {
                o += fix(&mut new, &mut n, o, tenc, false)?.1;
                o += fix(&mut new, &mut n, o, tenc, false)?.1;
            }
        }
        // the .eh_frame records
        if let Some(start) = ehf {
            // CIE offset -> (R encoding, L encoding, has P, z augmentation)
            let mut cies: std::collections::HashMap<i128, (u8, Option<u8>, bool, bool)> =
                std::collections::HashMap::new();
            let mut o = start;
            while o + 4 <= len {
                let ln = get(old, o, F::U32)?;
                if ln == 0 {
                    break;
                }
                if ln == 0xffff_ffff {
                    return Err("64-bit eh_frame record".into());
                }
                let cid = get(old, o + 4, F::U32)?;
                let end = o + 4 + ln;
                if cid == 0 {
                    // a CIE
                    let mut p = o + 9;
                    let ver = byte(old, o + 8)?;
                    let ps = usize::try_from(p).map_err(|_| "a CIE outside the module")?;
                    let aug_end = old
                        .get(ps..)
                        .and_then(|r| r.iter().position(|&c| c == 0))
                        .ok_or("a CIE augmentation without its end")?;
                    let aug = &old[ps..ps + aug_end];
                    p += aug.len() as i128 + 1;
                    p = uleb(old, p)?.1; // code alignment
                    while byte(old, p)? & 0x80 != 0 {
                        p += 1; // data alignment (sleb)
                    }
                    p += 1;
                    p = if ver == 1 { p + 1 } else { uleb(old, p)?.1 }; // return register
                    let (mut r_enc, mut l_enc, mut has_p) = (0u8, None, false);
                    let z = aug.first() == Some(&b'z');
                    if z {
                        p = uleb(old, p)?.1;
                        for &ch in &aug[1..] {
                            match ch {
                                b'R' => {
                                    r_enc = byte(old, p)?;
                                    p += 1;
                                }
                                b'L' => {
                                    l_enc = Some(byte(old, p)?);
                                    p += 1;
                                }
                                b'P' => {
                                    let pe = byte(old, p)?;
                                    p += 1;
                                    p += fix(&mut new, &mut n, p, pe, false)?.1;
                                    has_p = true;
                                }
                                b'S' | b'B' => {}
                                _ => {
                                    return Err(format!("CIE augmentation {}", py_bytes_repr(aug)));
                                }
                            }
                        }
                    }
                    cies.insert(o, (r_enc, l_enc, has_p, z));
                } else {
                    // an FDE
                    let cie = o + 4 - cid;
                    let &(r_enc, l_enc, has_p, z) = cies
                        .get(&cie)
                        .ok_or_else(|| format!("an FDE at {} without its CIE", px(o)))?;
                    let mut p = o + 8;
                    if r_enc != 0xff {
                        p += fix(&mut new, &mut n, p, r_enc, false)?.1;
                        let rf = fmt_info(r_enc & 0xf).ok_or_else(|| {
                            format!("FDE range encoding {} at {}", px(r_enc as i128), px(p))
                        })?;
                        p += rf.size() as i128; // the pc range: no application
                    }
                    if z {
                        let (aug_len, q) = uleb(old, p)?;
                        p = q;
                        if let Some(le) = l_enc.filter(|&e| aug_len > 0 && e != 0xff) {
                            let (lsda, _) = fix(&mut new, &mut n, p, le, true)?;
                            if lsda != 0 && has_p {
                                let lpe = byte(old, lsda)?;
                                if lpe != 0xff {
                                    fix(&mut new, &mut n, lsda + 1, lpe, false)?;
                                }
                            }
                        }
                    }
                }
                o = end;
            }
        }
    }

    let names = [
        "adrp", "adr", "literal", "reloc", "relr", "sym", "eh", "dyn",
    ];
    let notes = vec![
        format!("moved data by {}: {} -> {}", px(dd), px(dof), px(dof + dd)),
        names
            .iter()
            .zip(n)
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", "),
    ];
    let segments = [
        (0, tsz as u32),
        (ro as u32, rsz as u32),
        ((dof + dd) as u32, dsz as u32),
    ];
    Ok((
        Module {
            image: new,
            segments,
            bss: m.bss,
            build_id: m.build_id,
        },
        d,
        notes,
    ))
}
