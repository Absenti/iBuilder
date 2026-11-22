//! Regression tests of the ported steps. `fixtures_match_the_engines_python`: the small fixtures in `tests/fixtures/` (inputs written by the fuzz generators of `tools/difftest/fuzz.py`,
//! expected results produced by the engine's own Python function, see `tools/difftest/mkfixtures.py`). `corpora_match_the_stored_python_results`: the big differential corpora of
//! the work (cases, and the Python's result file for them), when they are present on this machine; they are not part of the repository.
use ibuilder_shader::difftest::{crc32, run};
use rayon::prelude::*;
use serde_json::Value;
use std::path::Path;

/// A JSON text with its object keys in sorted order, whatever order `serde_json::Map` keeps: the workspace enables serde_json's `preserve_order`
/// (the build driver needs the Python dict's insertion order), so a plain `to_string()` is not sorted there, and the Python's files are (`sort_keys`).
fn canon(text: &str) -> String {
    fn sorted(v: Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<String> = m.keys().cloned().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    let x = sorted(m[&k].clone());
                    out.insert(k, x);
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
            other => other,
        }
    }
    match serde_json::from_str::<Value>(text) {
        Ok(v) => sorted(v).to_string(),
        Err(_) => text.to_string(),
    }
}

#[test]
fn fixtures_match_the_engines_python() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let Some(base) = name.strip_suffix(".expect") else {
            continue;
        };
        let step = base.split('.').next().unwrap();
        let input = std::fs::read_to_string(dir.join(format!("{base}.in"))).unwrap();
        let args: Value =
            serde_json::from_slice(&std::fs::read(dir.join(format!("{base}.args.json"))).unwrap())
                .unwrap();
        let expect = std::fs::read_to_string(&p).unwrap();
        let mut it = expect.splitn(3, '\n');
        let (status, meta, text) = (
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap_or(""),
        );
        let got = run(step, &input, &args).unwrap_or_else(|| panic!("no step {step}"));
        assert_eq!(
            got.ok,
            status == "OK",
            "{base}: status (message: {})",
            got.msg
        );
        if got.ok {
            assert!(
                got.text == text,
                "{base}: the text differs from the Python's"
            );
            assert_eq!(canon(&got.meta.to_string()), canon(meta), "{base}: meta");
        }
        n += 1;
    }
    assert!(n >= 60, "only {n} fixtures");
}

/// replays `<corpus>/cases.tsv` through `difftest::run` and compares with `<corpus>/res/py.tsv` (status, same flag, length, crc32, meta)
#[test]
fn corpora_match_the_stored_python_results() {
    let root = std::env::var("SHADER_DIFFTEST_ROOT")
        .unwrap_or_else(|_| format!("{}/f1", ibuilder_testkit::SCRATCH));
    let mut total = 0usize;
    for c in ["corpus-real", "mut", "mut2", "fz", "fzc"] {
        let (cases, py) = (
            format!("{root}/{c}/cases.tsv"),
            format!("{root}/{c}/res/py.tsv"),
        );
        if !Path::new(&cases).exists() || !Path::new(&py).exists() {
            ibuilder_testkit::skip(&format!("corpus {c} is absent"));
            continue;
        }
        let want: std::collections::HashMap<String, Vec<String>> = std::fs::read_to_string(&py)
            .unwrap()
            .lines()
            .map(|l| {
                let v: Vec<String> = l.split('\t').map(String::from).collect();
                (v[0].clone(), v)
            })
            .collect();
        let mut rows: Vec<(String, String, String, Value)> = Vec::new();
        for l in std::fs::read_to_string(&cases).unwrap().lines() {
            let mut it = l.splitn(4, '\t');
            let (Some(id), Some(step), Some(path)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if !want.contains_key(id) {
                continue;
            }
            rows.push((
                id.to_string(),
                step.to_string(),
                path.to_string(),
                it.next()
                    .and_then(|x| serde_json::from_str(x).ok())
                    .unwrap_or(Value::Null),
            ));
        }
        let mut files: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        for r in &rows {
            if !files.contains_key(r.2.as_str()) {
                files.insert(&r.2, std::fs::read_to_string(&r.2).unwrap());
            }
        }
        let bad: Vec<String> = rows
            .par_iter()
            .filter_map(|(id, step, path, args)| {
                let w = &want[id];
                if w[1] == "UNKNOWN" {
                    return None;
                }
                let msl = &files[path.as_str()];
                let line = match run(step, msl, args) {
                    None => return None,
                    Some(o) if o.ok => format!(
                        "OK\t{}\t{}\t{}\t{}",
                        (o.text == *msl) as u8,
                        o.text.len(),
                        crc32(o.text.as_bytes()),
                        canon(&o.meta.to_string())
                    ),
                    Some(_) => "ERR\t0\t0\t0\tnull".to_string(),
                };
                let want_line = format!("{}\t{}", w[1..5].join("\t"), canon(&w[5]));
                if line != want_line {
                    Some(format!("{c}/{id} {step}: rust {line} / python {want_line}"))
                } else {
                    None
                }
            })
            .collect();
        total += rows.len();
        assert!(
            bad.is_empty(),
            "{} of the {c} corpus differ, first: {:?}",
            bad.len(),
            &bad[..bad.len().min(3)]
        );
    }
    eprintln!("{total} cases identical to the Python's stored results");
}
