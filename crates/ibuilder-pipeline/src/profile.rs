//! Everything the chain decides about a game's runtime profile and address space around the stage workers: the modules the profile loads, the first-boot heap
//! sizing (`presize_heap`, trap 46), the engine's heap check done by the phone's own last boot (`apply_heap_check`, GAME-APP.md §8.9, with the engine's
//! `heap_signals` / `depth_signal`), the alias region sized from the address space (trap 47), the address-space check (trap 44), the save and temp folders the
//! engine aborts without (trap 32), the share of the pipeline prewarm built before the first frame (about 500 lines, `DM_PSO_PREPARE_PERCENT`), the previous build's
//! leftovers (trap 39) and the per-game icon.

use crate::ctx::Ctx;
use crate::tsv::{Game, game_table_dir, last_guest_run, profile_hex, table_int};
use crate::util::{
    GIB, folder_bytes, is_dir, is_file, list_names, parse_int0, read_text, rmtree, round_up,
    write_atomic,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The Switch's application pool in the default memory mode (what the host logs as `pool`).
pub const APP_POOL_DEFAULT: i128 = 0xcd50_0000;
pub const ALIAS_TARGET: i128 = 48 << 30;
pub const ALIAS_MARGIN: i128 = 12 << 30;
/// `HEAP_CHECK_STDALLOC` in `profile_guess.py`: a starting point, not a measured minimum.
pub const HEAP_CHECK_STDALLOC: i128 = 0x4000_0000;

/// `game_table_dir` for this run's game.
pub fn table_dir(ctx: &Ctx, tid: &str) -> Option<PathBuf> {
    game_table_dir(&ctx.docs.profile_derived(), &ctx.bundle, tid)
}

/// `profile_modules`: the modules the game's profile loads (its `modules` row), or `None`: the config's `modules` was a fixed "main subsdk0 sdk" for every game,
/// while the engine's profile drops a module the game does not use and adds subsdk1.. or NRO-driven ones.
pub fn profile_modules(ctx: &Ctx) -> Option<String> {
    let tid = ctx.title_id()?;
    let d = table_dir(ctx, &tid)?;
    let text = read_text(&d.join("game.tsv")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("modules\t") {
            let mods: Vec<&str> = rest
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect();
            return if mods.is_empty() {
                None
            } else {
                Some(mods.join(" "))
            };
        }
    }
    None
}

// ---- the engine's heap check (dm/games/profile_guess.py) ------------------------------------------------------------------------------------

/// `profile_guess.heap_signals(log, g)`: `{key: (value, reason)}` from a heap-check run's host log (hle_svc.c SetHeapSize); `g` is the profile the run used.
/// `Err` is the KeyError Python raised for a table without `mem_reserve` / `mem_stack` / `mem_alias` / `app_pool`.
pub fn heap_signals(log: &str, g: &Game) -> Result<BTreeMap<String, (String, String)>, String> {
    let mut out = BTreeMap::new();
    let req: Vec<i128> = find_hex_after(log, "SetHeapSize OOM: req 0x");
    if let Some(&maxreq) = req.iter().max() {
        let reserve = g.int("mem_reserve")?;
        let stack = g.int("mem_stack")?;
        let heap = round_up(maxreq, 0x800_0000);
        let alias = reserve - 0x1000_0000 - stack - heap;
        let gb = maxreq as f64 / GIB as f64;
        if alias >= 0x4000_0000 {
            out.insert(
                "mem_alias".to_string(),
                (
                    format!("{alias:#x}"),
                    format!(
                        "the game asked for a {gb:.2} GB heap: a {:.3} GB heap region",
                        heap as f64 / GIB as f64
                    ),
                ),
            );
        } else {
            // not even 1 GB left for the alias region: the block grows around the alias region the profile has (at least 1 GB), which mem_reserve alone leaves as it
            // is, so the heap region gets the whole request (Ocarina of Time, issue #1361)
            let keep = g.int("mem_alias")?.max(0x4000_0000);
            out.insert(
                "mem_reserve".to_string(),
                (
                    format!("{:#x}", 0x1000_0000 + keep + stack + heap),
                    format!("the game asked for a {gb:.2} GB heap"),
                ),
            );
        }
    }
    let top: Vec<f64> = find_alias_tops(log);
    let alias_gb = g.int("mem_alias")? as f64 / GIB as f64;
    if req.is_empty()
        && !log.contains("[mem] heap ")
        && g.int_or("sys_resource", "0")? != 0
        && !top.is_empty()
        && top.iter().cloned().fold(f64::MIN, f64::max) > alias_gb - 0.25
        && g.int("app_pool")? > HEAP_CHECK_STDALLOC
        && g.int_or("stdalloc_cap", "0")? == 0
    {
        let tmax = top.iter().cloned().fold(f64::MIN, f64::max);
        out.insert(
            "stdalloc_cap".to_string(),
            (
                format!("{HEAP_CHECK_STDALLOC:#x}"),
                format!(
                    "no heap set and the alias region used up to +{tmax:.2} GB of {alias_gb:.2} GB in the heap check: 1 GB of address space for StandardAllocator, so its region leaves the game room for its own, while the pool the game reads as its memory stays the hardware's"
                ),
            ),
        );
    }
    Ok(out)
}

/// `profile_guess.depth_signal(log, g)`: `{'depth_mode': (value, reason)}` when a check boot's host log shows the game setting a depth mode the profile does not
/// say. Only the game's own calls count (the last `[nvn] depth mode near=` one). No call means unknown, not GL.
pub fn depth_signal(log: &str, g: &Game) -> BTreeMap<String, (String, String)> {
    let mut out = BTreeMap::new();
    const P: &str = "[nvn] depth mode near=";
    let mut last: Option<&str> = None;
    let mut from = 0;
    while let Some(i) = log[from..].find(P) {
        let s = from + i + P.len();
        let rest = &log[s..];
        let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
        if rest.starts_with('0') && !rest.as_bytes().get(1).map(|c| word(*c)).unwrap_or(false) {
            last = Some("0");
        } else if rest.starts_with("-w")
            && !rest.as_bytes().get(2).map(|c| word(*c)).unwrap_or(false)
        {
            last = Some("-w");
        }
        from = s;
    }
    let Some(last) = last else { return out };
    let seen = if last == "0" { "zero" } else { "gl" };
    if seen == g.get_or("depth_mode", "gl") {
        return out;
    }
    out.insert(
        "depth_mode".to_string(),
        (
            seen.to_string(),
            format!(
                "the game calls nvnDeviceSetDepthMode with near = {} in the check boot: the pipeline bases made before it runs are made for that mode",
                if seen == "zero" { "0" } else { "-w" }
            ),
        ),
    );
    out
}

/// every `<prefix><hex digits>` (lower-case) in `log`
fn find_hex_after(log: &str, prefix: &str) -> Vec<i128> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = log[from..].find(prefix) {
        let s = from + i + prefix.len();
        let digits: String = log[s..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || ('a'..='f').contains(c))
            .collect();
        if !digits.is_empty()
            && let Ok(v) = i128::from_str_radix(&digits, 16)
        {
            out.push(v);
        }
        from = s;
    }
    out
}

