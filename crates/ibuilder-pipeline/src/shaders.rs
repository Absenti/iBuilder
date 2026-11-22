//! Stage 3 on the phone: the game's shaders become a library THIS host takes, with no Mac (docs/GAME-APP.md §8.7), and everything around it that survives a kill:
//! the resumable work folder (`ShaderResume`, DECISIONS trap 74), the memory/thermal-gated threads (traps 71 and 76), the shared translation cache (trap 51), the
//! "this game holds no shader" marker, libraries that are stale because the split or the DLC set changed (traps 33, 67), and the live shaders the running game lacks (trap 68).
//!
//! The split and the translation themselves are `ShaderBackend` (the `ibuilder-shader` crate); the translator (`dmsh_xlate`, C#) and SPIRV-Cross (C) are the host's `ShaderTools`.

use crate::adapters::{
    BuildRequest, LiveEntry, LiveRequest, ShaderBackend, ShaderTools, SplitRequest,
};
use crate::ctx::Ctx;
use crate::error::{PipelineError, Result};
use crate::gate::ShaderMemoryGate;
use crate::layout::Docs;
use crate::live::LiveQueue;
use crate::log::Log;
use crate::memory::{MemSource, SHADER_PER_JOB, mem_line, release_memory, shader_jobs_for_memory};
use crate::settings::shader_pulling_wanted;
use crate::shaderpack::{
    aoc_images, dlc_signature, install_shader_packs, shader_cache_folder, stale_dlc, stale_split,
    top_up_pack,
};
use crate::stage::write_stage;
use crate::stop;
use crate::tsv::map_has_rows;
use crate::util::{
    MIB, folder_bytes, gib_text, is_dir, is_file, list_names, read_text, rmtree, utc_now_iso,
    write_atomic,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// What stage 3 leaves in its work folder when a person paused or cancelled the build (`stop`): the work is kept (the split, the shader cache when it is the work folder's own) and this file,
/// "<pause|cancel> <time>", says a person stopped it. The next run then does not take the folder for one an earlier run DIED under (the memory kill that makes a run read and translate on one
/// thread, below), and goes on at full speed.
pub const STOPPED_BY_USER: &str = "stopped.by.user";

pub const SHADER_TABLES: [&str; 4] = [
    "shader_map.tsv",
    "shader_facts.tsv",
    "shader_pairs.tsv",
    "pso_pairs.tsv",
];

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// `file_fingerprint`: a file's identity for "the same input as last time": its size and a hash of its first and last 4 MiB (a game's RomFS.img is many GB: reading all of it
/// would cost what the resume saves, and a rewritten copy of the same bytes, as a new extraction makes, must still count as the same file, which its modification time would not).
/// "-" when unreadable. (A sha256 prefix where the Python app used sha1's: a work folder left by the Python app does not resume once, which is harmless.)
pub fn file_fingerprint(path: &Path) -> String {
    const FOUR: u64 = 4 * MIB;
    let go = || -> std::io::Result<String> {
        let size = fs::metadata(path)?.len();
        let mut f = fs::File::open(path)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; FOUR as usize];
        let mut read_up_to = |f: &mut fs::File, h: &mut Sha256| -> std::io::Result<()> {
            let mut total = 0usize;
            while total < buf.len() {
                let n = f.read(&mut buf[total..])?;
                if n == 0 {
                    break;
                }
                total += n;
            }
            h.update(&buf[..total]);
            Ok(())
        };
        read_up_to(&mut f, &mut h)?;
        if size > 2 * FOUR {
            f.seek(SeekFrom::Start(size - FOUR))?;
        }
        read_up_to(&mut f, &mut h)?;
        Ok(format!("{size}:{}", &hex(&h.finalize())[..16]))
    };
    go().unwrap_or_else(|_| "-".to_string())
}

/// `shader_work_signature`: what an interrupted stage 3's work folder is valid for: the game (its RomFS and main), its DLC images, the shader split and translation chain of this
/// app, and the game's profile tables. Anything else changing means the work is not reused.
pub fn shader_work_signature(
    split_version: &str,
    tid: &str,
    chain: &str,
    dlc: &[PathBuf],
    romfs: &Path,
    main_nso: &Path,
    derived: Option<&Path>,
) -> String {
    let mut h = Sha256::new();
    let mut parts: Vec<String> = vec![
        "shader-work-1".into(),
        split_version.into(),
        tid.into(),
        chain.into(),
        dlc_signature(dlc),
        file_fingerprint(romfs),
        file_fingerprint(main_nso),
    ];
    parts.extend(dlc.iter().map(|p| file_fingerprint(p)));
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\0");
    }
    if let Some(d) = derived.filter(|d| is_dir(d)) {
        for name in list_names(d) {
            let path = d.join(&name);
            if name.ends_with(".tsv") && is_file(&path) {
                h.update(name.as_bytes());
                h.update(b"\0");
                if let Ok(b) = fs::read(&path) {
                    h.update(&b);
                }
            }
        }
    }
    hex(&h.finalize())[..24].to_string()
}

/// The game's stage 3 work folder (`Documents/shaders/<id>.work`), made resumable. iOS can end the app under stage 3 (SIGKILL, nothing logged); the folder it leaves holds what was finished:
///   * `splitjobs/`: every finished job of the shader split (the backend's memo), so the next split runs only the jobs that were not done and its output is byte-identical to an uninterrupted one;
///   * `split.ok`: the split finished (its shader count): `pkg/` and `tables/` are complete and the split is not run again;
///   * the translation's finished shaders are in the shader cache (`cas-<chain>.dmsb`, a folder with one file per shader, each renamed into place when it is finished: a kill keeps them), which this build keeps in the work folder (`cas/`) when the shared cache is off.
///
/// `resume.json` holds the signature of the inputs (`shader_work_signature`): the same game, DLC, split version and chain resume; a different signature empties the folder.
/// A partial job file (a kill mid-write) is a temp name and is dropped.
pub struct ShaderResume {
    pub work: PathBuf,
    sig: String,
    pub resumed: bool,
}

impl ShaderResume {
    pub const FILE: &'static str = "resume.json";
    pub const OK: &'static str = "split.ok";

    pub fn new(work: &Path, sig: &str) -> ShaderResume {
        ShaderResume {
            work: work.to_path_buf(),
            sig: sig.to_string(),
            resumed: false,
        }
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.work.join("splitjobs")
    }

    fn read(&self, name: &str) -> Option<Value> {
        serde_json::from_str(&read_text(&self.work.join(name)).ok()?).ok()
    }

    fn write(&self, name: &str, v: &Value) -> std::io::Result<()> {
        write_atomic(&self.work.join(name), v.to_string().as_bytes())
    }

    /// Keeps the folder when it was left by a run of the same inputs, else starts it empty. True when kept.
    pub fn open(&mut self) -> bool {
        let info = if is_dir(&self.work) {
            self.read(Self::FILE)
        } else {
            None
        };
        if info
            .as_ref()
            .and_then(|i| i.get("sig"))
            .and_then(|s| s.as_str())
            == Some(self.sig.as_str())
        {
            self.resumed = true;
            let mut stack = vec![self.work.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        stack.push(p);
                    } else if e.file_name().to_string_lossy().contains(".tmp") {
                        let _ = fs::remove_file(&p);
                    }
                }
            }
        } else {
            rmtree(&self.work);
            let _ = fs::create_dir_all(&self.work);
            let _ = self.write(
                Self::FILE,
                &json!({"sig": self.sig, "started": utc_now_iso()}),
            );
        }
        let _ = fs::create_dir_all(self.jobs_dir());
        self.resumed
    }

    /// The shader count of a finished split, when its pkg/ and tables/ are there; else `None`.
    pub fn split_done(&self) -> Option<usize> {
        let ok = self.read(Self::OK)?;
        if ok.get("sig").and_then(|s| s.as_str()) != Some(self.sig.as_str()) {
            return None;
        }
        let n = ok.get("n").and_then(|n| n.as_i64()).filter(|n| *n > 0)? as usize;
        let pkg = self.work.join("pkg");
        if !list_names(&pkg).iter().any(|n| n.ends_with(".bin")) {
            return None;
        }
        let tables = self.work.join("tables");
        if !["shader_map.tsv", "shader_facts.tsv"]
            .iter()
            .all(|t| is_file(&tables.join(t)))
        {
            return None;
        }
        Some(n)
    }

    pub fn mark_split(&self, n: usize) {
        let _ = self.write(Self::OK, &json!({"sig": self.sig, "n": n}));
    }
}

