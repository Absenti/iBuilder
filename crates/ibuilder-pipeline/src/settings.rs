//! The game's settings and the engine's environment switches (`DM_*`): what the pipeline file did around the engine's `settings.py` and before the
//! boot (`GAME_SETTINGS`, `LAUNCHER_NOTES`, `GAMEAPP_ROWS`, `write_launcher_settings`, `apply_game_settings`, `apply_env_files`, `launcher_*`, `game_gfxapi`).
//!
//! `Documents/games/<id>/launcher.json` is what the app's settings sheet draws; `Documents/games/<id>/settings.tsv` holds the player's choices (the
//! screen writes `mode`, `fps`, `picture`, `show_fps`, and a row per launcher switch by its name); `Documents/env.txt` and `games/<id>/env.txt` are
//! free-form switches for experiments without a new .ipa.

use crate::ctx::Ctx;
use crate::tsv::game_table_dir;
use crate::util::{is_file, read_text, write_atomic};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

/// `GAME_SETTINGS`: `settings.tsv` row -> the engine's player switch.
pub fn game_setting(key: &str, value: &str) -> Option<(&'static str, String)> {
    let hit = |env: &'static str, v: &str| Some((env, v.to_string()));
    match (key, value) {
        ("mode", "handheld") => hit("DM_OPMODE", "0"),
        ("mode", "docked") => hit("DM_OPMODE", "1"),
        ("fps", "30") => hit("DM_FPS_TARGET", "30"),
        ("fps", "60") => hit("DM_FPS_TARGET", "60"),
        ("picture", "fill") => hit("DM_PICTURE_FIT", "fill"),
        ("picture", "fit") => hit("DM_PICTURE_FIT", "fit"),
        ("show_fps", "1") => hit("DM_SHOW_FPS", "1"),
        ("show_fps", "0") => hit("DM_SHOW_FPS", "0"),
        // shaders missing from the library, translated while the game runs (paches/0008): the settings sheet and the in-game menu both write DM_LIVE_SHADERS
        ("DM_LIVE_SHADERS", m @ ("pause" | "skip" | "off")) => hit("DM_LIVE_SHADERS", m),
        _ => None,
    }
}

pub const LAUNCHER_DROP_UNLESS_PULLING: [&str; 2] = ["DM_PSO_MTL4", "DM_PSO_PREBUILD"];

/// A line under each row of the settings sheet that the engine's catalogue leaves without one (GameApp's words, from docs/env_switches.tsv's meaning of each switch).
// table rows are kept one per line as written (rustfmt would put every field of every row on its own line)
#[rustfmt::skip]
pub const LAUNCHER_NOTES: [(&str, &str); 14] = [
    ("DM_FPS_TARGET", "The frame-rate cap. Each frame stays on screen a whole number of refreshes, so the rate is steady (30, 60 or 120) instead of uneven."),
    ("DM_PRESENT_INTERVAL", "30 fps follows the rate the game sets for itself; 60 fps shows a frame at every refresh."),
    ("DM_RENDER_SCALE", "The size of the game's 3D image. Lower is faster and keeps the phone cooler; the picture is scaled up to the screen."),
    ("DM_UPSCALER", "The game's own upscaling shaders, or Apple's MetalFX in their place."),
    ("DM_LANG", "The language the game starts in."),
    ("DM_SHOW_FPS", "Shows the frames per second at the top of the screen. The gear's menu in the game turns it on and off too."),
    ("DM_TOUCH_HAPTICS", "A light tap you feel when you press an on-screen button."),
    ("DM_RUMBLE", "The game's vibrations, on a connected controller or on the phone."),
    ("DM_RUMBLE_GAIN", "How strong the vibrations are: 1 is the game's own strength, 0 is silent, 3 is three times stronger."),
    ("DM_RUMBLE_MIN", "Vibrations fainter than this level are dropped, and the rest keep their full range. Raise it if a controller keeps humming in a game that sends constant faint rumble (Mario Kart 8 Deluxe: 0.45). The phone is not affected."),
    ("DM_PAD_PRESS_TO_JOIN", "With several controllers: a controller becomes a player only when you press one of its buttons, and takes the first free player number. Off, each controller gets a player number when it connects."),
    ("DM_NVN_ENC_THREAD", "Prepares the GPU's work on a second thread while the game goes on. Faster in most games; turn it off only if a game shows glitches."),
    ("DM_FPS_HOLD", "When frames come late, each one stays on screen one more refresh: a steady 30 instead of an uneven 40 to 55 that stutters. Off, every frame aims for the cap: more frames, less even."),
    ("DM_METAL_HUD", "Apple's Metal overlay over the game: frame rate, frame and GPU time, memory. For checking performance."),
];

