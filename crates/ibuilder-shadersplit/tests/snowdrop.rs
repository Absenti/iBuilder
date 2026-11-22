//! Snowdrop's bare zlib cache files (`paches/engine/0104`, `shader_split.snowdrop_zlib`, marker `#SDFZLIB@`): the engine's own test cases (`dm/gfx/tests/test_snowdrop.py`: its two new ones and the
//! nine that were already there, rebuilt here from the same bytes), and two differential suites against the engine's Python (`tools/gen_snowdrop.py` writes `tests/data/vectors-snowdrop.jsonl.gz`):
//! `zlib` vectors (what `zlib.decompressobj().decompress(data[, max_length])` did with a byte string: the output, `eof`, the bytes used or the error text) for `zlibx`, and `berg` vectors
//! (a BERG data slice, cut, flipped and rewritten in every place a reader looks at: what `snowdrop_shaders` found, or the `[RED]` text) for the reader. The games under `tests/data/games/snowdrop*`
//! are split end to end by `tests/games.rs`.

use ibuilder_shadersplit::archive::{
    SNOWDROP_ZLIB_MAX_RAW, b64decode, snowdrop_shaders, snowdrop_zlib_capped,
};
use ibuilder_shadersplit::ctl::{CS_MAGIC, CTL_MAGIC, GFX_MAGIC, KeyRules, STAGES};
use ibuilder_shadersplit::scan::var_pairs_of;
use ibuilder_shadersplit::scan_types::{Ctx, Shader};
use ibuilder_shadersplit::split::FileIndex;
use ibuilder_shadersplit::unity::Trees;
use ibuilder_shadersplit::zlibx;
use ibuilder_shadersplit::{Failure, Ruzstd};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

// ---- the engine's builders (test_snowdrop.py) ------------------------------------------------------------------------------------------------

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn shader(stage: &str) -> (Vec<u8>, Vec<u8>) {
    let (mut ctl, mut code) = (vec![0u8; 0x880], vec![0u8; 0x100]);
    ctl[..4].copy_from_slice(&CTL_MAGIC);
    put32(&mut ctl, 0x20, 0x880);
    put32(&mut ctl, 0x704, 0x100);
    code[..4].copy_from_slice(if stage == "cs" { &CS_MAGIC } else { &GFX_MAGIC });
    if stage != "cs" {
        let i = STAGES.iter().position(|s| s.short() == stage).unwrap() as u32;
        put32(&mut code, 0x30, (i + 1) << 10 | 1);
    }
    if stage == "ps" {
        put32(&mut code, 0x48, 1); // reads generic location 0
    }
    (ctl, code)
}

fn both(stage: &str) -> Vec<u8> {
    let (c, k) = shader(stage);
    [c, k].concat()
}

