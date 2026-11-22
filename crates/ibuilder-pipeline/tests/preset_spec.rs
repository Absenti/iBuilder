//! The settings sheet's "Profile" row (`DM_PRESET`): Standard, or Memory saver (the Apple TV port of Ocarina of Time's memory set: DM_LOW_MEMORY, handheld, dynamic resolution, 30 fps,
//! render scale 0.5, no pipeline prebuild). Ocarina of Time starts at Memory saver; every other game at Standard. A row the player set, env.txt or the environment wins over the profile.
//! Hand-made fixtures only: every machine runs these.

mod common;

use common::*;
use ibuilder_pipeline::boot::prepare_boot;
use ibuilder_pipeline::env::Env;
use ibuilder_pipeline::settings::*;
use serde_json::{Value, json};

const OOT: &str = "0400486024DEC000";
const OTHER: &str = "0100DFA027504000";

fn game(tid: &str) -> Fx {
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{tid}\n"));
    fx
}

fn settings(fx: &Fx, tid: &str, text: &str) {
    write(fx.p(&format!("games/{tid}/settings.tsv")), text);
}

/// The rows the catalogue gives a game like Ocarina of Time (the ones the profile covers, with their catalogue defaults, plus one it does not).
fn catalogue_rows() -> Vec<Value> {
    vec![
        json!({"env": "DM_FPS_TARGET", "kind": "choice", "section": "Game", "label": "Frame rate", "default": "60", "options": [["30", "30 fps"], ["60", "60 fps"]]}),
        json!({"env": "DM_DYNAMIC_RESOLUTION", "kind": "toggle", "section": "Game", "label": "Dynamic resolution", "default": "0", "on": "1", "off": "0"}),
        json!({"env": "DM_RENDER_SCALE", "kind": "range", "section": "Game", "label": "Render scale", "default": "1.0", "min": 0.5, "max": 1.0, "step": 0.05}),
        json!({"env": "DM_OPMODE", "kind": "choice", "section": "Game", "label": "Console mode", "default": "1", "options": [["1", "Docked"], ["0", "Handheld"]]}),
        json!({"env": "DM_LOW_MEMORY", "kind": "toggle", "section": "Performance", "label": "Low memory", "default": "0", "on": "1", "off": "0"}),
        json!({"env": "DM_SHOW_FPS", "kind": "toggle", "section": "Game", "label": "Frame-rate counter", "default": "1", "on": "1", "off": "0"}),
    ]
}

fn default_of<'a>(rows: &'a [Value], env: &str) -> &'a str {
    rows.iter()
        .find(|r| r["env"] == env)
        .and_then(|r| r["default"].as_str())
        .unwrap()
}

#[test]
fn ocarina_starts_at_memory_saver_and_other_games_at_standard() {
    assert_eq!(preset_default(OOT), PRESET_MEMORY);
    assert_eq!(preset_default(&OOT.to_lowercase()), PRESET_MEMORY);
    assert_eq!(preset_default(OTHER), PRESET_STANDARD);
    assert!(preset_values(PRESET_STANDARD).is_empty());
    assert!(preset_values("nonsense").is_empty());
    assert_eq!(
        preset_values(PRESET_MEMORY),
        &[
            ("DM_OPMODE", "0"),
            ("DM_DYNAMIC_RESOLUTION", "1"),
            ("DM_FPS_TARGET", "30"),
            ("DM_RENDER_SCALE", "0.5"),
            ("DM_PSO_PREBUILD", "0"),
            ("DM_PSO_PREPARE_TAIL", "0"),
        ]
    );
}

#[test]
fn the_profile_row_comes_first_and_carries_both_sets_of_values() {
    let mut rows = catalogue_rows();
    add_preset_row(&mut rows, OOT);
    let p = &rows[0];
    assert_eq!(p["env"], PRESET_ENV);
    assert_eq!(p["kind"], "choice");
    assert_eq!(p["default"], PRESET_MEMORY);
    assert_eq!(
        p["options"],
        json!([["standard", "Standard"], ["memory", "Memory saver"]])
    );
    // standard: the catalogue's own defaults; memory: the profile's values; only the rows the sheet has (no DM_PSO_PREBUILD row, no DM_SHOW_FPS)
    assert_eq!(
        p["presets"]["standard"],
        json!({"DM_DYNAMIC_RESOLUTION": "0", "DM_FPS_TARGET": "60", "DM_OPMODE": "1", "DM_RENDER_SCALE": "1.0"})
    );
    assert_eq!(
        p["presets"]["memory"],
        json!({"DM_DYNAMIC_RESOLUTION": "1", "DM_FPS_TARGET": "30", "DM_OPMODE": "0", "DM_RENDER_SCALE": "0.5"})
    );
    // Ocarina's sheet shows what the game will run with
    for (env, v) in [
        ("DM_FPS_TARGET", "30"),
        ("DM_DYNAMIC_RESOLUTION", "1"),
        ("DM_RENDER_SCALE", "0.5"),
        ("DM_OPMODE", "0"),
    ] {
        assert_eq!(default_of(&rows, env), v, "{env}");
    }
    assert_eq!(
        default_of(&rows, "DM_SHOW_FPS"),
        "1",
        "a row the profile does not cover is untouched"
    );
    assert_eq!(
        default_of(&rows, "DM_LOW_MEMORY"),
        "0",
        "Low memory is no longer part of Memory saver (device log 2026-11-20 evening)"
    );
    // a second pass (a launcher.json written again) keeps one row
    add_preset_row(&mut rows, OOT);
    assert_eq!(rows.iter().filter(|r| r["env"] == PRESET_ENV).count(), 1);
}