/// Notes that replace the catalogue's own (`DM_PRESENT_SNAP`'s and `DM_OPMODE`'s say more than the catalogue's).
// table rows are kept one per line as written (rustfmt would put every field of every row on its own line)
#[rustfmt::skip]
pub const LAUNCHER_NOTES_REPLACE: [(&str, &str); 3] = [
    ("DM_FRAMEGEN", "Makes an extra frame between each two of the game's frames: a game at 30 fps looks like 60, one at 60 like 120 on a 120 Hz screen (not in Low Power Mode). Costs GPU time and about half a frame more delay; the HUD may shimmer; paused in menus."),
    ("DM_PRESENT_SNAP", "Copies each finished frame aside so the game can start the next one at once. Can smooth heavy scenes, at the cost of up to one frame more display latency."),
    ("DM_OPMODE", "Docked renders as on a TV (often up to 1080p); handheld as on the Switch's own screen (up to 720p): about half the pixels or fewer, so faster and cooler. This is the way to lower the resolution of any game. Read when the game starts."),
];

/// `GAMEAPP_ROWS`: rows GameApp adds to the engine's catalogue. Low latency: DM_PRESENT_DRAIN drains the frames standing in the display link's queue, and
/// DM_PRESENT_LATCH (which needs the drain) shows a present on the tick that woke it; both off on iOS where nxnative has not measured them, on on the Mac
/// (present-to-screen 48.3 -> 31.8 ms on Ocarina of Time, upstream PR #2757). One toggle sets both (`LINKED_SWITCHES`).
pub fn gameapp_rows() -> Vec<Value> {
    vec![
        json!({"env": "DM_PRESENT_DRAIN", "kind": "toggle", "section": "Performance", "label": "Low latency", "default": "0", "on": "1", "off": "0",
               "note": "Shows each frame sooner (about 15 ms less delay between your input and the screen at 60 fps). Not measured on iPhone by the engine: turn it off if frames stutter."}),
        // paches/0008: a shader the game's library lacks is translated while the game runs, and kept for the next starts
        json!({"env": "DM_LIVE_SHADERS", "kind": "choice", "section": "Graphics", "label": "Missing graphics", "default": "pause", "level": "basic",
               "options": [["pause", "Pause the game a moment"], ["skip", "Keep playing"], ["off", "Off"]],
               "note": "When the game draws something its shader library lacks, the app translates it while you play and keeps it for the next starts. Pause: the picture stops a moment and the object is drawn from its first frame. Keep playing: it appears a moment later. Off: it is not drawn. Also in the in-game menu."}),
        // engine patch 0414 (it replaces paches/0009): pipelines kept in a bounded per-game binary archive written at backgrounding, SIGTERM and exit (the host reads DM_PSO_BINARY_ARCHIVE: 0 off)
        json!({"env": "DM_PSO_BINARY_ARCHIVE", "kind": "toggle", "section": "Performance", "label": "Pipeline cache", "default": "1", "on": "1", "off": "0",
               "note": "Keeps the graphics pipelines the game builds (up to 256 MB on the phone) so the next starts do not build them again. Built in the background; cleared when the app, the OS or the shaders change. Turn it off if the phone runs short of storage."}),
        // DM_METALFX is expanded into the engine's own switches (METALFX_LEVELS, apply_game_settings)
        json!({"env": "DM_METALFX", "kind": "choice", "section": "Graphics", "label": "MetalFX upscaling", "default": "off", "level": "basic",
               "options": [["off", "Off"], ["quality", "Quality"], ["performance", "Performance"]],
               "note": "Apple's MetalFX scales the picture up to the screen instead of a plain stretch. Quality renders the game's 3D image at 0.75 of its size, Performance at 0.5, in the games that allow it (Render scale); the rest keep their size and only the final scaling changes. Uses some GPU time; cooler than a larger picture."}),
        // gameapp/GameAppExternal.cs reads this one (the environment, else the game's settings.tsv): no engine switch behind it
        json!({"env": "DM_EXTERNAL_DISPLAY", "kind": "choice", "section": "Graphics", "label": "External display", "default": "tv", "level": "basic",
               "options": [["tv", "Game on the TV, iPhone as controller"], ["mirror", "Mirror the iPhone"]],
               "note": "With a TV or monitor connected (USB-C or AirPlay), the game fills it at its own resolution and the iPhone becomes the controller: bigger buttons, the game menu, and \"Playing on the TV\" when a controller is connected. Mirror shows the iPhone on the display as it is. Taking the display away brings the game back to the phone without stopping it."}),
    ]
}