/// A `<pack>.none` marker ("this game holds no shader this split reads") is only believed when the split that wrote it is this app's:
/// a newer split may read the game (Beach Buggy Racing 2's Vector Unit .apf archive: 0 shaders, then 496 with SPLIT_REV 3).
/// A marker without a `split` line was written before the version was recorded and is stale for the same reason.
fn none_marker_current(text: &str, split_ver: &str) -> bool {
    text.lines().any(|l| l == format!("split {split_ver}"))
}

fn split_version(ctx: &Ctx) -> String {
    ctx.adapters
        .shaders
        .as_ref()
        .map(|b| b.split_version())
        .unwrap_or_else(|| "-".to_string())
}

/// `shaders_will_build`: whether `build_shaders_on_device` would translate this game's shaders (no side effects): it has no library of this host's chain yet, is not marked as
/// holding none, and its inputs are there.
pub fn shaders_will_build(ctx: &Ctx) -> bool {
    let Some(tid) = ctx.title_id() else {
        return false;
    };
    let chain = ctx.cfg.get_or("shader_chain", "");
    if chain.is_empty()
        || ctx.cfg.get_or("shader_build", "1") == "0"
        || ctx
            .cfg
            .get_nonempty("shader_manifest")
            .map(|m| is_dir(Path::new(m)))
            .unwrap_or(false)
    {
        return false;
    }
    let pack = ctx.shader_packs.join(&tid);
    let dlc_sig = dlc_signature(&aoc_images(ctx, &tid));
    let none_marker = with_ext(&pack, ".none");
    if is_file(&none_marker) {
        let t = read_text(&none_marker).unwrap_or_default();
        if none_marker_current(&t, &split_version(ctx))
            && (dlc_sig == "-" || t.contains(&format!("dlc {dlc_sig}\n")))
        {
            return false;
        }
    }
    let have = list_names(&pack)
        .iter()
        .any(|n| *n == format!("b-{chain}.img") || *n == format!("b-{chain}"));
    if have
        && map_has_rows(&pack.join("shader_map.tsv"))
        && stale_split(&pack, &split_version(ctx)).is_none()
        && stale_dlc(&pack, &dlc_sig).is_none()
    {
        return false;
    }
    crate::profile::table_dir(ctx, &tid).is_some()
        && Path::new(ctx.cfg.get_or("romfs", "")).exists()
        && Path::new(ctx.cfg.get_or("exe_dir", ""))
            .join("main")
            .exists()
}

fn with_ext(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// `app_setting`: a row of `Documents/gameapp.settings.tsv` (the app's settings screen writes it), or `default`.
pub fn app_setting(ctx: &Ctx, key: &str) -> Option<String> {
    let text = read_text(&ctx.docs.app_settings()).ok()?;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('\t')
            && k.trim() == key
        {
            return Some(v.split('#').next().unwrap_or("").trim().to_string());
        }
    }
    None
}

/// `shader_parallelism`: whether stage 3 finishes its shaders on several cores at once: on unless the player turned "Parallel shader preparation" off (`parallel_shaders 0`).
/// (The CPython build also needed a free-threaded runtime for this to mean anything; Rust threads always run in parallel.)
pub fn shader_parallelism(ctx: &Ctx) -> (bool, &'static str) {
    let on = !matches!(
        app_setting(ctx, "parallel_shaders")
            .unwrap_or_else(|| "1".into())
            .to_lowercase()
            .as_str(),
        "0" | "off" | "no" | "false"
    );
    if on {
        (true, "on")
    } else {
        (
            false,
            "off (the app setting \"Parallel shader preparation\")",
        )
    }
}

struct SplitLog<'a> {
    inner: &'a dyn Log,
    mem: &'a dyn MemSource,
    last: Mutex<Instant>,
}

impl Log for SplitLog<'_> {
    fn line(&self, line: &str) {
        self.inner.line(line);
        if let Some(rest) = line.strip_prefix("progress:") {
            // the decision is taken under the lock, the app's log is called outside it (a callback that logs back would deadlock)
            let due = {
                let mut last = self.last.lock().unwrap();
                if last.elapsed().as_secs() >= 15 {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            };
            if due {
                mem_line(&format!("reading {}", rest.trim()), self.inner, self.mem);
            }
        }
    }
}

/// `^\s*([\w .-]+): (\d+)/(\d+) shaders`: the translation's progress line.
pub fn parse_progress(line: &str) -> Option<(u64, u64)> {
    let t = line.trim_start();
    let (prefix, rest) = t.split_once(':')?;
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ' ' | '.' | '-'))
    {
        return None;
    }
    let rest = rest.strip_prefix(' ')?;
    let (a, rest) = rest.split_once('/')?;
    let b: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if a.is_empty()
        || !a.chars().all(|c| c.is_ascii_digit())
        || b.is_empty()
        || !rest[b.len()..].starts_with(" shaders")
    {
        return None;
    }
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// `build_shaders_on_device`: stage 3 on the phone: the game's shaders become a library THIS host takes, with no Mac (docs/GAME-APP.md §8.7). The result is installed as a shader pack,
/// `Documents/shaders/<id>/`, exactly where an imported pack goes. Skipped when this game already has a pack of the host's chain. Returns the pack folder, or `None` (nothing to build,
/// a stop, or a failure the log says: `build_shaders` tells the failure apart).
pub fn build_shaders_on_device(ctx: &mut Ctx) -> Option<PathBuf> {
    build_shaders(ctx).ok().flatten()
}

