//! `LauncherBase` for the app: the engine's `dm/games/settings.py` in Rust.
//!
//! The game's settings sheet in the app (and in an exported game's player) is drawn from `Documents/games/<id>/launcher.json`: the rows of `settings.tsv` whose condition holds for THIS game. The Python chain
//! got the base document by running the engine's `settings.py`; this is the same function (`build(gid, patched, control)`) over `ibuilder-profile`'s `game.tsv` reader and header macros, with the app's
//! platform fixed to `ios` (the Mac launcher's `mac-mic` condition and `|<mac>` defaults are never taken). The pipeline's own post-processing (`settings::write_launcher_settings`: the Metal 4 rows, GameApp's
//! notes, the catalogue digest) is applied on top by the pipeline, as it was applied on top of `settings.py`'s output.
//!
//! Verified against the real `settings.py` (python 3.14, the patched engine's copy) on Grizzy and the Lemmings: `tests/launcher_oracle.rs` (the two documents are the same JSON value) -- see `docs/APP-RUST.md`.
//! Every `sys.exit(message)` of the Python is an `Err(message)` here (a bad row of `settings.tsv` stops the settings, never the chain).

use ibuilder_pipeline::adapters::LauncherBase;
use ibuilder_pipeline::ctx::View;
use ibuilder_pipeline::error::{PipelineError, Result};
use ibuilder_profile::game::{Game, find_dir, header_macros};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub struct RustLauncher;

const SECTIONS: [&str; 3] = ["Game", "Controls", "Performance"];
const SCHEMA: i64 = 1;
const UNKNOWN_LANGUAGES: &str = "This game's files do not list its languages, so some of these may not be in the game. It may then show another one.";
/// `nacp.LANGUAGES`: Horizon's language order (one bit each of the NACP's SupportedLanguageFlag)
const LANGUAGES: [(&str, &str); 16] = [
    ("en-US", "American English"),
    ("en-GB", "British English"),
    ("ja", "Japanese"),
    ("fr", "French"),
    ("de", "German"),
    ("es-419", "Latin American Spanish"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("fr-CA", "Canadian French"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
    ("ko", "Korean"),
    ("zh-Hant", "Traditional Chinese"),
    ("zh-Hans", "Simplified Chinese"),
    ("pt-BR", "Brazilian Portuguese"),
];

fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(PipelineError::failed(msg))
}

/// `nacp.languages(path)`: the codes whose bit is set in the u32 at 0x302C; a missing file, a short one or no flag gives none.
pub fn nacp_languages(path: &Path) -> Vec<&'static str> {
    let Ok(b) = std::fs::read(path) else {
        return vec![];
    };
    if b.len() < 0x3030 {
        return vec![];
    }
    let f = u32::from_le_bytes([b[0x302C], b[0x302D], b[0x302E], b[0x302F]]);
    LANGUAGES
        .iter()
        .enumerate()
        .filter(|(i, _)| f >> i & 1 == 1)
        .map(|(_, (c, _))| *c)
        .collect()
}

/// `game.py upscaler_keys(g)` counted: distinct 16-digit keys (column 4) of the `compute` rows (column 3) whose source (column 5) ends in `#NDSR@0x<lower-case hex>`; none without a map
pub fn upscaler_count(shader_map: &Path) -> usize {
    let Ok(t) = std::fs::read_to_string(shader_map) else {
        return 0;
    };
    let mut keys = std::collections::BTreeSet::new();
    for line in t.lines() {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() >= 5
            && c[2] == "compute"
            && c[3].len() == 16
            && c[3]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            if let Some(i) = c[4].rfind("#NDSR@0x") {
                let h = &c[4][i + 8..];
                if !h.is_empty()
                    && h.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    keys.insert(c[3].to_string());
                }
            }
        }
    }
    keys.len()
}

