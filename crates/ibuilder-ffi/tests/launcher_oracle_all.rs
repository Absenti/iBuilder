//! The Rust `LauncherBase` against the real `dm/games/settings.py` (python 3.14) on EVERY registered game of the engine, in three variants each
//! (nvn without rewriter fixups, nvn with `DYNAMIC_RESOLUTION` and a `RENDER_SCALE` of 2, vulkan). Written by the conformity audit (docs/CONFORMITY-AUDIT.md, rows L-*): `launcher_oracle.rs`
//! only compared Grizzy, a derived game, so the conditions that read `DM_GAME_*` macros of a registered game's table had one data point.
//!
//! Skipped unless these are named (nothing is read from a fixed place, nothing is written):
//!   LAUNCHER_ORACLE_ALL  a folder of `<id>.<variant>.json` (variants `nvn`, `fx`, `vk`), each the output of `settings.py <id> <flat dir> <missing control.nacp> <out>` with GFXAPI / SETTINGS_PLATFORM=ios
//!   LAUNCHER_GAMES       the engine's `dm/games` folder (`<id>/game.tsv`, `settings.tsv`)
//!   LAUNCHER_FIXUPS      a folder holding `plain/` (an empty rewritten-modules folder) and `fx/main.fixups.tsv` (`# DYNAMIC_RESOLUTION<TAB>1` and `# RENDER_SCALE<TAB>0x2`), the two the Python was given
//! The documents must be the same JSON text, key order included. A game the Python refused (no document) is not compared; the test counts them.
#![cfg(feature = "real-stages")]
use ibuilder_ffi::launcher::build;
use ibuilder_profile::game::Game;
use std::path::PathBuf;

#[test]
fn the_rust_base_document_is_the_pythons_on_every_registered_game() {
    let (Some(oracle), Some(games), Some(fixups)) = (
        std::env::var_os("LAUNCHER_ORACLE_ALL"),
        std::env::var_os("LAUNCHER_GAMES"),
        std::env::var_os("LAUNCHER_FIXUPS"),
    ) else {
        ibuilder_testkit::skip(
            "set LAUNCHER_ORACLE_ALL, LAUNCHER_GAMES and LAUNCHER_FIXUPS (see the header of this file)",
        );
        return;
    };
    let (oracle, games, fixups) = (
        PathBuf::from(oracle),
        PathBuf::from(games),
        PathBuf::from(fixups),
    );
    let tsv = std::fs::read_to_string(games.join("settings.tsv")).expect("settings.tsv");
    let nacp = fixups.join("no-such-control.nacp");
    let (mut compared, mut refused) = (0usize, 0usize);
    let mut differ = Vec::new();
    let mut ids: Vec<String> = std::fs::read_dir(&games)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("game.tsv").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    for id in &ids {
        let g = match Game::load_dir(&games.join(id)) {
            Ok(g) => g,
            Err(e) => {
                differ.push(format!("{id}: Rust could not load the table: {e}"));
                continue;
            }
        };
        for (variant, flat, gfx) in [
            ("nvn", fixups.join("plain"), "nvn"),
            ("fx", fixups.join("fx"), "nvn"),
            ("vk", fixups.join("plain"), "vulkan"),
        ] {
            let path = oracle.join(format!("{id}.{variant}.json"));
            let got = build(&g, &tsv, &flat, &nacp, gfx, &|_| None);
            let Ok(text) = std::fs::read_to_string(&path) else {
                refused += 1;
                if got.is_ok() {
                    differ.push(format!(
                        "{id} {variant}: the Python wrote no document, Rust wrote one"
                    ));
                }
                continue;
            };
            let want: serde_json::Value = serde_json::from_str(&text).expect("oracle json");
            match got {
                Ok(got) => {
                    if serde_json::to_string(&got).unwrap() != serde_json::to_string(&want).unwrap()
                    {
                        differ.push(format!("{id} {variant}: the documents differ"));
                    }
                    compared += 1;
                }
                Err(e) => differ.push(format!(
                    "{id} {variant}: Rust refused ({e}) where the Python wrote a document"
                )),
            }
        }
    }
    eprintln!(
        "compared {compared} documents of {} games ({refused} refused by the Python and by Rust alike)",
        ids.len()
    );
    assert!(
        compared >= ids.len(),
        "too few documents compared ({compared} for {} games): is the oracle folder complete?",
        ids.len()
    );
    assert!(
        differ.is_empty(),
        "{} difference(s): {:#?}",
        differ.len(),
        &differ[..differ.len().min(20)]
    );
}