/// every `[mem] alias region mapped up to +<n.n> GB`
fn find_alias_tops(log: &str) -> Vec<f64> {
    const P: &str = "[mem] alias region mapped up to +";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = log[from..].find(P) {
        let s = from + i + P.len();
        let num: String = log[s..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if log[s + num.len()..].starts_with(" GB")
            && let Ok(v) = num.parse::<f64>()
        {
            out.push(v);
        }
        from = s;
    }
    out
}

fn same(a: &str, b: &str) -> bool {
    match (parse_int0(a), parse_int0(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Render `profile.tsv` again after the game table changed (`pd.render` with `DM_GAMES_DIR` naming the derived folder's parent).
fn render_profile(ctx: &Ctx, pdir: &Path, tid: &str) -> Result<(), String> {
    ctx.env.set(
        "DM_GAMES_DIR",
        &pdir.parent().unwrap_or(Path::new("")).to_string_lossy(),
    );
    let profile = ctx
        .cfg
        .get("profile")
        .map(PathBuf::from)
        .ok_or("no profile in the config")?;
    ctx.adapters
        .profile
        .render(&ctx.view(), tid, &profile)
        .map_err(|e| e.to_string())
}

/// `presize_heap`: before a game's FIRST boot on this device, its heap region is sized for the largest heap ANY game can ask for: its application pool (a heap
/// comes out of the pool, so no SetHeapSize can be larger). The provisional profile's layout gave the heap 128 MB and a large alias region; a game that sets a big
/// heap (South Park: 3.14 GB) stopped on `SetHeapSize OOM` at its first boot and played only after the heap check fixed the profile at the next launch. With room in
/// the address space (an iPhone on iOS 27 leaves ~64 GB in one hole) the block grows around the alias region the profile has, so a game that maps its memory instead
/// (Unity) keeps every byte it had; without that room, only a game whose survey found `main sets a heap` gets the heap region out of the alias region. Only for a
/// provisional profile, only when no boot of the game has been seen and no heap check ran (trap 46). Returns {key: value} set.
pub fn presize_heap(ctx: &Ctx) -> BTreeMap<String, i128> {
    let none = BTreeMap::new();
    let Some(tid) = ctx.title_id() else {
        return none;
    };
    let pdir = ctx.docs.derived_dir(&tid);
    let gtsv = pdir.join("game.tsv");
    if !is_file(&gtsv) || !last_guest_run(&ctx.docs.guest_log(), &tid).is_empty() {
        return none;
    }
    let Ok(text) = read_text(&gtsv) else {
        return none;
    };
    if text.contains("# heap check") || text.contains("# first boot heap") {
        return none;
    }
    let reserve = table_int(&text, "mem_reserve", 0);
    let alias = table_int(&text, "mem_alias", 0);
    let stack = table_int(&text, "mem_stack", 0x1000_0000);
    if reserve == 0 || alias == 0 {
        return none;
    }
    let pool = match table_int(&text, "app_pool", 0) {
        0 => APP_POOL_DEFAULT,
        p => p,
    };
    let heap_now = reserve - 0x1000_0000 - stack - alias;
    let heap = round_up(pool, 0x800_0000);
    if heap_now >= heap {
        return none;
    }
    let grown = 0x1000_0000 + alias + stack + heap;
    let largest = ctx
        .mem
        .address_space()
        .map(|a| a.largest as i128)
        .unwrap_or(0);
    let gb = |v: i128| v as f64 / GIB as f64;
    let mut sets: Vec<(&str, i128, String)> = Vec::new();
    if largest >= grown + (8i128 << 30) {
        // Metal and the rest still need addresses beside the block (hle_svc.c: "Metal etc. still need address space")
        sets.push((
            "mem_reserve",
            grown,
            format!("a {:.2} GB heap region (the {:.2} GB pool) beside the {:.3} GB alias region; {:.1} GB free in one piece", gb(heap), gb(pool), gb(alias), gb(largest)),
        ));
    } else if text.contains("sets a heap") && reserve - 0x1000_0000 - stack - heap >= 0x4000_0000 {
        sets.push((
            "mem_alias",
            reserve - 0x1000_0000 - stack - heap,
            format!("main sets a heap: a {:.2} GB heap region (the pool), the rest of the {:.2} GB block for the alias region", gb(heap), gb(reserve)),
        ));
    }
    if sets.is_empty() {
        return none;
    }
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    for (name, value, why) in &sets {
        for line in lines.iter_mut() {
            if line.split('\t').next().unwrap_or("").trim() == *name && !line.starts_with('#') {
                *line = format!("{name}\t{value:#x}");
            }
        }
        lines.push(format!(
            "# first boot heap (this device): {name} {value:#x}: {why}"
        ));
        ctx.log(&format!("first boot heap: {name} {value:#x} -- {why}"));
    }
    if let Err(e) = write_atomic(&gtsv, (lines.join("\n") + "\n").as_bytes()) {
        ctx.log(&format!(
            "game table: could not write {}: {e}",
            gtsv.display()
        ));
    }
    if let Err(e) = render_profile(ctx, &pdir, &tid) {
        // as apply_heap_check: the table keeps the keys, the boot after the next render takes them
        ctx.log(&format!(
            "first boot heap: [RED] the profile could not be rendered again: {e}"
        ));
    }
    sets.into_iter()
        .map(|(k, v, _)| (k.to_string(), v))
        .collect()
}

/// `apply_heap_check`: the engine's heap check, done by the boots themselves (docs/GAME-APP.md §8.9). The NSP script on a Mac boots a game with a provisional profile
/// until its first heap request and turns what the host logs into profile keys (mem_alias / mem_reserve when the heap did not fit, app_pool for a Unity game with no
/// heap, depth_mode). Here the phone's own last boot of the game is that run: if its guest log has a signal the profile does not already follow, the derived profile
/// (`Documents/profile-derived/<id>/game.tsv`) takes the keys, under a `# heap check` note, and profile.tsv is rendered again, so the next boot has the layout the game
/// asked for. Returns the keys it changed ({} for none).
pub fn apply_heap_check(ctx: &Ctx) -> BTreeMap<String, String> {
    let none = BTreeMap::new();
    let Some(tid) = ctx.title_id() else {
        return none;
    };
    let run = last_guest_run(&ctx.docs.guest_log(), &tid);
    let src = table_dir(ctx, &tid);
    let (false, Some(src)) = (run.is_empty(), src) else {
        return none;
    };
    let pdir = ctx.docs.derived_dir(&tid);
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if real(&src) != real(&pdir) {
        // a registered game: its corrected table becomes this app's own
        let _ = fs::create_dir_all(&pdir);
        let _ = fs::copy(src.join("game.tsv"), pdir.join("game.tsv"));
    }
    let gtsv = pdir.join("game.tsv");
    let sig: BTreeMap<String, (String, String)> = {
        let g = match Game::load(&pdir) {
            Ok(g) => g,
            Err(e) => {
                ctx.log(&format!(
                    "heap check: [RED] could not read the last boot against the profile: {e}"
                ));
                return none;
            }
        };
        match heap_signals(&run, &g) {
            Ok(h) => {
                let mut sig: BTreeMap<String, (String, String)> = h
                    .into_iter()
                    .filter(|(k, v)| !same(&g.get_or(k, ""), &v.0))
                    .collect();
                for (k, v) in depth_signal(&run, &g) {
                    if g.get_or(&k, "") != v.0 {
                        sig.insert(k, v);
                    }
                }
                sig
            }
            Err(e) => {
                ctx.log(&format!(
                    "heap check: [RED] could not read the last boot against the profile: {e}"
                ));
                return none;
            }
        }
    };
    if sig.is_empty() {
        return none;
    }
    let Ok(text) = read_text(&gtsv) else {
        return none;
    };
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut done = std::collections::BTreeSet::new();
    for line in lines.iter_mut() {
        let key = line.split('\t').next().unwrap_or("").trim().to_string();
        if let Some((v, _)) = sig.get(&key)
            && !line.starts_with('#')
        {
            *line = format!("{key}\t{v}");
            done.insert(key);
        }
    }
    for (key, (value, why)) in &sig {
        lines.push(format!("# heap check (this device): {key} {value}: {why}"));
        if !done.contains(key) {
            lines.push(format!("{key}\t{value}"));
        }
    }
    if let Err(e) = write_atomic(&gtsv, (lines.join("\n") + "\n").as_bytes()) {
        ctx.log(&format!(
            "game table: could not write {}: {e}",
            gtsv.display()
        ));
    }
    for (key, (value, why)) in &sig {
        ctx.log(&format!("heap check: {key} {value} -- {why}"));
    }
    match render_profile(ctx, &pdir, &tid) {
        Ok(()) => ctx.log("heap check: profile.tsv rendered again from the corrected profile: the next boot has the layout the game asked for"),
        Err(e) => ctx.log(&format!("heap check: [RED] the corrected profile could not be rendered: {e}")),
    }
    sig.into_iter().map(|(k, (v, _))| (k, v)).collect()
}

// ---- the address space -----------------------------------------------------------------------------------------------------------------

/// `set_alias_size`: the alias region sized from the address space this launch has, not only from the profile (trap 47). On a Switch it is 64 GB and games lay their
/// own regions out for that much; an iPhone used to leave an app ~4 GB of addresses, so the engine gives it the profile's (4.375 GB for a provisional one), and a
/// game that reserves more address space than that gets nothing back and writes through the failure (South Park: The Fractured but Whole: an 8 GB region in its static
/// constructors, `CRASH sig=11 addr=0x20000ffff`). On iOS 27 an app has ~64 GB in one hole below 2^39: DM_ALIAS_SIZE (hle_svc.c, an engine switch) makes the alias
/// region up to 48 GB of it, keeping 12 GB beside the block. Addresses only: the memory pool still bounds what is mapped. Not when the block was taken at launch
/// (DM_AS_AT: its size is fixed) or the switch is set.
pub fn set_alias_size(ctx: &Ctx) -> Option<i128> {
    if ctx.env.truthy("DM_AS_AT") || ctx.env.truthy("DM_ALIAS_SIZE") {
        return None;
    }
    let largest = ctx.mem.address_space()?.largest as i128;
    let profile = PathBuf::from(ctx.cfg.get_or("profile", ""));
    let reserve = profile_hex(&profile, "mem_reserve");
    let alias = profile_hex(&profile, "mem_alias");
    if reserve == 0 || alias == 0 || alias >= reserve {
        return None;
    }
    let rest = reserve - alias;
    let want = std::cmp::min(ALIAS_TARGET, largest - rest - ALIAS_MARGIN) & !((1i128 << 30) - 1);
    let g = |v: i128| v as f64 / GIB as f64;
    if want <= alias {
        ctx.log(&format!("alias region: the profile's {:.3} GB ({:.1} GB free in one piece leaves no room for more)", g(alias), g(largest)));
        return None;
    }
    ctx.env.set("DM_ALIAS_SIZE", &format!("{want:#x}"));
    ctx.log(&format!(
        "alias region: {:.0} GB instead of the profile's {:.3} GB (DM_ALIAS_SIZE {want:#x}; {:.1} GB free in one piece): a game that reserves address space as on a Switch gets it",
        g(want),
        g(alias),
        g(largest)
    ));
    Some(want)
}

/// What `check_address_space` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceCheck {
    pub extended: bool,
    pub free: u64,
    pub largest: u64,
    pub reserve: i128,
}

/// `check_address_space`: the guest's block (the profile's `mem_reserve`) against the address space this process has (trap 44). Without Extended Virtual Addressing an
/// iPhone leaves an app about 4 GB of free addresses, in pieces; a game whose profile reserves more (Ocarina of Time: 12.5 GB) stops at startup ("reserve guest AS").
/// Says so in plain words before the boot instead of after it. `None` off the device.
pub fn check_address_space(ctx: &Ctx) -> Option<SpaceCheck> {
    let a = ctx.mem.address_space()?;
    let profile = PathBuf::from(ctx.cfg.get_or("profile", ""));
    let mut reserve = profile_hex(&profile, "mem_reserve");
    if ctx.env.truthy("DM_ALIAS_SIZE") && reserve != 0 {
        // hle_svc.c: the block is mem_reserve - mem_alias + the alias size
        if let Some(sz) = ctx.env.get("DM_ALIAS_SIZE").and_then(|s| parse_int0(&s)) {
            reserve = reserve - profile_hex(&profile, "mem_alias") + sz;
        }
    }
    let g = |v: f64| v / GIB as f64;
    ctx.log(&format!(
        "address space: {}, {:.2} GB free, the largest hole {:.2} GB; the profile reserves {:.2} GB{}",
        if a.extended { "EXTENDED (Extended Virtual Addressing in this app's signature)" } else { "standard" },
        g(a.free as f64),
        g(a.largest as f64),
        g(reserve as f64),
        if ctx.env.truthy("DM_AS_AT") { " (taken at launch: DM_AS_AT)" } else { "" }
    ));
    if reserve != 0 && !ctx.env.truthy("DM_AS_AT") && reserve > a.largest as i128 {
        if a.extended {
            ctx.log(&format!(
                "address space: [RED] even the extended address space has no {:.2} GB hole now ({:.2} GB): the boot may stop on \"reserve guest AS\"",
                g(reserve as f64),
                g(a.largest as f64)
            ));
        } else {
            ctx.log(&format!(
                "address space: [RED] this game reserves {:.2} GB of addresses and this app has {:.2} GB in one piece: it will most likely stop at startup (\"reserve guest AS\"). It needs the extended-addressing build of GameApp (GameApp-…-with-eva.ipa), installed with a certificate that allows Extended Virtual Addressing (a paid developer account; a free Apple ID cannot)",
                g(reserve as f64),
                g(a.largest as f64)
            ));
        }
    }
    Some(SpaceCheck {
        extended: a.extended,
        free: a.free,
        largest: a.largest,
        reserve,
    })
}

/// `set_game_env`: the folders the engine's own iOS front makes and names before the guest runs (dm/ios/Sources/main.m set_data_env): DM_SAVE and DM_TEMP, under
/// `Documents/games/<title id>/` (Documents/ itself when the profile names no title). Without DM_SAVE the host does not answer "no save data": h_fs_MountSave calls
/// die() the moment the game mounts its save (device runs of 2026-11-17, pids 8546 and 8549). The folders are created here: the engine mkdirs a missing save only for
/// the read-only mount. A value already in the environment wins (a harness may point somewhere else).
pub fn set_game_env(ctx: &Ctx) {
    let profile = ctx.cfg.get_or("profile", "");
    let tid = if profile.is_empty() {
        None
    } else {
        crate::tsv::profile_title_id(Path::new(profile))
    };
    let base = match &tid {
        Some(t) => ctx.docs.game_dir(t),
        None => ctx.docs.root().to_path_buf(),
    };
    // A Switch 2 game's DLC (extract_nup): aoc/<index>.img, the host's add-on content folder (hle_fs.c DM_AOC_DIR).
    let aoc = base.join("aoc");
    if !ctx.env.truthy("DM_AOC_DIR") && is_dir(&aoc) {
        let n = list_names(&aoc)
            .iter()
            .filter(|n| n.ends_with(".img"))
            .count();
        if n > 0 {
            ctx.env.set("DM_AOC_DIR", &aoc.to_string_lossy());
            ctx.log(&format!(
                "game env: DM_AOC_DIR={} ({n} add-on content image(s))",
                aoc.display()
            ));
        }
    }
    // The shared-font stand-ins (dm/font/: standard.ttf, the Chinese/Korean cuts, nintendo_ext.ttf): the engine's note "Shared fonts on iOS" (docs/reference/services.md, found on every game on iOS):
    // without them `GetSharedFontLoadState` says "not loaded" and a game that waits for the font never gets past its boot (device log, 2026-11-19: Mario Kart 8 Deluxe, black screen,
    // "[RED] cannot open the standard shared font"). The app's bundle carries them in dm/font/ (scripts/build-app.sh); a DM_FONT_DIR already set (a harness, env.txt) wins.
    let fonts = ctx.bundle.join("dm").join("font");
    if !ctx.env.truthy("DM_FONT_DIR") && is_dir(&fonts) && is_file(&fonts.join("standard.ttf")) {
        ctx.env.set("DM_FONT_DIR", &fonts.to_string_lossy());
        ctx.log(&format!(
            "game env: DM_FONT_DIR={} (the shared-font stand-ins)",
            fonts.display()
        ));
    }
    crate::diag::host_env(ctx);
    // Libraries and pipelines the game needs and the machine has not compiled yet: by default the host builds them in the background and SKIPS the draws that need them ("its draws skipped meanwhile"),
    // which on a first visit of a track leaves the whole 3D scene black while the HUD draws (device log, 2026-11-19, Mario Kart 8 Deluxe: 112 libraries compiled from source in 15.6 s at the start of a race).
    // A game that is visible and hitches for a few seconds is better than one that cannot be seen: DM_PSO_NOWAIT=0 makes the draw WAIT for its pipeline. `pso_nowait=1` in the config, or a DM_PSO_NOWAIT
    // already set (a harness, env.txt), keeps the host's own choice; once "Prepare the shaders" has compiled the game's libraries nothing is waited for either way.
    set_pso_nowait(ctx);
    set_pso_prepare_percent(ctx);
    for (var, leaf) in [("DM_SAVE", "save"), ("DM_TEMP", "temp")] {
        if let Some(have) = ctx.env.get(var).filter(|s| !s.is_empty()) {
            ctx.log(&format!("game env: {var} was already set to {have}"));
            continue;
        }
        let path = base.join(leaf);
        if let Err(e) = fs::create_dir_all(&path) {
            ctx.log(&format!("game env: could not create {} for {var} ({e}) -- the guest will abort when it mounts its save", path.display()));
            continue;
        }
        ctx.env.set(var, &path.to_string_lossy());
        ctx.log(&format!("game env: {var}={}", path.display()));
    }
}

/// A seed list longer than this is only partly built before the first frame ([`PSO_PREPARE_HEAD_LINES`]) and leaves thousands of pipelines and libraries to the game itself: there a draw
/// that WAITS for its pipeline froze the game for minutes (device log 2026-11-20, Ocarina of Time, 25,163 seed lines: frame 4342 waited 161 s for 262 libraries, 942 s of host-blocked frames in a
/// 44 min session). So for such a game the host keeps its own policy (`DM_PSO_NOWAIT=1`, the engine's default on iOS: a frame's draws wait at most `DM_PSO_NOWAIT_MS`, 50 ms, in all, and a draw
/// whose pipeline is still building is skipped until it is built in the background). Among the 177 seed lists of the engine tree only Ocarina's is longer; Mario Kart 8 Deluxe (1,516) keeps the wait.
pub const PSO_NOWAIT_BIG_LIST_LINES: u64 = 10_000;

/// `DM_PSO_NOWAIT`: a draw waits for its pipeline (`0`) unless the player, the config or a long seed list chose otherwise. An `env.txt` / harness `1` or `pso_nowait=1` in the config leaves the
/// host's policy; an explicit `0` (environment or `pso_nowait=0`) is kept whatever the list; otherwise a seed list over [`PSO_NOWAIT_BIG_LIST_LINES`] leaves the host's policy and any other game waits.
fn set_pso_nowait(ctx: &Ctx) {
    const VAR: &str = "DM_PSO_NOWAIT";
    if ctx.env.truthy(VAR) || ctx.cfg.get_or("pso_nowait", "0") == "1" {
        return;
    }
    let explicit = ctx.env.get(VAR).filter(|s| !s.is_empty()).is_some()
        || ctx
            .cfg
            .get_nonempty("pso_nowait")
            .is_some_and(|v| v.trim() == "0");
    if !explicit
        && let Some(lines) = pso_seed_list_path(ctx)
            .ok()
            .and_then(|p| seed_list_lines(&p).ok())
            .filter(|&n| n > PSO_NOWAIT_BIG_LIST_LINES)
    {
        ctx.log(&format!(
            "game env: {VAR} left to the host (the seed list holds {lines} lines, more than {PSO_NOWAIT_BIG_LIST_LINES}: a draw waits at most 50 ms a frame for its pipeline and is skipped until it is built, instead of freezing the game; pso_nowait=0 in the config makes it wait)"
        ));
        return;
    }
    ctx.env.set(VAR, "0");
    ctx.log("game env: DM_PSO_NOWAIT=0 (a draw waits for its pipeline instead of being skipped: a scene is never left black while it compiles)");
}

/// What [`set_pso_nowait`] will choose for game `tid` when neither the player's row nor `env.txt` nor the environment chose: `"1"` (the host's policy: a draw waits at most 50 ms a frame,
/// then is skipped) for `pso_nowait=1` in the config or a seed list over [`PSO_NOWAIT_BIG_LIST_LINES`], else `"0"` (a draw waits). The settings sheet's row shows this as its default
/// (`settings::write_launcher_settings`), so the sheet says what the game does and a player can choose the other value (the app writes only a value that differs from the default;
/// review r4 follow-up B3). The seed list is the pack's, or the one the pack will get (`shaderpack::top_up_pack`: the derived folder's, the bundle's for a registered game).
pub fn pso_nowait_default(ctx: &Ctx, tid: &str) -> &'static str {
    match ctx.cfg.get_nonempty("pso_nowait").map(str::trim) {
        Some("1") => return "1",
        Some("0") => return "0",
        _ => {}
    }
    let candidates = [
        ctx.shader_packs.join(tid).join("pso_manifest.tsv"),
        ctx.docs.derived_dir(tid).join("pso_manifest.tsv"),
        ctx.bundle
            .join("dm")
            .join("games")
            .join(tid)
            .join("pso_manifest.tsv"),
    ];
    let lines = candidates
        .iter()
        .find(|p| is_file(p))
        .and_then(|p| seed_list_lines(p).ok())
        .unwrap_or(0);
    if lines > PSO_NOWAIT_BIG_LIST_LINES {
        "1"
    } else {
        "0"
    }
}

// ---- the pipeline prewarm's share before the first frame -------------------------------------------------------------------------------------

/// The number of prewarm lines the host builds BEFORE the first frame when the seed list is longer: the owner's decision of 2026-11-20 ("limit it to 500 shaders"). Field log behind it
/// (iPhone 15 Pro, Ocarina of Time, 25,163 seed lines): with 50% before the first frame every Play compiled 13,066 lines (9,124 libraries) in 1,061-1,708 s, and the system's Metal compile cache
/// kept only about 5,500-5,800 of them between sessions, so that wait came back at every Play. The rest of the lists is built by the host's background pass while the game runs (engine patch 0412's
/// tail), and a draw that needs a pipeline not built yet waits for it (`DM_PSO_NOWAIT=0`; for a list over [`PSO_NOWAIT_BIG_LIST_LINES`] at most 50 ms a frame, see [`set_pso_nowait`]).
pub const PSO_PREPARE_HEAD_LINES: u64 = 500;

/// The share the host takes for a seed list of `lines` data lines so that about [`PSO_PREPARE_HEAD_LINES`] of them are built before the first frame: `None` when the list is not longer than that
/// (all of it, the host's default), else `ceil(500 * 100 / lines)` within 1..=99. The host's knob is a whole percent of its TASKS (a render task stands for one or two lines), so the head is about
/// 500 lines, not exactly: 25,163 lines give 2 (about 500 lines), 1,516 give 33 (about 500), and a list above 50,000 lines cannot go under 1% (about 1% of it).
pub fn pso_head_percent(lines: u64) -> Option<u32> {
    if lines <= PSO_PREPARE_HEAD_LINES {
        return None;
    }
    let p = (PSO_PREPARE_HEAD_LINES * 100).div_ceil(lines);
    Some(p.clamp(1, 99) as u32)
}

/// The data lines of a prewarm list, counted the way the host counts them (nvn_mtl.m `warm_parse`: `pipeline prewarm list <path>: N lines`): a line is what ends in `\n` (or the file's last
/// piece), and it counts unless it is empty or starts with `#`. Streamed through a fixed 32 KiB buffer and compared byte by byte, so nothing grows with the file, a line of any length or any bytes
/// (the file is not required to be UTF-8, which the host's own reader is) costs the same, and a damaged file is at worst a long read, never an allocation. `Err` for a file that cannot be opened or read, and for a path that is not a file.
pub fn seed_list_lines(path: &Path) -> std::io::Result<u64> {
    use std::io::Read;
    if !fs::metadata(path)?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "it is not a regular file",
        ));
    }
    let mut f = fs::File::open(path)?;
    let mut buf = [0u8; 32 * 1024];
    let mut lines = 0u64;
    let mut at_start = true; // the next byte is the first of a line
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => return Ok(lines),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        for &b in &buf[..n] {
            if b == b'\n' {
                at_start = true;
            } else if at_start {
                at_start = false;
                if b != b'#' {
                    lines = lines.saturating_add(1);
                }
            }
        }
    }
}