/// `fixups_keys`: the `# KEY<TAB>value` header rows of a rewriter table, as {KEY: value}
fn fixups_keys(path: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(path) {
        for line in t.lines() {
            let Some(rest) = line.strip_prefix("# ") else {
                continue;
            };
            let Some((k, v)) = rest.split_once('\t') else {
                continue;
            };
            if !k.is_empty() && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                out.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    out
}

/// the `meta` column: `key=value` pairs separated by `;`, each key from META once, the value matching its pattern (`level`: `basic`; `needs`: `DM_[A-Z0-9_]+=[^;=]+`)
fn meta(text: &str, n: usize) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for pair in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            return fail(format!(
                "settings.tsv:{n}: unknown meta key {pair:?} (keys: level, needs)"
            ));
        };
        if key != "level" && key != "needs" {
            return fail(format!(
                "settings.tsv:{n}: unknown meta key {pair:?} (keys: level, needs)"
            ));
        }
        if out.iter().any(|(k, _)| k == key) {
            return fail(format!("settings.tsv:{n}: meta key {key} given twice"));
        }
        let ok = match key {
            "level" => value == "basic",
            _ => match value.split_once('=') {
                Some((var, val)) => {
                    var.len() > 3
                        && var.starts_with("DM_")
                        && var
                            .bytes()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                        && !val.is_empty()
                        && !val.contains([';', '='])
                }
                None => false,
            },
        };
        if !ok {
            let pat = if key == "level" {
                "basic"
            } else {
                "DM_[A-Z0-9_]+=[^;=]+"
            };
            return fail(format!(
                "settings.tsv:{n}: meta {key}={value:?} does not match {pat}"
            ));
        }
        out.push((key.to_string(), value.to_string()));
    }
    Ok(out)
}

/// `render_base`: "<w>x<h> <w>x<h>" (docked, then handheld) as {"docked": [w, h], "handheld": [w, h]}; None when the profile has none
fn render_base(g: &Game) -> Result<Option<Value>> {
    let v = g.get_or("render_base", "");
    if v.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = v.split_whitespace().collect();
    let dims = |s: &str| -> Option<(u64, u64)> {
        let (a, b) = s.split_once('x')?;
        if a.is_empty()
            || b.is_empty()
            || !a.bytes().all(|c| c.is_ascii_digit())
            || !b.bytes().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    match (parts.as_slice(), parts.len()) {
        ([d, h], 2) => match (dims(d), dims(h)) {
            (Some(d), Some(h)) => Ok(Some(json!({"docked": [d.0, d.1], "handheld": [h.0, h.1]}))),
            _ => fail(format!(
                "{}: render_base is \"<w>x<h> <w>x<h>\" (docked, then handheld): {v}",
                g.id
            )),
        },
        _ => fail(format!(
            "{}: render_base is \"<w>x<h> <w>x<h>\" (docked, then handheld): {v}",
            g.id
        )),
    }
}

struct Row<'a> {
    n: usize,
    c: Vec<&'a str>, // exactly 9 columns
}

fn rows(text: &str) -> Result<Vec<Row<'_>>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut c: Vec<&str> = line.split('\t').collect();
        if !matches!(c.len(), 7..=9) {
            return fail(format!(
                "settings.tsv:{n}: 7 to 9 columns expected, {} found",
                c.len()
            ));
        }
        if !SECTIONS.contains(&c[5]) {
            return fail(format!("settings.tsv:{n}: unknown section: {}", c[5]));
        }
        c.resize(9, "");
        out.push(Row { n, c });
    }
    Ok(out)
}

