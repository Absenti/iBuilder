//! A damaged or truncated module is an `Err` (or an empty answer where the Python's reader is lenient), never a panic: `ibuilder-nso` promises "a damaged or truncated file is an error, never a
//! wrong image", and the readers of `.eh_frame`, the dynamic symbols and the relocations (`fde.rs`) sit right behind it (review finding 3.3). Release builds are `panic = "abort"`, so a panic
//! there is the app dying, not an error line.
//!
//! The image below is built by hand (MOD0, dynamic section, eh_frame_hdr + CIE + FDE, a symbol, two RELA entries and a RELR one), a positive control proves every reader gives the right answer
//! on it, and then every reader is run on every truncation of it and on every byte / word of its metadata overwritten with an extreme value: none may panic. This runs on every machine (no fixture).

use ibuilder_rewrite::fde;
use std::panic::{AssertUnwindSafe, catch_unwind};

const LEN: usize = 0x2000;
const MOD0: usize = 0x80;
const EHS: usize = 0x200; // eh_frame_hdr
const EH: usize = 0x300; // eh_frame: the CIE, then the FDE
const DYN: usize = 0x400;
const RELA: usize = 0x500;
const RELR: usize = 0x540;
const HASH: usize = 0x580;
const SYMTAB: usize = 0x600;
const STRTAB: usize = 0x700;
const CODE: usize = 0x1000;
/// everything the readers look at lies below this
const META_END: usize = 0x800;

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], o: usize, v: u64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn good_image() -> Vec<u8> {
    let mut b = vec![0u8; LEN];
    put32(&mut b, 4, MOD0 as u32);
    b[MOD0..MOD0 + 4].copy_from_slice(b"MOD0");
    put32(&mut b, MOD0 + 4, (DYN - MOD0) as u32); // dynamic
    put32(&mut b, MOD0 + 16, (EHS - MOD0) as u32); // eh_frame_hdr
    put32(&mut b, MOD0 + 20, (EHS + 0x20 - MOD0) as u32);
    // eh_frame_hdr: version 1, eh_frame_ptr pcrel sdata4, fde_count udata4, table datarel sdata4, one entry
    b[EHS..EHS + 4].copy_from_slice(&[1, 0x1B, 0x03, 0x3B]);
    put32(&mut b, EHS + 4, (EH - (EHS + 4)) as u32);
    put32(&mut b, EHS + 8, 1);
    let fde = EH + 0x20;
    put32(&mut b, EHS + 12, (CODE - EHS) as u32);
    put32(&mut b, EHS + 16, (fde - EHS) as u32);
    // CIE: version 1, "zR", code align 1, data align -8, return register 30, augmentation length 1, R = pcrel sdata4
    put32(&mut b, EH, 0x14);
    b[EH + 8] = 1;
    b[EH + 9..EH + 12].copy_from_slice(b"zR\0");
    b[EH + 12..EH + 17].copy_from_slice(&[1, 0x78, 30, 1, 0x1B]);
    // FDE: the CIE pointer counts back from its own field; pc_begin pcrel, pc_range
    put32(&mut b, fde, 0x14);
    put32(&mut b, fde + 4, (fde + 4 - EH) as u32);
    put32(&mut b, fde + 8, (CODE as i64 - (fde as i64 + 8)) as u32);
    put32(&mut b, fde + 12, 0x100);
    // dynamic: SYMTAB, STRTAB, RELA, RELASZ (2 entries), HASH (nchain = 2 symbols), RELR, RELRSZ, end
    let tags: [(u64, u64); 8] = [
        (6, SYMTAB as u64),
        (5, STRTAB as u64),
        (7, RELA as u64),
        (8, 48),
        (4, HASH as u64),
        (36, RELR as u64),
        (35, 8),
        (0, 0),
    ];
    for (i, (t, v)) in tags.iter().enumerate() {
        put64(&mut b, DYN + i * 16, *t);
        put64(&mut b, DYN + i * 16 + 8, *v);
    }
    // RELA: a RELATIVE at 0x900 -> CODE, and an ABS64 of symbol 1 (+8) at 0x910
    put64(&mut b, RELA, 0x900);
    put64(&mut b, RELA + 8, 1027);
    put64(&mut b, RELA + 16, CODE as u64);
    put64(&mut b, RELA + 24, 0x910);
    put64(&mut b, RELA + 32, (1u64 << 32) | 257);
    put64(&mut b, RELA + 40, 8);
    // RELR: one address entry (0x908), whose content in place is 0x1234
    put64(&mut b, RELR, 0x908);
    put64(&mut b, 0x908, 0x1234);
    // HASH: nbucket, nchain = 2; symbol 1 "foo", defined (shndx 1), value CODE, size 0x20
    put32(&mut b, HASH + 4, 2);
    put32(&mut b, SYMTAB + 24, 1);
    b[SYMTAB + 24 + 6..SYMTAB + 24 + 8].copy_from_slice(&1u16.to_le_bytes());
    put64(&mut b, SYMTAB + 24 + 8, CODE as u64);
    put64(&mut b, SYMTAB + 24 + 16, 0x20);
    b[STRTAB..STRTAB + 5].copy_from_slice(b"\0foo\0");
    // code: mov x8, x1 ; str x0, [x8] ; ret
    put32(&mut b, CODE, 0xAA0103E8);
    put32(&mut b, CODE + 4, 0xF9000100);
    put32(&mut b, CODE + 8, 0xD65F03C0);
    b
}

