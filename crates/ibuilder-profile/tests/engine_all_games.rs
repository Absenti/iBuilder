//! `render_profile` (the engine's `game.py profile <id>`) against the real `dm/games/game.py` on EVERY registered game table of the engine the app is built from. Written by the conformity
//! audit (docs/CONFORMITY-AUDIT.md, row P-RENDER): the first port was checked on 179 tables, and the patched engine's tables have since gained values the port did not know
//! (`screen_aspect wide|both`: Ocarina of Time, 0400486024DEC000, stopped the app's profile stage with "'wide' is not in list").
//!
//! Skipped unless both are named (nothing is read from a fixed place, nothing is written):
//!   PROFILE_ORACLE_ALL  a folder of `<id>.tsv`, each the output of `python3 -B -I game.py profile <id> <out>` run in the engine's `dm/games`
//!   PROFILE_GAMES       the engine's `dm/games` folder (`<id>/game.tsv`)
//! The profile must be the same text from its second line on (the first names the table's path, which is spelled differently).
use ibuilder_profile::game::{Game, render_profile};
use std::path::PathBuf;

#[test]
fn the_profile_is_the_engines_on_every_registered_game() {
    let (Some(oracle), Some(games)) = (
        std::env::var_os("PROFILE_ORACLE_ALL"),
        std::env::var_os("PROFILE_GAMES"),
    ) else {
        ibuilder_testkit::skip(
            "set PROFILE_ORACLE_ALL and PROFILE_GAMES (see the header of this file)",
        );
        return;
    };
    let (oracle, games) = (PathBuf::from(oracle), PathBuf::from(games));
    let mut ids: Vec<String> = std::fs::read_dir(&games)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("game.tsv").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    let mut differ = Vec::new();
    let mut compared = 0;
    for id in &ids {
        let want = match std::fs::read_to_string(oracle.join(format!("{id}.tsv"))) {
            Ok(w) => w,
            Err(_) => {
                differ.push(format!("{id}: no oracle file"));
                continue;
            }
        };
        let got = match Game::load_dir(&games.join(id)).and_then(|g| render_profile(&g)) {
            Ok(r) => r.text,
            Err(e) => {
                differ.push(format!(
                    "{id}: Rust refused ({e}) where the engine wrote a profile"
                ));
                continue;
            }
        };
        let tail = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
        if tail(&got) != tail(&want) {
            let first = tail(&got)
                .lines()
                .zip(tail(&want).lines())
                .enumerate()
                .find(|(_, (a, b))| a != b)
                .map(|(i, (a, b))| format!("line {}: {a:?} against {b:?}", i + 2));
            differ.push(format!(
                "{id}: the profiles differ ({})",
                first.unwrap_or_else(|| "length".into())
            ));
        }
        compared += 1;
    }
    eprintln!("compared {compared} profiles of {} games", ids.len());
    assert!(
        differ.is_empty(),
        "{} difference(s): {:#?}",
        differ.len(),
        &differ[..differ.len().min(20)]
    );
}