/// The seed list the host reads, by its own rule (`warm_paths`): `DM_PSO_MANIFEST=<file>` alone, else `pso_manifest.tsv` BESIDE THE SHADER MAP (`DM_NVN_SHMAP`, which `set_shader_pack` /
/// `set_shader_map` have set by now: the game's pack folder `Documents/shaders/<id>/`, or the folder of the derived map when there is no pack). Without a shader map in the environment, the
/// game's pack folder. `Err` carries the reason there is nothing to count (`DM_PSO_MANIFEST=off`: no list is read at all; no title).
fn pso_seed_list_path(ctx: &Ctx) -> Result<PathBuf, String> {
    match ctx.env.get("DM_PSO_MANIFEST").filter(|s| !s.is_empty()) {
        Some(v) if v == "off" => {
            return Err("DM_PSO_MANIFEST=off: the host reads no prewarm list".to_string());
        }
        Some(v) => return Ok(PathBuf::from(v)),
        None => {}
    }
    if let Some(map) = ctx.env.get("DM_NVN_SHMAP").filter(|s| !s.is_empty()) {
        return Ok(Path::new(&map)
            .parent()
            .unwrap_or(Path::new(""))
            .join("pso_manifest.tsv"));
    }
    let tid = ctx.title_id().ok_or_else(|| "no shader map is set and the profile names no title id: there is no pack folder to look in".to_string())?;
    Ok(ctx.shader_packs.join(tid).join("pso_manifest.tsv"))
}

