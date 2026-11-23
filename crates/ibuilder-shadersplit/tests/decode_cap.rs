//! The most one decode may produce (review r2, 4.1): a truthful decompression bomb (zlib ~1000:1, a zstd frame of RLE blocks 32,768:1) cost about twice its size in memory before there was a cap
//! (measured: 1.25 GiB out of a 1.3 MB zlib stream or a 41 KB zstd frame -> 2.1-2.2 GB RSS). The cap is `inflate::MAX_DECODED`, 1 GiB; the tests use `*_capped` with a small one, and assert that nothing
//! under the cap changes.

use ibuilder_shadersplit::Failure;
use ibuilder_shadersplit::archive::{
    SNOWDROP_ZLIB_MAX_RAW, lz4_frame_capped, snowdrop_shaders, snowdrop_zlib_capped, unsized_frame,
    xnb_decompress,
};
use ibuilder_shadersplit::ctl::KeyRules;
use ibuilder_shadersplit::inflate::{self, MAX_DECODED, Wrap, cap_error};
use ibuilder_shadersplit::scan_types::Ctx;
use ibuilder_shadersplit::split::FileIndex;
use ibuilder_shadersplit::unity::Trees;
use ibuilder_shadersplit::zstd::{Ruzstd, Zstd};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

fn red(e: Failure) -> String {
    match e {
        Failure::Red(m) => m,
        other => panic!("not a red(): {other:?}"),
    }
}

/// `mib` MiB of zeros as one zlib stream (~1000:1)
fn zlib_zeros(mib: usize) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
    let z = vec![0u8; 1 << 20];
    for _ in 0..mib {
        e.write_all(&z).unwrap();
    }
    e.finish().unwrap()
}

/// `blocks` RLE blocks of 128 KiB of zeros in one zstd frame (4 bytes of input each)
fn zstd_rle(blocks: usize) -> Vec<u8> {
    let mut f = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x38]; // magic, no flags, a 128 KiB window
    for i in 0..blocks {
        let last = (i + 1 == blocks) as u32;
        f.extend_from_slice(&((0x20000u32 << 3) | (1 << 1) | last).to_le_bytes()[..3]);
        f.push(0);
    }
    f
}

#[test]
fn the_cap_is_one_gib_and_every_error_names_it() {
    assert_eq!(MAX_DECODED, 1 << 30);
    assert_eq!(SNOWDROP_ZLIB_MAX_RAW, MAX_DECODED);
    let e = cap_error(MAX_DECODED);
    assert!(e.contains("1073741824") && e.contains("1024 MiB"), "{e}");
}

#[test]
fn a_zlib_bomb_stops_at_the_cap_and_nothing_under_it_changes() {
    let c = zlib_zeros(4); // 4 MiB out of ~4 KB
    assert!(c.len() < 8192, "{}", c.len());
    let e = inflate::run_capped(&c, Wrap::Zlib, None, 1 << 20).unwrap_err();
    assert!(e.contains("1048576"), "{e}");
    assert!(
        inflate::run_capped(&c, Wrap::Zlib, None, (4 << 20) - 1).is_err(),
        "one byte under the real size"
    );
    let exact = inflate::run_capped(&c, Wrap::Zlib, None, 4 << 20).unwrap();
    assert!(
        exact.eof && exact.out.len() == 4 << 20,
        "a cap of exactly the real size lets it through"
    );
    // the same bytes, the same flags, as with no cap at all (the default is far above 4 MiB)
    let plain = inflate::run(&c, Wrap::Zlib, None).unwrap();
    assert_eq!(
        (plain.out == exact.out, plain.eof, plain.used),
        (true, exact.eof, exact.used)
    );
    // a truncated stream is still `Ok` with `eof == false` under the cap, as in Python
    let cut = inflate::run_capped(&c[..c.len() - 3], Wrap::Zlib, None, 8 << 20).unwrap();
    assert!(!cut.eof);
    // an explicit `max_out` is the caller's own limit, not the cap: it still stops where it was asked to
    assert_eq!(
        inflate::run_capped(&c, Wrap::Zlib, Some(100), 10)
            .unwrap()
            .out
            .len(),
        100
    );
    // zlib_decompress goes through the same cap
    assert_eq!(inflate::zlib_decompress(&c).unwrap().len(), 4 << 20);
}