/// What a row's variable can take, for `needs`: None when anything (a range, the game's languages)
fn takes(kind: &str, values: &str) -> Option<Vec<String>> {
    match kind {
        "toggle" => Some(if values == "-" {
            vec!["1".into(), "0".into()]
        } else {
            values.split('/').map(String::from).collect()
        }),
        "choice" if values != "languages" => Some(
            values
                .split(',')
                .map(|v| {
                    v.split('?')
                        .next()
                        .unwrap_or("")
                        .split('=')
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Python's `float(x)` for the range column
fn pyfloat(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        t.parse::<f64>().ok()
    }
}

struct Ctx<'a> {
    macros: BTreeMap<String, String>,
    keys: BTreeMap<String, String>,
    langs: Vec<(&'static str, &'static str)>,
    gfxapi: &'a str,
}

impl Ctx<'_> {
    fn macro_(&self, name: &str, n: usize) -> Result<String> {
        match self.macros.get(name) {
            Some(v) => Ok(v.trim_matches('"').to_string()),
            None => fail(format!(
                "settings.tsv:{n}: no {name} in the game profile header (game.py header)"
            )),
        }
    }

    /// `shown(cond)`: the platform is ios
    fn shown(&self, cond: &str, n: usize) -> Result<bool> {
        if cond.contains('&') {
            for c in cond.split('&') {
                if !self.shown(c, n)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if cond == "always" {
            return Ok(true);
        }
        if cond == "mac-mic" {
            return Ok(false); // a Mac app whose modules import the polling InputStream API: never the iOS app
        }
        if cond == "languages>1" {
            return Ok(self.langs.len() > 1);
        }
        if let Some(k) = cond.strip_prefix("fixups:") {
            return Ok(self.keys.contains_key(k));
        }
        if let Some(a) = cond.strip_prefix("gfxapi:") {
            return Ok(self.gfxapi == a);
        }
        if let Some(p) = cond.strip_prefix("platform:") {
            return Ok(p == "ios");
        }
        // (DM_GAME_\w+)(!=|==)(.*)
        if let Some(rest) = cond.strip_prefix("DM_GAME_") {
            let end = rest
                .bytes()
                .position(|c| !(c.is_ascii_alphanumeric() || c == b'_'))
                .unwrap_or(rest.len());
            let (name, tail) = rest.split_at(end);
            for (op, eq) in [("!=", false), ("==", true)] {
                if let Some(want) = tail.strip_prefix(op) {
                    let have = self.macro_(&format!("DM_GAME_{name}"), n)?;
                    return Ok((have == want) == eq);
                }
            }
        }
        fail(format!("settings.tsv:{n}: unknown condition {cond}"))
    }

    fn value(&self, v: &dyn Fn(&str) -> Option<String>, d: &str, n: usize) -> Result<String> {
        let mut d = d.to_string();
        if d.contains('|') {
            let parts: Vec<&str> = d.split('|').collect();
            if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
                return fail(format!(
                    "settings.tsv:{n}: platform default is <ios>|<mac>: {d}"
                ));
            }
            d = parts[0].to_string();
        }
        if d.starts_with("DM_GAME_") {
            return self.macro_(&d, n);
        }
        if let Some(var) = d.strip_prefix("build:") {
            let e = v(var).unwrap_or_default();
            return Ok(match e.as_str() {
                "YES" => "1".into(),
                "NO" | "" => "0".into(),
                _ => e,
            });
        }
        Ok(d)
    }

    fn offered(&self, v: &str, n: usize) -> Result<bool> {
        match v.split_once('?') {
            None => Ok(true),
            Some((_, conds)) => {
                for c in conds.split('&') {
                    if !self.shown(c, n)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
        }
    }
}

/// `settings.py build(gid, patched, control)` for the iOS app
pub fn build(
    g: &Game,
    settings_tsv: &str,
    main_dir: &Path,
    control: &Path,
    gfxapi: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Value> {
    let mut warnings = Vec::new();
    let macros: BTreeMap<String, String> = header_macros(g, &mut warnings)
        .map_err(|e| PipelineError::failed(e.to_string()))?
        .into_iter()
        .filter(|(k, _)| k.starts_with("DM_GAME_"))
        .collect();
    let mut macros = macros;
    if !macros.contains_key("DM_GAME_UPSCALER_N") {
        // ibuilder-profile's header leaves `UPSCALER_KEYS` out (the CLI's profile never needs them); settings.tsv's "Game's upscaler" row does:
        // game.py upscaler_keys(g): the compute shaders of shader_map.tsv whose source column ends in `#NDSR@0x<hex>`, as a set of 16-digit keys
        macros.insert(
            "DM_GAME_UPSCALER_N".into(),
            upscaler_count(&g.dir.join("shader_map.tsv")).to_string(),
        );
    }
    let keys = fixups_keys(&main_dir.join("main.fixups.tsv"));
    let have = nacp_languages(control);
    let langs: Vec<(&'static str, &'static str)> = LANGUAGES
        .iter()
        .copied()
        .filter(|(c, _)| have.is_empty() || have.contains(c))
        .collect();
    let cx = Ctx {
        macros,
        keys,
        langs,
        gfxapi,
    };
    let all = rows(settings_tsv)?;
    let table: BTreeMap<&str, Option<Vec<String>>> = all
        .iter()
        .map(|r| (r.c[0], takes(r.c[1], r.c[2])))
        .collect();
    let base = render_base(g)?;
    let mut out: Vec<Value> = Vec::new();
    for r in &all {
        let [
            var,
            kind,
            values,
            default,
            cond,
            section,
            label,
            note,
            extra,
        ] = [
            r.c[0], r.c[1], r.c[2], r.c[3], r.c[4], r.c[5], r.c[6], r.c[7], r.c[8],
        ];
        let n = r.n;
        let m = meta(extra, n)?;
        if let Some((_, needs)) = m.iter().find(|(k, _)| k == "needs") {
            let (need, want) = needs.split_once('=').unwrap_or((needs, ""));
            let Some(can) = table.get(need) else {
                return fail(format!(
                    "settings.tsv:{n}: needs names a variable no row sets: {needs}"
                ));
            };
            if let Some(can) = can {
                if !can.iter().any(|c| c == want) {
                    let mut sorted = can.clone();
                    sorted.sort();
                    sorted.dedup();
                    return fail(format!(
                        "settings.tsv:{n}: needs a value {need} never takes: {want} (it takes {})",
                        sorted.join(", ")
                    ));
                }
            }
        }
        if !cx.shown(cond, n)? {
            continue;
        }
        let mut s = Map::new();
        s.insert("env".into(), json!(var));
        s.insert("kind".into(), json!(kind));
        s.insert("section".into(), json!(section));
        s.insert("label".into(), json!(label));
        s.insert("note".into(), json!(note));
        let mut dflt = cx.value(env, default, n)?;
        s.insert("default".into(), json!(dflt));
        for (k, v) in &m {
            s.insert(k.clone(), json!(v));
        }
        if var == "DM_RENDER_SCALE" {
            if let Some(b) = &base {
                s.insert("base".into(), b.clone());
            }
        }
        match kind {
            "choice" => {
                let mut opts: Vec<Vec<String>> = Vec::new();
                if values == "languages" {
                    opts = cx
                        .langs
                        .iter()
                        .map(|(c, nm)| vec![c.to_string(), nm.to_string()])
                        .collect();
                } else {
                    for v in values.split(',') {
                        if !cx.offered(v, n)? {
                            continue;
                        }
                        let head = v.split('?').next().unwrap_or("");
                        opts.push(match head.split_once('=') {
                            Some((a, b)) => vec![a.to_string(), b.to_string()],
                            None => vec![head.to_string()],
                        });
                    }
                }
                if opts.iter().any(|o| o.len() != 2) {
                    return fail(format!(
                        "settings.tsv:{n}: choice values are value=label,..."
                    ));
                }
                if opts.len() < 2 && values != "languages" {
                    continue; // one option left on this platform: nothing to choose
                }
                if !opts.iter().any(|o| o[0] == dflt) {
                    dflt = opts.first().map(|o| o[0].clone()).unwrap_or_default();
                }
                s.insert("default".into(), json!(dflt));
                s.insert("options".into(), json!(opts));
                if values == "languages" && have.is_empty() {
                    s.insert("note".into(), json!(UNKNOWN_LANGUAGES));
                }
            }
            "range" => {
                let p: Vec<&str> = values.split(':').collect();
                let f: Vec<Option<f64>> = p.iter().map(|x| pyfloat(x)).collect();
                if p.len() != 3 || f.iter().any(Option::is_none) {
                    return fail(format!(
                        "settings.tsv:{n}: range values are low:high:step: {values}"
                    ));
                }
                let (lo, mut hi, st) = (f[0].unwrap(), f[1].unwrap(), f[2].unwrap());
                if var == "DM_RENDER_SCALE" {
                    if let Some(rs) = cx.keys.get("RENDER_SCALE").filter(|v| v.as_str() != "1") {
                        let t = rs.trim().trim_start_matches("0x").trim_start_matches("0X");
                        hi = match i64::from_str_radix(t, 16) {
                            Ok(v) => v as f64,
                            Err(_) => {
                                return fail(format!(
                                    "invalid literal for int() with base 16: {rs:?}"
                                ));
                            }
                        };
                    }
                }
                s.insert("min".into(), json!(lo));
                s.insert("max".into(), json!(hi));
                s.insert("step".into(), json!(st));
            }
            "toggle" => {
                if dflt != "0" && dflt != "1" {
                    return fail(format!(
                        "settings.tsv:{n}: a toggle defaults to 0 or 1: {dflt}"
                    ));
                }
                let on_off: Vec<String> = if values == "-" {
                    vec!["1".into(), "0".into()]
                } else {
                    values.split('/').map(String::from).collect()
                };
                if on_off.len() != 2
                    || on_off.iter().any(String::is_empty)
                    || on_off[0] == on_off[1]
                {
                    return fail(format!(
                        "settings.tsv:{n}: toggle values are - or <on>/<off>: {values}"
                    ));
                }
                s.insert("on".into(), json!(on_off[0]));
                s.insert("off".into(), json!(on_off[1]));
                s.insert(
                    "default".into(),
                    json!(if dflt == "1" { &on_off[0] } else { &on_off[1] }),
                );
            }
            _ => return fail(format!("settings.tsv:{n}: unknown kind {kind}")),
        }
        out.push(Value::Object(s));
    }
    Ok(json!({"schema": SCHEMA, "game": g.id, "settings": out}))
}

impl LauncherBase for RustLauncher {
    fn base_document(
        &self,
        v: &View<'_>,
        tid: &str,
        main_dir: &Path,
        control: &Path,
        gfxapi: &str,
        derived_root: Option<&Path>,
    ) -> Result<Value> {
        let games = v.bundle.join("dm").join("games");
        // game.load(gid): dm/games first, then DM_GAMES_DIR (the derived profiles of this app)
        let extra: Vec<std::path::PathBuf> = derived_root
            .map(|d| vec![d.to_path_buf()])
            .unwrap_or_default();
        let dir = find_dir(tid, &games, &extra, false);
        let g = Game::load_dir(&dir).map_err(|e| PipelineError::failed(e.to_string()))?;
        let tsv_path = games.join("settings.tsv");
        let tsv = std::fs::read_to_string(&tsv_path)
            .map_err(|e| PipelineError::io(format!("read {}", tsv_path.display()), e))?;
        build(&g, &tsv, main_dir, control, gfxapi, &|name| v.env.get(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "title_id\t0100AAAAAAAAAAAA\nprogram_id\t0100AAAAAAAAAAAA\nname\tMy Game\nromfs_marker\tData/x\nmodules\tmain sdk\nmem_reserve\t0x40000000\nmem_alias\t0x10000000\nmem_stack\t0x10000000\napp_pool\t0xCD500000\nfps_target\t30\nbuild\tv0\t1.0.0\tmain=abcd\tsdk=ef01\n";

    fn game() -> Game {
        Game::from_text(std::path::PathBuf::from("/x/0100AAAAAAAAAAAA"), GAME)
    }

    fn run(tsv: &str, gfx: &str) -> Result<Value> {
        build(
            &game(),
            tsv,
            Path::new("/nonexistent"),
            Path::new("/nonexistent"),
            gfx,
            &|_| None,
        )
    }

    #[test]
    fn conditions_options_toggles_and_ranges() {
        let tsv = "# header\n\
            DM_FPS_TARGET\tchoice\t30=30 fps,60=60 fps,120=120 fps?DM_GAME_FPS_MAX==120\tDM_GAME_FPS_TARGET\tDM_GAME_FPS_TARGET!=0\tGame\tFrame rate\t\tlevel=basic\n\
            DM_FRAMEGEN\ttoggle\t-\t0\tgfxapi:nvn&platform:ios\tGame\tFrame generation\tnote\n\
            DM_MAC_ONLY\ttoggle\t-\t0\tplatform:mac\tGame\tMac\t\n\
            DM_RENDER_SCALE\trange\t0.5:1:0.05\t1\talways\tPerformance\tScale\t\n";
        let d = run(tsv, "nvn").unwrap();
        let s = d["settings"].as_array().unwrap();
        assert_eq!(s.len(), 3, "the Mac-only row is not shown: {d}");
        assert_eq!(s[0]["env"], "DM_FPS_TARGET");
        assert_eq!(
            s[0]["options"],
            json!([["30", "30 fps"], ["60", "60 fps"]]),
            "120 fps is offered only when the profile allows it"
        );
        assert_eq!(s[0]["default"], "30");
        assert_eq!(s[0]["level"], "basic");
        assert_eq!(s[1]["on"], "1");
        assert_eq!(s[1]["default"], "0");
        assert_eq!(
            (
                s[2]["min"].as_f64(),
                s[2]["max"].as_f64(),
                s[2]["step"].as_f64()
            ),
            (Some(0.5), Some(1.0), Some(0.05))
        );
        assert_eq!(
            run(tsv, "vulkan").unwrap()["settings"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "gfxapi:nvn hides frame generation elsewhere"
        );
        assert_eq!(d["game"], "0100AAAAAAAAAAAA");
    }

    #[test]
    fn a_bad_row_is_an_error_with_the_pythons_words() {
        for (tsv, want) in [
            (
                "A\ttoggle\t-\t0\talways\tNowhere\tx\t\n",
                "settings.tsv:1: unknown section: Nowhere",
            ),
            (
                "A\ttoggle\t-\t0\talways\tGame\n",
                "settings.tsv:1: 7 to 9 columns expected, 6 found",
            ),
            (
                "A\ttoggle\t-\t0\twhenever\tGame\tx\t\n",
                "settings.tsv:1: unknown condition whenever",
            ),
            (
                "A\ttoggle\t-\t0\talways\tGame\tx\t\tbogus=1\n",
                "unknown meta key",
            ),
            (
                "A\ttoggle\t-\t2\talways\tGame\tx\t\n",
                "a toggle defaults to 0 or 1: 2",
            ),
            (
                "A\tchoice\ta=A,b=B\ta\tDM_GAME_NOPE==1\tGame\tx\t\n",
                "no DM_GAME_NOPE in the game profile header",
            ),
            (
                "A\ttoggle\t-\t0\talways\tGame\tx\t\tneeds=DM_B=1\n",
                "needs names a variable no row sets: DM_B=1",
            ),
        ] {
            let e = run(tsv, "nvn").unwrap_err().to_string();
            assert!(e.contains(want), "{tsv:?}: {e}");
        }
    }

    #[test]
    fn needs_must_name_a_value_the_row_can_take() {
        let tsv = "DM_A\ttoggle\t-\t0\talways\tGame\ta\t\nDM_B\ttoggle\t-\t0\talways\tGame\tb\t\tneeds=DM_A=7\n";
        let e = run(tsv, "nvn").unwrap_err().to_string();
        assert!(
            e.contains("needs a value DM_A never takes: 7 (it takes 0, 1)"),
            "{e}"
        );
    }

    #[test]
    fn languages_come_from_the_nacp_flag_and_every_language_without_one() {
        let d = std::env::temp_dir().join(format!("ibuilder-ffi-nacp-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("control.nacp");
        let mut b = vec![0u8; 0x4000];
        b[0x302C] = 0b0000_1001; // en-US and fr
        std::fs::write(&p, &b).unwrap();
        assert_eq!(nacp_languages(&p), vec!["en-US", "fr"]);
        assert!(nacp_languages(&d.join("missing")).is_empty());
        let tsv = "DM_LANG\tchoice\tlanguages\ten-US\tlanguages>1\tGame\tGame language\t\n";
        let with = build(&game(), tsv, &d, &p, "nvn", &|_| None).unwrap();
        assert_eq!(
            with["settings"][0]["options"],
            json!([["en-US", "American English"], ["fr", "French"]])
        );
        let without = build(&game(), tsv, &d, &d.join("missing"), "nvn", &|_| None).unwrap();
        assert_eq!(
            without["settings"][0]["options"].as_array().unwrap().len(),
            16
        );
        assert_eq!(without["settings"][0]["note"], UNKNOWN_LANGUAGES);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_upscaler_count_is_the_distinct_ndsr_compute_keys_of_the_shader_map() {
        let d = std::env::temp_dir().join(format!("ibuilder-ffi-ups-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let m = d.join("shader_map.tsv");
        std::fs::write(&m, "# c\n1\tx\tcompute\t00000000000000aa\t/Data/a#NDSR@0x1f\n2\tx\tcompute\t00000000000000aa\t/Data/a#NDSR@0x1f\n3\tx\tcompute\t00000000000000bb\t/Data/b#NDSR@0xzz\n4\tx\tpixel\t00000000000000cc\t/Data/c#NDSR@0x1\n5\tx\tcompute\t00000000000000dd\tmain#NDSR@0x2a\n").unwrap();
        assert_eq!(upscaler_count(&m), 2);
        assert_eq!(upscaler_count(&d.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_defaults_read_the_environment_yes_no() {
        let tsv = "DM_X\ttoggle\t-\tbuild:SOME_SWITCH\talways\tGame\tx\t\n";
        let on = build(
            &game(),
            tsv,
            Path::new("/n"),
            Path::new("/n"),
            "nvn",
            &|k| (k == "SOME_SWITCH").then(|| "YES".to_string()),
        )
        .unwrap();
        assert_eq!(on["settings"][0]["default"], "1");
        assert_eq!(run(tsv, "nvn").unwrap()["settings"][0]["default"], "0");
    }
}
