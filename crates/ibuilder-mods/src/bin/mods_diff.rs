//! The differential driver: runs a list of operations (JSON) through this crate and prints the results as JSON, in the same shape as the Python driver
//! (`mods_diff.py`, which runs the same operations through `gameapp/mods.py`), so a harness can compare the two byte for byte.
//!
//! `mods_diff <ops.json> <out.json>`: ops is a list of objects with an `op`; the result list has one entry per op. Paths inside the operations are used as
//! given; strings in the output keep them (the harness normalises the case directory).

use ibuilder_mods::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn err(e: &ModsError) -> Value {
    json!({"exc": e.exc_name(), "msg": if e.is_mod() { e.message().to_string() } else if e.is_os() { e.message().to_string() } else { String::new() }})
}
fn writes_json(w: &[(i128, Vec<u8>)]) -> Value {
    Value::Array(
        w.iter()
            .map(|(o, d)| json!([o.to_string(), hex(d)]))
            .collect(),
    )
}
fn patch_json(p: &Patch) -> Value {
    json!({"mod": p.mod_name, "source": p.source, "name": p.name, "bid": p.bid, "on": p.on, "writes": writes_json(&p.writes)})
}
fn patch_from(v: &Value) -> Patch {
    let writes = v["writes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w[0].as_str().unwrap().parse::<i128>().unwrap(),
                unhex(w[1].as_str().unwrap()),
            )
        })
        .collect();
    Patch::new(
        s(v, "mod"),
        s(v, "source"),
        s(v, "name"),
        s(v, "bid"),
        v["on"].as_bool().unwrap(),
        writes,
    )
}
fn ok<T: Into<Value>>(v: T) -> Value {
    json!({"ok": v.into()})
}

fn run_scenario(op: &Value) -> Value {
    let docs = PathBuf::from(s(op, "docs"));
    let tid = s(op, "tid").to_string();
    let exe = PathBuf::from(s(op, "exe"));
    let modules: Vec<String> = op["modules"]
        .as_array()
        .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    let mut logs: Vec<String> = Vec::new();
    let mut steps = Vec::new();
    let mut plan_cache: Option<ModPlan> = None;
    for st in op["steps"].as_array().unwrap() {
        let kind = s(st, "k");
        let r: Value = match kind {
            "plan" => match plan(
                &docs,
                &tid,
                &exe,
                &modules,
                &mut |l| logs.push(l.to_string()),
                st["quiet"].as_bool().unwrap_or(false),
            ) {
                Ok(p) => {
                    let v = ok(Value::Array(
                        p.modules
                            .iter()
                            .map(|(m, e)| {
                                json!([
                                    m,
                                    e.digest,
                                    e.patches.iter().map(patch_json).collect::<Vec<_>>()
                                ])
                            })
                            .collect(),
                    ));
                    plan_cache = Some(p);
                    v
                }
                Err(e) => err(&e),
            },
            "apply" => {
                let Some(p) = &plan_cache else {
                    steps.push(json!({"skip": true}));
                    continue;
                };
                let mut res = Vec::new();
                let mut failed = None;
                for (m, e) in &p.modules {
                    match apply_module(&docs, &tid, &exe, m, &e.patches, &mut |l| {
                        logs.push(l.to_string())
                    }) {
                        Ok(path) => {
                            res.push(json!([m, path.map(|p| p.to_string_lossy().into_owned())]))
                        }
                        Err(e) => {
                            failed = Some(err(&e));
                            break;
                        }
                    }
                }
                failed.unwrap_or_else(|| ok(res))
            }
            "remove" => {
                remove_patched(&docs, &tid);
                ok(Value::Null)
            }
            "mark" => match mark_applied(&docs, &tid, &mut |l| logs.push(l.to_string())) {
                Ok(()) => ok(Value::Null),
                Err(e) => err(&e),
            },
            "boot" => {
                let mut env: std::collections::BTreeMap<String, String> = st["env"]
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                match apply_boot(&docs, &tid, &mut |l| logs.push(l.to_string()), &mut env) {
                    Ok(out) => ok(json!({"set": out, "env": env})),
                    Err(e) => err(&e),
                }
            }
            "cheats" => match write_cheats_folder(&docs, &tid, &mut |l| logs.push(l.to_string())) {
                Ok(f) => ok(f.map(|p| p.to_string_lossy().into_owned())),
                Err(e) => err(&e),
            },
            "signature" => match exefs_signature(&docs, &tid) {
                Ok(t) => ok(t),
                Err(e) => err(&e),
            },
            "state" => match read_state(&docs.join("games").join(&tid).join("mods.tsv")) {
                Ok(st) => ok(json!({
                    "mod": st.mods.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                    "patch": st.patch.iter().map(|((a, b), v)| json!([a, b, v])).collect::<Vec<_>>(),
                    "cheat": st.cheat.iter().map(|((a, b), v)| json!([a, b, v])).collect::<Vec<_>>(),
                })),
                Err(e) => err(&e),
            },
            "list" => ok(
                json!({"mods": list_mods(&docs, &tid), "enabled": enabled_mods(&docs, &tid).map(|v| json!(v)).unwrap_or_else(|e| err(&e))}),
            ),
            "record" => match record_build_ids(
                &exe,
                &docs.join("games").join(&tid).join("build_ids.txt"),
                &mut |l| logs.push(l.to_string()),
            ) {
                Ok(()) => ok(Value::Null),
                Err(e) => err(&e),
            },
            "patches" => match exefs_patches(&docs, &tid, None) {
                Ok((ps, sk)) => ok(
                    json!({"patches": ps.iter().map(patch_json).collect::<Vec<_>>(), "skipped": sk}),
                ),
                Err(e) => err(&e),
            },
            other => json!({"bad": other}),
        };
        steps.push(r);
    }
    json!({"steps": steps, "logs": logs})
}