/// `build_shaders_on_device`, with a failure told apart (review r3 §3.1): `Ok(Some(pack))` the library is there, `Ok(None)` there is nothing to build (no chain in the config, `shader_build=0`,
/// a game whose split finds no shader) or a stop ended the stage, `Err` the game was to get a library and has none (too little space, no backend, inputs missing, the split, the translation or
/// the install failed). The chain goes on after an `Err` too; it counts as a failed stage 3.
pub fn build_shaders(ctx: &mut Ctx) -> Result<Option<PathBuf>> {
    let tid = ctx.title_id();
    let chain = ctx.cfg.get_or("shader_chain", "").to_string();
    let Some(tid) = tid.filter(|_| !chain.is_empty()) else {
        ctx.log("stage 3 shaders: no title id or no host chain in the config -- not built on this device");
        return Ok(None);
    };
    let pack = ctx.shader_packs.join(&tid);
    if let Some(what) = recover_pack(&pack) {
        ctx.log(&format!("stage 3 shaders: {what}"));
    }
    install_shader_packs(ctx); // an imported pack is used, never rebuilt and then overwritten
    let none_marker = with_ext(&pack, ".none");
    let dlc = aoc_images(ctx, &tid);
    let dlc_sig = dlc_signature(&dlc);
    let split_ver = split_version(ctx);
    if is_file(&none_marker) {
        let t = read_text(&none_marker).unwrap_or_default();
        if !none_marker_current(&t, &split_ver) {
            ctx.log(&format!("stage 3 shaders: {tid} held no shader for another version of the shader split; this app's is {split_ver} -- split again"));
            let _ = fs::remove_file(&none_marker);
        }
    }
    if is_file(&none_marker) {
        let t = read_text(&none_marker).unwrap_or_default();
        if !t.contains(&format!("dlc {dlc_sig}\n")) && dlc_sig != "-" {
            ctx.log(&format!("stage 3 shaders: {tid} held no shader without its DLC; DLC {dlc_sig} is installed now -- split again"));
            let _ = fs::remove_file(&none_marker);
        }
    }
    if is_file(&none_marker) {
        let t = read_text(&none_marker).unwrap_or_default();
        ctx.log(&format!(
            "stage 3 shaders: {tid} holds no shader this split reads ({}) -- not tried again",
            t.trim()
        ));
        return Ok(None);
    }
    if let Some(stale) = stale_split(&pack, &split_ver) {
        ctx.log(&format!(
            "stage 3 shaders: Documents/shaders/{tid} was split by another version of the shader split ({stale}, this app {split_ver}): it may miss shaders this version finds -- built again"
        ));
        rmtree(&pack);
    }
    if let Some((had, want)) = stale_dlc(&pack, &dlc_sig) {
        ctx.log(&format!("stage 3 shaders: Documents/shaders/{tid} was split with DLC {had}, installed now {want}: split again so the DLC's own shaders are translated too"));
        rmtree(&pack);
    }
    let lib_names = [format!("b-{chain}.img"), format!("b-{chain}")];
    if list_names(&pack).iter().any(|n| lib_names.contains(n))
        && map_has_rows(&pack.join("shader_map.tsv"))
    {
        ctx.log(&format!("stage 3 shaders: Documents/shaders/{tid} already holds a library of chain {chain} -- nothing to build"));
        write_stage(
            &ctx.docs,
            3,
            5,
            &format!("shaders: the library for {tid} is already here"),
            Some(&*ctx.log),
        );
        top_up_pack(ctx, &tid, &pack); // a pack built before the app carried the game's small input tables (pso_manifest.tsv, ...) gets them now
        return Ok(Some(pack));
    }
    if ctx.cfg.get_or("shader_build", "1") == "0" {
        ctx.log("stage 3 shaders: shader_build=0 -- not built on this device");
        return Ok(None);
    }
    // A library of another chain (an app update moved the chain: paches/engine) is one this host refuses; it goes before the build, which needs the space (a Grizzy library is ~2 GB).
    if is_dir(&pack) {
        for name in list_names(&pack) {
            if name.starts_with("b-") && !lib_names.contains(&name) {
                let path = pack.join(&name);
                let r = if is_dir(&path) {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                match r {
                    Ok(()) => ctx.log(&format!(
                        "stage 3 shaders: {name} removed (chain {}, this host takes {chain})",
                        name[2..].split('.').next().unwrap_or("")
                    )),
                    Err(e) => ctx.log(&format!(
                        "stage 3 shaders: {name} could not be removed: {e}"
                    )),
                }
            }
        }
    }
    let romfs = PathBuf::from(ctx.cfg.get_or("romfs", ""));
    let main_nso = Path::new(ctx.cfg.get_or("exe_dir", "")).join("main");
    let derived = crate::profile::table_dir(ctx, &tid); // the derived table, or the engine's own for a registered game
    let mut missing: Vec<String> = [&romfs, &main_nso]
        .into_iter()
        .filter(|p| !p.exists())
        .map(|p| p.display().to_string())
        .collect();
    if derived.is_none() {
        missing.push(format!("a profile table for {tid}"));
    }
    if !missing.is_empty() {
        let why = format!(
            "cannot build on this device, missing {}",
            missing.join(", ")
        );
        ctx.log(&format!("stage 3 shaders: [RED] {why}"));
        return Err(PipelineError::failed(why));
    }
    let derived = derived.unwrap();
    rmtree(&with_ext(&pack, ".part")); // a pack an earlier run was killed while installing: never a valid one, and it holds a whole library's room
    let need = (ctx.cfg.int_or("shader_free_gib", 8).max(0) as u64).saturating_mul(1 << 30); // ~4.4 GB of work at the peak + the 2.2 GB library (Grizzy) + margin
    // Review r5 P7, decided (2026-11-22): the gate counts the "important usage" capacity, Apple's API for space a user-initiated task may count on (iOS purges for it on demand), and not the
    // strict `statvfs` value: on the owner's device statvfs said 24 GB where Settings said more than 100 GB, and the strict number would refuse builds that fit. Both are logged below. When
    // the important-usage value cannot be read, `free_bytes` falls back to the strict one (never to 0); when neither can be read it is -1 and the gate is skipped, as before.
    let have = crate::sys::free_bytes(ctx.docs.root());
    if let Some(imp) = crate::sys::free_bytes_important(ctx.docs.root()) {
        let gb = |b: i64| b as f64 / (1u64 << 30) as f64;
        ctx.log(&format!(
            "stage 3 shaders: free: {:.1} GB for important use, {:.1} GB without purgeable space",
            gb(imp),
            gb(crate::sys::free_bytes_strict(ctx.docs.root()).max(0))
        ));
    }
    if have >= 0 && (have as u64) < need {
        let why = format!(
            "{:.1} GB free, the translation needs about {} GB",
            have as f64 / (1u64 << 30) as f64,
            need >> 30
        );
        ctx.log(&format!("stage 3 shaders: [RED] {why} -- free some space (Files app: old games, Documents/shaders/<id> of games you no longer play) and run again"));
        return Err(PipelineError::failed(why));
    }
    let (Some(backend), Some(tools)) = (ctx.adapters.shaders.clone(), ctx.adapters.tools.clone())
    else {
        let why = "no shader backend or no translator tools in this build";
        ctx.log(&format!(
            "stage 3 shaders: [RED] {why} -- not built on this device"
        ));
        return Err(PipelineError::failed(why));
    };
    ctx.thermal.set_log(ctx.log.clone());
    let gate = Arc::new(ShaderMemoryGate::new(
        ctx.mem.clone(),
        ctx.log.clone(),
        Some(ctx.thermal.clone()),
        ctx.clock.clone(),
    ));
    let gated = tools.gated(&gate);
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let shader_jobs = ctx.cfg.int_or("shader_jobs", 0).clamp(0, 256); // a negative or absurd value is not a thread count
    let mut jobs = if shader_jobs != 0 {
        shader_jobs as usize
    } else {
        cpus.clamp(2, 6)
    };
    let (parallel, why) = shader_parallelism(ctx);
    let mut build_jobs = jobs;
    if parallel && shader_jobs == 0 {
        build_jobs = std::cmp::min(6, cpus); // an explicit shader_jobs in the config still wins
    }
    let work = with_ext_dir(&ctx.shader_packs, &format!("{tid}.work"));
    // A work folder left behind is a translation that never finished: the process ended under it (iOS ended the app at 938 of 21,990 shaders on 2026-11-18, a memory kill: no signal in the
    // chain log). This one takes no chance. What that run finished is kept (ShaderResume): the same inputs resume, anything partial is done again.
    let stopped_marker = work.join(STOPPED_BY_USER);
    let user_stopped = is_file(&stopped_marker);
    let left_behind = is_dir(&work) && !user_stopped; // a build a person stopped is not one the OS ended
    if left_behind && shader_jobs == 0 {
        build_jobs = 1;
        jobs = 1;
        ctx.log(&format!("stage 3 shaders: [ORANGE] the previous shader build of {tid} never finished (its work folder was still there): this one reads and translates on 1 thread, slower but within memory"));
    }
    if ctx.thermal.read().1 && shader_jobs == 0 {
        // Low performance mode: at most 2 at once
        if std::cmp::max(jobs, build_jobs) > 2 {
            ctx.log("stage 3 shaders: Low performance mode is on: at most 2 jobs at once");
        }
        jobs = jobs.min(2);
        build_jobs = build_jobs.min(2);
    }
    // Reading: a worker can hold a whole archive (South Park: iOS ended the app at 483 of 499 files, the big ones last, six at a time); translating: a few hundred MB each (Grizzy: ended at 938 of 21,990 shaders).
    jobs = shader_jobs_for_memory(jobs, &*ctx.mem, &*ctx.log, 900 * MIB, "reading");
    build_jobs = shader_jobs_for_memory(
        build_jobs,
        &*ctx.mem,
        &*ctx.log,
        SHADER_PER_JOB,
        "translation",
    );
    ctx.log(&format!("stage 3 shaders: parallel shader preparation {why}; the build finishes shaders on {build_jobs} thread{} (Rust threads: no interpreter lock)", if build_jobs == 1 { "" } else { "s" }));
    release_memory("before-shaders", &*ctx.log, &*ctx.mem);
    let sig = shader_work_signature(
        &split_ver,
        &tid,
        &chain,
        &dlc,
        &romfs,
        &main_nso,
        Some(&derived),
    );
    let mut resume = ShaderResume::new(&work, &sig);
    let resumed = resume.open();
    if user_stopped {
        let _ = fs::remove_file(&stopped_marker);
        ctx.log(&format!(
            "stage 3 shaders: the earlier run was stopped by the user: its work {}",
            if resumed { "is continued at full speed (nothing in it was lost: what it finished is not done again)" } else { "is dropped: the game, its DLC or this app changed since" }
        ));
    }
    if left_behind {
        ctx.log(&format!(
            "stage 3 shaders: the earlier run's work {}",
            if resumed {
                "is kept: the same game, DLC and chain, so what it finished is not done again"
            } else {
                "is dropped: the game, its DLC or this app changed since"
            }
        ));
    }
    if ctx.cancelled() {
        return Ok(keep_work_for_stop(
            ctx,
            &work,
            "before the game's shaders were read",
        ));
    }
    ctx.stats.clear();
    ctx.stats.set_int("resumed", resumed as i64);
    ctx.stats.set_int("read_jobs", jobs as i64);
    if let Some(l) = crate::sys::qos::log_line(3) {
        ctx.log(&l);
    }
    ctx.stats.set_int("jobs", build_jobs as i64);
    let t0 = Instant::now();
    write_stage(
        &ctx.docs,
        3,
        5,
        &format!(
            "shaders: reading the game's shaders ({tid}{})",
            if dlc.is_empty() {
                String::new()
            } else {
                format!(" and {} DLC", dlc.len())
            }
        ),
        Some(&*ctx.log),
    );
    if !dlc.is_empty() {
        ctx.log(&format!("stage 3 shaders: the DLC is read with the game ({dlc_sig}): its own shaders get a translation too"));
    }
    let n: usize;
    {
        let split_log = SplitLog {
            inner: &*ctx.log,
            mem: &*ctx.mem,
            last: Mutex::new(Instant::now()),
        };
        let done = if resumed { resume.split_done() } else { None };
        if let Some(d) = done {
            ctx.log(&format!("stage 3 shaders: the split of the earlier run is complete ({d} shaders): not read again"));
            ctx.stats.set_int("split_reused", 1);
            n = d;
        } else {
            let (pkg, tables, memo) = (work.join("pkg"), work.join("tables"), resume.jobs_dir());
            let req = SplitRequest {
                romfs: &romfs,
                main_nso: &main_nso,
                pkg_dir: &pkg,
                tables_dir: &tables,
                profile_dir: derived.parent().unwrap_or(Path::new("")),
                game_id: &tid,
                dlc: &dlc,
                jobs,
                memo_dir: &memo,
                gate: &gate,
            };
            match backend.split(&req, &split_log) {
                Ok(r) => {
                    if r.jobs_reused > 0 {
                        ctx.log(&format!("stage 3 shaders: {} split job(s) came from the earlier run, {} ran now", r.jobs_reused, r.jobs_ran));
                    }
                    ctx.stats.set_int("split_jobs_reused", r.jobs_reused as i64);
                    if r.shaders > 0 {
                        resume.mark_split(r.shaders);
                    }
                    n = r.shaders;
                }
                Err(e) => {
                    if ctx.cancelled() {
                        ctx.log(&format!("stage 3 shaders: the split ended early ({e})"));
                        return Ok(keep_work_for_stop(
                            ctx,
                            &work,
                            "during the split (the jobs it finished are in splitjobs/)",
                        ));
                    }
                    ctx.log(&format!("stage 3 shaders: [RED] the split failed: {e}"));
                    rmtree(&work);
                    return Err(PipelineError::failed(format!("the split failed: {e}")));
                }
            }
        }
    }
    ctx.stats.set_float("split_s", t0.elapsed().as_secs_f64());
    ctx.stats.set_int("shaders", n as i64);
    release_memory("after-split", &*ctx.log, &*ctx.mem); // the split's buffers go back before the translation starts
    ctx.log(&format!(
        "stage 3 shaders: {n} shaders found in {:.0} s",
        t0.elapsed().as_secs_f64()
    ));
    if n == 0 {
        ctx.log("stage 3 shaders: the game ships no shader this split reads (it may compile them at run time: a Mac capture is needed for such a game) -- nothing built");
        rmtree(&work);
        let marker = format!(
            "the split found 0 shaders in RomFS.img and main\ndlc {dlc_sig}\nsplit {}\n",
            split_version(ctx)
        );
        if let Err(e) = write_atomic(&none_marker, marker.as_bytes()) {
            ctx.log(&format!(
                "stage 3 shaders: could not write {}: {e} (the next run splits again)",
                none_marker.display()
            ));
        }
        return Ok(None);
    }
    if ctx.cancelled() {
        return Ok(keep_work_for_stop(
            ctx,
            &work,
            "after the split, before the translation",
        ));
    }
    rmtree(&work.join("profile")); // made again from the tables: a resumed run finds the last one's
    rmtree(&work.join("out")); // nothing in it is kept: the build's own work and image
    let prof = work.join("profile").join(&tid);
    if let Err(e) = fs::create_dir_all(&prof) {
        ctx.log(&format!(
            "stage 3 shaders: [RED] the translation failed: {e}"
        ));
        rmtree(&work);
        return Err(PipelineError::failed(format!(
            "the translation failed: {e}"
        )));
    }
    for name in list_names(&derived) {
        // the table only (a registered game's folder in the bundle holds just game.tsv)
        if is_file(&derived.join(&name)) {
            let _ = fs::copy(derived.join(&name), prof.join(&name));
        }
    }
    for t in SHADER_TABLES {
        let src = work.join("tables").join(t);
        if is_file(&src) {
            let _ = fs::copy(&src, prof.join(t));
        }
    }
    write_stage(
        &ctx.docs,
        3,
        5,
        &format!("shaders: translating for this iPhone (0/{n} shaders)"),
        Some(&*ctx.log),
    );
    let out = work.join("out");
    let pulling = shader_pulling_wanted(&work.join("profile"), &ctx.cfg);
    ctx.log(&format!(
        "stage 3 shaders: pulling libraries (Metal 4 pipelines) {}",
        if pulling { "built (the profile turns pso_mtl4 on)" } else { "skipped (pso_mtl4 off and the app has no Metal 4 switch: about a quarter of the image and of the build)" }
    ));
    let t_build = Instant::now();
    let cas = shader_cache_folder(ctx, &chain, resumed).unwrap_or_else(|| {
        // no shared cache: the build's own, kept in the work folder, which is what a resumed run restores from
        let c = work.join("cas");
        let _ = fs::create_dir_all(&c);
        c
    });
    let img = {
        let last_mem = Mutex::new(Instant::now() - std::time::Duration::from_secs(60));
        let docs = ctx.docs.clone();
        let (log, mem) = (ctx.log.clone(), ctx.mem.clone());
        // the shader count in gameapp.stage: at most once a second (the app reads it once a second), the last count always (review r4 §6.3)
        let mut stage = crate::stage::StageThrottle::new(std::time::Duration::from_secs(1));
        let mut progress = |line: &str| {
            log.line(line);
            if let Some((done, total)) = parse_progress(line) {
                stage.offer(
                    &docs,
                    3,
                    5,
                    format!("shaders: translating for this iPhone ({done}/{total} shaders)"),
                    done >= total,
                    Instant::now(),
                );
                let mut lm = last_mem.lock().unwrap();
                if lm.elapsed().as_secs() >= 15 {
                    // the memory as the translation goes: what a killed run leaves to read
                    *lm = Instant::now();
                    mem_line(&format!("shaders {done}/{total}"), &*log, &*mem);
                }
            }
        };
        let req = BuildRequest {
            pkg_dir: &work.join("pkg"),
            profile_dir: &work.join("profile"),
            game_id: &tid,
            out_dir: &out,
            chain: &chain,
            jobs: build_jobs,
            tables: ctx.cfg.get_or("shader_tables", "1") != "0",
            pulling,
            cas: &cas,
            tools: &gated,
        };
        let built = backend.build(&req, &mut progress);
        stage.flush(&docs); // a count held back by the throttle: the last one lands, whatever the build's end
        built
    };
    // this chain's cache folder inside the cache, and its size: measured once with the whole cache's for the line below, and said again when it is removed after the build (nothing
    // writes into the cache in between: it walked the 22,000-43,000 files twice; review r4 §6.3)
    let chain_cache = cas.join(crate::shaderpack::shader_cache_name(&chain));
    let chain_cache_bytes: u64;
    let img = match img {
        Ok(i) => {
            let (total, in_chain) = cache_sizes(&cas, &chain_cache);
            chain_cache_bytes = in_chain;
            ctx.log(&format!(
                "shader cache: {}, {} after this build",
                cas.display(),
                gib_text(total as f64)
            ));
            i
        }
        Err(e) => {
            if ctx.cancelled() {
                // the finished shaders are whole in the cache (each is renamed into place when it is done): the next build restores them
                return Ok(keep_work_for_stop(
                    ctx,
                    &work,
                    &format!(
                        "during the translation ({e}); what finished is in the shader cache {}",
                        cas.display()
                    ),
                ));
            }
            ctx.log(&format!(
                "stage 3 shaders: [RED] the translation failed: {e}"
            ));
            rmtree(&work);
            return Err(PipelineError::failed(format!(
                "the translation failed: {e}"
            )));
        }
    };
    release_memory("after-build", &*ctx.log, &*ctx.mem); // the translation's buffers go back before the pack is installed and the framework assembled
    ctx.stats
        .set_float("translate_s", t_build.elapsed().as_secs_f64());
    ctx.stats.set_float("cooldown_s", ctx.thermal.paused_s());
    ctx.stats
        .set_int("peak_mib", (ctx.mem.mem().peak / MIB) as i64);
    // The pack folder: the library, and the tables the host reads BESIDE the shader map (c1_full and cb_extent come out of the build's own profile copy).
    let tmp = with_ext(&pack, ".part");
    rmtree(&tmp);
    let assemble = || -> std::io::Result<()> {
        fs::create_dir_all(&tmp)?;
        fs::rename(&img, tmp.join(img.file_name().unwrap_or_default()))?;
        for name in list_names(&prof) {
            if name.ends_with(".tsv") {
                fs::copy(prof.join(&name), tmp.join(&name))?;
            }
        }
        let built = out.join("_work").join("profile").join(&tid);
        for name in ["c1_full.tsv", "cb_extent.tsv"] {
            if is_file(&built.join(name)) {
                fs::copy(built.join(name), tmp.join(name))?;
            }
        }
        fs::write(
            tmp.join("pack.txt"),
            format!(
                "game\t{tid}\nchain\t{chain}\nmade\ton this device, {}\nshaders\t{n}\nsplit\t{split_ver}\ndlc\t{dlc_sig}\n",
                utc_now_iso()
            ),
        )?;
        install_pack(&tmp, &pack)
    };
    if let Err(e) = assemble() {
        rmtree(&tmp); // the half-made pack (a whole library: ~2.2 GB for Grizzy) is of no use; the old one, if any, is still in its place (install_pack)
        ctx.log(&format!(
            "stage 3 shaders: [RED] the pack could not be installed: {e}"
        ));
        return Err(PipelineError::failed(format!(
            "the pack could not be installed: {e}"
        )));
    }
    rmtree(&work);
    // The pack takes every .tsv of the game's derived folder; a registered game whose derived folder holds only game.tsv (after the heap check, which copies just that file) still gets the
    // small input tables the bundle carries for it
    top_up_pack(ctx, &tid, &pack);
    // The cache served this build (and would serve a resumed one); a FINISHED build has no use for it, and it is one small file per shader: 22,000 for Grizzy, 30,000 for Mario Kart 8, 43,000 for
    // Ocarina of Time, all under Documents. The app walks Documents to show its sizes and to draw its screens (device log, 2026-11-19: "launch: view built in" went from 173 ms to 2-3 s once two
    // games were built), so every walk of the phone's folders paid for them. `shader_cache_keep=1` keeps it (another game on the same chain would restore shared shaders from it).
    if ctx.cfg.get_or("shader_cache_keep", "0") != "1" {
        let dir = chain_cache;
        if dir.is_dir() {
            rmtree(&dir);
            ctx.log(&format!("shader cache: {} removed after the build ({} freed, many small files): shader_cache_keep=1 keeps it", dir.display(), gib_text(chain_cache_bytes as f64)));
        }
    }
    ctx.log(&format!(
        "stage 3 shaders: Documents/shaders/{tid} built on this device in {:.0} s ({n} shaders, Metal source: the host compiles each library at its first use)",
        t0.elapsed().as_secs_f64()
    ));
    Ok(Some(pack))
}

/// Put the finished pack `tmp` (`<id>.part`) in the place of `pack`. The old pack is moved aside (`<id>.old`) first and removed only once the new one is in its place: a rename that fails
/// puts it back, so a failed install never costs the library the game had (review r3 §4.6). An `<id>.old` with no `<id>` beside it is the ONLY copy of the previous library (a kill between
/// the two renames of an earlier install that `recover_pack` could not put back): it is never removed before the new pack is in place (review r4 §4.1).
pub(crate) fn install_pack(tmp: &Path, pack: &Path) -> std::io::Result<()> {
    let old = with_ext(pack, ".old");
    if exists(pack) {
        // beside a pack in place, an `.old` is an earlier generation (an install that finished but for its last removal): the pack in place is the copy that counts
        remove_path(&old);
        fs::rename(pack, &old)?;
    }
    if let Err(e) = fs::rename(tmp, pack) {
        if exists(&old) && !exists(pack) {
            let _ = fs::rename(&old, pack);
        }
        return Err(e);
    }
    remove_path(&old);
    Ok(())
}

/// What an install that a kill cut short left behind (review r4 §4.1). `install_pack` moves the old pack aside (`<id>.old`) before it renames the new one into place: a kill between the two
/// renames leaves `<id>.old` and no `<id>`, and that is the game's only library: it is put back. An `<id>.old` beside an `<id>` is what an install that finished left before its last step:
/// the new pack is in place, and the old one goes (it holds a whole library's room). `None` when there is nothing to do; else what was done, for the log.
pub fn recover_pack(pack: &Path) -> Option<String> {
    let old = with_ext(pack, ".old");
    if !exists(&old) {
        return None;
    }
    if exists(pack) {
        remove_path(&old);
        return Some(format!(
            "{} removed: an install that finished had not removed it yet ({} is the library)",
            old.display(),
            pack.display()
        ));
    }
    Some(match fs::rename(&old, pack) {
        Ok(()) => format!(
            "{} put back as {}: an install was cut short between its two renames, and this was the game's library",
            old.display(),
            pack.display()
        ),
        Err(e) => format!(
            "{} could not be put back as {} ({e}): it is kept (an install never removes it before a new pack is in place)",
            old.display(),
            pack.display()
        ),
    })
}

/// The chain's start (before a boot reads a pack or a build decides whether one is there): `recover_pack` for every `<id>.old` of the shader packs folder, and every `<id>.part` removed
/// (the pack an install was making when the process died: never a valid one, and it holds a whole library's room; only the chain makes them, and no install runs at its start).
pub fn recover_interrupted_installs(ctx: &Ctx) {
    for name in list_names(&ctx.shader_packs) {
        if let Some(id) = name.strip_suffix(".old")
            && !id.is_empty()
            && let Some(what) = recover_pack(&ctx.shader_packs.join(id))
        {
            ctx.log(&format!("shader pack: {what}"));
        } else if name.len() > ".part".len() && name.ends_with(".part") {
            remove_path(&ctx.shader_packs.join(&name));
            ctx.log(&format!(
                "shader pack: {name} removed (an install the process died in: never a valid pack)"
            ));
        }
    }
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

/// A folder with what is in it, or a file (`remove_dir_all` follows no link: a symlink is removed as a file).
fn remove_path(p: &Path) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => rmtree(p),
        Ok(_) => {
            let _ = fs::remove_file(p);
        }
        Err(_) => {}
    }
}