fn lcg(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

/// a zstd frame of one raw block (what `compression.zstd.compress` of incompressible data would give, byte for byte not needed): the decoder is what is under test
fn zstd_raw(data: &[u8]) -> Vec<u8> {
    let mut f = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    assert!(data.len() < 1 << 17);
    f.extend_from_slice(&((data.len() as u32) << 3 | 1).to_le_bytes()[..3]);
    f.extend_from_slice(data);
    f
}

fn zlib_whole(raw: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    e.write_all(raw).unwrap();
    e.finish().unwrap()
}

/// `record(raw, flags, legacy)`: a c5 (zlib) or c6 (zstd) record with its 20-byte header
fn record(raw: &[u8], flags: u32, legacy: bool) -> Vec<u8> {
    let packed = if legacy {
        zlib_whole(raw)
    } else {
        zstd_raw(raw)
    };
    let mut r = Vec::new();
    for v in [
        if legacy { 0xabcd00c5u32 } else { 0xabcd00c6 },
        flags,
        0,
        raw.len() as u32,
        packed.len() as u32,
    ] {
        r.extend_from_slice(&v.to_le_bytes());
    }
    r.extend_from_slice(&packed);
    r
}

fn archive(parts: &[&[u8]]) -> Vec<u8> {
    let mut d = b"BERG".to_vec();
    d.extend_from_slice(&22u32.to_le_bytes());
    d.extend_from_slice(&[0u8; 48]);
    for p in parts {
        d.extend_from_slice(p);
    }
    d
}

/// a bare .mcache file: the 0xabcd00d4 head (kind, 0, twice the length that follows), then the body
fn cache(raw: &[u8], kind: u32) -> Vec<u8> {
    let mut d = Vec::new();
    for v in [0xabcd00d4u32, kind, 0, raw.len() as u32, raw.len() as u32] {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(raw);
    d
}

/// as the SDF data slices store a file: one zlib stream per 64 KB, a small window (zlib header 0x28 0x15)
fn zlib_blocks(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in data.chunks(0x10000) {
        out.extend_from_slice(&[0x28, 0x15]);
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
        e.write_all(ch).unwrap();
        out.extend_from_slice(&e.finish().unwrap());
        out.extend_from_slice(&zlibx::adler32(ch).to_be_bytes());
    }
    out
}

// ---- running the reader ------------------------------------------------------------------------------------------------------------------------

fn run(d: &[u8], label: &str) -> Result<Vec<Shader>, Failure> {
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
    snowdrop_shaders(&cx, d, label, true)
}

fn stages(found: &[Shader]) -> Vec<&'static str> {
    found.iter().map(|s| s.st.short()).collect()
}

fn red_text(e: Failure) -> String {
    match e {
        Failure::Red(m) => m,
        other => panic!("not a red(): {other:?}"),
    }
}

// ---- test_snowdrop.py -------------------------------------------------------------------------------------------------------------------------

#[test]
fn zlib_records_and_headerless_compute() {
    let (mut ctl, mut code) = shader("cs");
    ctl.truncate(0x838);
    let n = ctl.len() as u32;
    put32(&mut ctl, 0x20, n);
    for (i, v) in [8u32, 8, 1].iter().enumerate() {
        put32(&mut ctl, 0x748 + 4 * i, *v);
    }
    code[..4].copy_from_slice(&[0x11, 0x07, 0x20, 0xe2]);
    let r1 = record(
        &[b"x".to_vec(), ctl.clone(), code.clone()].concat(),
        0x48,
        true,
    );
    let r2 = record(&both("ps"), 0x48, false);
    let found = run(&archive(&[&r1, &r2]), "/both encodings").unwrap();
    assert_eq!(stages(&found), ["cs", "ps"]);
    assert_eq!((&found[0].ctl, &found[0].code), (&ctl, &code));
}

#[test]
fn zlib_cache_records_inside_zstd_archive_chunks() {
    let raw = zstd_raw(&record(&both("ps"), 0x48, true));
    let found = run(
        &archive(&[&raw, &record(&both("vs"), 0x48, false)]),
        "/nested",
    )
    .unwrap();
    assert_eq!(stages(&found), ["vs", "ps"]);
    let nested: Vec<_> = found
        .iter()
        .filter(|f| f.label.contains("#SDFZSTD@"))
        .collect();
    assert_eq!(nested.len(), 1);
    assert_eq!(
        (&nested[0].ctl, &nested[0].code),
        (&shader("ps").0, &shader("ps").1)
    );
}

#[test]
fn wrapped_record_crossing_zstd_block_boundaries() {
    // the engine's case is an incompressible cache across several zstd blocks (zstd's 128 KB block size); a raw-block frame is one block per 128 KB, so build the frame of several
    let mut inner = record(&[lcg(0, 0x50000), both("ps")].concat(), 0x48, true);
    inner.extend_from_slice(&[]);
    let mut f = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
    f.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    let chunks: Vec<&[u8]> = inner.chunks(0x20000).collect();
    for (i, c) in chunks.iter().enumerate() {
        let last = (i + 1 == chunks.len()) as u32;
        f.extend_from_slice(&((c.len() as u32) << 3 | last).to_le_bytes()[..3]);
        f.extend_from_slice(c);
    }
    let found = run(&archive(&[&f]), "/large wrapped cache").unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].label.contains("#SDFZSTD@"));
    assert_eq!(
        (&found[0].ctl, &found[0].code),
        (&shader("ps").0, &shader("ps").1)
    );
}

#[test]
fn false_frame_magic_and_non_shader_chunks_are_ignored() {
    let data = archive(&[
        &[&[0x28u8, 0xb5, 0x2f, 0xfd][..], b"not a frame"].concat(),
        &zstd_raw(b"other asset"),
    ]);
    assert_eq!(run(&data, "/other assets").unwrap(), vec![]);
}

#[test]
fn byte_packed_secondary_stages() {
    // a control can be at an unaligned offset; a single asset may hold two stages
    let raw = [
        b"asset reflection\0x".to_vec(),
        both("vs"),
        b"variant".to_vec(),
        both("ps"),
    ]
    .concat();
    let r2 = record(&[b"x".to_vec(), both("cs")].concat(), 0x404, false);
    let found = run(
        &archive(&[&record(&raw, 0x48, false), &r2]),
        "/arbitrary-name",
    )
    .unwrap();
    assert_eq!(stages(&found), ["vs", "ps", "cs"]);
    for f in &found {
        let (c, k) = shader(f.st.short());
        assert_eq!((&f.ctl, &f.code), (&c, &k));
    }
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
    assert!(var_pairs_of(&cx, &found).unwrap().is_empty());
}

