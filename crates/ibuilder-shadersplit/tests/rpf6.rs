//! Rockstar RPF6 (`rpf6.rs`, `aes.rs`): the synthetic archive of tests/data/games/sync-rpf6 damaged in 400 ways, damaged key setups and key windows, and AES-256 blocks, against what
//! the engine's own rpf6.py (and CommonCrypto) answered for each (tests/data/vectors-rpf6.jsonl.gz, tools/gen_sync_rpf6.py). The key is a made-up one. No game data.

use ibuilder_shadersplit::Ruzstd;
use ibuilder_shadersplit::aes::Aes256;
use ibuilder_shadersplit::bytes::{hex, unhex};
use ibuilder_shadersplit::rpf6;
use ibuilder_shadersplit::source::MemSource;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// the made-up key of tools/gen_sync_rpf6.py
fn key() -> [u8; 32] {
    std::array::from_fn(|i| ((0x3b * i + 0x11) & 0xff) as u8)
}

fn h16(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

/// libzstd's words for a damaged frame are not the port's to copy
fn norm(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(match s.find("ZstdError: ") {
            Some(i) => format!("{}zstd refused", &s[..i]),
            None => s.clone(),
        }),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        v => v.clone(),
    }
}

fn outcome(d: &[u8]) -> Value {
    let src = MemSource(d);
    let r = match rpf6::open(&src, "P", &key()) {
        Ok(r) => r,
        Err(e) => return json!({"err": e}),
    };
    let files: Vec<Value> = r
        .files
        .iter()
        .map(|e| {
            let row = json!([e.path, e.off, e.stored, e.size, e.comp, e.rflags]);
            match rpf6::read(&Ruzstd, &src, "P", &key(), e) {
                Ok(x) => json!([row, {"ok": h16(&x)}]),
                Err(x) => json!([row, {"err": x}]),
            }
        })
        .collect();
    json!({ "ok": files })
}

fn damaged(base: &[u8], r: &Value) -> Vec<u8> {
    let mut b = base.to_vec();
    for m in r["muts"].as_array().unwrap() {
        b[m[0].as_u64().unwrap() as usize] = m[1].as_u64().unwrap() as u8;
    }
    if let Some(c) = r["cut"].as_u64() {
        b.truncate(c as usize);
    }
    b
}

#[test]
fn damaged_archives_and_key_setups_read_as_the_python_reads_them() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let raw = std::fs::read(data.join("vectors-rpf6.jsonl.gz"))
        .expect("tests/data/vectors-rpf6.jsonl.gz (tools/gen_sync_rpf6.py)");
    let mut s = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut s)
        .unwrap();
    let shaders = std::fs::read(data.join("games/sync-rpf6/RomFS/Data/shaders.rpf")).unwrap();
    let blk = shaders[16..32].to_vec();
    let (mut with_bad, mut setup, mut window) = (
        Vec::new(),
        (Vec::new(), (0u32, 0u32)),
        (Vec::new(), [(0u32, 0u32); 3]),
    );
    let (mut n, mut bad) = (0usize, Vec::new());
    for line in s.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let hx = |k: &str| unhex(r[k].as_str().unwrap()).unwrap();
        let got = match r["kind"].as_str().unwrap() {
            "aes" => {
                let mut ct = hx("ct");
                Aes256::new(&hx("key").try_into().unwrap()).decrypt(&mut ct);
                assert_eq!(
                    hex(&ct),
                    r["pt"].as_str().unwrap(),
                    "AES-256 against CommonCrypto"
                );
                continue;
            }
            "rpf-base" => {
                with_bad = hx("hex");
                continue;
            }
            "setup-base" => {
                let t = r["text"].as_array().unwrap();
                setup = (
                    hx("hex"),
                    (t[0].as_u64().unwrap() as u32, t[1].as_u64().unwrap() as u32),
                );
                continue;
            }
            "window-base" => {
                let sg = r["segments"].as_array().unwrap();
                let seg = |i: usize| {
                    (
                        sg[i][0].as_u64().unwrap() as u32,
                        sg[i][1].as_u64().unwrap() as u32,
                    )
                };
                window = (hx("hex"), [seg(0), seg(1), seg(2)]);
                continue;
            }
            "rpf" => outcome(&damaged(
                if r["base"] == "with_bad" {
                    &with_bad
                } else {
                    &shaders
                },
                &r,
            )),
            "setup" => {
                let (c, e) = rpf6::key_from_setup(&damaged(&setup.0, &r), setup.1);
                match e {
                    // the Python's generator raises when it reaches that setup: what it yielded before is not seen
                    Some(e) => json!({"err": e}),
                    None => json!({"ok": c.iter().map(|k| hex(k)).collect::<Vec<_>>()}),
                }
            }
            "window" => match rpf6::find_key(&damaged(&window.0, &r), &window.1, &blk) {
                Ok(k) => json!(k.map(|k| hex(&k))),
                Err(e) => json!({"err": e}),
            },
            k => panic!("vector kind {k}"),
        };
        n += 1;
        if norm(&got) != norm(&r["out"]) {
            bad.push(format!(
                "{} {:?} cut {:?}:\n  python {}\n  rust   {}",
                r["kind"], r["muts"], r["cut"], r["out"], got
            ));
        }
    }
    assert!(n > 600, "{n} vectors");
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn every_prefix_of_an_archive_reads_without_a_panic() {
    let d = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/games/sync-rpf6/RomFS/Data/shaders.rpf"),
    )
    .unwrap();
    for cut in (0..d.len().min(0x900)).step_by(3) {
        let _ = outcome(&d[..cut]);
    }
    for cut in (0x900..d.len()).step_by(17) {
        let _ = outcome(&d[..cut]);
    }
}

#[test]
fn names_and_hashes() {
    assert_eq!(rpf6::joaat(""), 0);
    assert_eq!(rpf6::joaat("a"), 0xca2e9442);
    assert_eq!(rpf6::joaat("Shaders.RPF"), rpf6::joaat("shaders.rpf"));
    // the regex's rule: a printable run followed by NUL, at most its last 255 characters; a run that no NUL ends gives nothing
    let mut nt = b"\0\0ab\0cd\x01ef\0".to_vec();
    nt.extend(std::iter::repeat_n(b'x', 300));
    nt.push(0);
    let v = rpf6::names_of(&nt);
    assert_eq!(v[0], b"ab");
    assert_eq!(v[1], b"ef");
    assert_eq!(v[2].len(), 255);
    assert_eq!(v.len(), 3);
}

#[test]
#[ignore] // a measurement: cargo test --release --test rpf6 -- --ignored --nocapture
fn key_window_search_speed() {
    // 4 MiB of random bytes as the data segment, no key in it: every window is tried
    let mut x = 0x9e3779b97f4a7c15u64;
    let img: Vec<u8> = (0..4 << 20)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 56) as u8
        })
        .collect();
    let t = std::time::Instant::now();
    let r = rpf6::find_key(&img, &[(0, 0), (0, 0), (0, 4 << 20)], &[7u8; 16]).unwrap();
    assert!(r.is_none());
    let s = t.elapsed().as_secs_f64();
    eprintln!(
        "{} windows in {s:.2} s: {:.0} windows/s",
        (4 << 20) / 4,
        ((4 << 20) / 4) as f64 / s
    );
}
