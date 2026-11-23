//! `job_for` against the Python's own jobs: `$S2/B/oracle/jobs-s300|s1800/<key>/{job.json, code.bin, c1.bin}` were dumped by an instrumented COPY of `dmsh_build.py`
//! (`translate()` writes `json.dumps(job)`, the bytes of `<key>.bin` and `c1` right after `job_for`). The job text must be identical (the `code` path is the one the Python's job carries).
mod common;
use common::*;
use ibuilder_shaderbuild::game::{Game, ShaderMap, load_facts};
use ibuilder_shaderbuild::jobs::{XlateOpts, job_for};
use ibuilder_shaderbuild::pyjson::{Opts, dumps};
use ibuilder_shaderbuild::records::Packages;
use serde_json::Value;

fn run(set: &str) {
    let dir = s2(&format!("B/oracle/{set}"));
    if !have(&dir) || !have(&pkg()) {
        return;
    }
    let gdir = profile().join(GAME);
    let _game = Game::load(&gdir).unwrap();
    let map = ShaderMap::load(&gdir.join("shader_map.tsv")).unwrap();
    let facts = load_facts(&gdir.join("shader_facts.tsv")).unwrap();
    let packs = Packages::new(&pkg());
    let mut keys: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    keys.sort();
    let (mut n, mut stages) = (0, std::collections::BTreeMap::<String, usize>::new());
    let (mut with_facts, mut tf, mut efs) = (0, 0, 0);
    for k in &keys {
        let kd = dir.join(k);
        let py_text = std::fs::read_to_string(kd.join("job.json")).unwrap();
        let py: Value = serde_json::from_str(&py_text).unwrap();
        let code_path = py["code"].as_str().unwrap().to_string();
        let (name, _) = &map.loc[k];
        let rec = packs.record(name, k).unwrap();
        let out = job_for(
            k,
            name,
            &rec,
            &code_path,
            facts.get(k),
            &XlateOpts::default(),
        )
        .unwrap_or_else(|e| panic!("{k}: {e}"));
        let mine = dumps(&Value::Object(out.job.clone()), &Opts::DEFAULT);
        assert_eq!(mine, py_text, "job of {k}");
        assert_eq!(
            out.code,
            std::fs::read(kd.join("code.bin")).unwrap(),
            "code of {k}"
        );
        assert_eq!(
            out.c1,
            std::fs::read(kd.join("c1.bin")).unwrap(),
            "c1 of {k}"
        );
        n += 1;
        *stages
            .entry(py["stage"].as_str().unwrap().to_string())
            .or_default() += 1;
        with_facts += py.get("no_facts").is_none() as usize;
        tf += py.get("tf").is_some() as usize;
        efs += py.get("early_fragment_tests").is_some() as usize;
    }
    eprintln!(
        "{set}: {n} jobs identical (job text, code bytes, c1 bytes); stages {stages:?}; with answer-table row {with_facts}, transform feedback {tf}, early fragment tests {efs}"
    );
    assert!(n > 100);
}

#[test]
fn jobs_of_the_300_shader_sample_are_the_pythons() {
    run("jobs-s300");
}

#[test]
fn jobs_of_the_1800_shader_sample_are_the_pythons() {
    run("jobs-s1800");
}

/// `job_for` over a captured build (local real data, never committed): `$JOBS_CAPTURE/<key>/{job.json, <key>.bin, c1.hex}` are what an instrumented copy of `dmsh_build.py`
/// wrote right after `job_for`, `$JOBS_PKG` the split packages and `$JOBS_PROFILE` the game's profile folder (`shader_map.tsv`, `shader_facts.tsv`). Skips when unset.
#[test]
fn jobs_of_a_captured_build_are_the_pythons() {
    let (Some(cap), Some(pkg), Some(prof)) = (
        std::env::var_os("JOBS_CAPTURE"),
        std::env::var_os("JOBS_PKG"),
        std::env::var_os("JOBS_PROFILE"),
    ) else {
        ibuilder_testkit::skip("JOBS_CAPTURE / JOBS_PKG / JOBS_PROFILE are not set");
        return;
    };
    let (cap, prof) = (
        std::path::PathBuf::from(cap),
        std::path::PathBuf::from(prof),
    );
    let map = ShaderMap::load(&prof.join("shader_map.tsv")).unwrap();
    let facts = load_facts(&prof.join("shader_facts.tsv")).unwrap();
    let packs = Packages::new(std::path::Path::new(&pkg));
    let mut keys: Vec<String> = std::fs::read_dir(&cap)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|k| cap.join(k).join("job.json").exists())
        .collect();
    keys.sort();
    let (mut n, mut stages) = (0, std::collections::BTreeMap::<String, usize>::new());
    let mut bad: Vec<String> = Vec::new();
    for k in &keys {
        let kd = cap.join(k);
        let py_text = std::fs::read_to_string(kd.join("job.json")).unwrap();
        let py: Value = serde_json::from_str(&py_text).unwrap();
        let code_path = py["code"].as_str().unwrap().to_string();
        let (name, _) = &map.loc[k];
        let rec = packs.record(name, k).unwrap();
        let out = match job_for(
            k,
            name,
            &rec,
            &code_path,
            facts.get(k),
            &XlateOpts::default(),
        ) {
            Ok(o) => o,
            Err(e) => {
                bad.push(format!("{k}: {e}"));
                continue;
            }
        };
        let c1_hex = std::fs::read_to_string(kd.join("c1.hex")).unwrap();
        if dumps(&Value::Object(out.job.clone()), &Opts::DEFAULT) != py_text {
            bad.push(format!("{k}: job text"));
        } else if out.code != std::fs::read(kd.join(format!("{k}.bin"))).unwrap() {
            bad.push(format!("{k}: code bytes"));
        } else if ibuilder_shaderbuild::records::hex(&out.c1) != c1_hex.trim_end() {
            bad.push(format!("{k}: c1"));
        } else {
            n += 1;
            let st = match py.get("mesh_stage") {
                Some(m) => format!("cs/{}", m["kind"].as_str().unwrap_or("?")),
                None => py["stage"].as_str().unwrap().to_string(),
            };
            *stages.entry(st).or_default() += 1;
        }
    }
    eprintln!(
        "captured build: {n} of {} jobs identical (job text, code bytes, c1); stages {stages:?}",
        keys.len()
    );
    assert!(
        bad.is_empty(),
        "{} differ: {:?}",
        bad.len(),
        &bad[..bad.len().min(20)]
    );
}
