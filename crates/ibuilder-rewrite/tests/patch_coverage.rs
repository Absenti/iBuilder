//! The per-game patches of the engine against what the Rust rewriter can rewrite (FR1 of docs/CONFORMITY-AUDIT.md): an engine update must not silently leave the rewriter stale.
//!
//! * `every_code_valued_registration_of_the_tables_is_ported`: always runs, needs nothing from the engine. `tables.rs` lists every FRAME_STEP / STUBS build ID and every
//!   MATCH_FRAME_STEP / MATCH_STUBS rule of the engine's patch files it was generated from; each needs a ported stub function (`stubs_games`) verified against the patch file's
//!   current code digest. Regenerating `tables.rs` from a newer engine without redoing the port fails here, listing the game, the build or rule and the digests.
//! * `the_engine_tree_registers_nothing_the_rewriter_cannot_rewrite`: with the engine's `dm` folder (`IBUILDER_ENGINE_DM`, else the shipped engine tree if this machine has it) runs
//!   `scripts/check-patch-coverage.sh`: `tables.rs` is what the engine's patch files generate, and every registration of the engine is covered. Skips (loudly, `ibuilder_testkit::skip`)
//!   without the engine tree or without Python.

use ibuilder_rewrite::{stubs_games, tables};
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn every_code_valued_registration_of_the_tables_is_ported() {
    let rows = stubs_games::coverage_rows();
    let covered = |kind: &str, profile: &str, key: &str| {
        rows.iter()
            .any(|r| r[0] == kind && r[1] == profile && r[2] == key)
    };
    let digest_of = |profile: &str| {
        tables::CODE_DIGESTS
            .iter()
            .find(|(p, _, _)| *p == profile)
            .map(|(_, _, d)| *d)
            .unwrap_or("?")
    };
    let game_of = |profile: &str| {
        tables::CODE_DIGESTS
            .iter()
            .find(|(p, _, _)| *p == profile)
            .map(|(_, g, _)| g.to_string())
            .unwrap_or_else(|| profile.to_string())
    };
    let mut missing: Vec<String> = Vec::new();
    for (kind, list) in [("FRAME_STEP", tables::FRAME_STEP), ("STUBS", tables::STUBS)] {
        for (build, profile) in list {
            if !covered(kind, profile, build) {
                missing.push(format!(
                    "{kind} of build {build}: {} ({profile}, patch file code digest {})",
                    game_of(profile),
                    digest_of(profile)
                ));
            }
        }
    }
    for (kind, list) in [
        ("MATCH_FRAME_STEP", tables::MATCH_FRAME_STEP),
        ("MATCH_STUBS", tables::MATCH_STUBS),
    ] {
        for (profile, module, rules) in list {
            for (i, r) in rules.iter().enumerate() {
                if !covered(kind, profile, &format!("{module}#{i}:{}", r.name)) {
                    missing.push(format!(
                        "{kind} rule {:?} of {module}: {} ({profile}, patch file code digest {})",
                        r.name,
                        game_of(profile),
                        digest_of(profile)
                    ));
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the rewriter cannot rewrite these (not ported, or the port was verified against older code than the tables' patch file: redo it with tools/stubtest.py, then put the new digest in its GamePort):\n  {}",
        missing.join("\n  ")
    );
    // and the other way: a port whose game the tables do not know is dead code
    for g in stubs_games::PORTS {
        assert!(
            tables::CODE_DIGESTS.iter().any(|(p, _, _)| *p == g.profile),
            "{} has a port but the tables list no code-valued patch for it",
            g.profile
        );
    }
}

fn engine_dm() -> Option<PathBuf> {
    let candidates = [
        ibuilder_testkit::local_env("IBUILDER_ENGINE_DM"),
        Some(format!(
            "{}/out-stage/out-engine/engine/dm",
            ibuilder_testkit::iosbuilder_worktree()
        )),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.join("tools/patch_nso.py").is_file() && p.join("games").is_dir())
}

#[test]
fn the_engine_tree_registers_nothing_the_rewriter_cannot_rewrite() {
    let Some(dm) = engine_dm() else {
        ibuilder_testkit::skip(
            "no engine tree on this machine (set IBUILDER_ENGINE_DM=<engine>/dm): the engine's patch files were not compared with the rewriter's tables and ports",
        );
        return;
    };
    let python = std::env::var("IBUILDER_PYTHON").unwrap_or_else(|_| "python3".to_string());
    if Command::new(&python).arg("--version").output().is_err() {
        ibuilder_testkit::skip(&format!(
            "{python} is not available: the engine's patch files cannot be read"
        ));
        return;
    }
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-patch-coverage.sh");
    let out = Command::new(&script)
        .arg(&dm)
        .env("IBR_BIN", env!("CARGO_BIN_EXE_ibuilder-rewrite"))
        .env("IBUILDER_PYTHON", &python)
        .output()
        .unwrap_or_else(|e| panic!("{}: {e}", script.display()));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "scripts/check-patch-coverage.sh failed against {}:\n{text}{}",
        dm.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("{}", text.trim());
}