/// "half" for 50, else "<n>%": the log's words for a share.
fn share_words(percent: u32) -> String {
    if percent == 50 {
        "half".to_string()
    } else {
        format!("{percent}%")
    }
}

/// `DM_PSO_PREPARE_PERCENT` (engine patch 0412): how much of the prewarm lists (this device's own `pso_user.tsv` first, then the profile's seed list: engine patch 0413) the host builds BEFORE the first frame; the rest goes
/// to a background pass while the game runs. The app aims at about [`PSO_PREPARE_HEAD_LINES`] lines before the first frame (see [`pso_head_percent`]), whatever the game: the first Play starts in
/// seconds to a couple of minutes instead of half an hour for Ocarina of Time (25,163 seed lines, 13,066 of them built before the first frame at 50%: 1,061-1,708 s on an iPhone 15 Pro).
///
/// The choice, in this order (each outcome is one log line):
///   1. a `DM_PSO_PREPARE_PERCENT` already in the environment wins (an env.txt line, the settings, a harness): left alone, as `DM_PSO_NOWAIT` and `DM_FONT_DIR`;
///   2. `pso_prepare_percent=<n>` in the config, a whole number 1..=100: `100` leaves the host's default, any other `n` is set (whatever the length of the list: it is the player's or the harness's word);
///      a value that is not such a number is said and ignored;
///   3. otherwise the seed list decides: more than [`PSO_PREPARE_HEAD_LINES`] data lines (see [`seed_list_lines`]) sets [`pso_head_percent`]; not more leaves the host's default (all of it). A list
///      that is absent, is not a file or cannot be read sets nothing and says why: never fatal, the boot goes on with the host's own default. `pso_user.tsv` is not counted (the host applies the
///      percent to both lists, so a long learned list makes the head somewhat longer than 500 lines).
///
/// `DM_PSO_PREPARE_TAIL` is not set: the host's default (the background pass builds the rest) stays. Returns the percent it set.
pub fn set_pso_prepare_percent(ctx: &Ctx) -> Option<u32> {
    const VAR: &str = "DM_PSO_PREPARE_PERCENT";
    if let Some(have) = ctx.env.get(VAR).filter(|s| !s.is_empty()) {
        ctx.log(&format!("game env: {VAR} left alone: it is already {have} (an env.txt line, the settings or a harness chose it)"));
        return None;
    }
    if let Some(raw) = ctx.cfg.get_nonempty("pso_prepare_percent") {
        match raw.trim().parse::<u32>() {
            Ok(100) => {
                ctx.log(&format!("game env: {VAR} left alone: pso_prepare_percent=100 in the config (all of the prewarm lists is built before the first frame, as before)"));
                return None;
            }
            Ok(n) if (1..100).contains(&n) => {
                ctx.env.set(VAR, &n.to_string());
                ctx.log(&format!(
                    "game env: {VAR}={n} (pso_prepare_percent={n} in the config: only the first {} of the prewarm lists is built before the first frame, the rest in the background)",
                    share_words(n)
                ));
                return Some(n);
            }
            _ => ctx.log(&format!("game env: pso_prepare_percent={raw} in the config is not a whole number from 1 to 100: ignored, the seed list decides")),
        }
    }
    let path = match pso_seed_list_path(ctx) {
        Ok(p) => p,
        Err(why) => {
            ctx.log(&format!("game env: {VAR} left alone: {why}"));
            return None;
        }
    };
    let lines = match seed_list_lines(&path) {
        Ok(n) => n,
        Err(e) => {
            let what = if e.kind() == std::io::ErrorKind::NotFound {
                "does not exist".to_string()
            } else {
                format!("could not be read ({e})")
            };
            ctx.log(&format!("game env: {VAR} left alone: the seed list {} {what}, so there is nothing to count and the host's default stays", path.display()));
            return None;
        }
    };
    let Some(pct) = pso_head_percent(lines) else {
        ctx.log(&format!(
            "game env: {VAR} left alone (the seed list holds {lines} lines, not more than {PSO_PREPARE_HEAD_LINES}: all of it is built before the first frame)"
        ));
        return None;
    };
    ctx.env.set(VAR, &pct.to_string());
    ctx.log(&format!(
        "game env: {VAR}={pct} (the seed list holds {lines} lines: about the first {PSO_PREPARE_HEAD_LINES} ({}) are built before the first frame, the rest in the background while the game runs)",
        share_words(pct)
    ));
    Some(pct)
}

