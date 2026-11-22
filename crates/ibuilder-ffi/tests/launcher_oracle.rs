//! The Rust `LauncherBase` against the real `dm/games/settings.py` (python 3.14) on Grizzy and the Lemmings.
//!
//! Skipped unless the three inputs are named (nothing is read from a fixed place, nothing is written):
//!   LAUNCHER_ORACLE   the JSON `settings.py <id> <main dir> <control.nacp> <out>` wrote (DM_GAMES_DIR = the derived profiles, GFXAPI=nvn, SETTINGS_PLATFORM=ios)
//!   LAUNCHER_DOCS     the Documents folder of a built game: profile-derived/<id>/game.tsv, extract/flat/main.fixups.tsv, extract/control.nacp
//!   LAUNCHER_TSV      the engine's dm/games/settings.tsv
//!   LAUNCHER_ID       the title id (default 0100DFA027504000)
//! The two documents must be the same JSON text (key order included), not only the same value.
#![cfg(feature = "real-stages")]
use ibuilder_ffi::launcher::build;
use ibuilder_profile::game::Game;
use std::path::PathBuf;

#[test]
fn the_rust_base_document_is_the_pythons_on_a_real_game() {
    let (Some(oracle), Some(docs), Some(tsv)) = (
        std::env::var_os("LAUNCHER_ORACLE"),
        std::env::var_os("LAUNCHER_DOCS"),
        std::env::var_os("LAUNCHER_TSV"),
    ) else {
        ibuilder_testkit::skip(
            "set LAUNCHER_ORACLE, LAUNCHER_DOCS and LAUNCHER_TSV (see the header of this file)",
        );
        return;
    };
    let id = std::env::var("LAUNCHER_ID").unwrap_or_else(|_| "0100DFA027504000".into());
    let docs = PathBuf::from(docs);
    let g =
        Game::load_dir(&docs.join("profile-derived").join(&id)).expect("the derived game table");
    let text = std::fs::read_to_string(tsv).expect("settings.tsv");
    let got = build(
        &g,
        &text,
        &docs.join("extract").join("flat"),
        &docs.join("extract").join("control.nacp"),
        "nvn",
        &|_| None,
    )
    .expect("build");
    let want: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(oracle).expect("oracle json")).expect("json");
    assert_eq!(
        serde_json::to_string(&got).unwrap(),
        serde_json::to_string(&want).unwrap(),
        "the documents differ (as text, key order included)"
    );
    assert_eq!(
        got["settings"].as_array().unwrap().len(),
        want["settings"].as_array().unwrap().len()
    );
    eprintln!(
        "identical: {} rows",
        want["settings"].as_array().unwrap().len()
    );
}
