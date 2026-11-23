//! Synthetic modules (no game code: hand-built jump tables, random instruction-template functions, and two registered
//! per-game stub builds) rewritten by the Rust port and compared with the SHA-256 of what the Python oracle
//! (`patch_nso.py`) produced for the same input. Covers the recognisers, windows, retry blocks, casp, sp frames, hops-free
//! sites and the ported stub functions that the Grizzy modules do not reach.
//!
//! The fixtures come from `tools/export_fixtures.py`; `manifest.tsv` from `tools/synth_manifest.py`, which runs the
//! SHIPPED engine's `patch_nso.py` (revision and stamp digest in the `#! shipped-engine` header) on every fixture. Every
//! case is a parity case: the Rust output must equal the shipped Python's, file for file and stdout, or, where the
//! shipped Python stops (an `error` row: `jump table N: the index reaches 0xffffffffffffffff`, the jump solver's refusal
//! of a 32-bit guard on a 64-bit index), the Rust must stop with the same message.

use ibuilder_rewrite::{Options, rewrite};
use ibuilder_testkit::sha256_hex;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Case {
    module: String,
    /// {file kind: sha256} of the shipped Python's output; or {"error": its message}
    rows: BTreeMap<String, String>,
}

struct Manifest {
    /// (source revision, sha256 of the engine's stamp.txt)
    shipped: (String, String),
    cases: BTreeMap<String, Case>,
}

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synth")
}

fn manifest() -> Manifest {
    let text = fs::read_to_string(dir().join("manifest.tsv")).unwrap();
    let mut shipped = None;
    let mut cases: BTreeMap<String, Case> = BTreeMap::new();
    for l in text.lines().filter(|l| !l.is_empty()) {
        let c: Vec<&str> = l.split('\t').collect();
        match c[0] {
            "#! shipped-engine" => shipped = Some((c[1].to_string(), c[2].to_string())),
            s if s.starts_with('#') => {}
            _ => {
                assert_eq!(
                    c.len(),
                    5,
                    "{l}: want case, module, file, sha256 or message, oracle"
                );
                assert_eq!(
                    c[4], "shipped",
                    "{l}: the only oracle is the shipped engine"
                );
                let e = cases.entry(c[0].to_string()).or_default();
                e.module = c[1].to_string();
                e.rows.insert(c[2].to_string(), c[3].to_string());
            }
        }
    }
    for (case, c) in &cases {
        assert!(
            !c.rows.contains_key("error") || c.rows.len() == 1,
            "{case}: a case the Python stops on has only its error row"
        );
    }
    Manifest {
        shipped: shipped.expect("manifest.tsv has no `#! shipped-engine` header"),
        cases,
    }
}

/// The case rewritten in a rayon pool of `threads` workers: the pool, not `Options::workers`, is what selects the 1-worker
/// paths or the parallel ones (batched decode of the per-function loop, parallel fingerprints, parallel scans)
fn rust_output_on(case: &str, module: &str, threads: usize) -> BTreeMap<String, String> {
    let mut got: BTreeMap<String, String> = BTreeMap::new();
    let file = fs::read(dir().join(format!("{case}.nso"))).unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let r = pool.install(|| {
        rewrite(
            &file,
            case,
            module,
            &Options {
                workers: threads,
                // the oracle is the pinned engine's patch_nso.py, which has no mod_layout: these synthetic modules' data shares the code's 16 KB page
                no_relayout: true,
                ..Default::default()
            },
        )
    });
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            got.insert("error".into(), e.msg);
            return got;
        }
    };
    got.insert(
        "stdout".into(),
        sha256_hex(format!("{}\n", r.log.join("\n")).as_bytes()),
    );
    let mut bin = Vec::new();
    r.write_patched(&mut bin).unwrap();
    got.insert("patched.bin".into(), sha256_hex(&bin));
    for (k, v) in [
        ("fixups.tsv", &r.fixups),
        ("sites.tsv", &r.sites),
        ("gates.tsv", &r.gates),
        ("cuts.tsv", &r.cuts),
        ("obs.tsv", &r.obs),
        ("changed.tsv", &r.changed),
    ] {
        got.insert(k.into(), sha256_hex(v.as_bytes()));
    }
    if let Some(m) = &r.match_tsv {
        got.insert("match.tsv".into(), sha256_hex(m.as_bytes()));
    }
    got
}

/// One worker and four: both paths must give the same bytes; the result is then compared with the oracle
fn rust_output(case: &str, module: &str) -> BTreeMap<String, String> {
    let one = rust_output_on(case, module, 1);
    let four = rust_output_on(case, module, 4);
    assert_eq!(
        one, four,
        "{case}: 1 worker and 4 workers give different outputs"
    );
    one
}

#[test]
fn synthetic_modules_match_the_python_oracle() {
    let m = manifest();
    assert!(m.cases.len() >= 20, "manifest has {} cases", m.cases.len());
    let (mut rewrites, mut stops) = (0, 0);
    for (case, c) in &m.cases {
        let got = rust_output(case, &c.module);
        assert_eq!(
            &got, &c.rows,
            "{case}: the Rust differs from the shipped Python oracle (engine {})",
            m.shipped.0
        );
        if c.rows.contains_key("error") {
            stops += 1;
        } else {
            rewrites += 1;
        }
    }
    eprintln!(
        "{} synthetic cases identical to the shipped Python oracle (engine {}): {rewrites} rewritten file for file, {stops} stopped with its message",
        rewrites + stops,
        m.shipped.0
    );
}

/// The manifest's hashes are those of the shipped engine: its `stamp.txt` (source revision and the digests of the
/// patches applied over it) must be the one the manifest was generated from. Skips without the engine tree.
#[test]
fn the_manifest_is_of_the_shipped_engine() {
    let m = manifest();
    let candidates = [
        ibuilder_testkit::local_env("IBUILDER_ENGINE_DM").map(|d| format!("{d}/../../stamp.txt")),
        Some(format!(
            "{}/out-stage/out-engine/stamp.txt",
            ibuilder_testkit::iosbuilder_worktree()
        )),
    ];
    let Some(stamp) = candidates
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.is_file())
    else {
        ibuilder_testkit::skip(
            "no shipped engine stamp on this machine (out-stage/out-engine/stamp.txt): the synthetic manifest's engine was not checked against it",
        );
        return;
    };
    let text = fs::read(&stamp).unwrap();
    let rev = String::from_utf8_lossy(&text)
        .lines()
        .next()
        .unwrap_or("")
        .to_string();
    assert_eq!(
        (rev.as_str(), sha256_hex(&text).as_str()),
        (m.shipped.0.as_str(), m.shipped.1.as_str()),
        "{}: the shipped engine is not the one tests/fixtures/synth/manifest.tsv was generated from: rerun tools/synth_manifest.py",
        stamp.display()
    );
}
