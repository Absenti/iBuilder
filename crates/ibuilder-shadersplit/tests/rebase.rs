//! What the rebased engine (nxnative a3663659 + the NX2Apple patches) changed in `shader_split.py` since the port was written, against the engine's own answers (`tools/gen_rebase.py` writes
//! `tests/data/vectors-rebase.jsonl.gz` and the games `tests/data/games/rebase-*` / `tests/data/games-err/*`): NVN2 ray tracing (`rt`: `check`, `same_shader`, the BNSH program's +0x38 pointer,
//! `dmsh_rt.parse_module`), CIVBSF revisions 0x1118 / 0x1218, bnd2 versions 2 / 3, Phyre's programs (`phyre_shaders`, `scan_blob`'s RYHP fall-through), Unravel TWO's KIT archives (`kit.py`),
//! and the games whose split ends in an error (a malformed KIT index, `main`'s `uniq_keys`). The games that succeed are split by `tests/games.rs`.

use ibuilder_shadersplit::archive::{b64decode, bnd2_resources, bnd2_shaders};
use ibuilder_shadersplit::ctl::*;
use ibuilder_shadersplit::kit;
use ibuilder_shadersplit::nvn::{bnsh_shaders, civbsf_shaders, phyre_shaders};
use ibuilder_shadersplit::scan::scan_blob;
use ibuilder_shadersplit::scan_types::{Ctx, Shader};
use ibuilder_shadersplit::split::FileIndex;
use ibuilder_shadersplit::unity::Trees;
use ibuilder_shadersplit::{Failure, NoGate, Ruzstd, SplitOptions, split};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn d64(v: &Value) -> Vec<u8> {
    b64decode(v.as_str().unwrap().as_bytes()).unwrap()
}

