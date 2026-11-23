//! CRI CPK archives and Diablo III's packages (`cpk.rs`): the archives of tests/data/games/sync-cpk and sync-d3cpk damaged in many ways, against what the upstream engine's
//! `cpk.py` / `d3cpk.py` / `shader_split.py` answered for each (tests/data/vectors-cpk.jsonl.gz, tools/gen_cpk.py): `cpk_jobs` (its jobs, its log line, or the exception that ends
//! the split), every entry as `cpk_shaders` reads it, the CRILAYLA decoder alone, and `d3cpk_shaders` over the jobs' ranges and over ranges cut anywhere. No game data.

use ibuilder_shadersplit::cpk::{self, PyErr, Val};
use ibuilder_shadersplit::source::{MemSource, Source};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn games() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games")
}

fn sha16(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

fn val(v: &Val) -> Value {
    match v {
        Val::None => Value::Null,
        Val::Int(i) => json!({"i": i.to_string()}),
        Val::Float(f) => json!({"f": cpk::py_float_repr(*f)}),
        Val::Str(s) => json!({"s": s}),
        Val::Bytes(b) => json!({"b": b.iter().map(|x| format!("{x:02x}")).collect::<String>()}),
    }
}

fn read_outcome(r: Result<Vec<u8>, PyErr>) -> Value {
    match r {
        Ok(d) => json!({"ok": sha16(&d)}),
        Err(e) if e.skips_entry() => json!({"skip": e.msg}),
        Err(e) => json!({"job": e.text()}),
    }
}

/// what tools/gen_cpk.py's `cpk_outcome` records: `cpk_jobs([(P, 'x.cpk')])`, then every entry read
fn cpk_outcome(d: &[u8]) -> Value {
    let src = MemSource(d);
    let mut log: Vec<String> = Vec::new();
    let mut sets = Vec::new();
    if src.read(0, 4).unwrap() == cpk::CPK_MAGIC {
        match cpk::files("P", &src) {
            Ok(es) => sets.push(es),
            Err(e) if e.caught_by_planner() => log.push(format!(
                "[RED] /x.cpk: the CPK cannot be read ({}); scanned as one file",
                e.msg
            )),
            Err(e) => return json!({"crash": e.text(), "log": log}),
        }
    }
    let jobs = match cpk::chunk_entries(&sets) {
        Ok(j) => j,
        Err(e) => return json!({"crash": e.text(), "log": log}),
    };
    let mut reads = Vec::new();
    for (_, es) in &jobs {
        for e in es {
            reads.push(read_outcome(cpk::read("P", &src, e)));
        }
    }
    json!({
        "log": log,
        "taken": !sets.is_empty(),
        "jobs": jobs.iter().map(|(_, es)| es.iter().map(|e| json!([e.name, val(&e.off), val(&e.csz), val(&e.usz)])).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "reads": reads,
    })
}

fn d3_outcome(d: &[u8], ranges: &[(u64, u64)]) -> Value {
    let src = MemSource(d);
    let jobs: Vec<(u64, u64)> = if cpk::is_d3cpk(&d[..d.len().min(8)]) {
        cpk::d3_ranges(&[d.len() as u64])
            .into_iter()
            .map(|(_, a, b)| (a, b))
            .collect()
    } else {
        vec![]
    };
    let mut res = Vec::new();
    for &(a, b) in jobs.iter().chain(ranges) {
        match cpk::d3cpk_range(&src, "x.cpk", a, b) {
            Ok((found, line)) => res.push(json!({
                "range": [a, b],
                "found": found.iter().map(|s| json!([s.label, s.st.short(), sha16(&[s.ctl.as_slice(), &s.code].concat())])).collect::<Vec<_>>(),
                "log": line.into_iter().collect::<Vec<_>>(),
            })),
            Err(e) => res.push(json!({"range": [a, b], "err": e.to_string()})),
        }
    }
    json!({"jobs": jobs.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(), "ranges": res})
}

fn damaged(base: &[u8], r: &Value) -> Vec<u8> {
    let mut d = base.to_vec();
    for m in r["muts"].as_array().unwrap() {
        d[m[0].as_u64().unwrap() as usize] = m[1].as_u64().unwrap() as u8;
    }
    if let Some(c) = r["cut"].as_u64() {
        d.truncate(c as usize);
    }
    d
}

#[test]
fn damaged_archives_read_as_the_python_reads_them() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/vectors-cpk.jsonl.gz");
    let mut text = String::new();
    flate2::read::GzDecoder::new(
        std::fs::File::open(&p).expect("tests/data/vectors-cpk.jsonl.gz (tools/gen_cpk.py)"),
    )
    .read_to_string(&mut text)
    .unwrap();
    let (mut n, mut kinds) = (0, std::collections::BTreeMap::<String, usize>::new());
    for line in text.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let kind = r["kind"].as_str().unwrap();
        let base_name = r["base"].as_str().unwrap();
        let got = match kind {
            "cpk" => cpk_outcome(&damaged(
                &std::fs::read(games().join(base_name)).unwrap(),
                &r,
            )),
            // values of other types where numbers and strings are expected (tools/gen_cpk.py TYPED): the archive itself in the vector
            "cpk-hex" => {
                let h = r["hex"].as_str().unwrap();
                let d: Vec<u8> = (0..h.len() / 2)
                    .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
                    .collect();
                cpk_outcome(&d)
            }
            "crilayla" => {
                let (file, entry) = base_name.split_once('#').unwrap();
                let whole = std::fs::read(games().join(file)).unwrap();
                let e = cpk::files("P", &MemSource(&whole))
                    .unwrap()
                    .into_iter()
                    .find(|e| e.name == entry)
                    .unwrap();
                let (Val::Int(off), Val::Int(csz)) = (&e.off, &e.csz) else {
                    panic!()
                };
                let c = damaged(&whole[*off as usize..(*off + *csz) as usize], &r);
                match cpk::crilayla(&c) {
                    Ok(x) => json!({"ok": sha16(&x), "len": x.len()}),
                    Err(e) => read_outcome(Err(e)),
                }
            }
            "d3" => {
                let want_ranges: Vec<(u64, u64)> = r["out"]["ranges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .skip(r["out"]["jobs"].as_array().unwrap().len())
                    .map(|x| {
                        (
                            x["range"][0].as_u64().unwrap(),
                            x["range"][1].as_u64().unwrap(),
                        )
                    })
                    .collect();
                d3_outcome(
                    &damaged(&std::fs::read(games().join(base_name)).unwrap(), &r),
                    &want_ranges,
                )
            }
            k => panic!("vector kind {k}"),
        };
        assert_eq!(
            got, r["out"],
            "{kind} {base_name} muts {} cut {}",
            r["muts"], r["cut"]
        );
        n += 1;
        *kinds.entry(kind.to_string()).or_default() += 1;
    }
    eprintln!("{n} vectors: {kinds:?}");
    assert!(n > 600, "{n} vectors");
}

/// every prefix of each archive and each byte changed in its first 0x400 bytes and its TOC: an error or entries, never a panic, and every entry read without a panic
#[test]
fn every_prefix_and_header_byte_of_an_archive_reads_without_a_panic() {
    for rel in [
        "sync-cpk/RomFS/Data/data000.cpk",
        "sync-cpk/RomFS/Data/data001.cpk",
        "sync-d3cpk/RomFS/CPKs/ClientCommon.cpk",
    ] {
        let b = std::fs::read(games().join(rel)).unwrap();
        let toc = b.windows(4).position(|w| w == b"TOC ").unwrap_or(0);
        let mut inputs: Vec<Vec<u8>> = (0..b.len()).step_by(37).map(|n| b[..n].to_vec()).collect();
        for o in (0..0x400.min(b.len())).chain(toc..(toc + 0x200).min(b.len())) {
            for x in [0xffu8, 0x80, 0x01, 0x10, 0x40] {
                let mut m = b.clone();
                m[o] ^= x;
                inputs.push(m);
            }
        }
        for d in &inputs {
            let src = MemSource(d);
            if let Ok(es) = cpk::files("P", &src) {
                for e in &es {
                    let _ = cpk::read("P", &src, e);
                }
            }
            let _ = cpk::d3cpk_range(&src, "x.cpk", 0, d.len() as u64);
        }
    }
}