#[test]
fn bare_frames_and_false_control_magics_are_not_shaders() {
    let data = archive(&[
        &zstd_raw(&both("ps")),
        &record(
            &[b"other asset".to_vec(), CTL_MAGIC.to_vec(), vec![0u8; 8]].concat(),
            0x48,
            false,
        ),
    ]);
    assert_eq!(run(&data, "/other assets").unwrap(), vec![]);
}

#[test]
fn record_bounds_and_decoded_length_are_checked() {
    for legacy in [false, true] {
        let data = archive(&[&record(&both("vs"), 0x48, legacy)]);
        let e = red_text(run(&data[..data.len() - 1], "/truncated").unwrap_err());
        assert!(e.contains("stored bytes out of bounds"), "{e}");
        let mut bad = data.clone();
        put32(&mut bad, 56 + 12, 1);
        let e = red_text(run(&bad, "/bad raw size").unwrap_err());
        assert!(e.contains("expected 1 raw bytes"), "{e}");
    }
}

#[test]
fn bare_zlib_cache_files() {
    // Older SDF slices keep each .mcache as a bare zlib stream with no c5 / c6 record header: one small, one over 64 KB (two streams), and other assets
    // (a zlib stream that is not a cache, uncompressed bytes) between them
    let small = cache(&[b"reflection\0".to_vec(), both("vs")].concat(), 0x13c);
    let big = cache(&[lcg(1, 0x12000), both("ps"), both("cs")].concat(), 0x65);
    let other = b"other asset".repeat(100);
    let data = archive(&[
        &zlib_blocks(&small),
        &zlib_blocks(&other),
        b"\x28\x15 not a stream",
        &zlib_blocks(&big),
        &zlib_blocks(&[b'x'; 50]),
    ]);
    let found = run(&data, "/bare").unwrap();
    assert_eq!(stages(&found), ["vs", "ps", "cs"]);
    assert!(
        found.iter().all(|f| f.label.contains("#SDFZLIB@")),
        "{:?}",
        found.iter().map(|f| &f.label).collect::<Vec<_>>()
    );
    for f in &found {
        let (c, k) = shader(f.st.short());
        assert_eq!((&f.ctl, &f.code), (&c, &k));
    }
}

#[test]
fn bare_zlib_cache_cut_short_is_red() {
    let big = zlib_blocks(&cache(&[lcg(2, 0x18000), both("vs")].concat(), 0x13c));
    let first = zlibx::decompress(&big, None).unwrap();
    assert!(first.eof);
    let e = run(&archive(&[&big[..first.used]]), "/short").unwrap_err(); // the first 64 KB stream only
    assert!(matches!(e, Failure::Red(_)), "{e:?}");
}

#[test]
fn bare_zlib_labels_and_order() {
    // the bare caches are read first (`out = snowdrop_zlib(d, label)`), then the c5 / c6 records; labels carry the stream's offset and the shader's offset in the decoded record
    let c = cache(&[b"x".to_vec(), both("vs")].concat(), 0x13c);
    let data = archive(&[&record(&both("ps"), 0x48, true), &zlib_blocks(&c)]);
    let found = run(&data, "/f").unwrap();
    assert_eq!(stages(&found), ["vs", "ps"]);
    let at = 56 + record(&both("ps"), 0x48, true).len();
    assert_eq!(found[0].label, format!("/f#SDFZLIB@{at:#x}:{:#x}", 20 + 1));
    assert!(
        found[1].label.starts_with("/f#SNOWDROP@0x38:"),
        "{}",
        found[1].label
    );
}

// ---- review r2 4.1: a bare cache file may not decode to more than the cap ---------------------------------------------------------------------

#[test]
fn the_cap_changes_nothing_under_it() {
    let small = cache(&[b"reflection\0".to_vec(), both("vs")].concat(), 0x13c);
    let big = cache(&[lcg(1, 0x12000), both("ps"), both("cs")].concat(), 0x65);
    let data = archive(&[
        &zlib_blocks(&small),
        b"\x28\x15 not a stream",
        &zlib_blocks(&big),
    ]);
    let want = run(&data, "/bare").unwrap();
    assert_eq!(stages(&want), ["vs", "ps", "cs"]);
    // any cap from the largest file's exact size up to "no cap" gives the same shaders; one byte under it refuses the file
    let exact = big.len();
    for cap in [exact, exact + 1, 1 << 20, SNOWDROP_ZLIB_MAX_RAW, usize::MAX] {
        assert_eq!(
            snowdrop_zlib_capped(&data, "/bare", cap).unwrap(),
            want,
            "cap {cap}"
        );
    }
    let e = red_text(snowdrop_zlib_capped(&data, "/bare", exact - 1).unwrap_err());
    assert!(
        e.contains(&format!("decodes past {} bytes", exact - 1))
            && e.contains(&format!("its head announces {exact}")),
        "{e}"
    );
}