#[test]
fn another_game_keeps_its_defaults_and_offers_the_profile() {
    let mut rows = catalogue_rows();
    add_preset_row(&mut rows, OTHER);
    assert_eq!(rows[0]["env"], PRESET_ENV);
    assert_eq!(rows[0]["default"], PRESET_STANDARD);
    for (env, v) in [
        ("DM_FPS_TARGET", "60"),
        ("DM_DYNAMIC_RESOLUTION", "0"),
        ("DM_RENDER_SCALE", "1.0"),
        ("DM_OPMODE", "1"),
        ("DM_LOW_MEMORY", "0"),
    ] {
        assert_eq!(default_of(&rows, env), v, "{env}");
    }
    // a catalogue with none of the covered rows gets no Profile row
    let mut none = vec![
        json!({"env": "DM_SHOW_FPS", "kind": "toggle", "default": "1", "on": "1", "off": "0"}),
    ];
    add_preset_row(&mut none, OOT);
    assert_eq!(none.len(), 1);
}

#[test]
fn ocarina_with_no_choice_gets_the_whole_memory_set() {
    let fx = game(OOT);
    let set = apply_preset(&fx.ctx);
    assert_eq!(set.len(), 6, "{set:?}");
    for (env, v) in preset_values(PRESET_MEMORY) {
        assert_eq!(fx.env.get(env).as_deref(), Some(*v), "{env}");
    }
    assert!(
        fx.log
            .any(|l| l.starts_with("game profile: memory (this game's default): ")),
        "{:?}",
        fx.log.lines()
    );
}

#[test]
fn a_row_the_player_set_env_txt_and_the_environment_win() {
    let fx = game(OOT);
    settings(&fx, OOT, "DM_FPS_TARGET\t60\nDM_OPMODE\t1\n");
    fx.env.set("DM_RENDER_SCALE", "0.75"); // env.txt or a harness put it there first
    let set = apply_preset(&fx.ctx);
    assert_eq!(set.get("DM_FPS_TARGET"), None);
    assert_eq!(set.get("DM_OPMODE"), None);
    assert_eq!(set.get("DM_RENDER_SCALE"), None);
    assert_eq!(fx.env.get("DM_RENDER_SCALE").as_deref(), Some("0.75"));
    assert_eq!(fx.env.get("DM_LOW_MEMORY"), None, "not part of the profile");
    assert_eq!(fx.env.get("DM_PSO_PREBUILD").as_deref(), Some("0"));
    assert_eq!(fx.env.get("DM_PSO_PREPARE_TAIL").as_deref(), Some("0"));
    assert!(
        fx.log.any(
            |l| l.contains("kept as chosen elsewhere: DM_OPMODE DM_FPS_TARGET DM_RENDER_SCALE")
        ),
        "{:?}",
        fx.log.lines()
    );
}

#[test]
fn standard_chosen_for_ocarina_sets_nothing() {
    let fx = game(OOT);
    settings(&fx, OOT, "DM_PRESET\tstandard\nDM_FPS_TARGET\t60\n");
    assert!(apply_preset(&fx.ctx).is_empty());
    assert_eq!(fx.env.get("DM_LOW_MEMORY"), None);
    assert_eq!(fx.env.get("DM_PSO_PREBUILD"), None);
}

#[test]
fn another_game_sets_nothing_unless_memory_saver_is_chosen() {
    let fx = game(OTHER);
    assert!(apply_preset(&fx.ctx).is_empty());
    assert_eq!(fx.env.get("DM_LOW_MEMORY"), None);
    let fx = game(OTHER);
    settings(&fx, OTHER, "DM_PRESET\tmemory\n");
    assert_eq!(apply_preset(&fx.ctx).len(), 6);
    assert!(
        fx.log
            .any(|l| l.starts_with("game profile: memory (chosen in the game's settings)")),
        "{:?}",
        fx.log.lines()
    );
    let fx = game(OTHER);
    settings(&fx, OTHER, "DM_PRESET\tturbo\n");
    assert!(apply_preset(&fx.ctx).is_empty());
    assert!(
        fx.log
            .any(|l| l.contains("'turbo'") && l.contains("nothing set")),
        "{:?}",
        fx.log.lines()
    );
}

#[test]
fn the_boot_applies_the_profile_after_the_settings_and_env_txt() {
    let fx = game(OOT);
    settings(&fx, OOT, "DM_FPS_TARGET\t60\n");
    // the settings' rows reach the engine only when the game's launcher.json offers them
    let mut rows = catalogue_rows();
    add_preset_row(&mut rows, OOT);
    write(
        fx.p(&format!("games/{OOT}/launcher.json")),
        json!({"settings": rows}).to_string(),
    );
    write(fx.p(&format!("games/{OOT}/env.txt")), "DM_LOW_MEMORY=0\n");
    prepare_boot(&fx.ctx);
    assert_eq!(
        fx.env.get("DM_FPS_TARGET").as_deref(),
        Some("60"),
        "the settings' row"
    );
    assert_eq!(fx.env.get("DM_LOW_MEMORY").as_deref(), Some("0"), "env.txt");
    assert_eq!(fx.env.get("DM_OPMODE").as_deref(), Some("0"), "the profile");
    assert_eq!(
        fx.env.get("DM_PSO_PREBUILD").as_deref(),
        Some("0"),
        "the profile, a switch with no row"
    );
}
