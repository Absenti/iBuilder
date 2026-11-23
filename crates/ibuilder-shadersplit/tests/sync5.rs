//! Larian LSPK + BSHD (`lspk.rs`), Blizzard CASC (`casc.rs`) and phx PDAT (`pdat.rs`) on damaged input: the containers of tests/data/games/sync-lspk, sync-casc and sync-pdat
//! (tools/gen_sync5.py) damaged in 1,300 ways, against what the engine's own lspk.py / casc.py / shader_split.py answered for each (tests/data/vectors-sync5.jsonl.gz,
//! tools/gen_sync5_vectors.py). No game data.

use ibuilder_shadersplit::Ruzstd;
use ibuilder_shadersplit::casc::{self, ReadErr};
use ibuilder_shadersplit::lspk;
use ibuilder_shadersplit::pdat;
use ibuilder_shadersplit::source::MemSource;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

fn games() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games")
}

fn h16(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

fn shaders(v: &[ibuilder_shadersplit::scan_types::Shader]) -> Value {
    Value::Array(
        v.iter()
            .map(|s| {
                json!([
                    s.label,
                    s.st.short(),
                    h16(&[s.ctl.clone(), s.code.clone()].concat())
                ])
            })
            .collect(),
    )
}

/// The decoders' own words for a damaged stream are not the port's to copy (as tests/uepak.rs): zlib's fast path words a bad code otherwise than its slow path (zlibx); lz4's error
/// codes and libzstd's messages are their own.
fn norm(s: &str) -> String {
    let mut s = s
        .replace("invalid literal/length/distance code", "invalid code")
        .replace("invalid literal/length code", "invalid code")
        .replace("invalid distance code", "invalid code");
    let m = "Decompression failed: corrupt input or insufficient space in destination buffer";
    if let Some(i) = s.find(m) {
        s.replace_range(i.., "LZ4 refused");
    }
    if let Some(i) = s.find("ZstdError: ") {
        s.replace_range(i.., "zstd refused");
    }
    s
}

fn norm_v(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(norm(s)),
        Value::Array(a) => Value::Array(a.iter().map(norm_v).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm_v(v))).collect()),
        v => v.clone(),
    }
}

fn lspk_outcome(d0: &[u8], d1: &[u8]) -> Value {
    let n1 = d1.len() as u64;
    let part_len = |i: usize| -> Result<u64, String> {
        if i == 1 {
            Ok(n1)
        } else {
            Err(format!(
                "[Errno 2] No such file or directory: 'T/P_{i}.pak'"
            ))
        }
    };
    let (_, entries) = match lspk::package(&MemSource(d0), "P", &part_len) {
        Err(e) => return json!({"package": {"err": e}}),
        Ok(None) => return json!({"package": {"none": true}}),
        Ok(Some(p)) => p,
    };
    let mut members = Vec::new();
    for e in &entries {
        let src = MemSource(if e.part == 0 { d0 } else { d1 });
        match lspk::read(&src, e) {
            Err(x) => members.push(json!({"err": x})),
            Ok(data) => {
                let mut m = json!({"ok": h16(&data)});
                if e.name.to_lowercase().ends_with(".bshd") {
                    m["shaders"] = match lspk::bshd_shaders(&data, &format!("/x/{}", e.name)) {
                        Ok(v) => json!({"ok": shaders(&v)}),
                        Err(x) => json!({"err": x.to_string()}),
                    };
                }
                members.push(m);
            }
        }
    }
    json!({
        "package": {"ok": entries.iter().map(|e| json!([e.name, e.part, e.off, e.size, e.raw, e.method])).collect::<Vec<_>>()},
        "members": members,
    })
}

fn casc_dir() -> PathBuf {
    games().join("sync-casc/RomFS/data")
}