/// The control image with a C++ exception table: its CIE is "zPLR" (a personality, an LSDA encoding), its FDE points at
/// an LSDA (no landing-pad base, no type table, uleb128 call sites) with one call site, CODE+0x10..+0x18, caught at
/// CODE+0x40 (`nso_code.landing_pads`)
const LSDA: usize = 0x780;
fn lsda_image() -> Vec<u8> {
    let mut b = good_image();
    // CIE: version 1, "zPLR", code align 1, data align -8, ra 30, augmentation length 11: P = absptr 0, L = pcrel sdata4, R = pcrel sdata4
    b[EH..EH + 0x20].fill(0);
    put32(&mut b, EH, 0x1C);
    b[EH + 8] = 1;
    b[EH + 9..EH + 14].copy_from_slice(b"zPLR\0");
    b[EH + 14..EH + 18].copy_from_slice(&[1, 0x78, 30, 11]);
    b[EH + 18] = 0x00; // P encoding, then 8 bytes of personality pointer (0)
    b[EH + 27] = 0x1B; // L
    b[EH + 28] = 0x1B; // R
    // FDE: augmentation length 4, then the LSDA pointer (pcrel sdata4)
    let fde = EH + 0x20;
    b[fde + 16] = 4;
    put32(&mut b, fde + 17, (LSDA as i64 - (fde as i64 + 17)) as u32);
    // LSDA: lpstart omitted (0xFF), no type table (0xFF), call sites uleb128 (0x01), 4 bytes of them: start 0x10, length 8, pad 0x40, action 0
    b[LSDA..LSDA + 8].copy_from_slice(&[0xFF, 0xFF, 0x01, 4, 0x10, 0x08, 0x40, 0x00]);
    b
}

#[test]
fn the_exception_tables_read_right() {
    let img = lsda_image();
    assert_eq!(
        fde::fde_ranges(&img),
        Ok(vec![(CODE as i64, CODE as i64 + 0x100)])
    );
    let pads = fde::landing_pads(&img).expect("the LSDA reads");
    assert_eq!(
        pads,
        vec![(CODE as i64 + 0x40, (CODE as i64 + 0x10, CODE as i64 + 0x18))]
    );
    // without a personality ('P') an LSDA is not a C++ one: no pads
    assert_eq!(fde::landing_pads(&good_image()).map(|p| p.len()), Ok(0));
}