/// An (engine switch, value) pair.
pub type EnvPair = (&'static str, &'static str);

/// `LINKED_SWITCHES`: one player toggle that needs a second switch.
pub const LINKED_SWITCHES: [(EnvPair, &[EnvPair]); 1] =
    [(("DM_PRESENT_DRAIN", "1"), &[("DM_PRESENT_LATCH", "1")])];

/// `METALFX_LEVELS`: DM_METALFX -> the engine's switches: `DM_PRESENT_UPSCALE=metalfx` (the present's spatial scaler, nvn_mtl.m fx_encode) and, where the game has a render scale stub,
/// `DM_RENDER_SCALE` (the host holds it to the stub's range and ignores it without one).
pub const METALFX_LEVELS: [(&str, &str); 2] = [("quality", "0.75"), ("performance", "0.5")];

/// The settings sheet's "Profile" row (`DM_PRESET`, read by the app and by [`apply_preset`], no engine switch behind it): a set of values for several rows at once. `standard` changes nothing;
/// `memory` is the memory-saving set derived from the Apple TV port of Ocarina of Time (2026-11-20): handheld mode, the game's dynamic resolution, 30 fps, the 3D views at half size (640 x 360 for
/// a 1280 x 720 handheld picture: the host's lowest render scale), no bulk pipeline prebuild before the first frame, and no background pass building the rest of the prewarm lists while the game
/// runs (`DM_PSO_PREPARE_TAIL=0`: what a draw needs is built when it is needed; iBuilder's own preparation fills the system's Metal cache beforehand, so those builds are cache hits).
/// `DM_LOW_MEMORY` was in the first version and was taken out after the device log of 2026-11-20 evening (Ocarina, iPhone 15 Pro): its two command allocators kept the submit thread waiting
/// 40-325 ms a frame for 11 ms of work, GPU event waits of 5 s appeared (none in the session without it), and the GPU memory still rose from 2.8 to 5.2 GB in play. Its row stays in the sheet.
/// Each row stays editable: a row the player set wins.
pub const PRESET_ENV: &str = "DM_PRESET";
pub const PRESET_STANDARD: &str = "standard";
pub const PRESET_MEMORY: &str = "memory";
/// (switch, value) of the `memory` profile. `DM_PSO_PREBUILD` (no row in GameApp's sheet: `LAUNCHER_DROP_UNLESS_PULLING`) and `DM_PSO_PREPARE_TAIL` (no row at all) are set by the boot from the profile alone.
// table rows are kept one per line as written
#[rustfmt::skip]
pub const PRESET_MEMORY_VALUES: [(&str, &str); 6] = [
    ("DM_OPMODE", "0"),
    ("DM_DYNAMIC_RESOLUTION", "1"),
    ("DM_FPS_TARGET", "30"),
    ("DM_RENDER_SCALE", "0.5"),
    ("DM_PSO_PREBUILD", "0"),
    ("DM_PSO_PREPARE_TAIL", "0"),
];
/// The games whose Profile row starts at `memory` (the player can pick Standard): Ocarina of Time (Switch 2), killed for its memory on an iPhone 15 Pro (device logs, 2026-11-20).
pub const PRESET_MEMORY_GAMES: [&str; 1] = ["0400486024DEC000"];

/// The profile a game starts with when the player has not chosen one.
pub fn preset_default(tid: &str) -> &'static str {
    if PRESET_MEMORY_GAMES
        .iter()
        .any(|g| g.eq_ignore_ascii_case(tid))
    {
        PRESET_MEMORY
    } else {
        PRESET_STANDARD
    }
}

/// The values a profile sets, by its name (`standard` and any unknown name: none).
pub fn preset_values(name: &str) -> &'static [(&'static str, &'static str)] {
    if name == PRESET_MEMORY {
        &PRESET_MEMORY_VALUES
    } else {
        &[]
    }
}

