//! `rt_build::make_job` against the shipped engine's `dmsh_rt_build.make_job`: 93 cases recorded by `tools/rt_cases.py` (`tests/fixtures/rt_cases.json`): the engine's own synthetic RTCore modules
//! (its `tests/test_dmsh_rt_build.py` builders), mutations of their control sections, and 60 random ones. Each case is the answer (the manifest, the translator's `rt` job, the SHA-256 and size
//! of the linked code, the local memory, every module's entries) or the exact text of the `InvalidModule` the Python raised.

use ibuilder_shaderbuild::rt_build::{RtRecord, make_job, module_entries_json, parse_module};
use serde_json::Value;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn record(v: &Value) -> RtRecord {
    let key = match &v["key"] {
        Value::Number(n) => n.as_u64().unwrap(),
        Value::String(s) => u64::from_str_radix(s, 16).unwrap(),
        other => panic!("key {other}"),
    };
    RtRecord {
        key,
        control: unhex(v["control"].as_str().unwrap()),
        code: unhex(v["code"].as_str().unwrap()),
    }
}

#[test]
fn make_job_answers_and_refuses_as_the_engine_does() {
    let cases: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rt_cases.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let (mut ok, mut err) = (0, 0);
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let result = if let Some(many) = case.get("many") {
            let base = record(&many["record"]);
            make_job((0..many["count"].as_u64().unwrap()).map(|k| RtRecord {
                key: k,
                ..base.clone()
            }))
        } else {
            make_job(case["records"].as_array().unwrap().iter().map(record))
        };
        match (
            result,
            case.get("ok").filter(|v| v.is_object()),
            case.get("error"),
        ) {
            (Ok(job), Some(want), _) => {
                assert_eq!(job.manifest(), want["manifest"], "{name}: manifest");
                assert_eq!(job.rt(), want["rt"], "{name}: rt");
                assert_eq!(
                    job.code.len() as u64,
                    want["code_len"].as_u64().unwrap(),
                    "{name}: code length"
                );
                assert_eq!(
                    job.local_memory_bytes as u64,
                    want["local_memory_bytes"].as_u64().unwrap(),
                    "{name}: local memory"
                );
                assert_eq!(
                    module_entries_json(&job),
                    want["module_entries"],
                    "{name}: module entries"
                );
                ok += 1;
            }
            (Err(e), None, Some(want)) => {
                assert_eq!(e, want.as_str().unwrap(), "{name}: the text of the refusal");
                err += 1;
            }
            (Ok(_), None, Some(want)) => {
                panic!("{name}: the engine refuses ({want}), this answers")
            }
            (Err(e), Some(_), _) => panic!("{name}: the engine answers, this refuses: {e}"),
            (Ok(_), None, None) | (Err(_), None, None) => panic!("{name}: no expectation recorded"),
        }
    }
    assert!(
        ok >= 30 && err >= 30,
        "the fixture should hold both answers and refusals: {ok} / {err}"
    );
    eprintln!("rt_build: {ok} answers and {err} refusals identical to the engine's");
}

#[test]
fn the_parser_of_the_split_and_this_one_agree_on_every_module_of_the_cases() {
    let cases: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rt_cases.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let mut seen = 0;
    for case in &cases {
        let Some(records) = case.get("records").and_then(|r| r.as_array()) else {
            continue;
        };
        for v in records.iter().take(3) {
            let r = record(v);
            if r.control.len() < 0x68 {
                continue;
            }
            let u = |o: usize| u32::from_le_bytes(r.control[o..o + 4].try_into().unwrap()) as usize;
            let (size, offset) = (u(0x5c), u(0x60));
            if offset > r.control.len() || size > r.control.len() - offset {
                continue;
            }
            let meta = &r.control[offset..offset + size];
            let mine = parse_module(meta, &r.code, None).map(|_| ());
            let theirs = ibuilder_shadersplit::rt::parse_module(meta, &r.code);
            assert_eq!(
                mine, theirs,
                "{}: the two readers of RT metadata disagree",
                case["name"]
            );
            seen += 1;
        }
    }
    assert!(seen > 100, "{seen}");
}
