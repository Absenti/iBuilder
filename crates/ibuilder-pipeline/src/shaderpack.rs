//! Shader packs and the shader map the engine's host reads at the boot, and what stage 3 needs to know about a library already on disk
//! (`install_shader_packs`, `set_shader_pack`, `set_shader_map`, `shader_cache_folder`, `aoc_images`, `dlc_signature`, `stale_split`, `stale_dlc`).
//!
//! A pack is what `tools/make_shader_pack.sh` makes on a Mac: the game's Switch shaders translated to Metal for THIS app's host (`b-<chain>.img`, the translation
//! chain version the host was built with) and the profile tables the host reads beside the shader map. It is imported as `Documents/<id>.shaders.tar` and unpacked
//! into `Documents/shaders/<id>/`; stage 3 builds the same folder on the device when no pack was imported.

use crate::ctx::Ctx;
use crate::tsv::{map_has_rows, map_rows, profile_title_id};
use crate::util::{
    folder_bytes, gib_text, is_dir, is_file, list_names, mtime_secs, read_text, rmtree,
    write_atomic,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const SHADER_PACK_SUFFIX: &str = ".shaders.tar";

/// The header-only placeholder `set_shader_map` writes when nothing was derived: an empty map is not an error for the engine (prefetch skips what it has no
/// stage for, and prog_get reports each such shader `[RED] shader <key> is not in the shader map` and skips its draw), a missing file is (DECISIONS trap 23).
pub const SHADER_MAP_HEADER: &str = "# kind: evidence  provisional profile: filled by the shader split (a Vulkan game's SPIR-V shaders go through the Vulkan layer and leave it empty)\npackage\tindex\tstage\tdm_ctl_sha16\tsource\n";

/// `use_config_paths`: an exported game boots from its bundle: its shader library is under `<bundle>/ExportedGame/shaders/<id>`, read-only, and the config says
/// so (`shader_packs=`). Nothing is built there.
pub fn use_config_paths(ctx: &mut Ctx) {
    if let Some(packs) = ctx.cfg.get_nonempty("shader_packs") {
        let packs = PathBuf::from(packs);
        if packs != ctx.shader_packs {
            ctx.log(&format!(
                "shader packs: {} (an exported game: its library is in its bundle)",
                packs.display()
            ));
            ctx.shader_packs = packs;
        }
    }
}

/// `_derived_shader_maps`: every `Documents/profile-derived/<id>/shader_map.tsv`, THIS game's first, then the most recently written.
pub fn derived_shader_maps(ctx: &Ctx, profile: Option<&Path>) -> Vec<PathBuf> {
    let root = ctx.docs.profile_derived();
    if !is_dir(&root) {
        return vec![];
    }
    let mut found: Vec<PathBuf> = list_names(&root)
        .into_iter()
        .map(|d| root.join(d).join("shader_map.tsv"))
        .filter(|p| is_file(p))
        .collect();
    let tid = profile.and_then(profile_title_id);
    found.sort_by(|a, b| {
        let key = |p: &PathBuf| {
            let dir = p
                .parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            (Some(dir) != tid, mtime_secs(p))
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0)
            .then(kb.1.partial_cmp(&ka.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    found
}

/// `set_shader_map`: point the engine at the shader map this app derived, and say which file that was. Candidates: the derived folders (this game's first),
/// then the file beside profile.tsv, then Documents itself. The first one WITH ROWS wins, so the header-only placeholder this function writes when nothing was
/// derived can never shadow a real map found later or elsewhere; with no map holding a row, the first file that exists is used. Nothing found at all: a
/// header-only map is written, because a missing file kills the boot and an empty one only costs a RED line per unpaired shader.
pub fn set_shader_map(ctx: &Ctx) -> PathBuf {
    if let Some(have) = ctx.env.get("DM_NVN_SHMAP").filter(|s| !s.is_empty())
        && is_file(Path::new(&have))
    {
        ctx.log(&format!(
            "shader map: DM_NVN_SHMAP was already set to {have} ({} rows)",
            map_rows(Path::new(&have))
        ));
        return PathBuf::from(have);
    }
    let prof = ctx.cfg.get_nonempty("profile").map(PathBuf::from);
    let mut cands = derived_shader_maps(ctx, prof.as_deref());
    if let Some(p) = &prof {
        cands.push(p.parent().unwrap_or(Path::new("")).join("shader_map.tsv"));
    }
    cands.push(ctx.docs.shader_map());
    let mut seen = Vec::new();
    cands.retain(|c| {
        if seen.contains(c) {
            false
        } else {
            seen.push(c.clone());
            true
        }
    });
    let existing: Vec<&PathBuf> = cands.iter().filter(|p| is_file(p)).collect();
    let path = existing
        .iter()
        .find(|p| map_has_rows(p))
        .or(existing.first())
        .map(|p| (*p).clone());
    let path = match path {
        None => {
            let p = ctx.docs.shader_map();
            if let Err(e) = write_atomic(&p, SHADER_MAP_HEADER.as_bytes()) {
                ctx.log(&format!("shader map: could not write {}: {e}", p.display()));
            }
            ctx.log(&format!(
                "shader map: none was derived, wrote an empty one at {} -- every shader the game uses will be reported [RED] and its draw skipped (dm/gfx/tools/shader_split.py fills it on the Mac)",
                p.display()
            ));
            p
        }
        Some(p) => {
            ctx.log(&format!("shader map: {} ({} rows) -- the engine default (<exe>/../games/<id>/) does not exist on a device", p.display(), map_rows(&p)));
            p
        }
    };
    ctx.env.set("DM_NVN_SHMAP", &path.to_string_lossy());
    path
}

// ---- packs ---------------------------------------------------------------------------------------------------------------------------

struct Member {
    name: String,
    is_dir: bool,
    is_file: bool,
}

fn list_members(path: &Path) -> io::Result<Vec<Member>> {
    let mut ar = tar::Archive::new(fs::File::open(path)?);
    let mut out = Vec::new();
    for e in ar.entries()? {
        let e = e?;
        let t = e.header().entry_type();
        if t.is_pax_global_extensions()
            || t.is_pax_local_extensions()
            || t.is_gnu_longname()
            || t.is_gnu_longlink()
        {
            continue;
        }
        let mut name = String::from_utf8_lossy(&e.path_bytes()).into_owned();
        let is_dir = t.is_dir();
        if is_dir {
            name = name.trim_end_matches('/').to_string();
        }
        out.push(Member {
            name,
            is_dir,
            is_file: t.is_file() || t.is_contiguous() || t.is_gnu_sparse(),
        });
    }
    Ok(out)
}

/// `install_shader_packs`: unpack every `Documents/<id>.shaders.tar` the user imported into `Documents/shaders/<id>/`, then remove the .tar. Members must stay
/// inside one `<id>/` folder; anything else refuses the whole pack (nothing is deleted on a refusal). Returns the ids installed.
pub fn install_shader_packs(ctx: &Ctx) -> Vec<String> {
    let mut done = Vec::new();
    let names: Vec<String> = list_names(ctx.docs.root())
        .into_iter()
        .filter(|n| n.to_lowercase().ends_with(SHADER_PACK_SUFFIX))
        .collect();
    for name in names {
        let path = ctx.docs.join(&name);
        let members = match list_members(&path) {
            Ok(m) => m,
            Err(e) => {
                ctx.log(&format!("shader pack {name} could not be installed: {e}"));
                continue;
            }
        };
        let mut tops: Vec<String> = members
            .iter()
            .map(|m| m.name.split('/').next().unwrap_or("").to_string())
            .collect();
        tops.sort();
        tops.dedup();
        let bad: Vec<&str> = members
            .iter()
            .filter(|m| {
                m.name.starts_with('/')
                    || m.name.split('/').any(|c| c == "..")
                    || !(m.is_file || m.is_dir)
            })
            .map(|m| m.name.as_str())
            .collect();
        if tops.len() != 1 || !bad.is_empty() {
            let what = if bad.is_empty() {
                tops.join(", ").chars().take(120).collect::<String>()
            } else {
                bad.iter().take(3).copied().collect::<Vec<_>>().join(", ")
            };
            ctx.log(&format!(
                "shader pack {name} refused: its members must all sit in one <id>/ folder ({what})"
            ));
            continue;
        }
        let tid = tops[0].to_uppercase();
        if !(tid.len() == 16
            && tid
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'A'..=b'F').contains(&c)))
        {
            ctx.log(&format!("shader pack {name} refused: its folder '{tid}' is not a title id (make it with tools/make_shader_pack.sh)"));
            continue;
        }
        let dest = ctx.shader_packs.join(&tid);
        let mut tmp_name = dest.as_os_str().to_owned();
        tmp_name.push(".part");
        let tmp = PathBuf::from(tmp_name);
        let result = (|| -> io::Result<()> {
            rmtree(&tmp);
            fs::create_dir_all(&tmp)?;
            let mut ar = tar::Archive::new(fs::File::open(&path)?);
            for e in ar.entries()? {
                let mut e = e?;
                let t = e.header().entry_type();
                if t.is_pax_global_extensions()
                    || t.is_pax_local_extensions()
                    || t.is_gnu_longname()
                    || t.is_gnu_longlink()
                {
                    continue;
                }
                let full = String::from_utf8_lossy(&e.path_bytes())
                    .trim_end_matches('/')
                    .to_string();
                let rel = match full.split_once('/') {
                    Some((_, r)) => r.to_string(),
                    None => String::new(),
                };
                if rel.is_empty() {
                    continue;
                }
                let target = tmp.join(&rel);
                if t.is_dir() {
                    fs::create_dir_all(&target)?;
                    continue;
                }
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p)?;
                }
                let mut out = fs::File::create(&target)?;
                io::copy(&mut e as &mut dyn Read, &mut out)?;
            }
            crate::shaders::install_pack(&tmp, &dest)?; // the old pack stays until the new one is in its place (review r3 §4.6, r4 §4.1)
            fs::remove_file(&path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                ctx.log(&format!("shader pack {name} installed into Documents/shaders/{tid} (the .tar was removed: the folder is the pack now)"));
                done.push(tid);
            }
            Err(e) => {
                rmtree(&tmp); // a half-extracted `.part` is of no use to anyone and takes the room of the next try
                ctx.log(&format!("shader pack {name} could not be installed: {e}"));
            }
        }
    }
    done
}

// ---- topping up a pack that already exists ----------------------------------------------------------------------------------------------

/// The registered game's small INPUT tables that ride with its shader pack (every `.tsv` of the game's table folder went into a pack the Python build made; the app's bundle carries these,
/// `scripts/build-app.sh`, because the game's folder in the engine holds them beside `game.tsv`): `bf_want.tsv` / `dual_want.tsv` (which pixel shaders get the extra variants),
/// `geo_mesh.tsv` (vertex/geometry pairs that get a mesh library), `shader_facts_manual.tsv` (hand-written facts merged over the split's), `pso_manifest.tsv` (the pipeline prewarm seed
/// list the host reads BESIDE the shader map at a start: "[RED] pipeline prewarm: seed list does not exist", Mario Kart 8 Deluxe, device log 2026-11-19) and `bench.tsv`.
/// The split's own tables (`shader_map`, `shader_pairs`, `pso_pairs`, `shader_facts`, `c1_full`, `cb_extent`) are never in this list: they belong to the library they were made with.
pub const PACK_INPUT_TABLES: [&str; 6] = [
    "bf_want.tsv",
    "dual_want.tsv",
    "geo_mesh.tsv",
    "shader_facts_manual.tsv",
    "pso_manifest.tsv",
    "bench.tsv",
];

/// A table bigger than this is not copied by a top-up (the largest registered `pso_manifest.tsv` is under 2 MiB): a step that runs at every boot stays cheap.
const TOP_UP_MAX_BYTES: u64 = 32 << 20;

/// Whether the library of a pack built without `name` is missing something the table asks for: a row of a build input, or a `G` row (a mesh pair) of the pipeline record.
fn table_changes_the_build(name: &str, text: &str) -> bool {
    match name {
        "bf_want.tsv" | "dual_want.tsv" | "geo_mesh.tsv" | "shader_facts_manual.tsv" => text
            .lines()
            .any(|l| !l.trim().is_empty() && !l.starts_with('#')),
        "pso_manifest.tsv" => text.lines().any(|l| l.starts_with("G\t")),
        _ => false,
    }
}

/// `top_up_pack`: a pack that is already on the phone was made before the app carried some of its game's small input tables (a bundle that held only `game.tsv`), so a table added to the
/// bundle later never reached it: a new app build does not build the library again, and the host asked for `pso_manifest.tsv` beside the shader map in vain. This puts the registered game's
/// tables that the pack LACKS beside its shader map: from the game's derived folder (`Documents/profile-derived/<id>/`), else from the bundle's (`dm/games/<id>/`), the first that has the
/// file, as the build itself takes them.
///
/// Cheap and safe, by construction: it reads no library and no map; only the [`PACK_INPUT_TABLES`] names are ever considered; a file the pack already has is never touched (not even an empty
/// one); each copy is written aside (`.<name>.part`) and renamed, so the host never reads half a file; a pack that is not a real installed one (no shader map with rows), or one that is not
/// the app's own writable `Documents/shaders` (an exported game boots from its read-only bundle), is left alone; a failure is one log line and the boot goes on. A boot that finds nothing to
/// add logs nothing. Returns the names it added.
///
/// What it cannot do is make the library use a table it was built without: a table that the BUILD reads (`bf_want`, `dual_want`, `geo_mesh`, `shader_facts_manual`, the `G` rows of
/// `pso_manifest`) and that was absent when this pack was built means the library lacks the variants it names; that is said ([ORANGE]) with the way to build it again. `pso_manifest.tsv`'s
/// seed list and `bench.tsv` are read at a start, so they take effect at the next boot.
pub fn top_up_pack(ctx: &Ctx, tid: &str, pack: &Path) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
    // an exported game's library is in its bundle, read-only: nothing is written there
    if ctx.shader_packs != ctx.docs.shaders() || !is_dir(pack) {
        return added;
    }
    let smap = pack.join("shader_map.tsv");
    if !map_has_rows(&smap) {
        return added;
    }
    let sources = [
        ctx.docs.derived_dir(tid),
        ctx.bundle.join("dm").join("games").join(tid),
    ];
    let mut build_inputs: Vec<&str> = Vec::new();
    for name in PACK_INPUT_TABLES {
        let dest = pack.join(name);
        if fs::symlink_metadata(&dest).is_ok() {
            continue; // never overwrite: not a file of ours, an earlier top-up's, or the build's own copy
        }
        let Some(src) = sources.iter().map(|d| d.join(name)).find(|p| {
            fs::symlink_metadata(p)
                .map(|m| m.is_file())
                .unwrap_or(false)
        }) else {
            continue;
        };
        let size = fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        if size > TOP_UP_MAX_BYTES {
            ctx.log(&format!("shader pack: {name} of {tid} not added: {size} bytes is more than a top-up copies ({TOP_UP_MAX_BYTES})"));
            continue;
        }
        let tmp = pack.join(format!(".{name}.part"));
        let copied = (|| -> io::Result<()> {
            fs::copy(&src, &tmp)?;
            // the pack is the host's to read at any moment: the name appears whole or not at all, and a file that showed up meanwhile is not replaced
            if fs::symlink_metadata(&dest).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "appeared meanwhile",
                ));
            }
            fs::rename(&tmp, &dest)
        })();
        match copied {
            Ok(()) => {
                if table_changes_the_build(name, &read_text(&dest).unwrap_or_default()) {
                    build_inputs.push(name);
                }
                added.push(name.to_string());
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                if e.kind() != io::ErrorKind::AlreadyExists {
                    ctx.log(&format!(
                        "shader pack: {name} of {tid} could not be added: {e}"
                    ));
                }
            }
        }
    }
    if !added.is_empty() {
        ctx.log(&format!("shader pack: Documents/shaders/{tid} topped up with its game's tables it lacked: {} (files the pack has were not touched)", added.join(", ")));
    }
    if !build_inputs.is_empty() {
        ctx.log(&format!(
            "shader pack: [ORANGE] {} (inputs of the shader BUILD) were not in Documents/shaders/{tid} when its library was built, so the variants, mesh libraries or facts they name are missing from it; \
             the tables are beside it now, and only a new build uses them: delete Documents/shaders/{tid} in the Files app and the next build or Play translates the shaders again",
            build_inputs.join(", ")
        ));
    }
    added
}

