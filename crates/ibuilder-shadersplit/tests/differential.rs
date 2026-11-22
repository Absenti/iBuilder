//! Differential tests against the engine's Python: `tests/data/vectors-*.jsonl` hold synthetic inputs (PRNG bytes in the layouts the readers expect, mutated) and what
//! `shader_split.py` / `unityfs.py` answered for them (`tools/gen_vectors.py` writes them; every answer is the Python's). Each Rust function must give the same answer:
//! equal values where Python returned one, an error where Python raised.

use ibuilder_shadersplit::Ruzstd;
use ibuilder_shadersplit::bytes::hex;
use ibuilder_shadersplit::ctl::*;
use ibuilder_shadersplit::scan::{self, var_pairs_of};
use ibuilder_shadersplit::scan_types::{Ctx, Shader};
use ibuilder_shadersplit::split::FileIndex;
use ibuilder_shadersplit::unity::Trees;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

fn lcg_fill(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

fn materialise(sp: &Value) -> Vec<u8> {
    let mut b = lcg_fill(
        sp["seed"].as_u64().unwrap(),
        sp["len"].as_u64().unwrap() as usize,
    );
    for s in sp["set"].as_array().unwrap() {
        let o = s[0].as_u64().unwrap() as usize;
        let v = unhex(s[1].as_str().unwrap());
        // Python's `b[o:o + len(v)] = v` on a bytearray: the slice is clamped to the buffer, and a set past the end appends
        let start = o.min(b.len());
        let stop = (o + v.len()).min(b.len());
        b.splice(start..stop, v.iter().copied());
    }
    b
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn vectors(name: &str) -> Option<Vec<Value>> {
    let p = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read(&p).ok()?;
    let text =
        String::from_utf8(ibuilder_shadersplit::inflate::gunzip(&raw, 1 << 30).unwrap()).unwrap();
    Some(
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

fn st_name(s: Option<St>) -> Value {
    match s {
        Some(s) => Value::String(s.short().to_string()),
        None => Value::Null,
    }
}

/// compare a Python outcome `{"ok": v}` / `{"err": ..}` with a Rust one
fn same<T: PartialEq + std::fmt::Debug>(
    py: &Value,
    rust: Result<T, String>,
    conv: impl Fn(&Value) -> T,
    what: &str,
    i: usize,
) {
    match (py.get("ok"), rust) {
        (Some(v), Ok(r)) => assert_eq!(conv(v), r, "{what}: vector {i}"),
        (Some(v), Err(e)) => panic!("{what}: vector {i}: Python answered {v}, Rust failed: {e}"),
        (None, Ok(r)) => panic!("{what}: vector {i}: Python raised {py}, Rust answered {r:?}"),
        (None, Err(_)) => {}
    }
}

fn run(file: &str, rules: KeyRules) {
    let vecs = match vectors(file) {
        Some(v) => v,
        None => {
            ibuilder_testkit::skip(&format!("{file} is not there (run tools/gen_vectors.py)"));
            return;
        }
    };
    let (mut n_ctl, mut n_same, mut n_rec, mut n_blob, mut n_other, mut n_unity) =
        (0, 0, 0, 0, 0, 0);
    let known = ibuilder_shadersplit::unity::known_trees();
    let ztr = Ruzstd;
    let empty_trees = Trees::new();
    let dicts: HashMap<u32, Vec<u8>> = HashMap::new();
    let files = FileIndex::empty();
    let log = |_: &str| {};
    for (i, v) in vecs.iter().enumerate() {
        let (inp, out) = (&v["in"], &v["out"]);
        match v["kind"].as_str().unwrap() {
            "ctl" => {
                n_ctl += 1;
                let (ctl, code) = (materialise(&inp["ctl"]), materialise(&inp["code"]));
                assert_eq!(
                    out["nvn2_ctl"].as_bool().unwrap(),
                    nvn2_ctl(&ctl),
                    "nvn2_ctl {i}"
                );
                same(
                    &out["ctl_len"],
                    ctl_len(&ctl).map_err(|e| e.to_string()),
                    |v| v.as_u64().unwrap() as usize,
                    "ctl_len",
                    i,
                );
                same(
                    &out["key_ctl"],
                    key_ctl(&rules, &ctl)
                        .map(|k| sha(&k))
                        .map_err(|e| e.to_string()),
                    |v| v.as_str().unwrap().to_string(),
                    "key_ctl",
                    i,
                );
                same(
                    &out["skey"],
                    skey(&rules, &ctl, &code).map_err(|e| e.to_string()),
                    |v| v.as_str().unwrap().to_string(),
                    "skey",
                    i,
                );
                assert_eq!(out["sph_stage"], st_name(sph_stage(&code)), "sph_stage {i}");
                assert_eq!(
                    out["nvn2_stage"],
                    st_name(nvn2_stage(&ctl, &code)),
                    "nvn2_stage {i}"
                );
                same(
                    &out["glslc_stage"],
                    glslc_stage(&ctl, &code)
                        .map(st_name)
                        .map_err(|e| e.to_string()),
                    |v| v.clone(),
                    "glslc_stage",
                    i,
                );
                for st in [St::Vs, St::Hs, St::Ds, St::Gs, St::Ps, St::Cs] {
                    same(
                        &out["check"][st.short()],
                        check(st, &ctl, &code)
                            .map(|r| r.map(|s| s.to_string()))
                            .map_err(|e| e.to_string()),
                        |v| v.as_str().map(|s| s.to_string()),
                        &format!("check {}", st.short()),
                        i,
                    );
                }
            }
            "same" => {
                n_same += 1;
                let a = materialise(&inp["a"]);
                // b: a with the byte differences applied, cut to `cut`, then `ext` appended
                let mut b = a.clone();
                for st in inp["b"]["sets"].as_array().unwrap() {
                    b[st[0].as_u64().unwrap() as usize] = unhex(st[1].as_str().unwrap())[0];
                }
                b.truncate(inp["b"]["cut"].as_u64().unwrap() as usize);
                b.extend(unhex(inp["b"]["ext"].as_str().unwrap()));
                let (ka, kb) = (
                    unhex(inp["ka"].as_str().unwrap()),
                    unhex(inp["kb"].as_str().unwrap()),
                );
                same(
                    out,
                    same_shader(&rules, (&a, &ka), (&b, &kb)).map_err(|e| e.to_string()),
                    |v| v.as_bool().unwrap(),
                    "same_shader",
                    i,
                );
            }
            "record" => {
                n_rec += 1;
                let (ctl, code) = (materialise(&inp["ctl"]), materialise(&inp["code"]));
                let key = inp["key"].as_str().unwrap();
                same(
                    &out["record"],
                    record(key, &ctl, &code).map(|r| sha(&r)),
                    |v| v.as_str().unwrap().to_string(),
                    "record",
                    i,
                );
                let facts = compute_facts(&ctl);
                same(
                    &out["facts"],
                    facts.map(|(l, s)| serde_json::json!({"local": l, "shared": s})),
                    |v| v.clone(),
                    "facts",
                    i,
                );
            }
            "blob" | "glslc" => {
                n_blob += 1;
                let is_glslc = v["kind"] == "glslc";
                let d = unhex(inp["hex"].as_str().unwrap());
                let label = inp["label"].as_str().unwrap();
                let cx = Ctx {
                    rules: &rules,
                    zstd: &ztr,
                    common: None,
                    borrowed: &empty_trees,
                    zdicts: &dicts,
                    log: &log,
                    files: &files,
                    bahs_var: RefCell::new(HashMap::new()),
                };
                let mut found: Vec<Shader> = Vec::new();
                let r = if is_glslc {
                    ibuilder_shadersplit::glslc::glslc_shaders(&d, label, false, &[])
                        .map(|f| found = f)
                } else {
                    scan::scan_blob(&cx, &d, label, &mut found, 0, None)
                };
                match (out.get("ok"), r) {
                    (Some(o), Ok(())) => {
                        let fj: Vec<Value> = found
                            .iter()
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
                            .collect();
                        assert_eq!(o["found"], Value::Array(fj), "blob found {i} ({label})");
                        let mut pairs: Vec<Vec<String>> = var_pairs_of(&cx, &found)
                            .unwrap()
                            .iter()
                            .map(|p| {
                                if p.prog {
                                    vec![p.a.clone(), p.b.clone(), "prog".to_string()]
                                } else {
                                    vec![p.a.clone(), p.b.clone()]
                                }
                            })
                            .collect();
                        pairs.sort();
                        let want: Vec<Vec<String>> = o["pairs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|p| {
                                p.as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|x| x.as_str().unwrap().to_string())
                                    .collect()
                            })
                            .collect();
                        assert_eq!(want, pairs, "blob pairs {i} ({label})");
                        let fr = scan::finish(&cx, "t", found.clone(), [], Trees::new());
                        match (o.get("uniq"), fr) {
                            (Some(u), Ok(fr)) => {
                                let got: Vec<Value> = fr
                                    .uniq
                                    .iter()
                                    .map(|(k, s)| serde_json::json!([k, s.label]))
                                    .collect();
                                assert_eq!(u, &Value::Array(got), "blob uniq {i} ({label})");
                            }
                            (None, Err(_)) => {}
                            (a, b) => panic!(
                                "blob uniq {i}: Python {a:?}, Rust {:?}",
                                b.map(|f| f.uniq.len())
                            ),
                        }
                    }
                    (None, Err(_)) => {}
                    (Some(o), Err(e)) => panic!(
                        "blob {i} ({label}): Python found {} shaders, Rust failed: {e}",
                        o["found"].as_array().map(|a| a.len()).unwrap_or(0)
                    ),
                    (None, Ok(())) => panic!(
                        "blob {i} ({label}): Python raised {out}, Rust found {} shaders",
                        found.len()
                    ),
                }
            }
            "yaz0" => {
                n_other += 1;
                let d = unhex(inp["hex"].as_str().unwrap());
                same(
                    out,
                    ibuilder_shadersplit::archive::yaz0(&d, "t")
                        .map(|r| sha(&r))
                        .map_err(|e| e.to_string()),
                    |v| v.as_str().unwrap().to_string(),
                    "yaz0",
                    i,
                );
            }
            "sarc" => {
                n_other += 1;
                let d = unhex(inp["hex"].as_str().unwrap());
                let r = ibuilder_shadersplit::archive::sarc_members(&d, "t")
                    .map(|m| {
                        m.into_iter()
                            .map(|(n, b)| serde_json::json!([n, sha(b)]))
                            .collect::<Vec<_>>()
                    })
                    .map_err(|e| e.to_string());
                same(out, r, |v| v.as_array().unwrap().clone(), "sarc", i);
            }
            "cover" => {
                n_other += 1;
                let ps: BTreeMap<String, Vec<u8>> = inp["ps"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), unhex(v.as_str().unwrap())))
                    .collect();
                same(
                    out,
                    scan::pixel_input_cover_pub(&ps).map_err(|e| e.to_string()),
                    |v| {
                        v.as_array()
                            .unwrap()
                            .iter()
                            .map(|x| x.as_str().unwrap().to_string())
                            .collect::<Vec<_>>()
                    },
                    "cover",
                    i,
                );
            }
            "unity_obj" => {
                n_other += 1;
                let (_, tree) = &known[inp["layout"].as_u64().unwrap() as usize];
                let d = unhex(inp["hex"].as_str().unwrap());
                same(
                    out,
                    ibuilder_shadersplit::unity::object_json(tree, &d, true)
                        .map_err(|e| e.to_string()),
                    |v| v.clone(),
                    "unity object",
                    i,
                );
            }
            "unity_file" => {
                n_unity += 1;
                let d = unhex(inp["hex"].as_str().unwrap());
                let label = inp["label"].as_str().unwrap();
                let common: HashMap<u32, String> = HashMap::new();
                let mut borrowed = Trees::new();
                if let Some(li) = inp["layout"].as_u64() {
                    let (k, tree) = &known[li as usize];
                    borrowed.insert(*k, Some(tree.clone()));
                }
                let printed = std::sync::Mutex::new(Vec::<String>::new());
                let plog = |l: &str| printed.lock().unwrap().push(l.to_string());
                let cx = Ctx {
                    rules: &rules,
                    zstd: &ztr,
                    common: Some(&common),
                    borrowed: &borrowed,
                    zdicts: &dicts,
                    log: &plog,
                    files: &files,
                    bahs_var: RefCell::new(HashMap::new()),
                };
                let mut found: Vec<Shader> = Vec::new();
                let mut trees = borrowed.clone();
                let bundle = inp.get("bundle").and_then(|b| b.as_bool()).unwrap_or(false);
                let src = ibuilder_shadersplit::source::MemSource(&d);
                let r = scan::unity_shaders(&cx, &src, bundle, label, &mut found, &mut trees);
                match (out.get("ok"), out.get("notree"), r) {
                    (Some(o), _, Ok(pairs)) => {
                        let fj: Vec<Value> = found
                            .iter()
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
                            .collect();
                        assert_eq!(o["found"], Value::Array(fj), "unity found {i} ({label})");
                        let pj: Vec<Value> = pairs
                            .iter()
                            .map(|(a, b)| serde_json::json!([a, b]))
                            .collect();
                        assert_eq!(o["pairs"], Value::Array(pj), "unity pairs {i}");
                        let mut tj: Vec<(i32, String, Value)> = trees
                            .iter()
                            .map(|((c, h), n)| {
                                (
                                    *c,
                                    hex(h),
                                    n.as_ref()
                                        .map(|n| ibuilder_shadersplit::unity::node_json(n))
                                        .unwrap_or(Value::Null),
                                )
                            })
                            .collect();
                        tj.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
                        let tj: Vec<Value> = tj
                            .into_iter()
                            .map(|(c, h, n)| serde_json::json!([c, h, n]))
                            .collect();
                        assert_eq!(o["trees"], Value::Array(tj), "unity trees {i}");
                        let pr: Vec<Value> = printed
                            .lock()
                            .unwrap()
                            .iter()
                            .map(|s| Value::String(s.clone()))
                            .collect();
                        assert_eq!(o["printed"], Value::Array(pr), "unity printed {i}");
                    }
                    (None, Some(_), Err(ibuilder_shadersplit::Failure::NoTree(_))) => {}
                    (None, None, Err(e))
                        if !matches!(e, ibuilder_shadersplit::Failure::NoTree(_)) => {}
                    (o, nt, r) => panic!(
                        "unity file {i} ({label}): Python {o:?} / notree {nt:?}, Rust {:?}",
                        r.map(|p| (found.len(), p.len()))
                    ),
                }
            }
            "live" => {
                n_other += 1;
                // shader_device.live_package: the same entries, the same refusals, and the same files
                let entries: Vec<(String, Vec<u8>, Vec<u8>)> = inp["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| {
                        (
                            e[0].as_str().unwrap().to_string(),
                            unhex(e[1].as_str().unwrap()),
                            unhex(e[2].as_str().unwrap()),
                        )
                    })
                    .collect();
                let dir = std::env::temp_dir().join(format!(
                    "ibuilder-shadersplit-livediff-{}-{i}",
                    std::process::id()
                ));
                let _ = std::fs::remove_dir_all(&dir);
                let (ok, bad) = ibuilder_shadersplit::live::package(
                    &rules,
                    &entries,
                    &dir.join("pkg"),
                    &dir.join("tables"),
                )
                .unwrap();
                let o = &out["ok"];
                let want_ok: BTreeMap<String, String> = o["ok"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect();
                assert_eq!(want_ok, ok, "live ok {i}");
                let want_bad: Vec<&String> = o["bad"].as_object().unwrap().keys().collect();
                assert_eq!(want_bad, bad.keys().collect::<Vec<_>>(), "live refused {i}");
                for (k, why) in o["bad"].as_object().unwrap() {
                    // the Python's ValueError texts are this crate's; a struct.error's text is Python's own
                    let w = why.as_str().unwrap();
                    if w.starts_with("control section of")
                        || w.starts_with("its bytes give key")
                        || w.starts_with("stage not recognised")
                        || w.starts_with("the control magic")
                        || w.starts_with("the record separator")
                        || w.starts_with("compute shader with")
                    {
                        assert_eq!(w, bad[k], "live refusal text {i}");
                    }
                }
                let mut files: Vec<(String, String)> = Vec::new();
                for sub in ["pkg", "tables"] {
                    let mut names: Vec<_> = std::fs::read_dir(dir.join(sub))
                        .unwrap()
                        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .collect();
                    names.sort();
                    for n in names {
                        files.push((
                            format!("{sub}/{n}"),
                            sha(&std::fs::read(dir.join(sub).join(&n)).unwrap()),
                        ));
                    }
                }
                let want_files: Vec<(String, String)> = o["files"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect();
                assert_eq!(want_files, files, "live files {i}");
                let _ = std::fs::remove_dir_all(&dir);
            }
            other => {
                eprintln!("vector {i}: kind {other} is checked elsewhere");
            }
        }
    }
    eprintln!(
        "{file}: {} vectors ok (ctl {n_ctl}, same_shader {n_same}, record {n_rec}, containers {n_blob}, unity files {n_unity}, other {n_other})",
        vecs.len()
    );
}

#[test]
fn skip_none() {
    run("vectors-skip0.jsonl.gz", KeyRules::default());
}

#[test]
fn skip_770_78() {
    run(
        "vectors-skip770.jsonl.gz",
        KeyRules {
            skip_off: 0x770,
            skip_len: 0x78,
        },
    );
}
