//! Fixture locations (read-only references from the first part of the work, and this package's own Python oracle runs: `$IBUILDER_SCRATCH_OLD` and `$IBUILDER_SCRATCH`). A test that needs one skips with a message when it is absent.
#![allow(dead_code)]
use std::path::{Path, PathBuf};

pub const CHAIN: &str = "6c53446391cc";
pub const GAME: &str = "0100DFA027504000";

pub fn old(p: &str) -> PathBuf {
    std::env::var("IBSB_OLD")
        .map(PathBuf::from)
        .unwrap_or_else(|_| ibuilder_testkit::scratch_old())
        .join(p)
}
pub fn s2(p: &str) -> PathBuf {
    std::env::var("IBSB_S2")
        .map(PathBuf::from)
        .unwrap_or_else(|_| ibuilder_testkit::scratch())
        .join(p)
}
/// `$OLD/shader/split-out/pkg`
pub fn pkg() -> PathBuf {
    old("shader/split-out/pkg")
}
/// `$OLD/shader/profile` (holds `<game>/game.tsv` and the tables)
pub fn profile() -> PathBuf {
    old("shader/profile")
}
pub fn have(p: &Path) -> bool {
    if p.exists() {
        true
    } else {
        ibuilder_testkit::skip(&format!("fixture {} is absent", p.display()));
        false
    }
}
/// `$OLD/shader/<name>`, one key a line; `None` (with a skip) when the file is absent: a missing fixture is a skip, not a panic
pub fn keys_file(name: &str) -> Option<Vec<String>> {
    let p = old(&format!("shader/{name}"));
    if !have(&p) {
        return None;
    }
    Some(
        std::fs::read_to_string(&p)
            .unwrap()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}
