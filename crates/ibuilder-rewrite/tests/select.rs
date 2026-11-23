//! The stack-free rewrite of a conditional select with an x18 source (engine patch 0409, upstream nxnative 0b5fd602; `src/select.rs`) against the real Python: synthetic modules
//! (`tools/selecttest.py`: random CSEL / CSINC / CSINV / CSNEG with x18 as Rn, Rm, both, neither or Rd, wzr / xzr, every condition, followed by the instructions the liveness proof
//! reads: writes and reads of x9..x17, branches both ways, calls, `ret`, a search longer than its budget) rewritten by `ibuilder-rewrite` and compared with the SHA-256 of every file
//! the engine's `patch_nso.py` with 0409 wrote for them and of what it printed. Each module here took the new path at least once; the corpus the tool ran (300 modules, 810 new-path
//! sites, 709 selects that kept the old path for want of a proof, 3,000 other x18 conditional selects) is in docs/crates/REWRITE-RUST.md. No game code is in the fixtures.

use ibuilder_rewrite::{Options, rewrite};
use ibuilder_testkit::sha256_hex;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[test]
fn x18_conditional_selects_match_the_python_oracle() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/select");
    let manifest = fs::read_to_string(dir.join("manifest.tsv")).unwrap();
    let mut cases: BTreeMap<String, (String, BTreeMap<String, String>)> = BTreeMap::new();
    for l in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let c: Vec<&str> = l.split('\t').collect();
        let e = cases
            .entry(c[0].to_string())
            .or_insert((c[1].to_string(), BTreeMap::new()));
        e.1.insert(c[2].to_string(), c[3].to_string());
    }
    assert!(cases.len() >= 24, "manifest has {} cases", cases.len());
    let mut selects = 0;
    for (case, (module, want)) in &cases {
        let file = fs::read(dir.join(format!("{case}.nso"))).unwrap();
        let r = rewrite(
            &file,
            case,
            module,
            &Options {
                workers: 2,
                // the oracle is the pinned engine's patch_nso.py, which has no mod_layout: these synthetic modules' data shares the code's 16 KB page
                no_relayout: true,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{case}: {e}"));
        let mut got: BTreeMap<String, String> = BTreeMap::new();
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
        assert_eq!(&got, want, "{case}: output differs from the Python oracle");
        selects += r
            .log
            .iter()
            .filter_map(|l| l.split("'select': ").nth(1))
            .filter_map(|t| t.split([',', '}']).next()?.trim().parse::<u32>().ok())
            .sum::<u32>();
    }
    assert!(
        selects >= cases.len() as u32,
        "only {selects} stack-free selects in {} modules: the fixtures do not exercise the new path",
        cases.len()
    );
    eprintln!(
        "{} select modules ({selects} stack-free selects) identical to the Python oracle",
        cases.len()
    );
}