/// `(folder_bytes(cas), folder_bytes(chain_dir))` in one walk, `chain_dir` being an entry of `cas` (this chain's cache folder): the same numbers, the same rules (a link is followed for its
/// size, never walked into), the big folder read once instead of twice. The chain's entry is found by its NAME (review r5 P17: comparing whole paths missed it when `cas` and `chain_dir` were
/// spelt differently, `a/../cas` against `cas`, and the line said 0 for it).
fn cache_sizes(cas: &Path, chain_dir: &Path) -> (u64, u64) {
    let (mut total, mut in_chain) = (0u64, 0u64);
    let Ok(rd) = fs::read_dir(cas) else {
        return (0, 0);
    };
    let chain_name = chain_dir.file_name();
    for e in rd.flatten() {
        let p = e.path();
        let n = match e.file_type() {
            Ok(t) if t.is_dir() => folder_bytes(&p),
            Ok(_) => crate::util::entry_bytes(&e),
            Err(_) => 0,
        };
        if chain_name == Some(e.file_name().as_os_str()) {
            in_chain = n;
        }
        total += n;
    }
    (total, in_chain)
}

/// Stage 3 ends early because a stop was asked for: what it finished stays (the work folder, the shader cache), the build's own folder goes, and the folder says a person stopped it. Always `None`:
/// the stage has no pack to hand back (`ShadersStage` turns that into the stop, not into "no library").
fn keep_work_for_stop(ctx: &Ctx, work: &Path, what: &str) -> Option<PathBuf> {
    rmtree(&work.join("out")); // the build's own work and image: nothing in it is kept (the next run makes them again)
    let text = format!("{} {}\n", stop::name(ctx.stop_kind()), utc_now_iso());
    match write_atomic(&work.join(STOPPED_BY_USER), text.as_bytes()) {
        Ok(()) => ctx.log(&format!("[stop] stage 3 shaders: stopped {what}: {} is kept, a later run continues from it", work.display())),
        Err(e) => ctx.log(&format!("[stop] stage 3 shaders: stopped {what}; {} is kept but could not be marked ({e}): the next run reads and translates on one thread", work.display())),
    }
    None
}

