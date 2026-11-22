//! The translate side against the real tools: jobs made here, run through the engine's `dmsh_xlate` (a child process), must give the translator the same answers the Python's jobs did:
//! `<key>.spv` and `<key>.res.json` byte-identical to what the instrumented Python copy saved right after its `translate()` (`$S2/B/oracle/jobs-*/<key>/tr.spv`, `tr.res.json`).
//! Needs the Mac binary (`--features host-tools`) and the oracle runs; skips otherwise.
#![cfg(feature = "host-tools")]
mod common;
use common::*;
use ibuilder_shader::tools::ShaderTools;
use ibuilder_shaderbuild::game::{ShaderMap, load_facts};
use ibuilder_shaderbuild::jobs::{XlateOpts, job_for};
use ibuilder_shaderbuild::pyjson::{Opts, dumps};
use ibuilder_shaderbuild::records::Packages;
use ibuilder_shaderbuild::tools::ProcessTools;
use serde_json::Value;

#[test]
fn the_translator_answers_our_jobs_as_it_answered_the_pythons() {
    let set = s2("B/oracle/jobs-s300");
    let engine = s2("B/py/engine-jobs");
    if !have(&set) || !have(&engine) || !have(&pkg()) {
        return;
    }
    let tools = ProcessTools::in_engine(&engine);
    let gdir = profile().join(GAME);
    let map = ShaderMap::load(&gdir.join("shader_map.tsv")).unwrap();
    let facts = load_facts(&gdir.join("shader_facts.tsv")).unwrap();
    let packs = Packages::new(&pkg());
    let work = std::env::temp_dir().join(format!("ibsb-tr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let mut keys: Vec<String> = std::fs::read_dir(&set)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    keys.sort();
    // one job file for all keys, as a translator call with many jobs (the build sends 1/NJ of a batch per call)
    let mut jobs = Vec::new();
    for k in &keys {
        let (name, _) = &map.loc[k];
        let rec = packs.record(name, k).unwrap();
        let code_path = work.join(format!("{k}.bin"));
        let o = job_for(
            k,
            name,
            &rec,
            &code_path.to_string_lossy(),
            facts.get(k),
            &XlateOpts::default(),
        )
        .unwrap();
        std::fs::write(&code_path, &o.code).unwrap();
        jobs.push(dumps(&Value::Object(o.job), &Opts::DEFAULT));
    }
    let jp = work.join("jobs.json");
    std::fs::write(&jp, format!("[{}]", jobs.join(", "))).unwrap();
    let r = tools.xlate(&[
        jp.to_string_lossy().into_owned(),
        work.to_string_lossy().into_owned(),
    ]);
    eprintln!("xlate rc {} stderr {} bytes", r.rc, r.stderr.len());
    let (mut spv, mut res) = (0, 0);
    for k in &keys {
        let want_spv = std::fs::read(set.join(k).join("tr.spv")).unwrap();
        let want_res = std::fs::read(set.join(k).join("tr.res.json")).unwrap();
        assert_eq!(
            std::fs::read(work.join(format!("{k}.spv"))).unwrap(),
            want_spv,
            "spv of {k}"
        );
        assert_eq!(
            std::fs::read(work.join(format!("{k}.res.json"))).unwrap(),
            want_res,
            "res.json of {k}"
        );
        spv += 1;
        res += 1;
    }
    eprintln!(
        "{} shaders: {spv} .spv and {res} .res.json byte-identical to the Python's translator outputs",
        keys.len()
    );
    let _ = std::fs::remove_dir_all(&work);
}
