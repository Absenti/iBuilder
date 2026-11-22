//! Differential tests of the round-3 ports (`finish::{tess, meshstage, mesh, lean}`) against the engine's own Python (`dmsh_tess`, `dmsh_meshstage`, `dmsh_mesh`, `dmsh_geometry.lean`):
//! a committed subset of the corpus `tools/tessmesh/gen_corpus.py` makes (`tests/data/tessmesh_cases.json`, made by `tools/tessmesh/mkfixture.py`). Every case holds the input, the
//! arguments and the Python's result line (`status same len crc32 meta`, the format of `tools/difftest/`); the test runs the Rust function through `difftest::run` and compares.
//!
//! The inputs are real SPIRV-Cross output of GLSL compiled by glslang (hull / domain shaders), hand-made MSL in the shapes of the engine's own tests (mesh kernels, lean kernels,
//! vertex libraries) and mutations of both; the larger corpora (tens of thousands of cases) are run with `tools/tessmesh/run.sh` (docs/shaders/SHADER-TESS-MESH-RUST.md section 3).
use ibuilder_shader::difftest::{run, sorted_json};
use serde_json::Value;

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn cases() -> Option<Vec<Value>> {
    let p = std::env::var("TESSMESH_CASES").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/tessmesh_cases.json"
        )
        .to_string()
    });
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice::<Vec<Value>>(&b).ok(),
        Err(_) => {
            ibuilder_testkit::skip(&format!(
                "no differential cases at {p} (tools/tessmesh/mkfixture.py makes them)"
            ));
            None
        }
    }
}

/// the result line the Python half writes for a case, from the Rust function
fn line(step: &str, input: &str, args: &Value, dir: &std::path::Path, n: usize) -> String {
    let mut a = args.clone();
    // the SPIR-V and the second MSL are files for the registry's functions
    if let Some(h) = args.get("spv").and_then(|v| v.as_str()) {
        let p = dir.join(format!("{n}.spv"));
        std::fs::write(&p, unhex(h)).unwrap();
        a["spv"] = Value::String(p.to_string_lossy().into_owned());
    }
    if let Some(t) = args.get("gs").and_then(|v| v.as_str()) {
        let p = dir.join(format!("{n}.gs.metal"));
        std::fs::write(&p, t).unwrap();
        a["gs"] = Value::String(p.to_string_lossy().into_owned());
    }
    match run(step, input, &a) {
        None => "UNKNOWN".to_string(),
        Some(o) if o.ok => format!(
            "OK\t{}\t{}\t{}\t{}",
            (o.text == input) as u8,
            o.text.len(),
            crc32(o.text.as_bytes()),
            sorted_json(o.meta)
        ),
        Some(_) => "ERR\t0\t0\t0\tnull".to_string(),
    }
}

#[test]
fn tessellation_task_mesh_and_passthrough_geometry_match_the_python() {
    let Some(cases) = cases() else { return };
    let dir = std::env::temp_dir().join(format!("tessmesh_cases_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut by: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default(); // step -> (cases, identical, Python errors)
    let mut bad: Vec<String> = Vec::new();
    for (n, c) in cases.iter().enumerate() {
        let step = c["step"].as_str().unwrap();
        let got = line(step, c["in"].as_str().unwrap(), &c["args"], &dir, n);
        let want = c["py"].as_str().unwrap();
        let e = by.entry(step.to_string()).or_default();
        e.0 += 1;
        if want.starts_with("ERR") {
            e.2 += 1;
        }
        if got == want {
            e.1 += 1;
        } else if bad.len() < 8 {
            bad.push(format!("{step} #{n}: python {want:.200} / rust {got:.200}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    for (s, (n, ok, err)) in &by {
        eprintln!("{s:18} {n:4} cases, {ok:4} identical ({err} of them Python errors)");
    }
    for b in &bad {
        eprintln!("  MISMATCH {b}");
    }
    assert!(
        bad.is_empty(),
        "{} case(s) differ from the Python",
        bad.len()
    );
}