fn with_ext_dir(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

/// `do_shaders`: stage 3: the game's shaders for this host (`build_shaders_on_device`), or the engine's `finish()` rewrite sequence over a captured manifest when the config names one.
pub fn do_shaders(ctx: &mut Ctx) -> Result<Option<PathBuf>> {
    let manifest = ctx.cfg.get_or("shader_manifest", "").to_string();
    if manifest.is_empty() || !is_dir(Path::new(&manifest)) {
        return build_shaders(ctx);
    }
    write_stage(
        &ctx.docs,
        3,
        5,
        &format!("shader post-rewrites ({manifest})"),
        Some(&*ctx.log),
    );
    let out = ctx
        .cfg
        .get("shader_out")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(ctx.cfg.get_or("extract_out", "")).join("shader_out"));
    fs::create_dir_all(&out)
        .map_err(|e| PipelineError::io(format!("create {}", out.display()), e))?;
    let backend = ctx
        .adapters
        .shaders
        .clone()
        .ok_or_else(|| PipelineError::failed("no shader backend in this build"))?;
    let rc = backend.post_rewrite(Path::new(&manifest), &out, &*ctx.log)?;
    let keys = read_text(&Path::new(&manifest).join("keys.txt"))
        .map(|t| t.split_whitespace().count())
        .unwrap_or(0);
    ctx.log(&format!("stage 3 shaders: {keys} keys -> rc={rc}"));
    Ok(Some(out))
}

