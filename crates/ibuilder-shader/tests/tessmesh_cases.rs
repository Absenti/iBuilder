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

/// the result line the Python half writes for a case, from the Rust function, and the error message (JSON) of an `ERR`
fn line(
    step: &str,
    input: &str,
    args: &Value,
    dir: &std::path::Path,
    n: usize,
) -> (String, String) {
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
        None => ("UNKNOWN".to_string(), String::new()),
        Some(o) if o.ok => (
            format!(
                "OK\t{}\t{}\t{}\t{}",
                (o.text == input) as u8,
                o.text.len(),
                crc32(o.text.as_bytes()),
                sorted_json(o.meta)
            ),
            "\"\"".to_string(),
        ),
        Some(o) => (
            "ERR\t0\t0\t0\tnull".to_string(),
            serde_json::to_string(&o.msg).unwrap(),
        ),
    }
}

/// whether a Python error message is a `ValueError` of the engine's own (its text is the port's too), not an exception the port reports in its own words (`KeyError: ...`, a
/// `struct.error`, a timeout)
fn own_message(msg: &str) -> bool {
    let m = msg.trim_start_matches('"');
    let typed = m
        .split_once(':')
        .is_some_and(|(t, _)| t.ends_with("Error") || t.ends_with("error") || t == "KeyError");
    !typed && m != "timeout\""
}

#[test]
fn tessellation_task_mesh_and_passthrough_geometry_match_the_python() {
    let Some(cases) = cases() else { return };
    let dir = std::env::temp_dir().join(format!("tessmesh_cases_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut by: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default(); // step -> (cases, identical, Python errors)
    let mut bad: Vec<String> = Vec::new();
    let mut msgs = 0;
    for (n, c) in cases.iter().enumerate() {
        let step = c["step"].as_str().unwrap();
        let (got, got_msg) = line(step, c["in"].as_str().unwrap(), &c["args"], &dir, n);
        let want = c["py"].as_str().unwrap();
        let want_msg = c["msg"].as_str().unwrap_or("");
        let e = by.entry(step.to_string()).or_default();
        e.0 += 1;
        if want.starts_with("ERR") {
            e.2 += 1;
        }
        if got == want && (!want.starts_with("ERR") || !own_message(want_msg)) {
            e.1 += 1;
        } else if got == want && got_msg == want_msg {
            e.1 += 1;
            msgs += 1;
        } else if got == want {
            if bad.len() < 8 {
                bad.push(format!(
                    "{step} #{n}: python message {want_msg:.200} / rust {got_msg:.200}"
                ));
            }
        } else if bad.len() < 8 {
            bad.push(format!("{step} #{n}: python {want:.200} / rust {got:.200}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    for (s, (n, ok, err)) in &by {
        eprintln!("{s:18} {n:4} cases, {ok:4} identical ({err} of them Python errors)");
    }
    eprintln!("{msgs} Python ValueError messages identical");
    for b in &bad {
        eprintln!("  MISMATCH {b}");
    }
    assert!(
        bad.is_empty(),
        "{} case(s) differ from the Python",
        bad.len()
    );
}

/// `meshstage::to_stage` over a real build's task / mesh shaders (local data, never committed): `$MESH_STAGE_REAL/cases.jsonl` and `<key>/pre.metal` are what
/// `target/agent-work/chain/mesh/py_stage.py` writes (HEAD's finish() replayed up to `to_stage`: its input, the info, the partner reads, the output's sha256 / length or its error);
/// with `$MESH_STAGE_IMAGE` (the Python's library image) the final text (`raw_noinline`, `mesh_subgroup_noinline`) is compared with the image's `<key>_b.metal` too. Skips when unset.
#[test]
fn to_stage_over_a_real_build_matches_the_python() {
    use ibuilder_shader::finish::meshstage::{MeshInfo, to_stage};
    use sha2::{Digest, Sha256};
    let Some(dir) = std::env::var_os("MESH_STAGE_REAL").map(std::path::PathBuf::from) else {
        ibuilder_testkit::skip("MESH_STAGE_REAL is not set");
        return;
    };
    let image_bytes = std::env::var_os("MESH_STAGE_IMAGE").map(|p| std::fs::read(p).unwrap());
    let image = image_bytes
        .as_deref()
        .map(|b| ibuilder_shader::romfs::Image::open(b).unwrap());
    let (mut n, mut same, mut errs, mut spills, mut img_same) = (0, 0, 0, 0, 0);
    let mut bad: Vec<String> = Vec::new();
    for l in std::fs::read_to_string(dir.join("cases.jsonl"))
        .unwrap()
        .lines()
    {
        let c: Value = serde_json::from_str(l).unwrap();
        let key = c["key"].as_str().unwrap();
        if c.get("stage_err").is_some() || c.get("refused").is_some() {
            continue;
        }
        n += 1;
        let pre = std::fs::read_to_string(dir.join(key).join("pre.metal")).unwrap();
        let mut i = MeshInfo::from_json(&c["info"]).unwrap();
        let reads: Option<std::collections::BTreeMap<i64, i64>> = c["reads"].as_object().map(|o| {
            o.iter()
                .map(|(k, v)| (k.parse().unwrap(), v.as_i64().unwrap()))
                .collect()
        });
        match (to_stage(&pre, &mut i, reads.as_ref()), c.get("err")) {
            (Err(e), Some(want)) if want.as_str() == Some(e.as_str()) => {
                same += 1;
                errs += 1;
            }
            (Ok(out), None) => {
                let sha: String = Sha256::digest(out.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                if sha == c["sha"].as_str().unwrap()
                    && out.len() as u64 == c["len"].as_u64().unwrap()
                    && i.spill == c["spill"].as_bool().unwrap()
                {
                    same += 1;
                    spills += i.spill as usize;
                } else {
                    bad.push(format!("{key}: output (len {} / {})", out.len(), c["len"]));
                    continue;
                }
                if let Some(img) = &image {
                    let mut fin = ibuilder_shader::steps::raw_noinline(&out, &i.kind).into_owned();
                    if i.kind == "ms" {
                        fin = ibuilder_shader::steps::mesh_subgroup_noinline(&fin).into_owned();
                    }
                    match img.find(&format!("{key}_b.metal")) {
                        Some(f) if img.bytes(f) == fin.as_bytes() => img_same += 1,
                        _ => bad.push(format!(
                            "{key}: the final text is not the image's <key>_b.metal"
                        )),
                    }
                }
            }
            (r, want) => bad.push(format!("{key}: rust {:?} / python {want:?}", r.err())),
        }
    }
    eprintln!(
        "to_stage over a real build: {same} of {n} identical ({errs} errors, {spills} with a device stage slice); {img_same} final texts equal to the image's"
    );
    assert!(
        bad.is_empty(),
        "{} differ: {:?}",
        bad.len(),
        &bad[..bad.len().min(10)]
    );
}