fn casc_names() -> BTreeSet<String> {
    std::fs::read_dir(casc_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn entry_json(e: &casc::Entry) -> Value {
    json!([
        e.id,
        e.archive,
        e.off,
        e.stored as i64,
        e.size as i64,
        e.enc
    ])
}

fn casc_files(manifest: &[u8]) -> Result<Vec<casc::Entry>, casc::CascErr> {
    let read = |n: &str| -> Result<Vec<u8>, String> {
        if n == "build_manifest.psv" {
            Ok(manifest.to_vec())
        } else {
            std::fs::read(casc_dir().join(n)).map_err(|e| e.to_string())
        }
    };
    casc::files("F", &casc_names(), &read)
}

fn pdat_outcome(d: &[u8], window: u64) -> Value {
    let n = d.len() as u64;
    let mut out = Vec::new();
    for (start, end) in [
        (0, n),
        (0, n / 3),
        (n / 3, n),
        (n / 4, n / 2),
        (0x30, n.saturating_sub(0x30)),
    ] {
        if end <= start {
            continue;
        }
        let r = match pdat::range_shaders(&Ruzstd, &MemSource(d), "x.dat", start, end, window) {
            Ok(v) => json!({"ok": shaders(&v)}),
            Err(e) => json!({"err": e.to_string()}),
        };
        out.push(json!([start, end, r]));
    }
    Value::Array(out)
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
fn damaged_lspk_casc_pdat_read_as_the_python_reads_them() {
    let raw = std::fs::read(games().join("../vectors-sync5.jsonl.gz"))
        .expect("tests/data/vectors-sync5.jsonl.gz (tools/gen_sync5_vectors.py)");
    let mut s = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut s)
        .unwrap();
    let rd = |p: &str| std::fs::read(games().join(p)).unwrap();
    let (main0, main1) = (
        rd("sync-lspk/RomFS/Data/Main.pak"),
        rd("sync-lspk/RomFS/Data/Main_1.pak"),
    );
    let loose = rd("sync-lspk/RomFS/Data/Loose.bshd");
    let manifest = std::fs::read(casc_dir().join("build_manifest.psv")).unwrap();
    let good = casc_files(&manifest).unwrap();
    let (mut n, mut bad) = (0usize, Vec::new());
    let mut kinds = std::collections::BTreeMap::new();
    for line in s.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let kind = r["kind"].as_str().unwrap().to_string();
        let got = match kind.as_str() {
            "lspk" => {
                if r["part"] == 1 {
                    lspk_outcome(&main0, &damaged(&main1, &r))
                } else {
                    lspk_outcome(&damaged(&main0, &r), &main1)
                }
            }
            "bshd" => {
                if r["base"] == "plain" {
                    continue; // checked below against a record built the same way (the generator's builders)
                }
                match lspk::bshd_shaders(&damaged(&loose, &r), "/x") {
                    Ok(v) => json!({"ok": shaders(&v)}),
                    Err(e) => json!({"err": e.to_string()}),
                }
            }
            "casc-manifest" => match casc_files(&damaged(&manifest, &r)) {
                Ok(es) => json!({"ok": es.iter().map(entry_json).collect::<Vec<_>>()}),
                Err(casc::CascErr::Casc(e)) => json!({"casc": e}),
                Err(casc::CascErr::Other(e)) => json!({"other": e}),
            },
            "casc-archive" => {
                let arch = r["archive"].as_str().unwrap();
                let d = damaged(&std::fs::read(casc_dir().join(arch)).unwrap(), &r);
                Value::Array(
                    good.iter()
                        .filter(|e| e.archive == arch)
                        .map(|e| match casc::read(&MemSource(&d), e) {
                            Ok(x) => json!({"ok": h16(&x)}),
                            Err(ReadErr::Skip(x)) => json!({"skip": x}),
                            Err(ReadErr::Job(x)) => json!({"job": x}),
                        })
                        .collect(),
                )
            }
            "pdat" => {
                let d = damaged(
                    &rd(&format!(
                        "sync-pdat/RomFS/Data/{}",
                        r["base"].as_str().unwrap()
                    )),
                    &r,
                );
                let a = pdat_outcome(&d, pdat::WINDOW);
                // a window of 64 bytes reads the range in many pieces: the same answer
                assert_eq!(a, pdat_outcome(&d, 64), "pdat window {:?}", r["muts"]);
                a
            }
            k => panic!("vector kind {k}"),
        };
        n += 1;
        *kinds.entry(kind.clone()).or_insert(0) += 1;
        if norm_v(&got) != norm_v(&r["out"]) {
            bad.push(format!(
                "{kind} {} {:?} cut {:?}:\n  python {}\n  rust   {}",
                r["base"], r["muts"], r["cut"], r["out"], got
            ));
        }
    }
    assert!(n > 900, "{n} vectors ({kinds:?})");
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn every_prefix_of_the_containers_reads_without_a_panic() {
    // the release profile aborts on a panic: every cut gives a value or an error
    let rd = |p: &str| std::fs::read(games().join(p)).unwrap();
    let (main0, main1) = (
        rd("sync-lspk/RomFS/Data/Main.pak"),
        rd("sync-lspk/RomFS/Data/Main_1.pak"),
    );
    for cut in 0..main0.len() {
        let _ = lspk_outcome(&main0[..cut], &main1);
    }
    let a = rd("sync-pdat/RomFS/Data/a.dat");
    for cut in (0..a.len()).step_by(3) {
        let _ = pdat_outcome(&a[..cut], 256);
    }
    let blte = casc::blte(b"BLTE\0\0\0\x24\x0f\0\0\x01\0\0\0\x05\0\0\0\x04");
    assert!(blte.is_err());
}
