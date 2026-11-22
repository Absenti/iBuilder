//! `compare_nxgame.py`: read two `game.manifest.json` files and report EVERY key.
//!
//! The union of the two manifests' keys is split into: equal, different, only in the reference, only in the
//! candidate, JSON type mismatch; plus whether key ORDER is the same. A difference is a defect unless it is NAMED
//! (`--reason KEY=TEXT`): the exit status is 0 only when every difference carries a reason.
use crate::json::{Dump, Json, dumps, loads};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Diff {
    pub key: String,
    pub reference: Json,
    pub candidate: Json,
}

#[derive(Clone, Debug)]
pub struct TypeMismatch {
    pub key: String,
    pub reference_type: &'static str,
    pub candidate_type: &'static str,
    pub reference: Json,
    pub candidate: Json,
}

#[derive(Clone, Debug)]
pub struct CompareReport {
    pub equal: Vec<String>,
    pub different: Vec<Diff>,
    pub only_reference: Vec<String>,
    pub only_candidate: Vec<String>,
    pub type_mismatch: Vec<TypeMismatch>,
    pub order_same: bool,
    pub reference_key_order: Vec<String>,
    pub candidate_key_order: Vec<String>,
    pub reasons: Vec<(String, String)>,
    pub unexplained: Vec<String>,
}

/// `compare(ref, cand, reasons)`; both must be JSON objects.
pub fn compare(reference: &Json, candidate: &Json, reasons: &[(String, String)]) -> CompareReport {
    let r_keys = reference.keys();
    let c_keys = candidate.keys();
    let has = |keys: &[String], k: &str| keys.iter().any(|x| x == k);
    let mut equal = Vec::new();
    let mut different = Vec::new();
    let mut type_mismatch = Vec::new();
    for k in &r_keys {
        if !has(&c_keys, k) {
            continue;
        }
        let (a, b) = (reference.get(k).unwrap(), candidate.get(k).unwrap());
        if a.py_eq(b) && a.type_name() == b.type_name() {
            equal.push(k.clone());
        } else if a.type_name() != b.type_name() {
            type_mismatch.push(TypeMismatch {
                key: k.clone(),
                reference_type: a.type_name(),
                candidate_type: b.type_name(),
                reference: a.clone(),
                candidate: b.clone(),
            });
        } else {
            different.push(Diff {
                key: k.clone(),
                reference: a.clone(),
                candidate: b.clone(),
            });
        }
    }
    let only_reference: Vec<String> = r_keys
        .iter()
        .filter(|k| !has(&c_keys, k))
        .cloned()
        .collect();
    let only_candidate: Vec<String> = c_keys
        .iter()
        .filter(|k| !has(&r_keys, k))
        .cloned()
        .collect();
    let named = |k: &str| reasons.iter().any(|(r, _)| r == k);
    let mut unexplained: Vec<String> = different
        .iter()
        .filter(|d| !named(&d.key))
        .map(|d| d.key.clone())
        .collect();
    unexplained.extend(only_reference.iter().filter(|k| !named(k)).cloned());
    unexplained.extend(only_candidate.iter().filter(|k| !named(k)).cloned());
    unexplained.extend(
        type_mismatch
            .iter()
            .filter(|d| !named(&d.key))
            .map(|d| d.key.clone()),
    );
    CompareReport {
        equal,
        different,
        only_reference,
        only_candidate,
        type_mismatch,
        order_same: r_keys == c_keys,
        reference_key_order: r_keys,
        candidate_key_order: c_keys,
        reasons: reasons.to_vec(),
        unexplained,
    }
}