fn run(op: &Value) -> Value {
    match s(op, "op") {
        "lz4" => {
            match lz4::decompress_safe(&unhex(s(op, "hex")), op["size"].as_u64().unwrap() as usize)
            {
                Ok(v) => ok(hex(&v)),
                Err(()) => json!({"exc": "LZ4BlockError", "msg": ""}),
            }
        }
        "ips" => match parse_ips(&unhex(s(op, "hex"))) {
            Ok(v) => ok(writes_json(&v)),
            Err(e) => err(&e),
        },
        "pch" => match parse_pchtxt(s(op, "text"), "m", "f.pchtxt") {
            Ok(v) => ok(Value::Array(v.iter().map(patch_json).collect())),
            Err(e) => err(&e),
        },
        "bidm" => ok(bid_matches(s(op, "wanted"), &unhex(s(op, "bid")))),
        "stem" => ok(stem_bid(s(op, "name"))),
        "sections" => ok(cheat_sections(s(op, "text"))),
        "cheatbid" => ok(cheat_file_bid(s(op, "name"))),
        "repr" => ok(pyfmt::repr(s(op, "text"))),
        "nso" => {
            let data = unhex(s(op, "hex"));
            let patches: Vec<Patch> = op["patches"]
                .as_array()
                .unwrap()
                .iter()
                .map(patch_from)
                .collect();
            match patch_nso(&data, &patches) {
                Ok(None) => ok(Value::Null),
                Ok(Some(p)) => ok(
                    json!({"data": hex(&p.data), "applied": p.applied.iter().map(|x| x.label()).collect::<Vec<_>>(), "notes": p.notes.iter().map(|(x, n)| json!([x.label(), n])).collect::<Vec<_>>()}),
                ),
                Err(e) => err(&e),
            }
        }
        "image" => match nso_image(&unhex(s(op, "hex"))) {
            Ok((h, segs)) => ok(
                json!({"header": hex(&h), "segs": segs.iter().map(|(m, b)| json!([m, hex(b)])).collect::<Vec<_>>()}),
            ),
            Err(e) => err(&e),
        },
        "scenario" => run_scenario(op),
        other => json!({"bad": other}),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: mods_diff <ops.json> <out.json>");
        std::process::exit(2);
    }
    let ops: Value = serde_json::from_str(&std::fs::read_to_string(&args[1]).expect("read ops"))
        .expect("parse ops");
    let out: Vec<Value> = ops
        .as_array()
        .expect("ops is a list")
        .iter()
        .map(run)
        .collect();
    std::fs::write(Path::new(&args[2]), serde_json::to_string(&out).unwrap()).expect("write out");
}