/// `set_shader_pack`: the installed pack of the game being booted (`Documents/shaders/<id>/`): its shader map, and its library folder as DM_MSL_DIR, where the
/// host looks for `b-<chain>.img`. Says which chain the pack carries and, when the config names the host's own (`shader_chain=`, from the app build), whether
/// they match: a host only accepts libraries of its own chain. Returns the pack folder, or `None`. An installed pack is topped up first with the game's small input tables it lacks
/// ([`top_up_pack`]), so the host finds the prewarm seed list beside the shader map.
pub fn set_shader_pack(ctx: &Ctx) -> Option<PathBuf> {
    install_shader_packs(ctx);
    let tid = ctx.title_id()?;
    let pack = ctx.shader_packs.join(&tid);
    let smap = pack.join("shader_map.tsv");
    if !map_has_rows(&smap)
        && let Some(api) = crate::layers::layer_of(&pack)
    {
        // GL / Vulkan (crate::layers): the host's layer finds <DM_MSL_DIR>/<api>/<api>_shaders.tsv and the MSL beside it
        if !ctx.env.truthy("DM_MSL_DIR") {
            ctx.env.set("DM_MSL_DIR", &pack.to_string_lossy());
        }
        ctx.log(&format!(
            "shader pack: Documents/shaders/{tid} holds the game's {api} layer library (DM_MSL_DIR)"
        ));
        return Some(pack);
    }
    if !map_has_rows(&smap) {
        ctx.log(&format!(
            "shader pack: none for {tid} -- the game runs without its shaders and draws nothing; stage 3 translates them on this device (or import {tid}{SHADER_PACK_SUFFIX} made on a Mac)"
        ));
        return None;
    }
    top_up_pack(ctx, &tid, &pack); // an installed pack lacks the game's tables the bundle gained since it was built (the prewarm seed list: pso_manifest.tsv)
    let mut chains: Vec<String> = list_names(&pack)
        .into_iter()
        .filter(|n| n.starts_with("b-"))
        .map(|n| n[2..].split('.').next().unwrap_or("").to_string())
        .collect();
    chains.sort();
    let want = ctx.cfg.get_or("shader_chain", "").to_string();
    if !want.is_empty() && !chains.contains(&want) {
        ctx.log(&format!(
            "shader pack: [RED] Documents/shaders/{tid} carries chain {}, this app's host is {want}: the host will refuse it -- make the pack again with the engine this app was built from",
            if chains.is_empty() { "none".to_string() } else { chains.join(", ") }
        ));
    }
    if !ctx.env.truthy("DM_NVN_SHMAP") {
        ctx.env.set("DM_NVN_SHMAP", &smap.to_string_lossy());
    }
    if !ctx.env.truthy("DM_MSL_DIR") {
        ctx.env.set("DM_MSL_DIR", &pack.to_string_lossy());
    }
    let shown = if ctx.shader_packs != ctx.docs.shaders() {
        pack.display().to_string()
    } else {
        format!("Documents/shaders/{tid}")
    };
    let lib = if chains.is_empty() {
        "MISSING".to_string()
    } else {
        chains
            .iter()
            .map(|c| format!("b-{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let host = if want.is_empty() {
        ""
    } else if chains.contains(&want) {
        " (the host's chain)"
    } else {
        ""
    };
    ctx.log(&format!(
        "shader pack: {shown}, {} shaders, library {lib}{host}",
        map_rows(&smap)
    ));
    Some(pack)
}

// ---- the translation cache and the DLC set -------------------------------------------------------------------------------------------

/// The suffix of the Rust build's translation cache: `cas-<chain>.dmsb`, a FOLDER of one immutable file per finished shader (`ibuilder_shaderbuild::cache::Store`: the
/// cli crate has a test that the two crates still spell it the same). The engine's own cache, `cas-<chain>.db` (SQLite, written by the Python build), is another format
/// this build cannot read.
pub const SHADER_CACHE_SUFFIX: &str = ".dmsb";

/// `cas-<chain>.dmsb`: the name of the chain's cache inside `Documents/shadercache`.
pub fn shader_cache_name(chain: &str) -> String {
    format!("cas-{chain}{SHADER_CACHE_SUFFIX}")
}

/// Why `name` (an entry of the cache folder that is not this chain's cache) is dropped, or `None` when it is not ours to drop (it does not start with `cas`).
fn stale_cache_reason(name: &str, chain: &str) -> Option<&'static str> {
    if !name.starts_with("cas") || name == shader_cache_name(chain) {
        return None;
    }
    // `cas-<chain>.db`, `.db-wal`, `.db-shm`: the engine's SQLite store (the Python build's), whatever its chain: this build's cache is a folder and cannot read it
    let rest = name.strip_prefix("cas-").unwrap_or(name);
    Some(if rest.contains(".db") {
        "the engine's SQLite cache: this build keeps its translations in cas-<chain>.dmsb and cannot read it"
    } else {
        "another chain version: none of its entries can match"
    })
}

/// `shader_cache_folder`: the translation cache every game's stage 3 shares (trap 51): `Documents/shadercache/cas-<chain>.dmsb`, a content-addressed FOLDER (one file per
/// finished shader, keyed by a digest of the shader's bytes and everything its translation reads; the Python build's was one SQLite file, `cas-<chain>.db`), so a shader seen in
/// an earlier build -- another game on the same engine, or this game built again -- is restored instead of translated. Before the build everything else that starts with `cas`
/// goes, files AND folders: other chains' caches can never hit (the digest covers the chain) and the engine's SQLite files are not readable here. Over the limit
/// (`shader_cache_gib`, default 2; the folder's total size, entries in sub-folders counted) the cache starts again. `None` when `shader_cache=0` (the build keeps its cache in
/// its work folder). `resuming`: this build resumes an interrupted one, whose finished shaders are in the cache: it is not started again over the limit.
pub fn shader_cache_folder(ctx: &Ctx, chain: &str, resuming: bool) -> Option<PathBuf> {
    if ctx.cfg.get_or("shader_cache", "1") == "0" {
        ctx.log("shader cache: off (shader_cache=0)");
        return None;
    }
    let dir = ctx.docs.shader_cache();
    if let Err(e) = fs::create_dir_all(&dir) {
        ctx.log(&format!("shader cache: [RED] {} cannot be made ({e}): this build keeps its cache in its work folder", dir.display()));
        return None;
    }
    for name in list_names(&dir) {
        let Some(why) = stale_cache_reason(&name, chain) else {
            continue;
        };
        let path = dir.join(&name);
        // `remove_file` cannot remove a folder (and `remove_dir_all` follows no link: a symlink is a file here)
        let removed = if fs::symlink_metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match removed {
            Ok(()) => ctx.log(&format!("shader cache: {name} removed ({why})")),
            Err(e) => ctx.log(&format!("shader cache: {name} could not be removed: {e}")),
        }
    }
    let limit = ctx
        .cfg
        .get_or("shader_cache_gib", "2")
        .trim()
        .parse::<f64>()
        .unwrap_or(2.0)
        * (1u64 << 30) as f64;
    let have = folder_bytes(&dir);
    if have as f64 > limit && resuming {
        ctx.log(&format!(
            "shader cache: {} is over the {} limit, kept for now: this build resumes an interrupted one that left its shaders in it",
            gib_text(have as f64),
            gib_text(limit)
        ));
    } else if have as f64 > limit {
        rmtree(&dir);
        let _ = fs::create_dir_all(&dir);
        ctx.log(&format!(
            "shader cache: {} was over the {} limit, started again",
            gib_text(have as f64),
            gib_text(limit)
        ));
    } else {
        ctx.log(&format!(
            "shader cache: {} ({}, limit {}): shaders already translated for any game are restored",
            dir.display(),
            gib_text(have as f64),
            gib_text(limit)
        ));
    }
    Some(dir)
}

/// `aoc_images`: the game's add-on content images, `Documents/games/<id>/aoc/<index>.img`, in index order (2 before 10): what the boot names as DM_AOC_DIR, and
/// what stage 3 reads beside the game's RomFS.
pub fn aoc_images(ctx: &Ctx, tid: &str) -> Vec<PathBuf> {
    let aoc = ctx.docs.game_dir(tid).join("aoc");
    let mut names: Vec<String> = list_names(&aoc)
        .into_iter()
        .filter(|n| n.ends_with(".img") && !n.starts_with('.'))
        .collect();
    names.sort_by_key(|n| {
        let stem = &n[..n.len() - 4];
        if !stem.is_empty() && stem.bytes().all(|c| c.is_ascii_digit()) {
            (
                0u8,
                stem.parse::<u128>().unwrap_or(u128::MAX),
                String::new(),
            )
        } else {
            (1u8, 0, n.clone())
        }
    });
    names.into_iter().map(|n| aoc.join(n)).collect()
}

/// `dlc_signature`: the DLC set a shader library was split with: `index:size` of each image ('-' for none). A DLC added, removed or replaced by another version
/// changes it, so the library is split again with it.
pub fn dlc_signature(images: &[PathBuf]) -> String {
    let sig: Vec<String> = images
        .iter()
        .filter_map(|p| {
            let size = fs::metadata(p).ok()?.len();
            let base = p.file_name()?.to_string_lossy().into_owned();
            Some(format!("{}:{size}", &base[..base.len().saturating_sub(4)]))
        })
        .collect();
    if sig.is_empty() {
        "-".to_string()
    } else {
        sig.join(",")
    }
}

fn pack_rows(pack: &Path) -> Option<BTreeMap<String, String>> {
    let text = read_text(&pack.join("pack.txt")).ok()?;
    let mut rows = BTreeMap::new();
    for l in text.lines() {
        if let Some((k, v)) = l.split_once('\t') {
            rows.insert(k.to_string(), v.to_string());
        }
    }
    Some(rows)
}

/// `stale_split`: the split version a library built ON THIS DEVICE records when it is not this app's (`none` for one from before the field), else `None`. An imported
/// pack (made on a Mac) is never second-guessed.
pub fn stale_split(pack: &Path, current_split: &str) -> Option<String> {
    let rows = pack_rows(pack)?;
    if !rows
        .get("made")
        .map(|m| m.starts_with("on this device"))
        .unwrap_or(false)
    {
        return None;
    }
    let have = rows
        .get("split")
        .cloned()
        .unwrap_or_else(|| "none".to_string());
    if have != current_split && current_split != "-" {
        Some(have)
    } else {
        None
    }
}

/// `stale_dlc`: `(had, want)` when a library built ON THIS DEVICE was split with another DLC set than the one installed now (a library from before the field counts
/// as split with none), else `None`. An imported pack is never second-guessed.
pub fn stale_dlc(pack: &Path, want: &str) -> Option<(String, String)> {
    let rows = pack_rows(pack)?;
    if !rows
        .get("made")
        .map(|m| m.starts_with("on this device"))
        .unwrap_or(false)
    {
        return None;
    }
    let have = rows
        .get("dlc")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "-".to_string());
    if have != want {
        Some((have, want.to_string()))
    } else {
        None
    }
}