// ---- leftovers -----------------------------------------------------------------------------------------------------------------------------

/// `invalidate_previous_build`: a new chain starts: nothing the previous one built may survive into this one (DECISIONS trap 39). The framework stamp goes first, so a
/// chain that fails half-way can never leave the screen offering to boot the OLD game's framework over the NEW game's files; the rewritten modules and the flat folder
/// go too, so a module this game lacks cannot be taken from the last one; NSZ conversions a killed launch left behind are swept.
pub fn invalidate_previous_build(ctx: &Ctx) {
    if fs::remove_file(ctx.docs.framework_stamp()).is_ok() {
        ctx.log("chain start: framework stamp removed (a new build is under way)");
    }
    let extract = ctx
        .cfg
        .get("extract_out")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.docs.extract());
    let patched = ctx
        .cfg
        .get("patched_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| extract.join("rewrite"));
    for sub in [patched, extract.join("flat")] {
        if is_dir(&sub) {
            rmtree(&sub);
            ctx.log(&format!(
                "chain start: {} removed (the previous game's output)",
                sub.display()
            ));
        }
    }
    let nsz = ctx.docs.nsz_dir();
    if is_dir(&nsz) {
        for name in list_names(&nsz) {
            if fs::remove_file(nsz.join(&name)).is_ok() {
                ctx.log(&format!(
                    "chain start: leftover {name} removed from Documents/.nsz"
                ));
            }
        }
    }
}