#[test]
fn a_zstd_bomb_stops_at_the_cap_and_nothing_under_it_changes() {
    let f = zstd_rle(16); // 2 MiB out of 64 bytes
    assert_eq!(f.len(), 6 + 16 * 4);
    let e = Ruzstd
        .decompress_capped(&f, None, None, 1 << 20)
        .unwrap_err();
    assert!(e.contains("1048576"), "{e}");
    assert!(
        Ruzstd
            .decompress_capped(&f, None, None, (2 << 20) - 1)
            .is_err(),
        "one byte under the real size"
    );
    let exact = Ruzstd.decompress_capped(&f, None, None, 2 << 20).unwrap();
    assert_eq!(exact.len(), 2 << 20);
    assert_eq!(
        Ruzstd.decompress(&f, None, None).unwrap(),
        exact,
        "under the real cap the trait method gives the same bytes"
    );
    // an explicit `max_length` is the caller's own limit (a peek of 20 bytes is not a cap violation)
    assert_eq!(
        Ruzstd
            .decompress_capped(&f, None, Some(20), 10)
            .unwrap()
            .len(),
        20
    );
}

#[test]
fn an_lz4_block_that_declares_more_than_the_cap_is_refused_with_the_cap_named() {
    // an XNB with the LZ4 flag whose declared decoded size is 4 GiB - 16: it used to be reserved as `vec![0; n]`
    let mut x = b"XNBw".to_vec();
    x.push(5); // version
    x.push(0x40); // flags: LZ4
    x.extend_from_slice(&(14u32).to_le_bytes()); // file size field
    x.extend_from_slice(&(0xFFFF_FFF0u32).to_le_bytes()); // decoded size
    let e = red(xnb_decompress(&x, "/x.xnb").unwrap_err());
    assert!(
        e.contains("1073741824") && e.contains("declares 4294967280"),
        "{e}"
    );
    // a sane size with a damaged body keeps the LZ4 text it always had
    let mut y = x.clone();
    y[10..14].copy_from_slice(&(64u32).to_le_bytes());
    y.extend_from_slice(&[0xF0, 0xFF, 0xFF]);
    let e = red(xnb_decompress(&y, "/x.xnb").unwrap_err());
    assert!(
        e.starts_with("LZ4BlockError:") && !e.contains("passes"),
        "{e}"
    );
}

// ---- snowdrop_zlib: the cap on one bare cache file ---------------------------------------------------------------------------------------------------------------------------------

