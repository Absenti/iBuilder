//! `dm/games/game.py`: a game's `game.tsv` (`key<TAB>values...`, `#` comments) and its `profile` subcommand, the
//! runtime profile the host reads from `DM_PROFILE` (`hle_profile.c set_kv`).
//!
//! `profile` goes through `header_one(g)` (the host's `game_profile.h` macros: all the validation lives there) and
//! writes the macros' values back as key/value rows. This port builds the same macro list, with the same
//! validation and messages, then renders the same rows, so a profile rendered here is byte-identical to the
//! Python's. Not ported because they cannot change the profile: the multi-program `header()`, `build_modules`
//! failures, and `UPSCALER_KEYS` (it reads `shader_map.tsv`, which the app bundle does not ship).
use crate::error::{Error, Result, fmt_err};
use crate::pyutil::{hex, py_int0, py_lstrip, py_repr, py_split_ws, py_splitlines, py_strip};
use std::path::{Path, PathBuf};

/// One game version of `Game::builds`: `(version, [(display version | module name, build ID)])`.
pub type Build = (String, Vec<(String, String)>);

pub struct Game {
    /// Resolved (symlinks followed), as `Path(d).resolve()`.
    pub dir: PathBuf,
    pub id: String,
    rows: Vec<(String, Vec<String>)>,
}

impl Game {
    pub fn load_dir(d: &Path) -> Result<Game> {
        let dir =
            std::fs::canonicalize(d).map_err(|e| Error::Format(format!("{}: {e}", d.display())))?;
        let path = dir.join("game.tsv");
        let text = String::from_utf8(std::fs::read(&path)?)
            .map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
        Ok(Game::from_text(dir, &text))
    }

    pub fn from_text(dir: PathBuf, text: &str) -> Game {
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut rows = Vec::new();
        for line in py_splitlines(text) {
            if py_strip(line).is_empty() || py_lstrip(line).starts_with('#') {
                continue;
            }
            let c: Vec<&str> = line.split('\t').collect();
            rows.push((
                py_strip(c[0]).to_string(),
                c[1..].iter().map(|v| py_strip(v).to_string()).collect(),
            ));
        }
        Game { dir, id, rows }
    }

    fn values(&self, key: &str) -> Vec<&Vec<String>> {
        self.rows
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect()
    }

    /// `g.get(key)`: the first row's values joined by tabs; a missing key is an error.
    pub fn get(&self, key: &str) -> Result<String> {
        match self.values(key).first() {
            Some(r) => Ok(r.join("\t")),
            None => fmt_err(format!(
                "'{} has no key {key}'",
                self.dir.join("game.tsv").display()
            )),
        }
    }

    /// `g.get(key, default)`.
    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.values(key)
            .first()
            .map_or_else(|| default.to_string(), |r| r.join("\t"))
    }

    pub fn words(&self, key: &str) -> Result<Vec<String>> {
        Ok(py_split_ws(&self.get(key)?)
            .into_iter()
            .map(str::to_string)
            .collect())
    }

    fn int(&self, key: &str) -> Result<i128> {
        let v = self.get(key)?;
        int0(&v)
    }

    /// `builds()`: `{version: {'display': display version, module name: build ID, ...}}`, insertion-ordered.
    pub fn builds(&self) -> Result<Vec<Build>> {
        let mut out: Vec<Build> = Vec::new();
        for r in self.values("build") {
            if r.len() < 2 {
                return fmt_err(format!(
                    "not enough values to unpack (expected at least 2, got {})",
                    r.len()
                ));
            }
            let mut d: Vec<(String, String)> = vec![("display".into(), r[1].clone())];
            for (i, m) in r[2..].iter().enumerate() {
                let Some((k, v)) = m.split_once('=') else {
                    return fmt_err(format!(
                        "dictionary update sequence element #{} has length 1; 2 is required",
                        i + 1
                    ));
                };
                match d.iter_mut().find(|(kk, _)| kk == k) {
                    Some(s) => s.1 = v.to_string(),
                    None => d.push((k.to_string(), v.to_string())),
                }
            }
            match out.iter_mut().find(|(v, _)| *v == r[0]) {
                Some(s) => s.1 = d,
                None => out.push((r[0].clone(), d)),
            }
        }
        Ok(out)
    }

    /// `build_modules()`: `{main build ID: [modules]}` for each build row with a main.
    pub fn build_modules(&self) -> Result<Vec<(String, Vec<String>)>> {
        let order = self.words("modules")?;
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for (_, b) in self.builds()? {
            if let Some((_, main)) = b.iter().find(|(k, _)| k == "main") {
                let mods: Vec<String> = order
                    .iter()
                    .filter(|m| b.iter().any(|(k, _)| k == *m))
                    .cloned()
                    .collect();
                match out.iter_mut().find(|(k, _)| k == main) {
                    Some(s) => s.1 = mods,
                    None => out.push((main.clone(), mods)),
                }
            }
        }
        Ok(out)
    }
}