/// `find_manifest`: a file as is; a folder: `game.manifest.json` directly, else the first
/// `*.framework/game.manifest.json`, else the first `game.manifest.json` anywhere below (sorted).
pub fn find_manifest(p: &Path) -> Result<PathBuf, String> {
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    if p.is_dir() {
        let direct = p.join("game.manifest.json");
        if direct.is_file() {
            return Ok(direct);
        }
        let mut hits: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                let q = e.path();
                if e.file_name().to_string_lossy().ends_with(".framework")
                    && q.join("game.manifest.json").is_file()
                {
                    hits.push(q.join("game.manifest.json"));
                }
            }
        }
        if hits.is_empty() {
            fn rglob(d: &Path, out: &mut Vec<PathBuf>) {
                if let Ok(rd) = fs::read_dir(d) {
                    for e in rd.flatten() {
                        let q = e.path();
                        if q.is_dir() {
                            rglob(&q, out);
                        } else if e.file_name() == "game.manifest.json" {
                            out.push(q);
                        }
                    }
                }
            }
            rglob(p, &mut hits);
        }
        hits.sort_by_key(|h| {
            h.to_string_lossy()
                .split('/')
                .map(str::to_string)
                .collect::<Vec<_>>()
        });
        if let Some(h) = hits.into_iter().next() {
            return Ok(h);
        }
    }
    Err(format!(
        "[RED] compare_nxgame: no game.manifest.json under {}",
        p.display()
    ))
}

fn j(v: &Json) -> String {
    dumps(v, &Dump::compact())
}

fn pad22(s: &str) -> String {
    format!("{s:<22}")
}

fn str_list(v: &[String]) -> Json {
    Json::Arr(v.iter().map(|s| Json::str(s.clone())).collect())
}

/// The `--json-out` report (`json.dumps(rep, indent=2) + '\n'`, ensure_ascii on).
pub fn report_json(rep: &CompareReport, reference: &Path, candidate: &Path, label: &str) -> String {
    let mut o = Json::obj();
    o.set("equal", str_list(&rep.equal));
    o.set(
        "different",
        Json::Arr(
            rep.different
                .iter()
                .map(|d| {
                    let mut x = Json::obj();
                    x.set("key", Json::str(d.key.clone()));
                    x.set("reference", d.reference.clone());
                    x.set("candidate", d.candidate.clone());
                    x
                })
                .collect(),
        ),
    );
    o.set("only_reference", str_list(&rep.only_reference));
    o.set("only_candidate", str_list(&rep.only_candidate));
    o.set(
        "type_mismatch",
        Json::Arr(
            rep.type_mismatch
                .iter()
                .map(|d| {
                    let mut x = Json::obj();
                    x.set("key", Json::str(d.key.clone()));
                    x.set("reference_type", Json::str(d.reference_type));
                    x.set("candidate_type", Json::str(d.candidate_type));
                    x.set("reference", d.reference.clone());
                    x.set("candidate", d.candidate.clone());
                    x
                })
                .collect(),
        ),
    );
    o.set("order_same", Json::Bool(rep.order_same));
    o.set("reference_key_order", str_list(&rep.reference_key_order));
    o.set("candidate_key_order", str_list(&rep.candidate_key_order));
    let mut r = Json::obj();
    for (k, t) in &rep.reasons {
        r.set(k, Json::str(t.clone()));
    }
    o.set("reasons", r);
    o.set("unexplained", str_list(&rep.unexplained));
    o.set("reference", Json::str(reference.display().to_string()));
    o.set("candidate", Json::str(candidate.display().to_string()));
    o.set("label", Json::str(label));
    dumps(&o, &Dump::indented(2, true)) + "\n"
}

