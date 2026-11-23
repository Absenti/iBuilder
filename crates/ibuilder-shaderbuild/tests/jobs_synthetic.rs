//! `job_for` on synthetic records the Grizzy sample does not reach (VertexA/B, transform feedback, early fragment tests, domain shaders, headered compute, Switch 2 records, malformed ones),
//! against the Python functions extracted from the engine's `dmsh_build.py` and run on the same records by `tools/gen_synth_jobs.py` (output: `$S2/B/oracle/synth/<case>/`).
//! Job text, code bytes, c1, the `sys.exit` message or the printed `[RED]` lines must be the Python's. Switch 2 task / mesh records (`dmsh_meshstage.info / memory / own_shared`, round 3) are compared
//! like the others; the records the Python crashes on (a control section too short for the fields, a stage byte that is neither task nor mesh) are errors in the port too.
//! The oracle is looked up in `$SYNTH_JOBS` when set (`tools/gen_synth_jobs.py <engine tools> <dir>`), else `$S2/sh3/synth` (round 3, with the task / mesh records), else the earlier
//! `$S2/B/oracle/synth`. A `ValueError` of `job_for` itself (`valueerror.txt`: a ray tracing record on the ordinary path, the build stops) is an error with the Python's text.
mod common;
use common::*;
use ibuilder_shaderbuild::jobs::{JobError, XlateOpts, job_for};
use ibuilder_shaderbuild::pyjson::{Opts, dumps};
use serde_json::Value;

#[test]
fn synthetic_records_give_the_pythons_jobs() {
    let dir = if let Some(d) = std::env::var_os("SYNTH_JOBS") {
        std::path::PathBuf::from(d)
    } else if s2("sh3/synth").exists() {
        s2("sh3/synth")
    } else {
        s2("B/oracle/synth")
    };
    if !have(&dir) {
        return;
    }
    let mut cases: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    cases.sort();
    let (mut jobs, mut exits, mut unported) = (0, 0, 0);
    let mut features = std::collections::BTreeSet::new();
    for c in &cases {
        let d = dir.join(c);
        let rec = std::fs::read(d.join("rec.bin")).unwrap();
        let name = std::fs::read_to_string(d.join("name.txt")).unwrap();
        let key = std::fs::read_to_string(d.join("key.txt")).unwrap();
        let facts: Option<Value> = std::fs::read_to_string(d.join("facts.json"))
            .ok()
            .map(|t| serde_json::from_str(&t).unwrap());
        let stdout = std::fs::read_to_string(d.join("stdout.txt")).unwrap_or_default();
        if d.join("crash.txt").exists() && (c.contains("nvn2_ms") || c.contains("nvn2_ts")) {
            // the Python raises (struct.error / IndexError / KeyError: a short control section, an SPH past the code area, a stage byte that is neither) where the port reports an error: both stop that shader
            let r = job_for(
                &key,
                &name,
                &rec,
                "x.bin",
                facts.as_ref(),
                &XlateOpts::default(),
            );
            assert!(
                r.is_err(),
                "{c}: the port made a job where the Python crashed"
            );
            unported += 1;
            continue;
        }
        if let Ok(msg) = std::fs::read_to_string(d.join("exit.txt"))
            .or_else(|_| std::fs::read_to_string(d.join("valueerror.txt")))
        {
            let r = job_for(
                &key,
                &name,
                &rec,
                "x.bin",
                facts.as_ref(),
                &XlateOpts::default(),
            );
            assert_eq!(r.err(), Some(JobError::Red(msg)), "{c}");
            exits += 1;
            continue;
        }
        assert!(
            !d.join("crash.txt").exists(),
            "{c}: the Python crashed: {}",
            std::fs::read_to_string(d.join("crash.txt")).unwrap()
        );
        let py_text = std::fs::read_to_string(d.join("job.json")).unwrap();
        let py: Value = serde_json::from_str(&py_text).unwrap();
        let code_path = py["code"].as_str().unwrap();
        let out = job_for(
            &key,
            &name,
            &rec,
            code_path,
            facts.as_ref(),
            &XlateOpts::default(),
        )
        .unwrap_or_else(|e| panic!("{c}: {e}"));
        assert_eq!(
            dumps(&Value::Object(out.job.clone()), &Opts::DEFAULT),
            py_text,
            "job of {c}"
        );
        assert_eq!(
            out.code,
            std::fs::read(d.join("code.bin")).unwrap(),
            "code of {c}"
        );
        assert_eq!(
            out.c1,
            std::fs::read(d.join("c1.bin")).unwrap(),
            "c1 of {c}"
        );
        let notes: String = out.notes.iter().map(|n| format!("{n}\n")).collect();
        assert_eq!(notes, stdout, "printed lines of {c}");
        jobs += 1;
        for k in [
            "tf",
            "early_fragment_tests",
            "tess_mode",
            "arch",
            "nvn2_cs",
            "stage_bank",
            "local",
            "no_facts",
            "mesh_stage",
            "stage_memory",
            "stage_input_words",
        ] {
            if py.get(k).is_some() {
                features.insert(k);
            }
        }
        if !stdout.is_empty() {
            features.insert("[RED] line");
        }
        if c.contains("vab_ok") {
            features.insert("vertex A+B merge");
        }
        if c.contains("vab_split") {
            features.insert("vertex A+B merge, c1 before B");
        }
    }
    eprintln!(
        "{} cases: {jobs} identical jobs, {exits} identical sys.exit messages, {unported} errors where the Python crashes; job features exercised: {features:?}",
        cases.len()
    );
    assert!(
        features.contains("vertex A+B merge")
            && features.contains("tf")
            && features.contains("early_fragment_tests")
            && features.contains("arch")
    );
}