// ---- live shaders (paches/0008) -------------------------------------------------------------------------------------------------------------

pub const LEARNED: &str = "learned_shaders";

/// What the live thread needs of the context, owned (the thread outlives any borrow).
#[derive(Clone)]
pub struct LiveCtx {
    pub docs: Docs,
    pub shader_packs: PathBuf,
    pub bundle: PathBuf,
    pub backend: Arc<dyn ShaderBackend>,
    pub tools: ShaderTools,
    pub log: Arc<dyn Log>,
}

impl LiveCtx {
    pub fn from_ctx(ctx: &Ctx) -> Option<LiveCtx> {
        Some(LiveCtx {
            docs: ctx.docs.clone(),
            shader_packs: ctx.shader_packs.clone(),
            bundle: ctx.bundle.clone(),
            backend: ctx.adapters.shaders.clone()?,
            tools: ctx.adapters.tools.clone()?,
            log: ctx.log.clone(),
        })
    }
}

pub fn learned_dir(docs: &Docs, tid: &str, chain: &str) -> PathBuf {
    docs.game_dir(tid).join(LEARNED).join(format!("b-{chain}"))
}

pub fn learned_raw(docs: &Docs, tid: &str) -> PathBuf {
    docs.game_dir(tid).join(LEARNED).join("raw")
}

/// `learned_keys`: `{key: stage}` of the shaders translated while this game ran, for this chain (`learned_map.tsv`).
pub fn learned_keys(docs: &Docs, tid: &str, chain: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(t) = read_text(&learned_dir(docs, tid, chain).join("learned_map.tsv")) {
        for line in t.lines() {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() > 3 && !line.starts_with('#') {
                out.insert(c[3].to_string(), c[2].to_string());
            }
        }
    }
    out
}

/// `learn_shaders`: translates shaders the game used that its library lacks (as the host hands them over, paches/0008) and keeps them for the next starts: the files the host reads go to
/// `learned_shaders/b-<chain>/` (each written aside and renamed, so the host never reads half a file), their rows to its `learned_map.tsv` (the host's DM_NVN_SHMAP_LEARNED), their
/// bytes to `raw/` (an app update with another chain translates them again, `relearn_shaders`). Returns `(key -> stage, key -> why refused)`.
pub fn learn_shaders(
    lc: &LiveCtx,
    entries: &[LiveEntry],
    tid: &str,
    chain: &str,
    pack: &Path,
    log: &dyn Log,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let derived = crate::tsv::game_table_dir(&lc.docs.profile_derived(), &lc.bundle, tid);
    let Some(derived) = derived else {
        return Ok((
            BTreeMap::new(),
            entries
                .iter()
                .map(|e| (e.key.clone(), format!("no profile table for {tid}")))
                .collect(),
        ));
    };
    let work = lc.docs.game_dir(tid).join(LEARNED).join("work");
    let cache = lc.docs.shader_cache();
    let cas = if is_dir(&cache) {
        Some(cache.as_path())
    } else {
        None
    };
    let r = lc.backend.live_translate(
        &LiveRequest {
            entries,
            pack,
            derived: &derived,
            game_id: tid,
            work: &work,
            chain,
            tools: &lc.tools,
            cas,
        },
        log,
    )?;
    let dest = learned_dir(&lc.docs, tid, chain);
    let raw = learned_raw(&lc.docs, tid);
    fs::create_dir_all(&dest)?;
    fs::create_dir_all(&raw)?;
    for (name, data) in &r.files {
        let tmp = dest.join(format!(".{name}.part"));
        fs::write(&tmp, data)?;
        fs::rename(&tmp, dest.join(name))?;
    }
    let have = learned_keys(&lc.docs, tid, chain);
    let stamp = utc_now_iso();
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dest.join("learned_map.tsv"))?;
        for (key, stage_name) in &r.ok {
            if !have.contains_key(key) {
                writeln!(f, "learned\t0\t{stage_name}\t{key}\tin play {stamp}")?;
            }
        }
    }
    for e in entries {
        if r.ok.contains_key(&e.key) {
            fs::write(raw.join(format!("{}.ctl", e.key)), &e.ctl)?;
            fs::write(raw.join(format!("{}.code", e.key)), &e.code)?;
        }
    }
    rmtree(&work);
    Ok((r.ok, r.bad))
}

