//! A fixture-dependent test says so through `ibuilder_testkit::skip` (counted, listed, a failure under `IBUILDER_STRICT_FIXTURES=1`), never with a bare `eprintln!("skip: ...")` and a `return`:
//! libtest counts that as PASSED and hides the line (review r2, N8: four such sites survived the triage). This reads the sources of every crate and fails on one.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if p.is_dir() {
            if name != "target" && !name.starts_with('.') {
                rust_files(&p, out);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_test_skips_with_a_bare_print() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(
        files.len() > 100,
        "found only {} sources under {}: the walk is broken",
        files.len(),
        crates.display()
    );
    let mut bad = Vec::new();
    for f in &files {
        if f.ends_with("no_raw_skips.rs") || f.ends_with("ibuilder-testkit/src/lib.rs") {
            continue; // this file names the pattern; the other one IS `skip`, which prints its line
        }
        let text = fs::read_to_string(f).unwrap_or_default();
        for (i, l) in text.lines().enumerate() {
            let t = l.trim_start();
            let lower = t.to_ascii_lowercase();
            if (lower.starts_with("eprintln!(\"skip") || lower.starts_with("println!(\"skip"))
                && !t.starts_with("//")
            {
                bad.push(format!(
                    "{}:{}: {}",
                    f.strip_prefix(&crates).unwrap_or(f).display(),
                    i + 1,
                    t
                ));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "a test that skips must call ibuilder_testkit::skip(reason), not print:\n{}",
        bad.join("\n")
    );
}