/// A TRUTHFUL zlib bomb: `chunks` consecutive zlib streams of `chunk` zeros each, the first one starting with the 0xabcd00d4 head that announces exactly `chunks * chunk` bytes.
/// The streams after the first are one encoding of `chunk` zeros, repeated: a 2 GiB bomb costs two encodings. Since nxnative e74c3fa7 every stream must decode to exactly 64 KB (the last one
/// to the rest), so a truthful bomb is made of 64 KB streams; a bigger stream is refused at its first block.
fn snowdrop_bomb(chunks: usize, chunk: usize) -> Vec<u8> {
    let total = chunks * chunk;
    let n = (total - 20) as u32;
    let enc = |z: &[u8]| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        e.write_all(z).unwrap();
        e.finish().unwrap()
    };
    let mut head = vec![0u8; chunk];
    for (k, v) in [0xabcd00d4u32, 0x13c, 0, n, n].iter().enumerate() {
        head[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut d = b"BERG".to_vec();
    d.extend_from_slice(&22u32.to_le_bytes());
    d.extend_from_slice(&[0u8; 48]);
    d.extend_from_slice(&enc(&head));
    let rest = enc(&vec![0u8; chunk]);
    for _ in 1..chunks {
        d.extend_from_slice(&rest);
    }
    d
}

#[test]
fn a_snowdrop_bomb_is_refused_at_the_cap_not_decoded_to_the_end() {
    let d = snowdrop_bomb(128, 1 << 16); // 8 MiB announced and real, in 64 KB streams, ~12 KB of input
    assert!(d.len() < 70_000, "the input is small: {}", d.len());
    let e = red(snowdrop_zlib_capped(&d, "/bomb", 2 << 20).unwrap_err());
    assert!(
        e.contains("decodes past 2097152 bytes") && e.contains("its head announces 8388608"),
        "{e}"
    );
    // the cap is the file's whole decoded size: a cap of exactly that size lets it through (it is zeros: no shader in it)
    assert_eq!(snowdrop_zlib_capped(&d, "/bomb", 8 << 20).unwrap(), vec![]);
    assert!(snowdrop_zlib_capped(&d, "/bomb", (8 << 20) - 1).is_err());
    // a stream that decodes to more than its 64 KB block is refused at once (e74c3fa7), whatever the cap
    let big = snowdrop_bomb(8, 1 << 20);
    let e = red(snowdrop_zlib_capped(&big, "/bomb", 8 << 20).unwrap_err());
    assert!(
        e.contains("block @0x38 did not finish at 65536 raw bytes"),
        "{e}"
    );
}

#[test]
fn a_bomb_far_over_the_cap_is_refused_at_the_cap_and_holds_little_memory() {
    // 4,096 x 64 KB = 256 MiB announced and real against a 16 MiB cap: refused before the block that would pass it. Uncapped this decodes all 256 MiB (~500 MB RSS); capped, the peak is the
    // test's own plus 16 MiB
    let d = snowdrop_bomb(4096, 1 << 16); // 256 MiB in 64 KB streams
    assert!(d.len() < 600_000, "{}", d.len());
    let e = red(snowdrop_zlib_capped(&d, "/bomb", 16 << 20).unwrap_err());
    assert!(
        e.contains("decodes past 16777216 bytes")
            && e.contains(&format!("its head announces {}", 4096u64 * (1 << 16))),
        "{e}"
    );
    let peak = ibuilder_testkit::peak_rss_bytes();
    assert!(peak < 320 << 20, "peak RSS {} MiB", peak >> 20);
}

/// a scan context with nothing in it (no dictionaries, no other files), for the readers that take one
fn with_cx<T>(f: impl FnOnce(&Ctx<'_>) -> T) -> T {
    let rules = KeyRules::default();
    let (trees, dicts, files, log) = (
        Trees::new(),
        HashMap::new(),
        FileIndex::empty(),
        |_: &str| {},
    );
    let cx = Ctx {
        rules: &rules,
        zstd: &Ruzstd,
        common: None,
        borrowed: &trees,
        zdicts: &dicts,
        log: &log,
        files: &files,
        bahs_var: RefCell::new(HashMap::new()),
    };
    f(&cx)
}

/// The real cap, through the public entry: it needs a real 1 GiB decode (~20 s and ~1 GiB RSS in a debug build), so it is `#[ignore]`d; the 2 GiB bomb is announced above the cap and the decode stops at it
#[test]
#[ignore = "decodes 1 GiB: about 20 s and 1 GiB of RAM in a debug build; run with -- --ignored"]
fn the_real_cap_bites_through_snowdrop_shaders() {
    let d = snowdrop_bomb(32768, 1 << 16); // 2 GiB announced and real, in 64 KB streams, ~4 MB of input
    assert!(d.len() < 6_000_000, "{}", d.len());
    let e = red(with_cx(|cx| snowdrop_shaders(cx, &d, "/bomb", true)).unwrap_err());
    assert!(e.contains(&format!("decodes past {SNOWDROP_ZLIB_MAX_RAW} bytes, the most one cache file may take (its head announces {})", 32768u64 * (1 << 16))), "{e}");
    // the decode stopped at the cap: the peak is ~the cap plus the turn in flight (measured ~1 GiB); the uncapped decode of this bomb peaks at ~2x its 2 GiB
    let peak = ibuilder_testkit::peak_rss_bytes();
    assert!(peak < 2 << 30, "peak RSS {} MiB", peak >> 20);
}

// ---- an LZ4 frame (`unsized_frame`): ~255:1, so 4 MB of input make 1 GiB -------------------------------------------------------------------------------------------------------

/// `mib` MiB of zeros as one LZ4 frame
fn lz4_zeros(mib: usize) -> Vec<u8> {
    let mut e = lz4_flex::frame::FrameEncoder::new(Vec::new());
    let z = vec![0u8; 1 << 20];
    for _ in 0..mib {
        e.write_all(&z).unwrap();
    }
    e.finish().unwrap()
}

#[test]
fn an_lz4_frame_stops_at_the_cap_and_nothing_under_it_changes() {
    let f = lz4_zeros(8); // 8 MiB out of ~33 KB
    assert!(f.len() < 100_000, "{}", f.len());
    let e = lz4_frame_capped(&f, 2 << 20).map(|v| v.len()).unwrap_err(); // (`map`: a failure prints a length, not 8 MiB of zeros)
    assert!(e.contains("2097152"), "{e}");
    assert!(
        lz4_frame_capped(&f, (8 << 20) - 1).is_err(),
        "one byte under the real size"
    );
    let exact = lz4_frame_capped(&f, 8 << 20).unwrap();
    assert_eq!(
        exact.len(),
        8 << 20,
        "a cap of exactly the real size lets it through"
    );
    assert_eq!(
        lz4_frame_capped(&f, MAX_DECODED).unwrap(),
        exact,
        "under the real cap: the same bytes"
    );
    // a damaged frame keeps the text it always had
    let e = lz4_frame_capped(&f[..f.len() - 20], 8 << 20)
        .map(|v| v.len())
        .unwrap_err();
    assert!(!e.contains("passes"), "{e}");
}

#[test]
fn a_size_prefixed_lz4_frame_under_the_cap_is_decoded_as_before() {
    let want = b"shader bytes ".repeat(1000);
    let mut d = (want.len() as u32).to_le_bytes().to_vec();
    d.extend_from_slice(&{
        let mut e = lz4_flex::frame::FrameEncoder::new(Vec::new());
        e.write_all(&want).unwrap();
        e.finish().unwrap()
    });
    assert_eq!(
        with_cx(|cx| unsized_frame(cx, &d, "/x.cmp").map(|c| c.into_owned())).unwrap(),
        want
    );
}