#[test]
fn the_control_image_reads_right() {
    let img = good_image();
    assert_eq!(fde::eh_frame_hdr(&img), Ok((EHS as i64, EHS as i64 + 0x20)));
    assert_eq!(
        fde::fde_table(&img),
        Ok(vec![(CODE as i64, (EH + 0x20) as i64)])
    );
    assert_eq!(
        fde::fde_ranges(&img),
        Ok(vec![(CODE as i64, CODE as i64 + 0x100)])
    );
    assert!(fde::data_blocks(&img, LEN as i64).is_ok());
    let sym = fde::defined(&img);
    assert_eq!(sym.get("foo"), Some(&(CODE as i64, 0x20)));
    assert_eq!(sym.len(), 1);
    let got = fde::got_values(&img);
    assert_eq!(got.get(0x900), Some(CODE as i64));
    assert_eq!(got.get(0x910), Some(CODE as i64 + 8));
    assert_eq!(got.get(0x908), Some(0x1234));
    assert_eq!(got.len(), 3);
    assert_eq!(
        fde::out_store(&img, CODE as i64, 12),
        Ok((CODE as i64 + 4, 0))
    );
}

/// Every reader of `fde.rs` on `img`; the name of the first one that panics.
fn panicking_reader(img: &[u8]) -> Option<&'static str> {
    macro_rules! run {
        ($name:expr, $e:expr) => {
            if catch_unwind(AssertUnwindSafe(|| {
                let _ = $e;
            }))
            .is_err()
            {
                return Some($name);
            }
        };
    }
    run!("eh_frame_hdr", fde::eh_frame_hdr(img));
    run!("landing_pads", fde::landing_pads(img));
    run!("fde_table", fde::fde_table(img));
    run!("fde_ranges", fde::fde_ranges(img));
    run!("data_blocks", fde::data_blocks(img, img.len() as i64));
    run!(
        "data_blocks(text_end past the image)",
        fde::data_blocks(img, img.len() as i64 + 0x4000)
    );
    run!("defined", fde::defined(img));
    run!("got_values", fde::got_values(img));
    run!("out_store", fde::out_store(img, CODE as i64, 12));
    run!(
        "out_store(outside)",
        fde::out_store(img, img.len() as i64 - 4, 0x100)
    );
    run!("out_store(negative)", fde::out_store(img, -8, 12));
    run!("out_store(huge)", fde::out_store(img, 8, i64::MAX));
    None
}

#[test]
fn no_reader_panics_on_a_truncated_image() {
    for good in [good_image(), lsda_image()] {
        let mut bad = Vec::new();
        for len in 0..=good.len() {
            if let Some(r) = panicking_reader(&good[..len]) {
                bad.push(format!("{r} on the first {len:#x} bytes"));
            }
        }
        assert!(
            bad.is_empty(),
            "{} truncations panic, e.g. {:?}",
            bad.len(),
            &bad[..bad.len().min(8)]
        );
    }
}