/// `relearn_shaders`: before a boot: the shaders this game learned in play (`raw/`) that this chain has no translation of -- an app update moved the chain, or a session ended before
/// they were kept -- are translated now (a few seconds each at most), and the folders of other chains go. Returns how many were translated.
pub fn relearn_shaders(lc: &LiveCtx, tid: &str, chain: &str, pack: &Path) -> usize {
    let base = lc.docs.game_dir(tid).join(LEARNED);
    if !is_dir(&base) {
        return 0;
    }
    for name in list_names(&base) {
        if name.starts_with("b-") && name != format!("b-{chain}") {
            rmtree(&base.join(&name));
            lc.log.line(&format!(
                "learned shaders: {name} removed (another chain: this host takes b-{chain})"
            ));
        }
    }
    let raw = learned_raw(&lc.docs, tid);
    let have = learned_keys(&lc.docs, tid, chain);
    let mut keys: Vec<String> = list_names(&raw)
        .into_iter()
        .filter(|n| n.ends_with(".ctl"))
        .map(|n| n[..n.len() - 4].to_string())
        .filter(|k| !have.contains_key(k))
        .collect();
    keys.sort();
    keys.dedup();
    let mut entries = Vec::new();
    for k in keys {
        if let (Ok(ctl), Ok(code)) = (
            fs::read(raw.join(format!("{k}.ctl"))),
            fs::read(raw.join(format!("{k}.code"))),
        ) {
            entries.push(LiveEntry { key: k, ctl, code });
        }
    }
    if entries.is_empty() {
        return 0;
    }
    lc.log.line(&format!("learned shaders: {} learned in play have no translation of chain {chain} yet -- translating them now", entries.len()));
    match learn_shaders(lc, &entries, tid, chain, pack, &*lc.log) {
        Ok((ok, bad)) => {
            for (k, why) in bad {
                lc.log
                    .line(&format!("learned shaders: {k} refused ({why})"));
            }
            ok.len()
        }
        Err(e) => {
            lc.log.line(&format!("learned shaders: [RED] could not translate them ({e}): they are translated again when the game meets them"));
            0
        }
    }
}

/// The live translation thread of one boot (paches/0008): it takes the shaders the host queues (`LiveQueue::next`), translates them (`learn_shaders`) and answers (`done`).
/// `start` sets the host's switches; `stop` after the boot.
pub struct LiveShaders {
    thread: Option<std::thread::JoinHandle<()>>,
    count: Arc<[AtomicUsize; 2]>,
    queue: Arc<LiveQueue>,
    log: Arc<dyn Log>,
    env: Arc<dyn crate::env::Env>,
}

impl LiveShaders {
    pub fn start(ctx: &Ctx) -> LiveShaders {
        let count = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let mut me = LiveShaders {
            thread: None,
            count: count.clone(),
            queue: ctx.live.clone(),
            log: ctx.log.clone(),
            env: ctx.env.clone(),
        };
        let log = ctx.log.clone();
        let tid = ctx.title_id();
        let chain = ctx.cfg.get_or("shader_chain", "").to_string();
        let mode = ctx
            .env
            .get("DM_LIVE_SHADERS")
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "pause".to_string());
        let Some(lc) = LiveCtx::from_ctx(ctx) else {
            log.line("live shaders: this app has no live translation (no shader backend)");
            return me;
        };
        if mode == "off" {
            log.line("live shaders: off (the game's settings): a shader missing from the library is drawn without");
            return me;
        }
        let pack = tid
            .as_ref()
            .map(|t| ctx.shader_packs.join(t))
            .unwrap_or_default();
        let (Some(tid), false) = (tid, chain.is_empty()) else {
            log.line("live shaders: no library for this game yet (or no host chain in the config): nothing to complete");
            return me;
        };
        if !is_file(&pack.join("shader_map.tsv")) {
            log.line("live shaders: no library for this game yet (or no host chain in the config): nothing to complete");
            return me;
        }
        relearn_shaders(&lc, &tid, &chain, &pack);
        let dest = learned_dir(&ctx.docs, &tid, &chain);
        let _ = fs::create_dir_all(&dest);
        ctx.env.set("DM_MSL_LEARNED", &dest.to_string_lossy());
        ctx.env.set(
            "DM_NVN_SHMAP_LEARNED",
            &dest.join("learned_map.tsv").to_string_lossy(),
        );
        ctx.env.set("GAMEAPP_LIVE_THREAD", "1");
        let n = learned_keys(&ctx.docs, &tid, &chain).len();
        log.line(&format!(
            "live shaders: {mode} -- a shader missing from the library is translated while the game runs{}; {n} learned in earlier sessions",
            if mode == "pause" { " (the game pauses a moment for it)" } else { " (drawn a moment later)" }
        ));
        let queue = ctx.live.clone();
        let mem = ctx.mem.clone();
        let spawned = std::thread::Builder::new()
            .name("live-shaders".into())
            .spawn(move || {
                loop {
                    let Some(batch) = queue.next(1000) else {
                        return;
                    };
                    if batch.is_empty() {
                        continue;
                    }
                    let t0 = Instant::now();
                    let entries: Vec<LiveEntry> = batch
                        .into_iter()
                        .map(|(key, ctl, code)| LiveEntry { key, ctl, code })
                        .collect();
                    let (ok, bad) = match learn_shaders(
                        &lc,
                        &entries,
                        &tid,
                        &chain,
                        &pack,
                        &crate::log::NullLog,
                    ) {
                        Ok(r) => r,
                        Err(e) => (
                            BTreeMap::new(),
                            entries
                                .iter()
                                .map(|en| (en.key.clone(), format!("translation failed: {e}")))
                                .collect(),
                        ),
                    };
                    for (k, st) in &ok {
                        queue.done(k, Some(st));
                    }
                    for (k, why) in &bad {
                        queue.done(k, None);
                        lc.log
                            .line(&format!("live shaders: [RED] {k} refused: {why}"));
                    }
                    count[0].fetch_add(ok.len(), Ordering::SeqCst);
                    count[1].fetch_add(bad.len(), Ordering::SeqCst);
                    // The game needs the memory more: what the translation freed goes back
                    mem.trim();
                    lc.log.line(&format!(
                        "live shaders: {} translated in {:.1} s ({}){}",
                        ok.len(),
                        t0.elapsed().as_secs_f64(),
                        ok.iter()
                            .map(|(k, v)| format!("{k} {v}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        if bad.is_empty() {
                            String::new()
                        } else {
                            format!(", {} refused", bad.len())
                        }
                    ));
                }
            });
        match spawned {
            Ok(t) => me.thread = Some(t),
            // no thread: the host must not hand shaders to a queue nobody reads (`GAMEAPP_LIVE_THREAD` is what wires its hook); the boot goes on without (review r3 §5.8: a release build
            // aborts on a panic, so `expect` here ended the app over a missing convenience)
            Err(e) => {
                ctx.env.remove("GAMEAPP_LIVE_THREAD");
                log.line(&format!("live shaders: [RED] the live translation thread could not be started ({e}): a shader missing from the library is drawn without"));
            }
        }
        me
    }