// ---- differential: zlibx against zlib ----------------------------------------------------------------------------------------------------------

fn vectors() -> Option<Vec<Value>> {
    let p = format!(
        "{}/tests/data/vectors-snowdrop.jsonl.gz",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read(p).ok()?;
    let text =
        String::from_utf8(ibuilder_shadersplit::inflate::gunzip(&raw, 1 << 30).unwrap()).unwrap();
    Some(
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// This Mac's zlib (Apple's 1.2.12) words an invalid code met by its fast path as one message; zlib's slow path, and upstream's fast path, give the two
fn zmsg(s: &str) -> String {
    s.replace("invalid literal/length/distance code", "invalid code")
        .replace("invalid literal/length code", "invalid code")
        .replace("invalid distance code", "invalid code")
}

fn zlib_outcome(py: &Value, rust: Result<zlibx::Z, String>, what: &str) {
    match (py.get("ok"), rust) {
        (Some(o), Ok(z)) => {
            assert_eq!(
                o["len"].as_u64().unwrap() as usize,
                z.out.len(),
                "{what}: output length"
            );
            assert_eq!(o["sha"].as_str().unwrap(), sha(&z.out), "{what}: output");
            assert_eq!(o["eof"].as_bool().unwrap(), z.eof, "{what}: eof");
            if z.eof {
                assert_eq!(
                    o["used"].as_u64().unwrap() as usize,
                    z.used,
                    "{what}: bytes used"
                );
            }
        }
        (Some(o), Err(e)) => panic!("{what}: zlib answered {o}, this reader failed: {e}"),
        (None, Ok(z)) => panic!(
            "{what}: zlib raised {}, this reader answered {} bytes (eof {})",
            py["err"],
            z.out.len(),
            z.eof
        ),
        (None, Err(e)) => assert_eq!(
            zmsg(py["err"].as_str().unwrap()),
            zmsg(&e),
            "{what}: error text"
        ),
    }
}

#[test]
fn zlibx_is_zlib() {
    let Some(vecs) = vectors() else {
        ibuilder_testkit::skip(
            "tests/data/vectors-snowdrop.jsonl.gz is not there (run tools/gen_snowdrop.py)",
        );
        return;
    };
    let mut n = 0;
    for (i, v) in vecs.iter().enumerate().filter(|(_, v)| v["kind"] == "zlib") {
        let d = b64decode(v["in"]["b64"].as_str().unwrap().as_bytes()).unwrap();
        zlib_outcome(
            &v["out"]["full"],
            zlibx::decompress(&d, None),
            &format!("vector {i} (whole)"),
        );
        for (l, o) in v["out"]["lim"].as_object().unwrap() {
            let lim: usize = l.parse().unwrap();
            zlib_outcome(
                o,
                zlibx::decompress(&d, Some(lim)),
                &format!("vector {i} (max_length {lim})"),
            );
        }
        n += 1;
    }
    eprintln!(
        "{n} zlib streams, whole and cut at 1, 7, 20 and 100 bytes of output: the same as zlib's"
    );
}

// ---- differential: the reader against the engine's `snowdrop_shaders` --------------------------------------------------------------------------------

#[test]
fn snowdrop_shaders_is_the_engines() {
    let Some(vecs) = vectors() else {
        ibuilder_testkit::skip(
            "tests/data/vectors-snowdrop.jsonl.gz is not there (run tools/gen_snowdrop.py)",
        );
        return;
    };
    let (mut ok, mut red, mut zstd_cases) = (0, 0, 0);
    let mut bad: Vec<String> = Vec::new();
    for (i, v) in vecs.iter().enumerate().filter(|(_, v)| v["kind"] == "berg") {
        let d = b64decode(v["in"]["b64"].as_str().unwrap().as_bytes()).unwrap();
        let (py, rust) = (&v["out"], run(&d, "/v"));
        let mut diff: Option<String> = None;
        match (py.get("ok"), rust) {
            (Some(o), Ok(found)) => {
                let got: Vec<Value> = found
                    .iter()
                    .map(|s| {
                        serde_json::json!([
                            s.label,
                            s.st.short(),
                            sha(&s.ctl),
                            sha(&s.code),
                            s.ctl.len(),
                            s.code.len()
                        ])
                    })
                    .collect();
                if o == &Value::Array(got.clone()) {
                    ok += 1;
                } else {
                    diff = Some(format!(
                        "berg vector {i}: Python found {o}, Rust {}",
                        Value::Array(got)
                    ));
                }
            }
            (Some(o), Err(e)) => {
                diff = Some(format!(
                    "berg vector {i}: Python found {} shaders, Rust failed: {e}",
                    o.as_array().unwrap().len()
                ))
            }
            (None, Ok(f)) => {
                diff = Some(format!(
                    "berg vector {i}: Python raised {}, Rust found {} shaders",
                    py["err"],
                    f.len()
                ))
            }
            (None, Err(e)) => {
                let want = py["err"].as_str().unwrap();
                // red() lines are the same text; the other exceptions (the zlib and zstd libraries', raised uncaught by the older record readers) only have to be errors
                if let Some(w) = want.strip_prefix("[RED] ") {
                    if !w.contains("Zstandard data") {
                        if zmsg(w) != zmsg(&e.to_string()) {
                            diff = Some(format!(
                                "berg vector {i}: red() text: Python {w:?}, Rust {:?}",
                                e.to_string()
                            ));
                        }
                    } else if !matches!(e, Failure::Red(_)) {
                        diff = Some(format!("berg vector {i}: {e:?}"));
                    }
                    red += 1;
                }
            }
        }
        if let Some(m) = diff {
            // libzstd and ruzstd answer differently for some damaged frames (a block larger than the frame's content size, a frame cut inside a compressed block): a slice with a zstd frame in it
            // is allowed to differ when one of the two ended in an error; the same shaders are required wherever both succeed
            let has_zstd = d
                .windows(4)
                .any(|w| w == ibuilder_shadersplit::zstd::ZSTD_MAGIC);
            let one_failed = py.get("ok").is_none() || m.contains("Rust failed");
            if has_zstd && one_failed {
                zstd_cases += 1;
                eprintln!("zstd decoder difference (allowed): {m}");
            } else {
                bad.push(m);
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{} of the berg vectors differ, the first ones:\n{}",
        bad.len(),
        bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(
        zstd_cases <= 3,
        "{zstd_cases} slices with zstd frames differ: more than the known libzstd / ruzstd differences"
    );
    eprintln!(
        "berg slices: {ok} found the engine's shaders, {red} ended with its [RED] text, {zstd_cases} differ in a damaged zstd frame (libzstd and ruzstd disagree)"
    );
}

// ---- nxnative e74c3fa7: every block of a bare cache decodes to exactly 64 KB (the last one to the rest) -----------------------------------------------------------------------------

#[test]
fn a_bare_cache_is_read_in_blocks_of_exactly_64_kb() {
    let total = 0x18000usize; // 96 KB: one block of 64 KB, one of 32 KB
    let mut raw = vec![0u8; total];
    for (k, v) in [
        0xabcd00d4u32,
        0x13c,
        0,
        (total - 20) as u32,
        (total - 20) as u32,
    ]
    .iter()
    .enumerate()
    {
        raw[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    let z = |b: &[u8]| {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
        e.write_all(b).unwrap();
        e.finish().unwrap()
    };
    let slice = |cuts: &[usize]| {
        let mut d = b"BERG".to_vec();
        d.extend_from_slice(&22u32.to_le_bytes());
        d.extend_from_slice(&[0u8; 48]);
        let mut at = 0;
        for &c in cuts {
            d.extend_from_slice(&z(&raw[at..c]));
            at = c;
        }
        d
    };
    // the engine's layout: read (zeros: no shader in it)
    assert_eq!(
        snowdrop_zlib_capped(&slice(&[0x10000, total]), "/s", 1 << 30).unwrap(),
        vec![]
    );
    // two 48 KB streams: the first block is short of its 64 KB (the reader before e74c3fa7 took it)
    let e = red_text(snowdrop_zlib_capped(&slice(&[0xc000, total]), "/s", 1 << 30).unwrap_err());
    assert_eq!(
        e,
        "/s Snowdrop zlib record @0x38: block @0x38 did not finish at 65536 raw bytes"
    );
    // one stream of all 96 KB: more than its block
    let e = red_text(snowdrop_zlib_capped(&slice(&[total]), "/s", 1 << 30).unwrap_err());
    assert!(e.ends_with("did not finish at 65536 raw bytes"), "{e}");
}