#[test]
fn no_reader_panics_on_a_byte_overwritten_with_an_extreme_value() {
    for good in [good_image(), lsda_image()] {
        let mut bad = Vec::new();
        for at in 0..META_END {
            for v in [0x00u8, 0x7F, 0x80, 0xFF] {
                let mut img = good.clone();
                img[at] = v;
                if let Some(r) = panicking_reader(&img) {
                    bad.push(format!("{r} with byte {at:#x} = {v:#x}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "{} byte overwrites panic, e.g. {:?}",
            bad.len(),
            &bad[..bad.len().min(8)]
        );
    }
}

#[test]
fn no_reader_panics_on_a_word_overwritten_with_an_extreme_value() {
    for good in [good_image(), lsda_image()] {
        let mut bad = Vec::new();
        for at in (0..META_END).step_by(4) {
            for v in [0x7FFF_FFFFu32, 0x8000_0000, 0xFFFF_FFFF, 0x0000_FFFF] {
                let mut img = good.clone();
                put32(&mut img, at, v);
                if let Some(r) = panicking_reader(&img) {
                    bad.push(format!("{r} with word {at:#x} = {v:#x}"));
                }
            }
            for v in [
                u64::MAX,
                0x7FFF_FFFF_FFFF_FFFF,
                0x8000_0000_0000_0000,
                0x0000_0001_0000_0000,
            ] {
                let mut img = good.clone();
                put64(&mut img, at, v);
                if let Some(r) = panicking_reader(&img) {
                    bad.push(format!("{r} with dword {at:#x} = {v:#x}"));
                }
            }
        }
        assert!(
            bad.is_empty(),
            "{} word overwrites panic, e.g. {:?}",
            bad.len(),
            &bad[..bad.len().min(8)]
        );
    }
}

/// The reader that `scan::run` calls first on every module answers a damaged `.eh_frame` with an error, which the rewriter turns into a refused module.
#[test]
fn a_damaged_eh_frame_is_an_error_not_an_empty_answer() {
    let good = good_image();
    // the FDE table claims far more entries than the module holds
    let mut img = good.clone();
    put32(&mut img, EHS + 8, 0x7FFF_FFFF);
    assert!(
        fde::fde_table(&img).is_err(),
        "a table that runs off the image must be an error"
    );
    // the CIE pointer of the FDE points before the image
    let mut img = good.clone();
    put32(&mut img, EH + 0x20 + 4, 0x7FFF_0000);
    assert!(fde::fde_ranges(&img).is_err());
    // an augmentation string with no NUL before the image ends
    let mut img = good.clone();
    for b in &mut img[EH + 9..] {
        *b = b'z';
    }
    assert!(fde::fde_ranges(&img).is_err());
    // the module cut inside its eh_frame_hdr
    assert!(fde::data_blocks(&good[..EHS + 6], (EHS + 6) as i64).is_err());
}

/// An uncompressed NSO0 whose text segment is `image` (ro and data empty, after it): enough for `ibuilder_nso::load`, which is where a real file comes in.
fn nso_of(image: &[u8]) -> Vec<u8> {
    let page = |v: usize| (v + 0xFFF) & !0xFFF;
    let mut f = vec![0u8; 0x100];
    f[..4].copy_from_slice(b"NSO0");
    // segment headers: {file offset, memory offset, size} of text, ro, data; flags (word at 0xc) = 0: stored, not compressed, no hash checks
    let end = page(image.len());
    for (i, (fo, mo, sz)) in [
        (0x100usize, 0usize, image.len()),
        (0x100 + image.len(), end, 0),
        (0x100 + image.len(), end, 0),
    ]
    .into_iter()
    .enumerate()
    {
        put32(&mut f, 0x10 + i * 0x10, fo as u32);
        put32(&mut f, 0x14 + i * 0x10, mo as u32);
        put32(&mut f, 0x18 + i * 0x10, sz as u32);
        put32(&mut f, 0x60 + 4 * i, sz as u32);
    }
    f.extend_from_slice(image);
    f
}

fn rewrite_text(image: &[u8]) -> Result<String, String> {
    let nso = nso_of(image);
    ibuilder_nso::load(&nso, "t.nso").map_err(|e| format!("load: {e}"))?;
    match catch_unwind(AssertUnwindSafe(|| {
        ibuilder_rewrite::rewrite(
            &nso,
            "t.nso",
            "sdk",
            &ibuilder_rewrite::Options {
                workers: 1,
                ..Default::default()
            },
        )
    })) {
        Ok(Ok(r)) => Ok(r.log.join("\n")),
        Ok(Err(e)) => Err(format!("{e}")),
        Err(_) => panic!("rewrite() panicked"),
    }
}

/// The whole rewrite on a damaged module (what `ibuilder rewrite` and the app's stage 2 run): an `Err` that says what is wrong, not a panic and not a module written from what could be read.
#[test]
fn rewriting_a_module_with_a_damaged_eh_frame_is_refused_with_the_reason() {
    let good = good_image();
    let ok = rewrite_text(&good);
    assert!(ok.is_ok(), "the control module must rewrite: {ok:?}");
    // the FDE table claims 0x7FFFFFFF entries
    let mut img = good.clone();
    put32(&mut img, EHS + 8, 0x7FFF_FFFF);
    let e = rewrite_text(&img).expect_err("a table that runs off the image is refused");
    assert!(e.contains("FDE table") || e.contains("short read"), "{e}");
    // the FDE's CIE pointer leaves the image
    let mut img = good.clone();
    put32(&mut img, EH + 0x20 + 4, 0x7FFF_0000);
    assert!(rewrite_text(&img).is_err());
    // the eh_frame_hdr lies outside the image
    let mut img = good.clone();
    put32(&mut img, MOD0 + 16, 0x7000_0000);
    put32(&mut img, MOD0 + 20, 0x7000_0100);
    assert!(rewrite_text(&img).is_err());
    // every truncation of the metadata area of the module is an error or a rewrite, never a panic (rewrite_text panics on one)
    for cut in (0..META_END).step_by(0x10) {
        let mut img = good.clone();
        for b in &mut img[cut..META_END] {
            *b = 0xFF;
        }
        let _ = rewrite_text(&img);
    }
}

#[test]
fn try_got_values_says_what_got_values_hides() {
    let good = good_image();
    let ok = fde::try_got_values(&good).expect("the control image's relocations read");
    assert_eq!(ok.len(), 3);
    // DT_RELASZ names more entries than the image holds
    let mut img = good.clone();
    put64(&mut img, DYN + 3 * 16 + 8, 24 * 0x1000_0000);
    assert!(fde::try_got_values(&img).is_err());
    assert_eq!(
        fde::got_values(&img).len(),
        2,
        "got_values keeps the two RELA entries it read before the table ran off the image (the RELR part is never reached)"
    );
    // DT_RELASZ without DT_RELA
    let mut img = good.clone();
    put64(&mut img, DYN + 2 * 16, 0x7F00);
    let e = fde::try_got_values(&img).expect_err("no DT_RELA");
    assert!(e.contains("DT_RELA"), "{e}");
    // a symbol index of an ABS64 relocation outside the symbol table
    let mut img = good.clone();
    put64(&mut img, RELA + 32, (0x7FFF_FFFFu64 << 32) | 257);
    assert!(fde::try_got_values(&img).is_err());
    // a RELR entry that places the next address outside the image
    let mut img = good.clone();
    put64(&mut img, RELR, 0x7FFF_FFFF_FFFF_FFF8);
    assert!(fde::try_got_values(&img).is_err());
}

/// The whole rewrite (scan, detectors, writers) on every module the metadata sweep makes: whatever it answers, it answers. (A panic here is `rewrite_text` panicking.)
#[test]
fn rewrite_never_panics_on_a_module_whose_metadata_was_overwritten() {
    let good = good_image();
    let mut errs = 0usize;
    let mut oks = 0usize;
    for at in (0..META_END).step_by(2) {
        for v in [0x00u8, 0x7F, 0xFF] {
            let mut img = good.clone();
            img[at] = v;
            match rewrite_text(&img) {
                Ok(_) => oks += 1,
                Err(_) => errs += 1,
            }
        }
    }
    for at in (0..META_END).step_by(4) {
        for v in [0x7FFF_FFFFu32, 0x8000_0000, 0xFFFF_FFFF] {
            let mut img = good.clone();
            put32(&mut img, at, v);
            match rewrite_text(&img) {
                Ok(_) => oks += 1,
                Err(_) => errs += 1,
            }
        }
    }
    assert!(
        errs > 10 && oks > 100,
        "the sweep must reach both outcomes: {oks} rewrites, {errs} refused"
    );
}