fn vectors() -> Option<Vec<Value>> {
    let raw = std::fs::read(format!(
        "{}/tests/data/vectors-rebase.jsonl.gz",
        env!("CARGO_MANIFEST_DIR")
    ))
    .ok()?;
    let text =
        String::from_utf8(ibuilder_shadersplit::inflate::gunzip(&raw, 1 << 30).unwrap()).unwrap();
    Some(
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

fn st_name(s: Option<St>) -> Value {
    s.map(|s| Value::String(s.short().to_string()))
        .unwrap_or(Value::Null)
}

fn found_json(f: &[Shader]) -> Value {
    Value::Array(
        f.iter()
            .map(|s| {
                serde_json::json!([
                    s.label,
                    s.st.short(),
                    sha(&s.ctl),
                    sha(&s.code),
                    s.ctl.len(),
                    s.code.len()
                ])
            })
            .collect(),
    )
}

/// This Mac's zlib (Apple's 1.2.12) words an invalid code met by its fast path as one message; upstream's and zlib's slow path give two
fn zmsg(s: &str) -> String {
    s.replace("invalid literal/length/distance code", "invalid code")
        .replace("invalid literal/length code", "invalid code")
        .replace("invalid distance code", "invalid code")
}

/// `{"ok": v}` against a Rust result: the same value, or (Python raised) an error; a `red()` text must be the same text
fn outcome(py: &Value, rust: Result<Value, String>, what: &str, bad: &mut Vec<String>) {
    match (py.get("ok"), rust) {
        (Some(o), Ok(r)) => {
            if o != &r {
                bad.push(format!("{what}: Python {o}, Rust {r}"));
            }
        }
        (Some(o), Err(e)) => bad.push(format!("{what}: Python answered {o}, Rust failed: {e}")),
        (None, Ok(r)) => bad.push(format!(
            "{what}: Python raised {}, Rust answered {r}",
            py["err"]
        )),
        (None, Err(e)) => {
            let want = py["err"].as_str().unwrap();
            if let Some(w) = want.strip_prefix("[RED] ")
                && zmsg(w) != zmsg(&e)
            {
                bad.push(format!("{what}: red() text: Python {w:?}, Rust {e:?}"));
            }
        }
    }
}

fn run_cx<T>(f: impl FnOnce(&Ctx<'_>) -> T) -> T {
    let rules = KeyRules::default();
    let (trees, dicts, files, log) = (
        Trees::new(),
        HashMap::new(),
        FileIndex::empty(),
        |_: &str| {},
    );
    let cx = Ctx {
        rules: &rules,
        zstd: &Ruzstd,
        common: None,
        borrowed: &trees,
        zdicts: &dicts,
        log: &log,
        files: &files,
        bahs_var: RefCell::new(HashMap::new()),
    };
    f(&cx)
}

fn e2s(e: Failure) -> String {
    e.to_string()
}

#[test]
fn the_rebased_engine_is_the_ports() {
    let Some(vecs) = vectors() else {
        ibuilder_testkit::skip(
            "tests/data/vectors-rebase.jsonl.gz is not there (run tools/gen_rebase.py)",
        );
        return;
    };
    let rules = KeyRules::default();
    let mut bad: Vec<String> = Vec::new();
    let mut n: HashMap<String, usize> = HashMap::new();
    for (i, v) in vecs.iter().enumerate() {
        let (inp, out) = (&v["in"], &v["out"]);
        let kind = v["kind"].as_str().unwrap().to_string();
        *n.entry(kind.clone()).or_default() += 1;
        match kind.as_str() {
            "rt_check" => {
                let (ctl, code) = (d64(&inp["ctl"]), d64(&inp["code"]));
                let what = format!("rt_check {i}");
                outcome(
                    &out["check"],
                    check(St::Rt, &ctl, &code)
                        .map(|r| {
                            r.map(|s| Value::String(s.to_string()))
                                .unwrap_or(Value::Null)
                        })
                        .map_err(e2s),
                    &what,
                    &mut bad,
                );
                for st in ["vs", "cs"] {
                    let s = St::from_short(st).unwrap();
                    outcome(
                        &out["check_other"][st],
                        check(s, &ctl, &code)
                            .map(|r| {
                                r.map(|s| Value::String(s.to_string()))
                                    .unwrap_or(Value::Null)
                            })
                            .map_err(e2s),
                        &format!("{what} ({st})"),
                        &mut bad,
                    );
                }
                if out["stage"] != st_name(nvn2_stage(&ctl, &code)) {
                    bad.push(format!(
                        "{what}: nvn2_stage Python {}, Rust {}",
                        out["stage"],
                        st_name(nvn2_stage(&ctl, &code))
                    ));
                }
                outcome(
                    &out["glslc_stage"],
                    glslc_stage(&ctl, &code).map(st_name).map_err(e2s),
                    &format!("{what} glslc_stage"),
                    &mut bad,
                );
                outcome(
                    &out["skey"],
                    skey(&rules, &ctl, &code).map(Value::String).map_err(e2s),
                    &format!("{what} skey"),
                    &mut bad,
                );
                if out["nvn2"].as_bool().unwrap() != nvn2_ctl(&ctl) {
                    bad.push(format!("{what}: nvn2_ctl"));
                }
            }
            "rt_same" => {
                let (a, ca, b, cb) = (
                    d64(&inp["a"]),
                    d64(&inp["ca"]),
                    d64(&inp["b"]),
                    d64(&inp["cb"]),
                );
                let what = format!("rt_same {i}");
                outcome(
                    &out["same"],
                    same_shader(&rules, (&a, &ca), (&b, &cb))
                        .map(Value::Bool)
                        .map_err(e2s),
                    &format!("{what} a,b"),
                    &mut bad,
                );
                outcome(
                    &out["same_ba"],
                    same_shader(&rules, (&b, &cb), (&a, &ca))
                        .map(Value::Bool)
                        .map_err(e2s),
                    &format!("{what} b,a"),
                    &mut bad,
                );
                outcome(
                    &serde_json::json!({"ok": out["ka"]}),
                    skey(&rules, &a, &ca).map(Value::String).map_err(e2s),
                    &format!("{what} ka"),
                    &mut bad,
                );
                outcome(
                    &out["kb"],
                    skey(&rules, &b, &cb).map(Value::String).map_err(e2s),
                    &format!("{what} kb"),
                    &mut bad,
                );
            }
            "fn_bnsh" => {
                let d = d64(&inp["b64"]);
                outcome(
                    out,
                    bnsh_shaders(&d, 0, "/t")
                        .map(|f| found_json(&f))
                        .map_err(e2s),
                    &format!("fn_bnsh {i}"),
                    &mut bad,
                );
            }
            "fn_civbsf" => {
                let d = d64(&inp["b64"]);
                outcome(
                    out,
                    civbsf_shaders(&d, "/t")
                        .map(|f| found_json(&f))
                        .map_err(e2s),
                    &format!("fn_civbsf {i}"),
                    &mut bad,
                );
            }
            "fn_bnd2" => {
                let d = d64(&inp["b64"]);
                let r = bnd2_resources(&d, "/t")
                    .map(|rs| {
                        Value::Array(
                            rs.iter()
                                .map(|(r, t, k, x)| serde_json::json!([r, t, k, sha(x), x.len()]))
                                .collect(),
                        )
                    })
                    .map_err(e2s);
                outcome(
                    &out["resources"],
                    r,
                    &format!("fn_bnd2 {i} resources"),
                    &mut bad,
                );
                outcome(
                    &out["shaders"],
                    bnd2_shaders(&d, "/t").map(|f| found_json(&f)).map_err(e2s),
                    &format!("fn_bnd2 {i} shaders"),
                    &mut bad,
                );
            }
            "fn_phyre" => {
                let d = d64(&inp["b64"]);
                outcome(
                    &out["phyre"],
                    phyre_shaders(&d, "/t").map(|f| found_json(&f)).map_err(e2s),
                    &format!("fn_phyre {i}"),
                    &mut bad,
                );
                let r = run_cx(|cx| {
                    let mut found = Vec::new();
                    scan_blob(cx, &d, "/t", &mut found, 0, None).map(|_| found_json(&found))
                });
                outcome(
                    &out["blob"],
                    r.map_err(e2s),
                    &format!("fn_phyre {i} scan_blob"),
                    &mut bad,
                );
            }
            "kit" => kit_vector(i, inp, out, &mut bad),
            other => panic!("vector {i}: kind {other}"),
        }
    }
    assert!(
        bad.is_empty(),
        "{} differences, the first ones:\n{}",
        bad.len(),
        bad.iter().take(15).cloned().collect::<Vec<_>>().join("\n")
    );
    let mut k: Vec<_> = n.into_iter().collect();
    k.sort();
    eprintln!(
        "rebase vectors: {}",
        k.iter()
            .map(|(a, b)| format!("{a} {b}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// the part of a message before the LZ4 library's own wording
fn lz4_cut(s: &str) -> &str {
    s.split("Decompression failed").next().unwrap()
}

fn kit_vector(i: usize, inp: &Value, out: &Value, bad: &mut Vec<String>) {
    let index = d64(&inp["index"]);
    let parts: HashMap<u16, Vec<u8>> = inp["parts"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.parse().unwrap(), d64(v)))
        .collect();
    let path = "<P>/x.kit";
    let part_len = |n: u16| parts.get(&n).map(|p| p.len() as u64);
    let rust = kit::entries(path, &index, &part_len);
    match (out["entries"].get("ok"), &rust) {
        (Some(o), Ok(es)) => {
            let got: Vec<Value> = es
                .iter()
                .map(|e| {
                    serde_json::json!([
                        e.index,
                        e.part,
                        e.offset,
                        e.length,
                        e.size,
                        e.compressed,
                        e.sha1
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ])
                })
                .collect();
            if o != &Value::Array(got) {
                bad.push(format!("kit {i}: entries differ"));
                return;
            }
            let reads = out["reads"].as_array().unwrap();
            for (e, want) in es.iter().zip(reads) {
                let bytes = parts[&e.part]
                    .get(e.offset as usize..)
                    .map(|s| &s[..s.len().min(e.length as usize)])
                    .unwrap_or(&[]);
                match (want.get("ok"), kit::read(path, e, bytes)) {
                    (Some(h), Ok(d)) => {
                        if h.as_str().unwrap() != sha(&d) {
                            bad.push(format!("kit {i} member {}: decoded bytes differ", e.index));
                        }
                    }
                    (Some(_), Err(m)) => bad.push(format!(
                        "kit {i} member {}: Python read it, Rust: {m}",
                        e.index
                    )),
                    (None, Ok(_)) => bad.push(format!(
                        "kit {i} member {}: Python raised {}, Rust read it",
                        e.index, want["err"]
                    )),
                    (None, Err(m)) => {
                        let py = want["err"].as_str().unwrap();
                        if lz4_cut(py) != lz4_cut(&format!("ValueError: {m}")) {
                            bad.push(format!(
                                "kit {i} member {}: text: Python {py:?}, Rust {m:?}",
                                e.index
                            ));
                        }
                    }
                }
            }
        }
        (Some(_), Err(m)) => bad.push(format!("kit {i}: Python read the index, Rust: {m}")),
        (None, Ok(_)) => bad.push(format!(
            "kit {i}: Python raised {}, Rust read the index",
            out["entries"]["err"]
        )),
        (None, Err(m)) => {
            let py = out["entries"]["err"].as_str().unwrap();
            if py != format!("ValueError: {m}") {
                bad.push(format!("kit {i}: index error: Python {py:?}, Rust {m:?}"));
            }
        }
    }
}

// ---- games that end in an error ------------------------------------------------------------------------------------------------------------------

fn run_err_game(
    g: &Path,
    jobs: usize,
) -> (
    Result<ibuilder_shadersplit::SplitStats, ibuilder_shadersplit::SplitError>,
    PathBuf,
) {
    let out = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-err-{}-{}-{jobs}",
        g.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    let (romfs, main, prof) = (g.join("RomFS"), g.join("main"), g.join("profile"));
    let (pkg, tables) = (out.join("pkg"), out.join("tables"));
    let mut o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, "0100TESTTEST0000");
    o.jobs = jobs;
    let r = split(&o, &Ruzstd, &NoGate, &|_| {});
    (r, out)
}

#[test]
fn a_split_that_ends_in_an_error_says_what_the_engine_says() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games-err");
    let Ok(rd) = std::fs::read_dir(&root) else {
        ibuilder_testkit::skip("tests/data/games-err is not there (run tools/gen_rebase.py)");
        return;
    };
    let mut games: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("expected.json").is_file())
        .collect();
    games.sort();
    assert!(!games.is_empty());
    for g in &games {
        let name = g.file_name().unwrap().to_string_lossy().into_owned();
        let exp: Value =
            serde_json::from_slice(&std::fs::read(g.join("expected.json")).unwrap()).unwrap();
        let want = exp["error"].as_str().unwrap();
        for jobs in [1usize, 8] {
            let (r, out) = run_err_game(g, jobs);
            let got = match r {
                Ok(s) => panic!(
                    "{name}: the split succeeded ({} shaders), the engine's ended with {want:?}",
                    s.shaders
                ),
                Err(e) => format!(
                    "[RED] {}",
                    e.to_string()
                        .replace(&g.join("RomFS").display().to_string(), "@ROMFS@")
                ),
            };
            assert_eq!(want, got, "{name} ({jobs} threads)");
            let _ = std::fs::remove_dir_all(&out);
        }
    }
}
