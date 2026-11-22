//! A game's language when nobody chose one (`settings::apply_default_language`, run by `prepare_boot` after the settings and env.txt): the one its settings sheet SHOWS (the launcher row's default),
//! so the sheet and the game agree, in iBuilder and in an exported game (its launcher.json travels with it). A saved choice, an env.txt line or a harness's variable wins.

mod common;

use common::*;
use ibuilder_pipeline::env::Env;
use ibuilder_pipeline::settings::{
    apply_default_language, apply_env_files, apply_game_settings, launcher_default_language,
};

const TID: &str = "010095300B6A4000";

fn launcher(default: &str) -> String {
    format!(
        "{{\"game\": \"{TID}\", \"settings\": [{{\"env\": \"DM_FPS_TARGET\", \"kind\": \"choice\", \"default\": \"30\", \"options\": [[\"30\", \"30\"], [\"60\", \"60\"]]}}, \
         {{\"env\": \"DM_LANG\", \"kind\": \"choice\", \"section\": \"Game\", \"label\": \"Game language\", \"default\": \"{default}\", \"options\": [[\"en-US\", \"American English\"], [\"ru\", \"Russian\"], [\"en-GB\", \"British English\"]]}}]}}"
    )
}

#[test]
fn no_choice_takes_the_language_the_sheet_shows() {
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    write(
        fx.p(&format!("games/{TID}/launcher.json")),
        launcher("en-US"),
    );
    assert_eq!(
        launcher_default_language(&fx.ctx, TID).as_deref(),
        Some("en-US")
    );
    assert!(
        apply_game_settings(&fx.ctx).is_empty(),
        "no settings.tsv: nothing chosen"
    );
    assert_eq!(apply_default_language(&fx.ctx).as_deref(), Some("en-US"));
    assert_eq!(fx.env.get("DM_LANG").as_deref(), Some("en-US"));
    assert!(
        fx.log
            .contains("game settings: DM_LANG=en-US (no language chosen"),
        "{:?}",
        fx.log.lines()
    );
    // a sheet that shows another language (the game does not list American English): the game gets THAT one, not the host's en-US
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    write(
        fx.p(&format!("games/{TID}/launcher.json")),
        launcher("en-GB"),
    );
    assert_eq!(apply_default_language(&fx.ctx).as_deref(), Some("en-GB"));
}

#[test]
fn a_saved_choice_an_env_txt_line_or_the_environment_wins() {
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    write(
        fx.p(&format!("games/{TID}/launcher.json")),
        launcher("en-US"),
    );
    write(fx.p(&format!("games/{TID}/settings.tsv")), "DM_LANG\tru\n");
    apply_game_settings(&fx.ctx);
    assert_eq!(apply_default_language(&fx.ctx), None);
    assert_eq!(fx.env.get("DM_LANG").as_deref(), Some("ru"));
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    write(
        fx.p(&format!("games/{TID}/launcher.json")),
        launcher("en-US"),
    );
    write(fx.p(&format!("games/{TID}/env.txt")), "DM_LANG=ja\n");
    apply_game_settings(&fx.ctx);
    apply_env_files(&fx.ctx);
    assert_eq!(
        apply_default_language(&fx.ctx),
        None,
        "env.txt ran first and named it"
    );
    assert_eq!(fx.env.get("DM_LANG").as_deref(), Some("ja"));
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    write(
        fx.p(&format!("games/{TID}/launcher.json")),
        launcher("en-US"),
    );
    fx.env.set("DM_LANG", "fr");
    assert_eq!(apply_default_language(&fx.ctx), None);
    assert_eq!(fx.env.get("DM_LANG").as_deref(), Some("fr"));
}

#[test]
fn no_launcher_json_or_no_language_row_sets_nothing() {
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{TID}\n"));
    assert_eq!(apply_default_language(&fx.ctx), None);
    write(
        fx.p(&format!("games/{TID}/launcher.json")),
        "{\"settings\": [{\"env\": \"DM_FPS_TARGET\", \"default\": \"30\"}]}",
    );
    assert_eq!(apply_default_language(&fx.ctx), None);
    write(fx.p(&format!("games/{TID}/launcher.json")), "not json");
    assert_eq!(apply_default_language(&fx.ctx), None);
    assert_eq!(fx.env.get("DM_LANG"), None);
    let fx = Fx::new();
    assert_eq!(apply_default_language(&fx.ctx), None, "no title id");
}