fn int0(v: &str) -> Result<i128> {
    py_int0(v).ok_or_else(|| {
        Error::Format(format!(
            "invalid literal for int() with base 0: {}",
            py_repr(v)
        ))
    })
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn onoff(g: &Game, key: &str, default: &str) -> Result<String> {
    let v = g.get_or(key, default);
    if v != "on" && v != "off" {
        return fmt_err(format!("{}: {key} must be on or off: {v}", g.id));
    }
    Ok(v)
}

fn choice(g: &Game, key: &str, default: &str, allowed: &[&str], what: &str) -> Result<String> {
    let v = g.get_or(key, default);
    if !allowed.contains(&v.as_str()) {
        return fmt_err(format!("{}: {key} must be {what}: {v}", g.id));
    }
    Ok(v)
}

fn is_hex_lit(s: &str) -> bool {
    s.len() > 2
        && (s.starts_with("0x") || s.starts_with("0X"))
        && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// `re.fullmatch(r'(0x[0-9a-fA-F]+|\d+)\+(0x[0-9a-fA-F]+|\d+)', s)`
fn skip_parts(s: &str) -> Option<(&str, &str)> {
    let (a, b) = s.split_once('+')?;
    let ok = |x: &str| is_hex_lit(x) || is_digits(x);
    (ok(a) && ok(b)).then_some((a, b))
}

/// `re.fullmatch(r'(applegpu_)?[A-Za-z0-9]+', w)`
fn is_gpu_word(w: &str) -> bool {
    let t = w.strip_prefix("applegpu_").unwrap_or(w);
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `header_one(g)`: the `#define DM_GAME_*` lines, in the Python's order, as `(macro, value)`.
pub fn header_macros(g: &Game, warnings: &mut Vec<String>) -> Result<Vec<(String, String)>> {
    let id = &g.id;
    let mods = g.words("modules")?;
    let builds = g.builds()?;
    let vers = builds
        .iter()
        .filter_map(|(_, b)| {
            let main = b.iter().find(|(k, _)| k == "main")?;
            let disp = b.iter().find(|(k, _)| k == "display")?;
            Some(format!("{{{}, {}}}", q(&main.1), q(&disp.1)))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let pi = choice(
        g,
        "present_interval",
        "game",
        &["game", "display"],
        "game or display",
    )?;
    let ft = choice(g, "fps_target", "0", &["0", "30", "60"], "0, 30 or 60")?;
    let fm = choice(g, "fps_max", "60", &["60", "120"], "60 or 120")?;
    let fh = choice(g, "fps_hold", "on", &["on", "off"], "on or off")?;
    let utp = g.get_or("ue_texture_pool", "0");
    if !is_digits(&utp) {
        return fmt_err(format!(
            "{id}: ue_texture_pool must be megabytes (0 = the game's own pool): {utp}"
        ));
    }
    let ufc = g.get_or("ue_fps_cvar", "t.MaxFPS");
    let ufc_ok = {
        let b = ufc.as_bytes();
        !b.is_empty()
            && b.len() <= 31
            && b[0].is_ascii_alphabetic()
            && b.iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'.')
    };
    if !ufc_ok {
        return fmt_err(format!(
            "{id}: ue_fps_cvar must be a console variable name: {ufc}"
        ));
    }
    let ps = onoff(g, "present_snap", "off")?;
    let p4 = onoff(g, "pso_mtl4", "off")?;
    let pg_s = g.get_or("pso_gpu", "");
    let pg = py_split_ws(&pg_s);
    if !pg.is_empty() && (pg.len() > 4 || pg.iter().any(|w| *w == "none" || !is_gpu_word(w))) {
        return fmt_err(format!(
            "{id}: pso_gpu takes 1-4 whitespace-separated GPU architectures such as 'g18p applegpu_g19p' (each an optional applegpu_ prefix then an alphanumeric token; there is no none: omit the key for g18p with pso_mtl4 on, or no archive with pso_mtl4 off): [{}]",
            pg.iter().map(|w| py_repr(w)).collect::<Vec<_>>().join(", ")
        ));
    }
    let et = onoff(g, "enc_thread", "off")?;
    let pf = onoff(g, "nvn_prefetch", "on")?;
    let cp = onoff(g, "core_preempt", "off")?;
    let aba = onoff(g, "alloc_big_align", "off")?;
    let sf = choice(g, "shared_font", "bfttf", &["bfttf", "ttf"], "bfttf or ttf")?;
    let pl = choice(
        g,
        "pad_layout",
        "simple",
        &["simple", "full", "touch"],
        "simple, full or touch",
    )?;
    let sks = g.get_or("shader_key_skip", "");
    let m = if sks.is_empty() {
        None
    } else {
        skip_parts(&sks)
    };
    if !sks.is_empty() {
        let bad = match m {
            None => true,
            Some((a, b)) => match (py_int0(a), py_int0(b)) {
                (Some(o), Some(l)) => {
                    !(0x30 <= o) || !(0 < l) || o + l > 0x880 || (o < 0x760 && o + l > 0x700)
                }
                _ => true,
            },
        };
        if bad {
            return fmt_err(format!(
                "{id}: shader_key_skip takes <offset>+<length> within the first 0x880 bytes of the control section, past its header (+0x30) and clear of +0x700..+0x75f: {sks}"
            ));
        }
    }
    let (sko, skl) = match m {
        Some((a, b)) => (py_int0(a).unwrap_or(0), py_int0(b).unwrap_or(0)),
        None => (0, 0),
    };
    if !g.get_or("render_cond", "").is_empty() {
        warnings.push(format!("{id}: render_cond is ignored: the host evaluates NVN conditional rendering for every game"));
    }
    let tfu = onoff(g, "tex_flush_upload", "off")?;
    let tdu = g.get_or("tex_dirty_upload", "off");
    if (tdu != "on" && tdu != "off") || (tdu == "on" && tfu != "on") {
        return fmt_err(format!(
            "{id}: tex_dirty_upload must be on or off, and needs tex_flush_upload on: {tdu}"
        ));
    }
    let tbc = onoff(g, "tex_bc_cpu", "off")?;
    let tds = onoff(g, "tex_dyn_slots", "off")?;
    let vcr = onoff(g, "vconv_range", "off")?;
    let mml = onoff(g, "mem_map_limit", "off")?;
    let dbr = choice(
        g,
        "draw_barriers",
        "full",
        &["full", "game"],
        "full or game",
    )?;
    let om = choice(
        g,
        "op_mode",
        "console",
        &["console", "handheld"],
        "console or handheld",
    )?;
    if pl == "touch" && om != "handheld" {
        return fmt_err(format!(
            "{id}: pad_layout touch needs op_mode handheld (the touch screen reaches the game only in handheld mode)"
        ));
    }
    let pm = choice(
        g,
        "perf_mode",
        "follow",
        &["follow", "boost"],
        "follow or boost",
    )?;
    let _dmo = choice(g, "depth_mode", "gl", &["gl", "zero"], "gl or zero")?;
    let api = g.get_or("device_api", "55.15");
    let api_parts = api
        .split_once('.')
        .filter(|(a, b)| is_digits(a) && is_digits(b));
    if api_parts.is_none() {
        return fmt_err(format!("{id}: device_api must be major.minor: {api}"));
    }

    let b = |x: bool| (x as u8).to_string();
    let ull = |v: i128| format!("{}ull", hex(v));
    let mut l: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, v: String| l.push((format!("DM_GAME_{k}"), v));
    add("ID", q(id));
    add("NAME", q(&g.get("name")?));
    let prog = {
        let p = g.get("program_id")?;
        let t = py_strip(&p);
        let t = t
            .strip_prefix("0x")
            .or_else(|| t.strip_prefix("0X"))
            .unwrap_or(t);
        i128::from_str_radix(&t.replace('_', ""), 16).map_err(|_| {
            Error::Format(format!(
                "invalid literal for int() with base 16: {}",
                py_repr(&p)
            ))
        })?
    };
    add("PROGRAM_ID", format!("0x{prog:016X}ull"));
    add(
        "MODULES",
        format!(
            "{{{}}}",
            mods.iter().map(|m| q(m)).collect::<Vec<_>>().join(", ")
        ),
    );
    add("NMOD", mods.len().to_string());
    let bm = g.build_modules()?;
    add(
        "BUILD_MODULES",
        if bm.is_empty() {
            "{0}".to_string()
        } else {
            bm.iter()
                .map(|(bid, ms)| format!("{{{}, {}}}", q(bid), q(&ms.join(" "))))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    add("ROMFS_MARKER", q(&g.get("romfs_marker")?));
    add("MEM_RESERVE", ull(g.int("mem_reserve")?));
    add("MEM_ALIAS", ull(g.int("mem_alias")?));
    add("MEM_STACK", ull(g.int("mem_stack")?));
    add("APP_POOL", ull(g.int("app_pool")?));
    add("MEM_USED", ull(int0(&g.get_or("mem_used", "0x4000000"))?));
    add(
        "SYS_RESOURCE",
        ull(int0(&g.get_or("sys_resource", "0x2000000"))?),
    );
    add("ARGS", q(&g.get_or("game_args", "")));
    add("ENV", q(&g.get_or("game_env", "")));
    add("LAUNCH_PARAM", q(&g.get_or("launch_param", "")));
    add("MEM_MAP_LIMIT", b(mml == "on"));
    add("STDALLOC_CAP", ull(int0(&g.get_or("stdalloc_cap", "0"))?));
    let sa = g.get_or("screen_aspect", "off");
    add(
        "SCREEN_ASPECT",
        match sa.as_str() {
            "off" => "0".into(),
            "on" => "1".into(),
            // the patched engine's tables: `wide` (stubs follow a screen wider than 16:9: Ocarina of Time) and `both` (taller or wider); game.py: ("off", "on", "wide", "both").index(...)
            "wide" => "2".into(),
            "both" => "3".into(),
            _ => return fmt_err(format!("{} is not in list", py_repr(&sa))),
        },
    );
    add("DISPLAY_VERSIONS", vers);
    add("PRESENT_DISPLAY", b(pi == "display"));
    add("CORE_PREEMPT", b(cp == "on"));
    add("ALLOC_BIG_ALIGN", b(aba == "on"));
    add("FPS_TARGET", ft.clone());
    add("FPS_MAX", fm.clone());
    add("FPS_HOLD", b(fh == "on"));
    add(
        "UE_TEXTURE_POOL",
        if utp.trim_start_matches('0').is_empty() {
            "0".to_string()
        } else {
            utp.trim_start_matches('0').to_string()
        },
    ); // int(utp)
    add("UE_FPS_CVAR", q(&ufc));
    add("PRESENT_SNAP", b(ps == "on"));
    add("PSO_MTL4", b(p4 == "on"));
    add("ENC_THREAD", b(et == "on"));
    add("NVN_PREFETCH", b(pf == "on"));
    if !g.get_or("shared_font", "").is_empty() {
        add("SHARED_FONT_TTF", b(sf == "ttf"));
    }
    add(
        "PAD_LAYOUT",
        ["simple", "full", "touch"]
            .iter()
            .position(|x| *x == pl)
            .unwrap()
            .to_string(),
    );
    add("OPMODE", b(om == "console"));
    add("PERF_BOOST", b(pm == "boost"));
    add("TEX_FLUSH_UPLOAD", b(tfu == "on"));
    add("SHADER_KEY_SKIP_OFF", hex(sko));
    add("SHADER_KEY_SKIP_LEN", hex(skl));
    add("TEX_DIRTY_UPLOAD", b(tdu == "on"));
    add("TEX_BC_CPU", b(tbc == "on"));
    add("TEX_DYN_SLOTS", b(tds == "on"));
    add("DRAW_BARRIERS_GAME", b(dbr == "game"));
    add("VCONV_RANGE", b(vcr == "on"));
    if !g.get_or("device_api", "").is_empty() {
        let (a, bb) = api_parts.unwrap();
        add("NVN_API_MAJOR", a.to_string());
        add("NVN_API_MINOR", bb.to_string());
    }
    Ok(l)
}

/// `val(macro)` of the `profile` subcommand: the header's C spelling -> the file's plain spelling.
fn val(m: &[(String, String)], macro_: &str) -> Option<String> {
    let name = format!("DM_GAME_{macro_}");
    let v = m
        .iter()
        .rev()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| py_strip(v).to_string())?;
    if v.starts_with('{') && v.ends_with('}') {
        // {"main", "sdk"} -> main sdk
        return Some(
            v[1..v.len() - 1]
                .split(',')
                .map(py_strip)
                .filter(|x| !x.is_empty())
                .map(|x| x.trim_matches('"').to_string())
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if v.starts_with('"') {
        return Some(
            v.chars()
                .skip(1)
                .take(v.chars().count().saturating_sub(2))
                .collect(),
        );
    }
    Some(match v.strip_suffix("ull") {
        Some(x) => x.to_string(),
        None => v,
    })
}

/// `re.findall(r'\{"([0-9a-f]+)", "([^"]*)"\}', s)`
fn display_versions(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i + 2 <= b.len() {
        if &b[i..i + 2] == b"{\"" {
            let mut j = i + 2;
            while j < b.len() && matches!(b[j], b'0'..=b'9' | b'a'..=b'f') {
                j += 1;
            }
            if j > i + 2 && s[j..].starts_with("\", \"") {
                let k = j + 4;
                if let Some(e) = s[k..].find('"')
                    && s[k + e..].starts_with("\"}")
                {
                    out.push((s[i + 2..j].to_string(), s[k..k + e].to_string()));
                    i = k + e + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// The runtime profile text and the warnings `game.py profile` prints to stderr.
#[derive(Debug)]
pub struct Rendered {
    pub text: String,
    pub warnings: Vec<String>,
}

const KEYS: [(&str, &str); 36] = [
    ("title_id", "ID"),
    ("program_id", "PROGRAM_ID"),
    ("name", "NAME"),
    ("romfs_marker", "ROMFS_MARKER"),
    ("modules", "MODULES"),
    ("mem_reserve", "MEM_RESERVE"),
    ("mem_alias", "MEM_ALIAS"),
    ("mem_stack", "MEM_STACK"),
    ("app_pool", "APP_POOL"),
    ("mem_used", "MEM_USED"),
    ("sys_resource", "SYS_RESOURCE"),
    ("mem_map_limit", "MEM_MAP_LIMIT"),
    ("present_interval", "PRESENT_DISPLAY"),
    ("core_preempt", "CORE_PREEMPT"),
    ("alloc_big_align", "ALLOC_BIG_ALIGN"),
    ("fps_target", "FPS_TARGET"),
    ("fps_hold", "FPS_HOLD"),
    ("present_snap", "PRESENT_SNAP"),
    ("pso_mtl4", "PSO_MTL4"),
    ("shared_font", "SHARED_FONT_TTF"),
    ("pad_layout", "PAD_LAYOUT"),
    ("op_mode", "OPMODE"),
    ("perf_mode", "PERF_BOOST"),
    ("feedback_snap", "FEEDBACK_SNAP"),
    ("tex_flush_upload", "TEX_FLUSH_UPLOAD"),
    ("tex_dirty_upload", "TEX_DIRTY_UPLOAD"),
    ("tex_dyn_slots", "TEX_DYN_SLOTS"),
    ("gpu_timestamps", "GPU_TS_SEGMENT"),
    ("vconv_range", "VCONV_RANGE"),
    ("stdalloc_cap", "STDALLOC_CAP"),
    ("tex_bc_cpu", "TEX_BC_CPU"),
    ("ue_texture_pool", "UE_TEXTURE_POOL"),
    ("enc_thread", "ENC_THREAD"),
    ("draw_barriers", "DRAW_BARRIERS_GAME"),
    ("game_args", "ARGS"),
    ("game_env", "ENV"),
];

/// `game.py profile <id>` as text.
pub fn render_profile(g: &Game) -> Result<Rendered> {
    let mut warnings = Vec::new();
    let m = header_macros(g, &mut warnings)?;
    let mut l: Vec<String> = vec![
        format!("# Runtime game profile of {} ({}), written by dm/games/game.py profile from {}/game.tsv", g.id, g.get("name")?, g.dir.display()),
        "# Read by the host from DM_PROFILE (hle_profile.c set_kv): KEY<TAB>VALUE, # starts a comment. Every value the compiled".into(),
        "# profile would hold is written, so the file alone describes the game.".into(),
    ];
    for (key, macro_) in KEYS.iter() {
        if let Some(v) = val(&m, macro_) {
            l.push(format!("{key}\t{v}"));
        }
    }
    let (maj, min) = (val(&m, "NVN_API_MAJOR"), val(&m, "NVN_API_MINOR"));
    l.push(match maj {
        Some(a) => format!(
            "device_api\t{a}.{}",
            min.filter(|x| !x.is_empty()).unwrap_or_else(|| "0".into())
        ),
        None => "device_api\t0.0".into(),
    });
    if val(&m, "SHARED_FONT_TTF").is_none() {
        l.push("shared_font\tauto".into());
    }
    l.push(format!(
        "shader_key_skip\t{}+{}",
        val(&m, "SHADER_KEY_SKIP_OFF").unwrap_or_default(),
        val(&m, "SHADER_KEY_SKIP_LEN").unwrap_or_default()
    ));
    let dv = m
        .iter()
        .rev()
        .find(|(k, _)| k == "DM_GAME_DISPLAY_VERSIONS")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    for (bid, d) in display_versions(&dv) {
        l.push(format!("display_version\t{bid}\t{d}"));
    }
    Ok(Rendered {
        text: l.join("\n") + "\n",
        warnings,
    })
}

/// The `profile` subcommand: write `out` only when its text changes (keeps the mtime). Returns the row count the
/// Python's `render()` logs (`len(read_text().splitlines())`).
pub fn write_profile(g: &Game, out: &Path) -> Result<(usize, Vec<String>)> {
    let r = render_profile(g)?;
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    if std::fs::read_to_string(out).ok().as_deref() != Some(r.text.as_str()) {
        std::fs::write(out, &r.text)?;
    }
    Ok((py_splitlines(&r.text).len(), r.warnings))
}

/// `find_dir(name)`: a path as is; else the first of `dirs` holding `<name>/game.tsv`; else `dirs[0]/name`
/// (`dm/games` first, then `DM_GAMES_DIR`; swap with `games_first` for `DM_GAMES_FIRST=1`).
pub fn find_dir(
    name: &str,
    games_dir: &Path,
    extra_dirs: &[PathBuf],
    games_first: bool,
) -> PathBuf {
    if name.contains('/') {
        return PathBuf::from(name);
    }
    let mut dirs: Vec<PathBuf> = vec![games_dir.to_path_buf()];
    if games_first {
        let mut d = extra_dirs.to_vec();
        d.push(games_dir.to_path_buf());
        dirs = d;
    } else {
        dirs.extend_from_slice(extra_dirs);
    }
    dirs.iter()
        .map(|d| d.join(name))
        .find(|d| d.join("game.tsv").exists())
        .unwrap_or_else(|| games_dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(text: &str) -> Game {
        Game::from_text(PathBuf::from("/x/0100AAAAAAAAAAAA"), text)
    }

    const MIN: &str = "# c\ntitle_id\t0100AAAAAAAAAAAA\nprogram_id\t0100AAAAAAAAAAAA\nname\tMy \"Game\"\nromfs_marker\tData/x\nmodules\tmain sdk\nmem_reserve\t0x40000000\nmem_alias\t0x10000000\nmem_stack\t0x10000000\napp_pool\t0xCD500000\nbuild\tv0\t1.0.0\tmain=abcd\tsdk=ef01\n";

    #[test]
    fn minimal_profile() {
        let r = render_profile(&game(MIN)).unwrap();
        let lines: Vec<&str> = r.text.lines().collect();
        assert!(
            lines[0].starts_with(
                "# Runtime game profile of 0100AAAAAAAAAAAA (My \"Game\"), written by"
            )
        );
        assert!(r.text.contains("\nname\tMy \\\"Game\\\"\n"), "{}", r.text); // the macro's escaping is kept, as in the Python
        assert!(r.text.contains("\nprogram_id\t0x0100AAAAAAAAAAAA\n"));
        assert!(r.text.contains("\nmodules\tmain sdk\n"));
        assert!(r.text.contains("\napp_pool\t0xcd500000\n"));
        assert!(r.text.contains("\nmem_used\t0x4000000\n"));
        assert!(r.text.contains("\nop_mode\t1\n"));
        assert!(r.text.contains("\ndevice_api\t0.0\n") && r.text.contains("\nshared_font\tauto\n"));
        assert!(
            r.text
                .ends_with("\nshader_key_skip\t0x0+0x0\ndisplay_version\tabcd\t1.0.0\n")
        );
    }

    #[test]
    fn validation_messages() {
        let bad = |extra: &str| {
            render_profile(&game(&format!("{MIN}{extra}")))
                .err()
                .map(|e| e.to_string())
        };
        assert_eq!(
            bad("fps_target\t45\n").unwrap(),
            "0100AAAAAAAAAAAA: fps_target must be 0, 30 or 60: 45"
        );
        assert_eq!(
            bad("pad_layout\ttouch\n").unwrap(),
            "0100AAAAAAAAAAAA: pad_layout touch needs op_mode handheld (the touch screen reaches the game only in handheld mode)"
        );
        assert!(
            bad("device_api\t55\n")
                .unwrap()
                .ends_with("device_api must be major.minor: 55")
        );
        assert!(
            bad("shader_key_skip\t0x10+4\n")
                .unwrap()
                .contains("shader_key_skip takes")
        );
        assert!(
            render_profile(&game("name\tx\n"))
                .unwrap_err()
                .to_string()
                .contains("has no key modules")
        );
    }

    #[test]
    fn device_api_and_font_and_skip() {
        let r = render_profile(&game(&format!("{MIN}device_api\t55.15\nshared_font\tttf\nshader_key_skip\t0x770+0x78\nop_mode\thandheld\npad_layout\ttouch\n"))).unwrap();
        assert!(
            r.text.contains("\ndevice_api\t55.15\n")
                && r.text.contains("\nshared_font\t1\n")
                && r.text.contains("\nshader_key_skip\t0x770+0x78\n")
        );
        assert!(r.text.contains("\nop_mode\t0\n") && r.text.contains("\npad_layout\t2\n"));
    }

    #[test]
    fn screen_aspect_takes_the_four_values_of_the_patched_engine() {
        // game.py: ("off", "on", "wide", "both").index(...): Ocarina of Time's table says `wide`, 01006F8002326000's `both`; the port knew only off / on and stopped stage 0 with "'wide' is not in list"
        let m = |v: &str| {
            header_macros(
                &game(&format!("{MIN}screen_aspect\t{v}\n")),
                &mut Vec::new(),
            )
            .map(|m| {
                m.iter()
                    .find(|(k, _)| k == "DM_GAME_SCREEN_ASPECT")
                    .map(|(_, v)| v.clone())
                    .unwrap()
            })
        };
        assert_eq!(m("off").unwrap(), "0");
        assert_eq!(m("on").unwrap(), "1");
        assert_eq!(m("wide").unwrap(), "2");
        assert_eq!(m("both").unwrap(), "3");
        assert_eq!(m("tall").unwrap_err().to_string(), "'tall' is not in list");
        let r = render_profile(&game(&format!("{MIN}screen_aspect\twide\n"))).unwrap();
        assert!(
            r.text.contains("\nscreen_aspect\t2\n") || !r.text.contains("screen_aspect"),
            "{}",
            r.text
        );
    }

    #[test]
    fn display_versions_regex() {
        assert_eq!(
            display_versions("{\"ab12\", \"1.0\"}, {\"cd\", \"x\\\"y\"}, {\"ef\", \"2\"}"),
            vec![
                ("ab12".to_string(), "1.0".to_string()),
                ("ef".to_string(), "2".to_string())
            ]
        );
    }
}