/// The "Profile" row of a game's launcher.json, built from the rows the sheet already has: its `presets` map gives, for each profile, the value of each row it covers (`standard`: the
/// row's own default as the catalogue resolved it for this game; `memory`: [`PRESET_MEMORY_VALUES`]); the app writes those values into the rows when the player picks a profile. When the
/// game's default profile is `memory`, the covered rows' defaults become the memory values, so the sheet shows what the game will run with. Rows the sheet does not have are left out.
pub fn add_preset_row(rows: &mut Vec<Value>, tid: &str) {
    rows.retain(|r| r.get("env").and_then(|e| e.as_str()) != Some(PRESET_ENV));
    let default = preset_default(tid);
    let mut standard = Map::new();
    let mut memory = Map::new();
    for r in rows.iter_mut() {
        let env = r
            .get("env")
            .and_then(|e| e.as_str())
            .unwrap_or("")
            .to_string();
        let Some((_, v)) = PRESET_MEMORY_VALUES.iter().find(|(k, _)| *k == env) else {
            continue;
        };
        let Some(m) = r.as_object_mut() else { continue };
        let own = m
            .get("default")
            .map(|d| {
                d.as_str()
                    .map(String::from)
                    .unwrap_or_else(|| d.to_string())
            })
            .unwrap_or_default();
        standard.insert(env.clone(), json!(own));
        memory.insert(env.clone(), json!(v));
        if default == PRESET_MEMORY {
            m.insert("default".into(), json!(v));
        }
    }
    if memory.is_empty() {
        return;
    }
    let covered: Vec<String> = memory.keys().cloned().collect();
    rows.insert(0, json!({
        "env": PRESET_ENV, "kind": "choice", "section": "Game", "label": "Profile", "default": default, "level": "basic",
        "options": [[PRESET_STANDARD, "Standard"], [PRESET_MEMORY, "Memory saver"]],
        "presets": {PRESET_STANDARD: standard, PRESET_MEMORY: memory},
        "note": format!("Sets several rows at once; each can still be changed below. Memory saver: handheld mode, dynamic resolution, 30 fps, 3D at half size, no pipeline prebuild before the first frame and no shader compiling in the background while you play. Less memory and a cooler phone, at the cost of a blurrier 3D image, less fluid motion and short pauses the first time something is drawn. Applies the next time the game starts. ({})", covered.join(", ")),
    }));
}

/// `apply_preset`: the profile's values for the switches that nothing chose yet (the environment, a row of settings.tsv, env.txt: each wins over it), after the settings and env.txt.
/// The profile is settings.tsv's `DM_PRESET` row, else the game's default ([`preset_default`]). Returns {env: value} set.
pub fn apply_preset(ctx: &Ctx) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(tid) = ctx.title_id() else {
        return out;
    };
    let text = read_text(&ctx.docs.game_dir(&tid).join("settings.tsv")).unwrap_or_default();
    let mut chosen: Option<String> = None;
    let mut explicit = BTreeSet::new();
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('\t') else {
            continue;
        };
        let key = k.trim().to_uppercase();
        if key == PRESET_ENV {
            chosen = Some(v.trim().to_string());
        }
        explicit.insert(key);
    }
    let (name, why) = match chosen {
        Some(c) if !c.is_empty() => (c, "chosen in the game's settings"),
        _ => (preset_default(&tid).to_string(), "this game's default"),
    };
    let values = preset_values(&name);
    if values.is_empty() {
        if name != PRESET_STANDARD {
            ctx.log(&format!(
                "game profile: '{name}' ({why}) is not a profile this app knows: nothing set"
            ));
        }
        return out;
    }
    let mut kept = vec![];
    for (env, v) in values {
        if explicit.contains(*env) || ctx.env.get(env).is_some() {
            kept.push(*env);
            continue;
        }
        ctx.env.set(env, v);
        out.insert(env.to_string(), v.to_string());
    }
    ctx.log(&format!(
        "game profile: {name} ({why}): {}{}",
        if out.is_empty() {
            "nothing left to set".to_string()
        } else {
            out.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        },
        if kept.is_empty() {
            String::new()
        } else {
            format!(" (kept as chosen elsewhere: {})", kept.join(" "))
        }
    ));
    out
}

/// Prefixes of the names `env.txt` may set: engine switches (docs/env_switches.tsv) and Metal's own diagnostics.
pub const ENV_FILE_PREFIXES: [&str; 2] = ["DM_", "MTL_"];

fn py_repr(s: &str) -> String {
    format!("'{s}'")
}

// ---- reading what the app wrote ------------------------------------------------------------------------------------------------