/// `save_game_icon`: the game's icon (the extraction writes `Documents/extract/icon.jpg` from the control data) kept as `Documents/games/<id>/icon.jpg`: extract/ belongs
/// to the last game extracted, the library shows every game's.
pub fn save_game_icon(ctx: &Ctx) -> Option<PathBuf> {
    let tid = ctx.title_id()?;
    let src = Path::new(ctx.cfg.get_or("extract_out", "")).join("icon.jpg");
    if !is_file(&src) {
        return None;
    }
    let dir = ctx.docs.game_dir(&tid);
    let go = || -> std::io::Result<PathBuf> {
        fs::create_dir_all(&dir)?;
        crate::util::copy_atomic(&src, &dir.join("icon.jpg"))?; // this writer's own temporary name, not a shared `icon.jpg.tmp` (review r4 §4.1)
        let _ = fs::remove_file(dir.join("icon.none"));
        Ok(dir.join("icon.jpg"))
    };
    match go() {
        Ok(p) => {
            ctx.log(&format!("game icon: {}/icon.jpg", dir.display()));
            Some(p)
        }
        Err(e) => {
            ctx.log(&format!("game icon: not kept: {e}"));
            None
        }
    }
}

/// `drop_exefs`: extract/ExeFS (the game's code as extracted: 80 MB for Grizzy, more for a large game) is read by stage 2 and by stage 3's shader split only. Once the
/// framework is signed and the game has its shader library, nothing reads it: the boot takes flat/, the framework and RomFS.img, and a rebuild extracts it again. Kept
/// when the shaders are not there (the boot builds them from it) and with `keep_exefs=1` (the harness's chain-only runs reuse it). Returns the bytes freed.
pub fn drop_exefs(ctx: &Ctx) -> u64 {
    let exe = ctx.cfg.get_or("exe_dir", "");
    if ctx.cfg.get_or("keep_exefs", "0") == "1" || exe.is_empty() || !is_dir(Path::new(exe)) {
        return 0;
    }
    let size = folder_bytes(Path::new(exe));
    rmtree(Path::new(exe));
    ctx.log(&format!("extract/ExeFS removed after the build ({:.1} MB): the game runs from the framework, flat/ and RomFS.img", size as f64 / 1048576.0));
    size
}