/// The whole of `compare_nxgame.py main()`: find, load, compare, print the report into `stdout`; returns the exit
/// status (0 ok, 1 unexplained differences). `Err` is a `[RED] ...` usage/IO failure (exit 1 too).
pub fn compare_files_cli(
    reference: &Path,
    candidate: &Path,
    reasons: &[(String, String)],
    label: &str,
    json_out: Option<&Path>,
    stdout: &mut String,
) -> Result<i32, String> {
    let rp = find_manifest(reference)?;
    let cp = find_manifest(candidate)?;
    let load = |p: &Path| -> Result<Json, String> {
        let t = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let v = loads(&t).map_err(|e| format!("{}: {e}", p.display()))?;
        if v.is_obj() {
            Ok(v)
        } else {
            Err(format!("{}: a manifest is a JSON object", p.display()))
        }
    };
    let (rf, cf) = (load(&rp)?, load(&cp)?);
    let rep = compare(&rf, &cf, reasons);
    let mut all = rf.keys();
    for k in cf.keys() {
        if !all.contains(&k) {
            all.push(k);
        }
    }
    let why = |k: &str| {
        reasons
            .iter()
            .find(|(r, _)| r == k)
            .map(|(_, t)| t.as_str())
    };
    let w = stdout;
    writeln!(
        w,
        "== manifest comparison{} ==",
        if label.is_empty() {
            String::new()
        } else {
            format!(" [{label}]")
        }
    )
    .unwrap();
    writeln!(w, "  reference {}", rp.display()).unwrap();
    writeln!(w, "  candidate {}", cp.display()).unwrap();
    writeln!(
        w,
        "  equal {}/{}   different {}   only_reference {}   only_candidate {}   type_mismatch {}   order_same {}",
        rep.equal.len(),
        all.len(),
        rep.different.len(),
        rep.only_reference.len(),
        rep.only_candidate.len(),
        rep.type_mismatch.len(),
        if rep.order_same { "True" } else { "False" }
    )
    .unwrap();
    for k in &rep.equal {
        writeln!(w, "  = {} {}", pad22(k), j(rf.get(k).unwrap())).unwrap();
    }
    for d in &rep.different {
        let tail = match why(&d.key) {
            Some(t) => format!("   reason: {t}"),
            None => "   <<< UNEXPLAINED".to_string(),
        };
        writeln!(
            w,
            "  ! {} ref={}  ours={}{}",
            pad22(&d.key),
            j(&d.reference),
            j(&d.candidate),
            tail
        )
        .unwrap();
    }
    for k in &rep.only_reference {
        let tail = match why(k) {
            Some(t) => format!("   reason: {t}"),
            None => "   <<< MISSING".to_string(),
        };
        writeln!(
            w,
            "  - {} only in the reference: {}{}",
            pad22(k),
            j(rf.get(k).unwrap()),
            tail
        )
        .unwrap();
    }
    for k in &rep.only_candidate {
        let tail = match why(k) {
            Some(t) => format!("   reason: {t}"),
            None => "   <<< EXTRA".to_string(),
        };
        writeln!(
            w,
            "  + {} only in ours: {}{}",
            pad22(k),
            j(cf.get(k).unwrap()),
            tail
        )
        .unwrap();
    }
    for d in &rep.type_mismatch {
        writeln!(
            w,
            "  T {} ref {}={}  ours {}={}",
            pad22(&d.key),
            d.reference_type,
            j(&d.reference),
            d.candidate_type,
            j(&d.candidate)
        )
        .unwrap();
    }
    if !rep.order_same {
        let l = |v: &[String]| {
            format!(
                "[{}]",
                v.iter()
                    .map(|s| format!("'{s}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        writeln!(
            w,
            "  ORDER differs:\n    reference {}\n    candidate {}",
            l(&rep.reference_key_order),
            l(&rep.candidate_key_order)
        )
        .unwrap();
    }
    if let Some(p) = json_out {
        fs::write(p, report_json(&rep, &rp, &cp, label))
            .map_err(|e| format!("{}: {e}", p.display()))?;
        writeln!(w, "  report: {}", p.display()).unwrap();
    }
    if !rep.unexplained.is_empty() {
        let mut u = rep.unexplained.clone();
        u.sort();
        u.dedup();
        writeln!(
            w,
            "  RESULT: {} unexplained difference(s): {}",
            rep.unexplained.len(),
            u.join(", ")
        )
        .unwrap();
        return Ok(1);
    }
    writeln!(
        w,
        "  RESULT: every present-in-both key equal; every difference named"
    )
    .unwrap();
    Ok(0)
}