fn launcher_json_path(ctx: &Ctx, tid: &str) -> std::path::PathBuf {
    ctx.docs.game_dir(tid).join("launcher.json")
}

/// `launcher_envs`: the switches the game's launcher.json offers, or an empty set.
pub fn launcher_envs(ctx: &Ctx, tid: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(text) = read_text(&launcher_json_path(ctx, tid)) else {
        return out;
    };
    let Ok(doc) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    if let Some(rows) = doc.get("settings").and_then(|s| s.as_array()) {
        for r in rows {
            if let Some(env) = r.get("env").and_then(|e| e.as_str())
                && env.starts_with("DM_")
            {
                out.insert(env.to_string());
            }
        }
    }
    out
}

/// `launcher_catalogue`: a digest of what a launcher.json is made from: the bundled settings catalogue (`dm/games/settings.tsv`, which an engine patch can
/// extend: 0203's Skip draws, 0205's Frame generation) and GameApp's own notes and rows. A game built by an older app keeps its rows until this changes;
/// then its next boot writes them again (`launcher_stale`). The player's choices live apart from launcher.json and are kept.
/// (Not the Python app's digest: sha256 over a canonical form, so the first boot after the switch rewrites every launcher.json once.)
pub fn launcher_catalogue(ctx: &Ctx) -> String {
    let mut h = Sha256::new();
    if let Ok(b) = fs::read(ctx.bundle.join("dm").join("games").join("settings.tsv")) {
        h.update(&b);
    }
    let mut notes: Vec<_> = LAUNCHER_NOTES.to_vec();
    notes.sort();
    let mut repl: Vec<_> = LAUNCHER_NOTES_REPLACE.to_vec();
    repl.sort();
    h.update(
        serde_json::to_string(&json!([
            notes,
            repl,
            gameapp_rows(),
            PRESET_MEMORY_VALUES,
            PRESET_MEMORY_GAMES
        ]))
        .unwrap_or_default()
        .as_bytes(),
    );
    hex16(&h.finalize())
}

