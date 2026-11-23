//! Camelot's CRA archive and NARC containers (`cra.rs`): the synthetic NROs, archives and containers of tests/data/games/sync-cra and sync-cra2021 (tools/gen_sync_cra.py) damaged in
//! 700 ways, against what the engine's own cra.py answered for each (tests/data/vectors-cra.jsonl.gz). No game data.

use ibuilder_shadersplit::Ruzstd;
use ibuilder_shadersplit::bytes::unhex;
use ibuilder_shadersplit::cra::{self, CraErr, Entry};
use ibuilder_shadersplit::source::MemSource;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

fn h16(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

/// the decoders' own words for a damaged stream (lz4, libzstd) are not the port's to copy
fn norm(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(if s.starts_with("LZ4BlockError: ") {
            "LZ4BlockError".to_string()
        } else if s.starts_with("ZstdError: ") {
            "ZstdError".to_string()
        } else {
            s.clone()
        }),
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
        v => v.clone(),
    }
}

fn entry_json(e: &Entry) -> Value {
    json!([
        e.i,
        e.path,
        e.size,
        e.off,
        e.sum,
        if e.y2021 { "cra2021" } else { "cra" }
    ])
}

fn typ(t: &str) -> String {
    t.split(':').next().unwrap_or(t).to_string()
}

fn files_of(n: &[u8]) -> Value {
    match cra::index_files(n) {
        Ok(es) => json!({"ok": es.iter().map(entry_json).collect::<Vec<_>>()}),
        Err((true, CraErr::Cra(m))) => json!({"skip": m}),
        Err((_, CraErr::Cra(_))) => json!({"crash": "Error"}),
        Err((_, CraErr::Other(t))) => json!({"crash": typ(&t)}),
    }
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
fn damaged_tables_archives_and_containers_read_as_the_python_reads_them() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let raw = std::fs::read(data.join("vectors-cra.jsonl.gz"))
        .expect("tests/data/vectors-cra.jsonl.gz (tools/gen_sync_cra.py)");
    let mut s = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut s)
        .unwrap();
    let a42 = std::fs::read(data.join("games/sync-cra/RomFS/42")).unwrap();
    let (mut nro, mut nro2021, mut a2021) = (Vec::new(), Vec::new(), Vec::new());
    // the NARC containers: the files of the archive, unscrambled
    let mut narcs: std::collections::HashMap<String, Vec<u8>> = Default::default();
    let mut members: std::collections::HashMap<String, Vec<u8>> = Default::default();
    let (mut n, mut bad, mut hangs) = (0usize, Vec::new(), 0usize);
    for line in s.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let got = match r["kind"].as_str().unwrap() {
            "bases" => {
                let hx = |k: &str| unhex(r[k].as_str().unwrap()).unwrap();
                (nro, nro2021, a2021) = (hx("nro"), hx("nro2021"), hx("a2021"));
                for e in cra::index_files(&nro).unwrap() {
                    if let Ok(d) = cra::read(&MemSource(&a42), &e) {
                        narcs.insert(e.path.clone(), d);
                    }
                }
                continue;
            }
            "index" => {
                let base = if r["base"] == "nro" { &nro } else { &nro2021 };
                let got = files_of(&damaged(base, &r));
                if r["out"].get("hang").is_some() {
                    // the Python loops for ever on this prefix chain; the port stops it
                    hangs += 1;
                    assert_eq!(got, json!({"crash": "RecursionError"}), "{:?}", r["muts"]);
                    continue;
                }
                got
            }
            "read" => {
                let (base, ents) = if r["base"] == "42" {
                    (&a42, cra::index_files(&nro).unwrap())
                } else {
                    (&a2021, cra::index_files(&nro2021).unwrap())
                };
                let d = damaged(base, &r);
                let src = MemSource(&d);
                Value::Array(
                    ents.iter()
                        .map(|e| {
                            let hd = match cra::head(&src, e) {
                                Ok(h) => h16(&h),
                                Err(x) => x.text(),
                            };
                            match cra::read(&src, e) {
                                Ok(x) => json!([hd, {"ok": h16(&x)}]),
                                Err(x) => json!([hd, {"err": x.text()}]),
                            }
                        })
                        .collect(),
                )
            }
            "narc" => {
                let path = r["base"].as_str().unwrap().replace("font/$", "font/font");
                let d = damaged(&narcs[&path], &r);
                match cra::narc_members(&d, "N") {
                    Err(e) => json!({"err": e}),
                    Ok(ms) => json!({"ok": ms
                        .iter()
                        .map(|m| match cra::undo(&Ruzstd, m) {
                            Ok(u) => json!({"ok": h16(&u)}),
                            Err(x) => json!({"err": x.text()}),
                        })
                        .collect::<Vec<_>>()}),
                }
            }
            "member-base" => {
                members.insert(
                    r["base"].as_str().unwrap().to_string(),
                    unhex(r["hex"].as_str().unwrap()).unwrap(),
                );
                continue;
            }
            "member" => {
                match cra::undo(&Ruzstd, &damaged(&members[r["base"].as_str().unwrap()], &r)) {
                    Ok(u) => json!({"ok": h16(&u)}),
                    Err(x) => json!({"err": x.text()}),
                }
            }
            k => panic!("vector kind {k}"),
        };
        n += 1;
        if norm(&got) != norm(&r["out"]) {
            bad.push(format!(
                "{} {} {:?} cut {:?}:\n  python {}\n  rust   {}",
                r["kind"], r["base"], r["muts"], r["cut"], r["out"], got
            ));
        }
    }
    assert!(
        n > 600,
        "{n} vectors ({hangs} prefix chains the Python does not end)"
    );
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn every_prefix_reads_without_a_panic() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games/sync-cra");
    let nro = std::fs::read(data.join("nro/b_main.nro")).unwrap();
    for cut in (0..nro.len()).step_by(7) {
        let _ = cra::index_files(&nro[..cut]);
    }
    let a42 = std::fs::read(data.join("RomFS/42")).unwrap();
    let es = cra::index_files(&nro).unwrap();
    for e in &es {
        let Ok(d) = cra::read(&MemSource(&a42), e) else {
            continue;
        };
        for cut in (0..d.len()).step_by(3) {
            if let Ok(ms) = cra::narc_members(&d[..cut], "N") {
                for m in ms {
                    let _ = cra::undo(&Ruzstd, m);
                }
            }
            let _ = cra::undo(&Ruzstd, &d[..cut]);
        }
    }
}