    /// After the boot: no more live translation.
    pub fn stop(&mut self) {
        let Some(t) = self.thread.take() else { return };
        self.queue.stop();
        let t0 = Instant::now();
        while !t.is_finished() && t0.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if t.is_finished() {
            let _ = t.join();
        }
        self.env.remove("GAMEAPP_LIVE_THREAD");
        self.log.line(&format!(
            "live shaders: this session translated {} shader(s) the library lacked ({} refused); they are kept for the next starts",
            self.count[0].load(Ordering::SeqCst),
            self.count[1].load(Ordering::SeqCst)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::MemLog;
    use crate::memory::FakeMem;
    use std::time::Duration;

    fn lib(pack: &std::path::Path) -> Option<String> {
        fs::read_to_string(pack.join("b-x.img")).ok()
    }

    /// Review r3 §4.6: the old library stays until the new pack is in its place; a pack that cannot be put there leaves the old one as it was and nothing beside it.
    #[test]
    fn a_pack_that_cannot_be_put_in_place_leaves_the_old_library() {
        let d = crate::testutil::TempDir::new("install-pack");
        let pack = d.join("0100DFA027504000");
        crate::testutil::write(pack.join("b-x.img"), "OLD");
        let tmp = with_ext(&pack, ".part"); // never made: the rename fails
        assert!(install_pack(&tmp, &pack).is_err());
        assert_eq!(lib(&pack).as_deref(), Some("OLD"));
        assert_eq!(
            list_names(d.path()),
            ["0100DFA027504000"],
            "nothing left beside it"
        );
        // and a pack that is there replaces the old one, leaving nothing beside it
        crate::testutil::write(tmp.join("b-x.img"), "NEW");
        install_pack(&tmp, &pack).unwrap();
        assert_eq!(lib(&pack).as_deref(), Some("NEW"));
        assert_eq!(list_names(d.path()), ["0100DFA027504000"]);
        // a first install (no old pack)
        let pack2 = d.join("0100000000010000");
        let tmp2 = with_ext(&pack2, ".part");
        crate::testutil::write(tmp2.join("b-x.img"), "FIRST");
        install_pack(&tmp2, &pack2).unwrap();
        assert_eq!(lib(&pack2).as_deref(), Some("FIRST"));
        assert!(!tmp2.exists());
    }

    /// Review r4 §4.1: a kill between `install_pack`'s two renames leaves `<id>.old` and no `<id>`. That is the game's only library: `recover_pack` puts it back, and an install that
    /// comes first never removes it before the new pack is in place, nor when the new pack cannot be put there.
    #[test]
    fn the_only_copy_of_a_library_survives_a_kill_between_the_renames_and_a_second_failure() {
        let d = crate::testutil::TempDir::new("install-pack-old");
        let pack = d.join("0100DFA027504000");
        let old = with_ext(&pack, ".old");
        let tmp = with_ext(&pack, ".part");
        // the kill's state: the previous library aside, no pack
        crate::testutil::write(old.join("b-x.img"), "OLD");
        // a second install that fails (its pack is not there): the only copy is put back, not removed
        assert!(install_pack(&tmp, &pack).is_err());
        assert_eq!(
            lib(&pack).as_deref(),
            Some("OLD"),
            "the only copy is back in place"
        );
        assert_eq!(list_names(d.path()), ["0100DFA027504000"]);
        // the kill's state again, then an install that works: the new pack in place, the old one gone
        fs::rename(&pack, &old).unwrap();
        crate::testutil::write(tmp.join("b-x.img"), "NEW");
        install_pack(&tmp, &pack).unwrap();
        assert_eq!(lib(&pack).as_deref(), Some("NEW"));
        assert_eq!(list_names(d.path()), ["0100DFA027504000"]);
        // the next start after the kill: recover_pack puts the library back
        fs::rename(&pack, &old).unwrap();
        let what = recover_pack(&pack).expect("something to do");
        assert!(what.contains("put back"), "{what}");
        assert_eq!(lib(&pack).as_deref(), Some("NEW"));
        assert!(!old.exists());
        assert_eq!(recover_pack(&pack), None, "nothing to do the second time");
        // an `.old` beside a pack in place: an install that finished but for its last removal; the pack stays, the `.old` goes
        crate::testutil::write(old.join("b-x.img"), "OLDER");
        assert!(recover_pack(&pack).unwrap().contains("removed"));
        assert_eq!(lib(&pack).as_deref(), Some("NEW"));
        assert!(!old.exists());
        // and an install with an `.old` beside the pack: the earlier generation goes, the pack in place becomes the fallback until the new one is there
        crate::testutil::write(old.join("b-x.img"), "OLDER");
        crate::testutil::write(tmp.join("b-x.img"), "NEWER");
        install_pack(&tmp, &pack).unwrap();
        assert_eq!(lib(&pack).as_deref(), Some("NEWER"));
        assert_eq!(list_names(d.path()), ["0100DFA027504000"]);
    }

    /// The cache's size and its chain folder's size in one walk are what two `folder_bytes` walks said (a link is followed for its size, never walked into).
    #[test]
    fn the_cache_sizes_of_one_walk_are_those_of_two() {
        let d = crate::testutil::TempDir::new("cache-sizes");
        let cas = d.join("shadercache");
        let chain = cas.join("cas-abc.dmsb");
        for i in 0..40 {
            crate::testutil::write(
                chain
                    .join(format!("{:02x}", i % 7))
                    .join(format!("{i:064x}")),
                vec![b'x'; i * 13],
            );
        }
        crate::testutil::write(cas.join("loose.bin"), "12345");
        crate::testutil::write(cas.join("other/deep/f"), "abc");
        #[cfg(unix)]
        std::os::unix::fs::symlink(cas.join("loose.bin"), cas.join("link")).unwrap();
        assert_eq!(
            cache_sizes(&cas, &chain),
            (folder_bytes(&cas), folder_bytes(&chain))
        );
        assert!(folder_bytes(&chain) > 0);
        assert_eq!(cache_sizes(&d.join("none"), &chain), (0, 0));
        // review r5 P17: the same folders spelt another way (`x/../shadercache`, a trailing `.`) still find the chain's entry
        std::fs::create_dir_all(d.join("x")).unwrap();
        let other = d.join("x").join("..").join("shadercache");
        assert_eq!(
            cache_sizes(&other, &chain),
            (folder_bytes(&cas), folder_bytes(&chain))
        );
        assert_eq!(
            cache_sizes(&cas, &other.join("cas-abc.dmsb")),
            (folder_bytes(&cas), folder_bytes(&chain))
        );
    }

    /// The progress line of the split is forwarded at once, and the memory line it triggers at most every 15 s; the rate limiter's lock is not held while the app's log runs
    /// (a log that comes back into the limiter would otherwise wait for itself).
    #[test]
    fn the_split_log_forwards_every_line_and_the_memory_line_every_15_seconds() {
        let log = MemLog::default();
        let mem = FakeMem::default();
        let split = SplitLog {
            inner: &log,
            mem: &mem,
            last: Mutex::new(Instant::now() - Duration::from_secs(20)),
        };
        split.line("progress: 10/100");
        split.line("progress: 11/100");
        split.line("not a progress line");
        let lines = log.lines();
        assert_eq!(
            lines.iter().filter(|l| l.starts_with("progress:")).count(),
            2,
            "{lines:?}"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("[mem:reading 10/100]"))
                .count(),
            1,
            "{lines:?}"
        );
        assert!(lines.contains(&"not a progress line".to_string()));
        assert!(
            split.last.try_lock().is_ok(),
            "the limiter's lock is free after a line"
        );
    }
}