fn hex16(d: &[u8]) -> String {
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `launcher_stale`: true when the game has no launcher.json, or one written from another catalogue.
pub fn launcher_stale(ctx: &Ctx, tid: &str) -> bool {
    let Ok(text) = read_text(&launcher_json_path(ctx, tid)) else {
        return true;
    };
    let Ok(doc) = serde_json::from_str::<Value>(&text) else {
        return true;
    };
    if doc.get("catalogue").and_then(|c| c.as_str()) != Some(launcher_catalogue(ctx).as_str()) {
        return true;
    }
    // the "Skip draws while they compile" row's default follows the game's seed list (profile::pso_nowait_default), which may have come with the pack after the sheet was written
    let nowait = doc
        .get("settings")
        .and_then(|s| s.as_array())
        .and_then(|rows| {
            rows.iter()
                .find(|r| r.get("env").and_then(|e| e.as_str()) == Some("DM_PSO_NOWAIT"))
        });
    nowait.is_some_and(|r| {
        r.get("default").and_then(|d| d.as_str())
            != Some(crate::profile::pso_nowait_default(ctx, tid))
    })
}

// ---- before the boot --------------------------------------------------------------------------------------------------------------

/// `apply_game_settings`: the player's settings for this game (the screen writes `Documents/games/<id>/settings.tsv`: mode, fps, picture, show_fps) as the
/// engine's own switches (DM_OPMODE, DM_FPS_TARGET, DM_PICTURE_FIT, DM_SHOW_FPS), set before the boot. `auto` and missing keys leave the profile's choice; an
/// environment that already names a switch wins. Returns {env: value} set.
pub fn apply_game_settings(ctx: &Ctx) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(tid) = ctx.title_id() else {
        return out;
    };
    let path = ctx.docs.game_dir(&tid).join("settings.tsv");
    let Ok(text) = read_text(&path) else {
        return out;
    };
    let offered = launcher_envs(ctx, &tid);
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('\t') else {
            continue;
        };
        let (key, value) = (k.trim(), v.trim());
        let mut hit = game_setting(key, value).map(|(e, v)| (e.to_string(), v));
        if hit.is_none()
            && offered.contains(&key.to_uppercase())
            && !value.is_empty()
            && !value.contains('\n')
        {
            hit = Some((key.to_uppercase(), value.to_string())); // a row of the game's own launcher.json, by its switch's name
        }
        let Some((env, v)) = hit else { continue };
        if ctx.env.get(&env).is_some() {
            continue;
        }
        ctx.env.set(&env, &v);
        out.insert(env, v);
    }
    for ((env, value), extra) in LINKED_SWITCHES {
        if out.get(env).map(|s| s.as_str()) == Some(value) {
            for (e2, v2) in extra {
                if ctx.env.get(e2).is_none() {
                    ctx.env.set(e2, v2);
                    out.insert(e2.to_string(), v2.to_string());
                }
            }
        }
    }
    // one player choice, two engine switches; one the environment already names stays
    let scale = out
        .get("DM_METALFX")
        .and_then(|v| METALFX_LEVELS.iter().find(|(k, _)| k == v))
        .map(|(_, s)| *s);
    if let Some(scale) = scale {
        for (e2, v2) in [
            ("DM_PRESENT_UPSCALE", "metalfx"),
            ("DM_RENDER_SCALE", scale),
        ] {
            if ctx.env.get(e2).is_none() {
                ctx.env.set(e2, v2);
                out.insert(e2.to_string(), v2.to_string());
            }
        }
    }
    if !out.is_empty() {
        ctx.log.line(&format!(
            "game settings ({}): {}",
            path.display(),
            out.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    out
}

/// `apply_env_files`: free-form engine switches for experiments without a new .ipa: `Documents/env.txt` (every game) and `Documents/games/<id>/env.txt` (this game,
/// read last, so it wins over the global file), one `NAME=value` per line, `#` comments. Only DM_* and MTL_* names are taken; a switch the environment already names
/// (the settings sheet's, a harness's) is left alone. Returns {name: value}.
pub fn apply_env_files(ctx: &Ctx) -> BTreeMap<String, String> {
    let mut paths = vec![ctx.docs.env_txt()];
    if let Some(tid) = ctx.title_id() {
        paths.push(ctx.docs.game_dir(&tid).join("env.txt"));
    }
    let mut wanted: BTreeMap<String, String> = BTreeMap::new();
    for p in paths {
        let Ok(text) = read_text(&p) else { continue };
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((n, v)) = line.split_once('=') else {
                continue;
            };
            let (name, value) = (n.trim(), v.trim());
            if ENV_FILE_PREFIXES.iter().any(|p| name.starts_with(p))
                && !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
            {
                wanted.insert(name.to_string(), value.to_string());
            }
        }
    }
    let mut out = BTreeMap::new();
    for (name, value) in wanted {
        if let Some(have) = ctx.env.get(&name) {
            ctx.log.line(&format!(
                "env file: {name} already set to {}, {} from env.txt ignored",
                py_repr(&have),
                py_repr(&value)
            ));
            continue;
        }
        ctx.env.set(&name, &value);
        out.insert(name, value);
    }
    if !out.is_empty() {
        ctx.log.line(&format!(
            "env file: {}",
            out.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    out
}

// ---- the graphics API --------------------------------------------------------------------------------------------------------------

/// `game_gfxapi`: the graphics API the game renders with, as the engine's survey decides it (`profile_guess.shader_sources`): `nvn` when main imports
/// nvnBootstrapLoader, `vulkan` when it imports Vulkan functions without it, `gl` for EGL with glShaderSource; `nvn` when the module cannot be read.
/// Read from the rewritten (decompressed) main, in blocks: main is 160 MB and the phone's memory is the budget.
pub fn game_gfxapi(patched_main: &Path) -> &'static str {
    let Ok(mut f) = fs::File::open(patched_main) else {
        return "nvn";
    };
    const NVN: &[u8] = b"nvnBootstrapLoader\0";
    const VK: &[u8] = b"vkCreateInstance\0";
    const EGL: &[u8] = b"eglCreateWindowSurface\0";
    const GLS: &[u8] = b"glShaderSource";
    let keep = [NVN.len(), VK.len(), EGL.len(), GLS.len()]
        .into_iter()
        .max()
        .unwrap()
        - 1;
    let (mut vk, mut egl, mut gls) = (false, false, false);
    let mut buf = vec![0u8; 4 << 20];
    let mut carry = 0usize;
    loop {
        let n = match f.read(&mut buf[carry..]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return "nvn",
        };
        let data = &buf[..carry + n];
        if find(data, NVN) {
            return "nvn";
        }
        vk |= find(data, VK);
        egl |= find(data, EGL);
        gls |= find(data, GLS);
        let tail = data.len().min(keep);
        let start = data.len() - tail;
        buf.copy_within(start..start + tail, 0);
        carry = tail;
    }
    if vk {
        "vulkan"
    } else if egl && gls {
        "gl"
    } else {
        "nvn"
    }
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    hay.len() >= needle.len() && hay.windows(needle.len()).any(|w| w == needle)
}

/// `shader_pulling_wanted`: the engine's `ios_wants_pulling` for this app: the "pulling" libraries (_bp, ~27% of a pack) are read only by the Metal 4
/// pipelines (DM_PSO_MTL4 / the profile's pso_mtl4), and GameApp has no switch for them, so they are built only when the profile turns pso_mtl4 on
/// (engine docs/reference/rendering.md, the IOS_PULL rule). `shader_pull=1` / `0` in the config forces it.
pub fn shader_pulling_wanted(profile_dir: &Path, cfg: &crate::config::Config) -> bool {
    match cfg.get("shader_pull") {
        Some("0") => return false,
        Some("1") => return true,
        _ => {}
    }
    let Ok(text) = read_text(&profile_dir.join("game.tsv")) else {
        return false;
    };
    for line in text.lines() {
        let (key, _, value) = partition_tab(line);
        if key.trim() == "pso_mtl4" {
            return matches!(
                value
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase()
                    .as_str(),
                "on" | "1" | "yes" | "true"
            );
        }
    }
    false
}

fn partition_tab(line: &str) -> (&str, &str, &str) {
    match line.split_once('\t') {
        Some((a, b)) => (a, "\t", b),
        None => (line, "", ""),
    }
}

// ---- launcher.json ----------------------------------------------------------------------------------------------------------------------

/// `write_launcher_settings`: the game's own settings screen, as the engine's launcher has it: the engine's `settings.py` (the `LauncherBase` adapter: the rows
/// of settings.tsv whose condition holds for THIS game) written as `Documents/games/<id>/launcher.json`, which the app's settings sheet draws. The Metal 4
/// rows are dropped unless the game's pack has the pulling libraries they need (`shader_pulling_wanted`), and rows get GameApp's line of explanation where
/// the catalogue has none (`LAUNCHER_NOTES`). `main_dir` holds `main.patched.bin` and `main.fixups.tsv` (rewrite/main, or flat/). Returns the path, or `None`.
pub fn write_launcher_settings(ctx: &Ctx, main_dir: &Path) -> Option<std::path::PathBuf> {
    let Some(tid) = ctx.title_id() else {
        ctx.log("launcher settings: no title id");
        return None;
    };
    let Some(launcher) = ctx.adapters.launcher.clone() else {
        ctx.log(
            "launcher settings: no settings generator (dm/games/settings.py port) in this build",
        );
        return None;
    };
    let out_dir = ctx.docs.game_dir(&tid);
    let out = out_dir.join("launcher.json");
    let run = || -> Result<(Vec<String>, &'static str), String> {
        fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
        let control = Path::new(ctx.cfg.get_or("extract_out", "")).join("control.nacp");
        let gfx = game_gfxapi(&main_dir.join("main.patched.bin"));
        let derived = ctx.docs.derived_dir(&tid);
        let derived_root = if is_file(&derived.join("game.tsv")) {
            Some(ctx.docs.profile_derived())
        } else {
            None
        };
        let mut doc = launcher
            .base_document(
                &ctx.view(),
                &tid,
                main_dir,
                &control,
                gfx,
                derived_root.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        let table =
            game_table_dir(&ctx.docs.profile_derived(), &ctx.bundle, &tid).unwrap_or_default();
        let obj = doc
            .as_object_mut()
            .ok_or("the settings document is not an object")?;
        let settings = obj
            .entry("settings")
            .or_insert_with(|| Value::Array(vec![]));
        let rows = settings.as_array_mut().ok_or("settings is not a list")?;
        if !crate::settings::shader_pulling_wanted(&table, &ctx.cfg) {
            rows.retain(|r| {
                !LAUNCHER_DROP_UNLESS_PULLING
                    .contains(&r.get("env").and_then(|e| e.as_str()).unwrap_or(""))
            });
        }
        for r in rows.iter_mut() {
            let env = r
                .get("env")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            let Some(m) = r.as_object_mut() else { continue };
            if env == "DM_SHOW_FPS" {
                m.insert("default".into(), json!("1")); // GameApp shows the counter unless told not to (load/front_pad.m fps_wanted)
            }
            if env == "DM_PSO_NOWAIT" {
                // what the boot will do for this game (profile::set_pso_nowait), not the engine's "1": the sheet showed ON while the game waited, and ON was never written (review r4 B3)
                m.insert(
                    "default".into(),
                    json!(crate::profile::pso_nowait_default(ctx, &tid)),
                );
            }
            if let Some((_, note)) = LAUNCHER_NOTES_REPLACE.iter().find(|(e, _)| *e == env) {
                m.insert("note".into(), json!(note));
            } else if m
                .get("note")
                .map(|n| n.as_str().map(|s| s.is_empty()).unwrap_or(n.is_null()))
                .unwrap_or(true)
                && let Some((_, note)) = LAUNCHER_NOTES.iter().find(|(e, _)| *e == env)
            {
                m.insert("note".into(), json!(note));
            }
        }
        if gfx == "nvn" {
            let have: BTreeSet<String> = rows
                .iter()
                .filter_map(|r| r.get("env").and_then(|e| e.as_str()).map(String::from))
                .collect();
            for r in gameapp_rows() {
                if !have.contains(r["env"].as_str().unwrap_or("")) {
                    rows.push(r);
                }
            }
            // MetalFX upscaling has one row (DM_METALFX); the engine's Off / MetalFX "Upscaling" row would be a second, competing one
            rows.retain(|r| r.get("env").and_then(|e| e.as_str()) != Some("DM_PRESENT_UPSCALE"));
            add_preset_row(rows, &tid); // "Profile": several rows at once (Memory saver)
        }
        let names: Vec<String> = rows
            .iter()
            .map(|r| {
                r.get("env")
                    .and_then(|e| e.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        obj.insert("gfxapi".into(), json!(gfx));
        obj.insert("catalogue".into(), json!(launcher_catalogue(ctx)));
        let text = to_python_json(&doc);
        write_atomic(&out, text.as_bytes()).map_err(|e| e.to_string())?;
        Ok((names, gfx))
    };
    match run() {
        Ok((names, gfx)) => {
            ctx.log(&format!(
                "launcher settings: {} ({} rows: {}; graphics {})",
                out.display(),
                names.len(),
                names.join(" "),
                gfx
            ));
            Some(out)
        }
        Err(e) => {
            ctx.log(&format!("launcher settings: [RED] not written: {e}"));
            None
        }
    }
}

/// `json.dump(doc, f, indent=1)`: one-space indent, `ensure_ascii` (non-ASCII as \uXXXX), keys in insertion order.
pub fn to_python_json(v: &Value) -> String {
    use serde::Serialize;
    use serde_json::ser::{Formatter, PrettyFormatter, Serializer};
    struct Ascii<'a>(PrettyFormatter<'a>);
    macro_rules! fwd {
        ($($name:ident($($arg:ident: $ty:ty),*);)*) => { $(fn $name<W: ?Sized + std::io::Write>(&mut self, w: &mut W $(, $arg: $ty)*) -> std::io::Result<()> { self.0.$name(w $(, $arg)*) })* };
    }
    impl Formatter for Ascii<'_> {
        fwd! {
            begin_array(); end_array(); begin_array_value(first: bool); end_array_value();
            begin_object(); end_object(); begin_object_key(first: bool); end_object_key(); begin_object_value(); end_object_value();
        }
        fn write_string_fragment<W: ?Sized + std::io::Write>(
            &mut self,
            w: &mut W,
            fragment: &str,
        ) -> std::io::Result<()> {
            let mut buf = [0u16; 2];
            for ch in fragment.chars() {
                if ch.is_ascii() {
                    w.write_all(&[ch as u8])?;
                } else {
                    for u in ch.encode_utf16(&mut buf) {
                        write!(w, "\\u{:04x}", u)?;
                    }
                }
            }
            Ok(())
        }
    }
    let mut out = Vec::new();
    {
        let mut ser =
            Serializer::with_formatter(&mut out, Ascii(PrettyFormatter::with_indent(b" ")));
        v.serialize(&mut ser).expect("a JSON value serializes");
    }
    // `json.dump` writes no final newline
    String::from_utf8(out).expect("ascii")
}

/// Build a settings row the way `settings.py` does, for adapters that generate documents (a convenience for the CLI and the mock).
pub fn empty_document(game: &str) -> Value {
    let mut m = Map::new();
    m.insert("schema".into(), json!(1));
    m.insert("game".into(), json!(game));
    m.insert("settings".into(), json!([]));
    Value::Object(m)
}
