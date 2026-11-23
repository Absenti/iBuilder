//! `shader_device.build` + `dmsh_build.py` (module level, batches, `translate`, `fin`, summary, `publish_library`) for the iPhone target in source mode.
//!
//! What a build does (the Python's order, its two phases and its batch size kept; everything that was a file, a SQLite row or an image rewrite is a stream here):
//!
//! 1. the profile folder is copied to `<out>/_work/profile/<game>` (the tables are written there; the pipeline reads them from that place) and read: `game.tsv`, the shader map, the answer
//!    table, the pair tables, `bf_want` / `dual_want`;
//! 2. every shader's digest (`key::shader_digest`; a vertex shader with pairs takes its pixel shaders') and a look into the cache (`cache::Store`): the **hits** need nothing more;
//! 3. the **misses** as one stream: `jobs` workers, each taking a few shaders at a time (`BuildOptions::xlate_group`), making their jobs (`job_for`), running the translator once over
//!    them and finishing them one by one (`Finisher`) into the cache. A vertex shader with pixel-shader partners (`vs_pad` reads their MSL's locations) is taken as soon as its own
//!    partners are finished (the Python's two phases, per shader instead of a barrier over all of phase 1); its partners are taken first. A group's working folder goes with the group;
//! 4. the tables (`c1_full.tsv`, `cb_extent.tsv`: rows came with each shader), the stamp (`inputs.tsv`) and the library image, streamed from the entries in name order.
//!
//! A shader that fails (translator, spirv-cross, `finish`) has no files and is counted by its status, as in the Python; the build still returns its image.
use crate::cache::{Meta, Store, Stored, write_atomic};
use crate::finish::{Finished, Finisher};
use crate::game::{
    Game, STAMP, ShaderMap, game_dir, load_facts, load_key_set, load_pairs, stamp_text,
};
use crate::jobs::{JobError, XlateOpts, job_for};
use crate::key::{DigestBase, pair_keyed};
use crate::library::{Source, write_image};
use crate::mesh::{RecordPartners, build_mesh_libs};
use crate::pyjson::{Opts, dumps};
use crate::records::Packages;
use crate::tablerows::{CB_COMMENT, c1_lines, cb_lines, rows_of, table_head};
use crate::toolmemo::{self, MemoTools};
use crate::tools::CountingTools;
use crate::util::{copy_tree, head_chars, iso_now, rmtree};
use ibuilder_pipeline::stop;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use rayon::prelude::*;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// stack of a finishing worker thread
const WORKER_STACK: usize = 8 << 20;

pub struct BuildOptions {
    /// the split's packages (`pkg/{ps,vs,cs,gs}_{data,main}.bin`)
    pub pkg_dir: PathBuf,
    /// the game's profile folder (holds `game.tsv` and the tables) or the folder that holds `<game_id>/`
    pub profile_dir: PathBuf,
    pub game_id: String,
    /// the build's folder: it gets `b-<chain>.img` and `_work/`
    pub out_dir: PathBuf,
    /// the host's chain id (`DMSH_CHAIN`)
    pub chain: String,
    /// threads that translate and finish (the Python's `J`)
    pub jobs: usize,
    /// translator jobs per translator call: a worker takes up to this many shaders at once (fewer near the end, so that no worker sits on several while others are idle), translates
    /// them in one call and finishes them. It changes no output (the translator makes every job on its own, `toolmemo` relies on it too); it trades the per-call cost (a process spawn
    /// on the Mac) against the time a worker holds shaders others could take. Default [`DEFAULT_XLATE_GROUP`], or `IBUILDER_XLATE_GROUP` from the environment; 0 counts as 1
    pub xlate_group: usize,
    /// write `c1_full.tsv` / `cb_extent.tsv` (a full build only)
    pub tables: bool,
    /// the pulling libraries (`_bp`) are built: part of every shader's digest, and what the finisher is told
    pub pulling: bool,
    /// the translation cache folder (`Documents/shadercache`, or the work folder's `cas`)
    pub cas: PathBuf,
    /// a partial build of these shaders (and their paired pixel shaders): 16-hex keys; `None` = every shader of the map
    pub keys: Option<Vec<String>>,
    pub xlate: XlateOpts,
    /// keep `_work/tmp` and the stage folder (debugging)
    pub keep_work: bool,
    /// the process-wide stop request (`ibuilder_pipeline::stop`: the app's Pause and Cancel) ends the build, as `BuildEnv::cancel` does. On for every build of the chain; the live translation of a
    /// running game turns it off (a request that came late is not for the game's shaders)
    pub follow_stop: bool,
    /// the second cache level (`toolmemo`): the translator's and SPIRV-Cross's answers kept in the cache folder, so that a change of the finishing code re-finishes the shaders without
    /// translating them again. Off by default: it about doubles the cache folder, which the pipeline wipes past `shader_cache_gib` (2 GiB by default; see `docs/shaders/SHADERBUILD-RUST.md`)
    pub tool_memo: bool,
}

/// What a build's translator call holds by default: 4 jobs, what the old batches gave each call (a batch of `4 * jobs` in `jobs` calls)
pub const DEFAULT_XLATE_GROUP: usize = 4;

/// [`DEFAULT_XLATE_GROUP`], or `IBUILDER_XLATE_GROUP` (a number from 1 to 64) from the process environment: a measuring knob that needs no option on the callers
pub fn default_xlate_group() -> usize {
    std::env::var("IBUILDER_XLATE_GROUP")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| (1..=64).contains(n))
        .unwrap_or(DEFAULT_XLATE_GROUP)
}

impl BuildOptions {
    pub fn new(
        pkg_dir: &Path,
        profile_dir: &Path,
        game_id: &str,
        out_dir: &Path,
        chain: &str,
        cas: &Path,
    ) -> BuildOptions {
        BuildOptions {
            pkg_dir: pkg_dir.to_path_buf(),
            profile_dir: profile_dir.to_path_buf(),
            game_id: game_id.to_string(),
            out_dir: out_dir.to_path_buf(),
            chain: chain.to_string(),
            jobs: 4,
            xlate_group: default_xlate_group(),
            tables: true,
            pulling: true,
            cas: cas.to_path_buf(),
            keys: None,
            xlate: XlateOpts::default(),
            keep_work: false,
            follow_stop: true,
            tool_memo: false,
        }
    }
}

pub struct BuildEnv<'a> {
    pub tools: &'a dyn ShaderTools,
    pub finisher: &'a dyn Finisher,
    /// every line the Python printed or logged, including the progress lines (`cache check: 12/300 shaders`, `compiling: 12/300 shaders`); called from any thread
    pub log: &'a (dyn Fn(&str) + Sync),
    /// set to stop the build (it returns the error `"cancelled"`): the workers finish the shader in hand and start no other, so at most one shader per worker is done after the flag is raised.
    /// The process-wide request (`ibuilder_pipeline::stop`, `BuildOptions::follow_stop`) stops it the same way.
    pub cancel: Option<&'a AtomicBool>,
}

#[derive(Clone, Debug)]
pub struct ShaderResult {
    pub key: String,
    /// `"ok"`, or the failure kind
    pub status: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct BuildReport {
    pub image: PathBuf,
    pub shaders: usize,
    pub ok: usize,
    /// status -> count, failures only
    pub failed: BTreeMap<String, usize>,
    pub failures: Vec<ShaderResult>,
    pub cache_hit: usize,
    pub cache_miss: usize,
    pub files: usize,
    pub bytes: u64,
    pub c1_unproven: usize,
    pub cb_shaders: usize,
    pub inputs_digest: String,
    /// vertex / geometry pairs the profile asks mesh libraries for (`dmsh_mesh.py`, `mesh_finish`): libraries built, pairs lacking a library of the two shaders, pairs `compose` refused
    pub mesh_built: usize,
    pub mesh_missing: usize,
    pub mesh_not_composed: usize,
    /// ray-tracing modules (stage `rt` / `raytracing`) the map lists: left out of the per-shader path as the Python does, and NOT built into `rt_library` (not ported)
    pub rt_modules: usize,
    pub translate_calls: u64,
    pub translate_s: f64,
    pub spirv_cross_calls: u64,
    pub spirv_cross_s: f64,
    pub wall_s: f64,
    pub digest_s: f64,
    pub library_s: f64,
    /// the tool memo (`BuildOptions::tool_memo`): translator jobs and SPIRV-Cross calls answered from it, and run for real while it was on (0 when it is off)
    pub memo_xlate_hits: u64,
    pub memo_xlate_misses: u64,
    pub memo_cross_hits: u64,
    pub memo_cross_misses: u64,
}

// ------------------------------------------------------------------------------------------------------------------------- the stream

/// Where the workers take the misses from (indices into the build's misses, in key order). Every shader without pairs is ready from the start, the pixel shaders that paired vertex
/// shaders wait for first; a paired vertex shader is ready once the last of its partners this build finishes is done (finished or failed: what the old phase 2 saw after its barrier,
/// per shader), and is then taken before any other.
struct Sched {
    m: Mutex<SchedState>,
    cv: Condvar,
    /// miss -> the paired vertex shaders (misses) waiting for it
    dependents: Vec<Vec<usize>>,
    nj: usize,
    group: usize,
}

struct SchedState {
    /// paired vertex shaders whose partners are done: taken first
    released: VecDeque<usize>,
    ready: VecDeque<usize>,
    /// miss -> its partners not done yet (paired vertex shaders only)
    left: Vec<usize>,
    /// paired vertex shaders still waiting for a partner
    waiting: usize,
    /// a worker ended by a panic: nobody takes work any more (the scope then hands the panic on instead of the others waiting for its shaders)
    broken: bool,
}

/// how often a worker with nothing to take looks at the stop request (a stop is one byte, nobody signals it)
const IDLE_POLL: std::time::Duration = std::time::Duration::from_millis(50);

impl Sched {
    fn new(
        misses: &[String],
        pairs: &HashMap<String, BTreeSet<String>>,
        nj: usize,
        group: usize,
    ) -> Sched {
        let index: HashMap<&str, usize> = misses
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let mut dependents = vec![Vec::new(); misses.len()];
        let mut left = vec![0usize; misses.len()];
        let mut released = VecDeque::new();
        let mut waiting = 0usize;
        for (v, k) in misses.iter().enumerate() {
            let Some(ps) = pairs.get(k) else {
                continue;
            };
            for p in ps {
                // a partner this build finishes now. A partner that has pairs itself is not waited for (the old phase 2 did not wait for its own shaders either; no profile seen has
                // one): waiting for it could wait forever. A hit or a shader the map lacks is no wait (its locations are known, or never will be)
                if let Some(&i) = index.get(p.as_str())
                    && !pairs.contains_key(p)
                {
                    dependents[i].push(v);
                    left[v] += 1;
                }
            }
            if left[v] == 0 {
                released.push_back(v);
            } else {
                waiting += 1;
            }
        }
        let mut ready: VecDeque<usize> = VecDeque::with_capacity(misses.len());
        let mut rest = Vec::new();
        for (i, k) in misses.iter().enumerate() {
            if pairs.contains_key(k) {
                continue;
            }
            if dependents[i].is_empty() {
                rest.push(i);
            } else {
                ready.push_back(i);
            }
        }
        ready.extend(rest);
        Sched {
            m: Mutex::new(SchedState {
                released,
                ready,
                left,
                waiting,
                broken: false,
            }),
            cv: Condvar::new(),
            dependents,
            nj,
            group: group.max(1),
        }
    }

    /// The next shaders for one translator call: up to `group`, fewer when less than `group` per worker is ready (the end of the build, or vertex shaders released one by one).
    /// `None`: nothing is left to take (every shader is taken, or a stop)
    fn take(&self, sh: &Shared) -> Option<Vec<usize>> {
        let mut g = self.m.lock().unwrap();
        loop {
            if g.broken || sh.stopping() {
                return None;
            }
            let avail = g.released.len() + g.ready.len();
            if avail > 0 {
                let n = self.group.min(avail.div_ceil(self.nj));
                let mut out = Vec::with_capacity(n);
                while out.len() < n {
                    match g.released.pop_front().or_else(|| g.ready.pop_front()) {
                        Some(i) => out.push(i),
                        None => break,
                    }
                }
                return Some(out);
            }
            if g.waiting == 0 {
                return None;
            }
            g = self.cv.wait_timeout(g, IDLE_POLL).unwrap().0;
        }
    }

    /// shader `i` is done (finished, or failed): the vertex shaders that waited for it alone are ready
    fn done(&self, i: usize) {
        let deps = &self.dependents[i];
        if deps.is_empty() {
            return;
        }
        let mut g = self.m.lock().unwrap();
        let mut any = false;
        for &v in deps {
            g.left[v] -= 1;
            if g.left[v] == 0 {
                g.released.push_back(v);
                g.waiting -= 1;
                any = true;
            }
        }
        drop(g);
        if any {
            self.cv.notify_all();
        }
    }
}

/// Set on a worker's stack: a worker that ends by a panic tells the others to stop taking work (with `panic = "abort"` the process ends anyway)
struct BreakOnPanic<'a>(&'a Sched);

impl Drop for BreakOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Ok(mut g) = self.0.m.lock() {
                g.broken = true;
            }
            self.0.cv.notify_all();
        }
    }
}

/// progress lines, at most one every half second per step (the Python's `progress()`), and the last one of a step always
struct Progress<'a> {
    log: &'a (dyn Fn(&str) + Sync),
    last: Mutex<Instant>,
}

impl<'a> Progress<'a> {
    fn new(log: &'a (dyn Fn(&str) + Sync)) -> Self {
        Progress {
            log,
            last: Mutex::new(Instant::now() - std::time::Duration::from_secs(10)),
        }
    }
    fn step(&self, step: &str, done: usize, total: usize, force: bool) {
        let mut l = self.last.lock().unwrap();
        if !force && l.elapsed().as_millis() < 500 {
            return;
        }
        *l = Instant::now();
        (self.log)(&format!("{step}: {done}/{total} shaders"));
    }
}

// ------------------------------------------------------------------------------------------------------------------------- shared state

/// A pending job: (the job, its c1).
type PendingJob = (Map<String, Value>, Vec<u8>);

/// The shaders one worker translates in one translator call and then finishes, one after the other; its working folder goes with it
struct Group {
    dir: PathBuf,
    /// key -> (the job, its c1), taken by the shader's `fin`
    jobs: HashMap<String, PendingJob>,
    /// key -> (status, detail) for shaders that never got a job
    pre_fail: HashMap<String, (String, String)>,
    /// the translator's `[RED] key: message` lines
    xerr: HashMap<String, String>,
}

impl Drop for Group {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Shared<'a> {
    env: &'a BuildEnv<'a>,
    /// the caller's tools, counted, behind the tool memo when it is on
    tools: &'a dyn ShaderTools,
    o: &'a BuildOptions,
    map: &'a ShaderMap,
    facts: &'a HashMap<String, Value>,
    pairs: &'a HashMap<String, BTreeSet<String>>,
    packs: &'a Packages,
    store: &'a Store,
    tmp: PathBuf,
    digests: &'a HashMap<String, String>,
    /// key -> the shader's entry (hits and finished shaders)
    stored: Mutex<HashMap<String, Arc<Stored>>>,
    /// key -> the `user(locnN)` numbers of the finished shaders' MSL (a pixel shader's matter to its paired vertex shaders)
    locs: Mutex<HashMap<String, BTreeSet<u32>>>,
    results: Mutex<Vec<ShaderResult>>,
    done: AtomicUsize,
    total: usize,
    progress: Progress<'a>,
    /// a stop was seen: kept, so that a request withdrawn while the build winds down cannot turn it back into a complete one with shaders missing
    stopped: AtomicBool,
}

/// the build's own token or the process-wide request
fn stop_asked(env: &BuildEnv, o: &BuildOptions) -> bool {
    env.cancel
        .map(|c| c.load(Ordering::SeqCst))
        .unwrap_or(false)
        || (o.follow_stop && stop::requested() != stop::NONE)
}

impl Shared<'_> {
    fn log(&self, s: &str) {
        (self.env.log)(s);
    }

    /// True from the first time a stop is seen on. One relaxed load of the process-wide byte per call when there is none.
    fn stopping(&self) -> bool {
        if self.stopped.load(Ordering::Relaxed) {
            return true;
        }
        if stop_asked(self.env, self.o) {
            if !self.stopped.swap(true, Ordering::SeqCst) {
                let kind = if self.o.follow_stop {
                    stop::requested()
                } else {
                    stop::NONE
                };
                self.log(&format!(
                    "[stop] the shader build was asked to {}: the shaders in hand are finished and kept, no other is started ({}/{} done)",
                    if kind == stop::PAUSE { "pause" } else { "stop" },
                    self.done.load(Ordering::SeqCst),
                    self.total
                ));
            }
            return true;
        }
        false
    }

    fn finished_one(&self) {
        let n = self.done.fetch_add(1, Ordering::SeqCst) + 1;
        self.progress.step("compiling", n, self.total, false);
    }

    fn record_result(&self, key: &str, status: &str, detail: &str) {
        self.results.lock().unwrap().push(ShaderResult {
            key: key.to_string(),
            status: status.to_string(),
            detail: detail.to_string(),
        });
    }
}

/// `xlate_errors`: `[RED] <key>: <message>` lines of the translator's stderr
pub(crate) fn parse_xlate_errors(stderr: &str, into: &mut HashMap<String, String>) {
    for ln in stderr.lines() {
        let Some(rest) = ln.strip_prefix("[RED] ") else {
            continue;
        };
        let key: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if key.is_empty() {
            continue;
        }
        if let Some(msg) = rest[key.len()..].strip_prefix(": ") {
            into.insert(key, head_chars(msg, 160));
        }
    }
}

/// `translate(batch)` for one worker's group: the shaders' jobs, and one translator call over them (the Python's `miss[i::nj]` chunk of a batch, here the group). A stop seen while the jobs
/// are made leaves the rest without one and calls no translator: the worker starts none of them
fn translate_group(sh: &Shared, id: usize, keys: &[&str]) -> Group {
    let dir = sh.tmp.join(format!("group_{id}"));
    let _ = std::fs::create_dir_all(&dir);
    let mut jobs: HashMap<String, PendingJob> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    let mut pre_fail: HashMap<String, (String, String)> = HashMap::new();
    let mut xerr: HashMap<String, String> = HashMap::new();
    for &k in keys {
        if sh.stopping() {
            return Group {
                dir,
                jobs,
                pre_fail,
                xerr,
            };
        }
        let (name, _) = &sh.map.loc[k];
        let code_path = dir.join(format!("{k}.bin"));
        let made = sh
            .packs
            .record(name, k)
            .map_err(JobError::Red)
            .and_then(|rec| {
                job_for(
                    k,
                    name,
                    &rec,
                    &code_path.to_string_lossy(),
                    sh.facts.get(k),
                    &sh.o.xlate,
                )
            });
        match made {
            Ok(j) => {
                for n in &j.notes {
                    sh.log(n);
                }
                if let Err(e) = std::fs::write(&code_path, &j.code) {
                    pre_fail.insert(
                        k.to_string(),
                        (
                            "translation failed".into(),
                            format!("{}: {e}", code_path.display()),
                        ),
                    );
                    continue;
                }
                jobs.insert(k.to_string(), (j.job, j.c1));
                order.push(k);
            }
            Err(JobError::Red(m)) => {
                sh.log(&m);
                pre_fail.insert(
                    k.to_string(),
                    ("translation failed".into(), head_chars(&m, 160)),
                );
            }
            Err(JobError::Unported(m)) => {
                pre_fail.insert(k.to_string(), (format!("unported: {m}"), String::new()));
            }
        }
    }
    if !order.is_empty() && !sh.stopping() {
        let text = format!(
            "[{}]",
            order
                .iter()
                .map(|k| dumps(&Value::Object(jobs[*k].0.clone()), &Opts::DEFAULT))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let jp = dir.join("job.json");
        match std::fs::write(&jp, text) {
            Err(e) => {
                for k in &order {
                    xerr.insert(k.to_string(), format!("{}: {e}", jp.display()));
                }
            }
            Ok(()) => {
                let r = sh.tools.xlate(&[
                    jp.to_string_lossy().into_owned(),
                    dir.to_string_lossy().into_owned(),
                ]);
                parse_xlate_errors(&r.stderr, &mut xerr);
            }
        }
    }
    Group {
        dir,
        jobs,
        pre_fail,
        xerr,
    }
}

/// the working files of a finished shader (`<key>.bin` / `.spv` / `.res.json` / `.metal`, the geometry variants' `<key>_g<n>.*`, `job1_<key>.json`)
fn remove_work_files(dir: &Path, key: &str) {
    let job1 = format!("job1_{key}");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(key) || n.starts_with(&job1) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// `fin(key, wb)` for a miss
fn fin(sh: &Shared, b: &mut Group, key: &str) {
    let digest = &sh.digests[key];
    let outcome = (|| -> (String, String, Option<Arc<Stored>>) {
        if let Some((st, d)) = b.pre_fail.get(key) {
            return (st.clone(), d.clone(), None);
        }
        let spv = b.dir.join(format!("{key}.spv"));
        if let Some(e) = b.xerr.get(key) {
            return ("translation failed".into(), e.clone(), None);
        }
        if !spv.exists() {
            return ("translation failed".into(), "no spv".into(), None);
        }
        let Some((job, c1)) = b.jobs.remove(key) else {
            return ("translation failed".into(), "no job".into(), None);
        };
        let mut job = Value::Object(job);
        let package_of = |ps: &str| sh.map.loc.get(ps).map(|l| l.0.clone());
        let partners = RecordPartners {
            packs: sh.packs,
            package_of: &package_of,
            locs: &sh.locs,
        };
        let fnr = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sh.env
                .finisher
                .finish_with(key, &b.dir, &mut job, &c1, sh.tools, &partners)
        }));
        let f: Finished = match fnr {
            Ok(f) => f,
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                Finished::failed("finish panicked", head_chars(&msg, 160))
            }
        };
        for l in &f.log {
            sh.log(l);
        }
        if !f.is_ok() {
            return (f.status, f.detail, None);
        }
        let mut meta = if sh.o.tables && sh.o.keys.is_none() {
            rows_of(&f.outputs)
        } else {
            Meta::default()
        };
        meta.locs = f.ps_locs.iter().copied().collect();
        // a vertex shader built without a paired pixel shader's MSL (that shader failed, or was not built) is not cached: its padding is incomplete
        // (`publish_cache`'s rule); pairs naming a shader the map does not hold have no MSL in the Python either, and do not stop the cache
        let cacheable = sh
            .pairs
            .get(key)
            .map(|ps| {
                ps.iter()
                    .filter(|p| sh.map.loc.contains_key(*p))
                    .all(|p| sh.locs.lock().unwrap().contains_key(p))
            })
            .unwrap_or(true);
        match sh.store.commit(digest, &meta, &f.outputs, cacheable) {
            Ok(st) => {
                sh.locs
                    .lock()
                    .unwrap()
                    .insert(key.to_string(), f.ps_locs.clone());
                ("ok".into(), String::new(), Some(Arc::new(st)))
            }
            Err(e) => (
                "library write failed".into(),
                head_chars(&e.to_string(), 160),
                None,
            ),
        }
    })();
    remove_work_files(&b.dir, key);
    if let Some(st) = outcome.2 {
        sh.stored.lock().unwrap().insert(key.to_string(), st);
    }
    sh.record_result(key, &outcome.0, &outcome.1);
    sh.finished_one();
}

/// The build ends because of a stop. What it made for this run only (the stage folder of hard links, the batches' folders) goes; every shader that finished is in the translation cache, whole.
/// The error text is the one `BuildEnv::cancel` has always given.
/// What a build that could not write its outputs leaves: nothing of its own but the cache entries (the stage, the work files, the mesh outputs and the tables it wrote go;
/// `keep_work` keeps the work folders for a look)
fn clean_failed_build(
    o: &BuildOptions,
    store: &Store,
    tmp: &Path,
    mesh_dir: Option<&Path>,
    tables: &[PathBuf],
) {
    if !o.keep_work {
        store.remove_stage();
        rmtree(tmp);
        if let Some(m) = mesh_dir {
            rmtree(m);
        }
    }
    for t in tables {
        let _ = std::fs::remove_file(t);
    }
}

/// The second half of a failed build's red line: what the next build reuses, and what to do (upstream's wording)
fn failed_build_hint(store: &Store, disk_full: bool) -> String {
    let reuse = if store.cache_full.load(Ordering::Relaxed) {
        "The shader cache filled up earlier in this build, so only the shaders cached before that will be reused"
    } else {
        "The shaders compiled so far are in the shader cache, and a build again takes them from there"
    };
    let action = if disk_full {
        "free space on the drive and build again"
    } else {
        "fix the error and build again"
    };
    format!("{reuse}: {action}")
}

fn stopped_build(o: &BuildOptions, store: &Store, tmp: &Path) -> String {
    if !o.keep_work {
        store.remove_stage();
        rmtree(tmp);
    }
    "cancelled".to_string()
}

/// `dmsh_build.build_keys`: a partial build takes its paired pixel shaders along (vs_pad reads their MSL); a full build every shader of the map
fn build_keys(
    want: Option<&Vec<String>>,
    map: &ShaderMap,
    pairs: &HashMap<String, BTreeSet<String>>,
) -> Result<Vec<String>, String> {
    match want {
        None => {
            let mut k: Vec<String> = map.loc.keys().cloned().collect();
            k.sort();
            Ok(k)
        }
        Some(w) => {
            let mut set: BTreeSet<String> = w.iter().cloned().collect();
            for k in w {
                if !map.loc.contains_key(k) {
                    return Err(format!("[RED] {k} is not in the shader map"));
                }
                for ps in pairs.get(k).into_iter().flatten() {
                    if map.loc.contains_key(ps) {
                        set.insert(ps.clone());
                    }
                }
            }
            Ok(set.into_iter().collect())
        }
    }
}

/// The identity of the tools for the digest: what `spirv-cross --version` says (the Python's `tool_fingerprint`); the chain id covers the translator's sources. Also the answer itself
/// (the tool memo takes SPIRV-Cross's identity from it)
fn tools_fingerprint(t: &dyn ShaderTools) -> (String, ToolRun) {
    let r = t.spirv_cross(&["--version".to_string()]);
    (
        format!("spirv-cross {}\0{}", r.stdout.trim(), r.stderr.trim()),
        r,
    )
}

pub fn build(o: &BuildOptions, env: &BuildEnv) -> Result<BuildReport, String> {
    let t_start = Instant::now();
    let log = env.log;
    let nj = o.jobs.max(1);
    let out_dir = if o.out_dir.is_absolute() {
        o.out_dir.clone()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(&o.out_dir)
    };
    let work = out_dir.join("_work");
    rmtree(&work);
    let tmp = work.join("tmp");
    let prof = work.join("profile");
    for d in [&tmp, &prof] {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    // the profile folder is copied first, nothing in it is changed (the tables go into the copy)
    let src = game_dir(&o.profile_dir, &o.game_id);
    if !src.join("game.tsv").is_file() {
        return Err(format!(
            "{} holds no profile of game {} (<dir>/{}/game.tsv)",
            o.profile_dir.display(),
            o.game_id,
            o.game_id
        ));
    }
    let gdir = prof.join(&o.game_id);
    copy_tree(&src, &gdir).map_err(|e| format!("copy of the profile {}: {e}", src.display()))?;
    let game = Game::load(&gdir)?;
    let inputs = game.inputs_digest();
    let map = ShaderMap::load(&game.table("shader_map.tsv"))?;
    let facts_path = game.table("shader_facts.tsv");
    if !facts_path.exists() {
        return Err(format!(
            "[RED] no answer table {} (it comes with the game profile; an empty table = every query answered with defaults)",
            facts_path.display()
        ));
    }
    let facts = load_facts(&facts_path)?;
    let pairs = load_pairs(&game, &o.pkg_dir)?;
    let bf_want = load_key_set(&game.table("bf_want.tsv"), false);
    let dual_want = load_key_set(&game.table("dual_want.tsv"), true);
    let keys = build_keys(o.keys.as_ref(), &map, &pairs)?;
    // `keys = [key for key in keys if key not in RT_KEYS]`: the ray-tracing modules are not shaders of the per-shader path (the Python gives them one universal library)
    let rt_modules = map.rt_keys.len();
    let keys: Vec<String> = keys
        .into_iter()
        .filter(|k| !map.rt_keys.contains(k))
        .collect();
    if rt_modules > 0 {
        log(&format!(
            "[RED] {rt_modules} ray-tracing module(s) in the shader map: the universal RT library (dmsh_rt_build / dmsh_rt_compile, `rt_library`) is not built by this port -- ray-traced draws of this game will not work"
        ));
    }
    let counting = CountingTools::new(env.tools);
    let (mut tools_fp, cross_version) = tools_fingerprint(&counting);
    // with the tool memo, the translator states its identity (`--source-sha`): the memo keys on it, and so does every shader's digest (the chain id covers the translator's sources too,
    // but a chain given by hand on the Mac may not; without the memo nothing changes, the translator is not asked)
    let xlate_id = o.tool_memo.then(|| {
        let r = env.tools.xlate(&["--source-sha".to_string()]);
        tools_fp.push_str(&format!("\0xlate {}\0{}", r.stdout.trim(), r.rc));
        toolmemo::xlate_identity(&r)
    });
    let base = DigestBase::new(
        &o.chain,
        &env.finisher.fingerprint(),
        &tools_fp,
        o.pulling,
        &o.xlate,
        &game,
    );
    let packs = Packages::new(&o.pkg_dir);

    let swept = Store::sweep_other_chains(&o.cas, &o.chain);
    for n in &swept {
        log(&format!(
            "shader cache: {n} removed (another chain version: none of its entries can match)"
        ));
    }
    let store = Store::open(&o.cas, &o.chain, &work.join("stage"))
        .map_err(|e| format!("shader cache {}: {e}", o.cas.display()))?;
    // the second level: the tools' own answers, keyed by the tools' identities and their exact inputs (`toolmemo`); a tool that states no identity is not memoized
    let memo = xlate_id.map(|xid| {
        toolmemo::sweep(&store.cache);
        let cid = toolmemo::cross_identity(&cross_version);
        let m = MemoTools::new(&counting, &store.cache, &o.chain, xid, cid);
        log(&format!(
            "tool memo: on (translator {}, SPIRV-Cross {})",
            if m.translator_memoized() {
                "kept"
            } else {
                "not kept: it gave no --source-sha"
            },
            if m.spirv_cross_memoized() {
                "kept"
            } else {
                "not kept: its --version gave no commit line"
            }
        ));
        m
    });
    let tools: &dyn ShaderTools = match &memo {
        Some(m) => m,
        None => &counting,
    };

    // ---- digests ---------------------------------------------------------------------------------------------------------------------
    let t_dig = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(nj)
        .start_handler(|_| ibuilder_pipeline::sys::qos::lower_worker_qos())
        .build()
        .map_err(|e| e.to_string())?;
    let plain = |k: &str| -> Result<String, String> {
        if stop_asked(env, o) {
            return Err("cancelled".into()); // one read of the process-wide byte per shader: the 22,000 digests are a long loop too
        }
        let (name, _) = &map.loc[k];
        let rec = packs.record(name, k)?;
        Ok(base.shader_digest(
            name,
            &rec,
            map.rows.get(k).map(|v| v.as_slice()),
            map.conflict_rows.get(k).map(|v| v.as_slice()),
            facts.get(k).map(|v| (k, v)),
            pairs.get(k),
            dual_want.contains(k),
            bf_want.contains(k),
        ))
    };
    let plain_digests: Vec<Result<String, String>> =
        pool.install(|| keys.par_iter().map(|k| plain(k)).collect());
    if stop_asked(env, o) {
        log(
            "[stop] the shader build was asked to stop while the digests were made: nothing was translated yet",
        );
        return Err(stopped_build(o, &store, &tmp));
    }
    let mut digests: HashMap<String, String> = HashMap::with_capacity(keys.len());
    for (k, d) in keys.iter().zip(plain_digests) {
        digests.insert(k.clone(), d?);
    }
    for k in &keys {
        if let Some(ps) = pairs.get(k) {
            let mut pd = Vec::new();
            for p in ps {
                pd.push(match digests.get(p) {
                    Some(d) => d.clone(),
                    None if map.loc.contains_key(p) => plain(p)?, // a paired pixel shader that is not built this time
                    None => "missing".to_string(),
                });
            }
            let d = pair_keyed(&digests[k], &pd);
            digests.insert(k.clone(), d);
        }
    }
    let digest_s = t_dig.elapsed().as_secs_f64();

    // ---- the cache -------------------------------------------------------------------------------------------------------------------
    let progress = Progress::new(log);
    progress.step("cache check", 0, keys.len(), true);
    let looked: Vec<Option<Stored>> = pool.install(|| {
        keys.par_iter()
            .map(|k| {
                if stop_asked(env, o) {
                    None
                } else {
                    store.lookup(&digests[k])
                }
            })
            .collect()
    });
    if stop_asked(env, o) {
        log(
            "[stop] the shader build was asked to stop during the cache check: nothing was translated yet",
        );
        return Err(stopped_build(o, &store, &tmp));
    }
    let tables_wanted = o.tables && o.keys.is_none();
    let mut stored: HashMap<String, Arc<Stored>> = HashMap::new();
    let mut locs_map: HashMap<String, BTreeSet<u32>> = HashMap::new();
    let mut hits: Vec<String> = Vec::new();
    for (k, st) in keys.iter().zip(looked) {
        if let Some(mut st) = st {
            if tables_wanted && !st.meta.tables {
                // an entry made by a build without tables: its rows are made from its files now (the entry itself stays as it is)
                match st.read_all() {
                    Ok(files) => st.meta = rows_of(&files),
                    Err(_) => continue,
                }
            }
            hits.push(k.clone());
            locs_map.insert(k.clone(), st.meta.locs.iter().copied().collect());
            stored.insert(k.clone(), Arc::new(st));
        }
    }
    progress.step("cache check", keys.len(), keys.len(), true);
    let hit_set: BTreeSet<&String> = hits.iter().collect();
    let misses: Vec<String> = keys
        .iter()
        .filter(|k| !hit_set.contains(k))
        .cloned()
        .collect();

    let shared = Shared {
        env,
        tools,
        o,
        map: &map,
        facts: &facts,
        pairs: &pairs,
        packs: &packs,
        store: &store,
        tmp: tmp.clone(),
        digests: &digests,
        stored: Mutex::new(stored),
        locs: Mutex::new(locs_map),
        results: Mutex::new(
            hits.iter()
                .map(|k| ShaderResult {
                    key: k.clone(),
                    status: "ok".into(),
                    detail: String::new(),
                })
                .collect(),
        ),
        done: AtomicUsize::new(hits.len()),
        total: keys.len(),
        progress: Progress::new(log),
        stopped: AtomicBool::new(false),
    };

    // ---- translate and finish --------------------------------------------------------------------------------------------------------
    // One stream: every worker takes a few shaders, translates them in one call and finishes them, until none is left. The paired vertex shaders read their pixel shaders' MSL
    // (vs_pad): each waits for its own partners only (`Sched`), not for every shader without pairs.
    let sched = Sched::new(&misses, &pairs, nj, o.xlate_group);
    let group_ids = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for i in 0..nj {
            let (shared, sched, misses, group_ids) = (&shared, &sched, &misses, &group_ids);
            // a worker runs the finisher's scanners over 20-45 KB texts with recursive descent in places: more than the default 2 MiB stack, as virtual address space only
            std::thread::Builder::new()
                .name(format!("shaderbuild-{i}"))
                .stack_size(WORKER_STACK)
                .spawn_scoped(s, move || {
                    ibuilder_pipeline::sys::qos::lower_worker_qos();
                    let _break = BreakOnPanic(sched);
                    while let Some(taken) = sched.take(shared) {
                        let keys: Vec<&str> = taken.iter().map(|&m| misses[m].as_str()).collect();
                        let mut g = translate_group(
                            shared,
                            group_ids.fetch_add(1, Ordering::Relaxed),
                            &keys,
                        );
                        for (&m, k) in taken.iter().zip(&keys) {
                            if shared.stopping() {
                                break; // a stop: no shader is started any more (the one in hand was finished above)
                            }
                            fin(shared, &mut g, k);
                            sched.done(m);
                        }
                    }
                })
                .expect("spawn a shader build worker");
        }
    });
    if shared.stopping() {
        // every shader that was finished is in the cache whole (an entry is written beside its name and renamed into place by the worker that made it, before that worker takes the next one)
        log(&format!(
            "[stop] shader build stopped: {} of {} shaders are finished and kept in the shader cache ({} of them from before this build); the next build restores them",
            shared.done.load(Ordering::SeqCst),
            keys.len(),
            hits.len()
        ));
        return Err(stopped_build(o, &store, &tmp));
    }
    shared.progress.step(
        "compiling",
        shared.done.load(Ordering::SeqCst),
        keys.len(),
        true,
    );

    // ---- summary ---------------------------------------------------------------------------------------------------------------------
    let results = shared.results.lock().unwrap().clone();
    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    for r in &results {
        *by_status.entry(r.status.clone()).or_default() += 1;
    }
    let ok = by_status.get("ok").copied().unwrap_or(0);
    let failed: BTreeMap<String, usize> = by_status
        .iter()
        .filter(|(k, _)| *k != "ok")
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    log(&format!(
        "shaders {}: ok {ok}{}",
        results.len(),
        failed
            .iter()
            .map(|(k, v)| format!(", {k} {v}"))
            .collect::<String>()
    ));
    log(&format!(
        "shader cache: hit {}, rebuilt {}",
        hits.len(),
        misses.len()
    ));
    if !misses.is_empty() {
        log(&format!("  new key {}", misses.len()));
    }
    for st in failed.keys() {
        let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
        for r in results.iter().filter(|r| &r.status == st) {
            *reasons.entry(r.detail.as_str()).or_default() += 1;
        }
        let mut v: Vec<(&str, usize)> = reasons.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        for (d, n) in v.into_iter().take(10) {
            log(&format!("  [{st}] {n:5}  {d}"));
        }
    }
    // an entry that could not be written (a full drive): the library would miss shaders that did build, which is worse than no library (the Python rolled the batch back and went on from files; here the
    // finished entries are in the cache, so the rerun after freeing space resumes)
    if let Some(n) = failed.get("library write failed") {
        let why = results
            .iter()
            .find(|r| r.status == "library write failed")
            .map(|r| r.detail.clone())
            .unwrap_or_default();
        // the stage and the work files go (the next build starts from an empty work folder anyway), so a full drive gets that space back now
        clean_failed_build(o, &store, &tmp, None, &[]);
        return Err(format!(
            "[RED] {n} finished shader(s) could not be written ({why}): is the drive full? {}",
            failed_build_hint(
                &store,
                why.contains("No space left") || why.contains("Disk quota")
            )
        ));
    }
    // a stop that came after the last shader: the library is not composed (every shader is in the cache, the next build only has to compose it)
    if stop_asked(env, o) {
        log(
            "[stop] the shader build was asked to stop after the last shader: every shader is in the shader cache, the library is not composed",
        );
        return Err(stopped_build(o, &store, &tmp));
    }
    // `mesh_finish`: passthrough geometry as mesh shaders, once every shader is finished (dmsh_build.py l.1080-1117)
    let mesh_dir = work.join("mesh");
    let (mesh_files, mesh_stats) = build_mesh_libs(
        &shared.stored.lock().unwrap(),
        &game.mesh_pairs(),
        &mesh_dir,
        &pool,
        &|l| log(l),
    );
    log(&mesh_stats.line());

    // ---- tables, stamp, library ------------------------------------------------------------------------------------------------------
    let t_lib = Instant::now();
    let stored = shared.stored.into_inner().unwrap();
    let mut entries: Vec<(&String, &Arc<Stored>)> = stored.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut files: Vec<(String, Source)> = Vec::new();
    let mut total_names = 0usize;
    for (_, st) in &entries {
        for (n, off, len) in &st.files {
            files.push((
                n.clone(),
                Source::Range {
                    path: st.path.clone(),
                    off: *off,
                    len: *len,
                },
            ));
            total_names += 1;
        }
    }
    let _ = total_names;
    files.extend(mesh_files);
    let (mut c1_n, mut cb_n) = (0usize, 0usize);
    if tables_wanted {
        let head = table_head(&o.chain, &inputs, &iso_now());
        let c1 = c1_lines(entries.iter().map(|(_, s)| s.meta.c1.clone()));
        let cb = cb_lines(entries.iter().map(|(_, s)| s.meta.cb.clone()));
        c1_n = c1.len();
        cb_n = cb.len();
        let mut c1_text = head.clone();
        for l in &c1 {
            c1_text.push_str(l);
            c1_text.push('\n');
        }
        let mut cb_text = head;
        cb_text.push_str(CB_COMMENT);
        for l in &cb {
            cb_text.push_str(l);
            cb_text.push('\n');
        }
        // whole or not at all: stage 3 of the pipeline copies these two out of the build's profile copy after the build returns
        write_atomic(&game.table("c1_full.tsv"), c1_text.as_bytes()).map_err(|e| e.to_string())?;
        write_atomic(&game.table("cb_extent.tsv"), cb_text.as_bytes())
            .map_err(|e| e.to_string())?;
        files.push(("c1_full.tsv".into(), Source::Bytes(c1_text.into_bytes())));
        files.push(("cb_extent.tsv".into(), Source::Bytes(cb_text.into_bytes())));
        let slots: usize = cb.iter().map(|r| r.matches(':').count()).sum();
        let ro: usize = cb
            .iter()
            .map(|r| {
                r.split_once('\t')
                    .map(|x| x.1)
                    .unwrap_or("")
                    .split_whitespace()
                    .filter(|w| {
                        w.starts_with('s')
                            && w[1..].bytes().all(|b| b.is_ascii_digit())
                            && w.len() > 1
                    })
                    .count()
            })
            .sum();
        log(&format!("library stamp: game {}, inputs {inputs}", game.id));
        log(&format!("c1_full.tsv: not proven bounded: {c1_n} shaders"));
        log(&format!(
            "cb_extent.tsv: bounded uniform reads: {slots} slots; storage slots never written: {ro}; {cb_n} shaders"
        ));
    }
    files.push((
        STAMP.into(),
        Source::Bytes(stamp_text(&game.id, &inputs).into_bytes()),
    ));
    let img = out_dir.join(format!("b-{}.img", o.chain));
    // nxnative e3f2d21f: a library that cannot be written (a full drive at the end of a build) leaves no partial output (write_image removes its .part; the stage, the work
    // files, the mesh outputs and this build's two tables go) and stops with one red line that says what to do
    let (size, nfiles) = write_image(files, &img).map_err(|e| {
        let tables: Vec<PathBuf> = if tables_wanted {
            vec![game.table("c1_full.tsv"), game.table("cb_extent.tsv")]
        } else {
            Vec::new()
        };
        clean_failed_build(o, &store, &tmp, Some(&mesh_dir), &tables);
        format!(
            "[RED] could not write {}: {e}. {}",
            img.display(),
            failed_build_hint(&store, crate::cache::is_disk_full(&e))
        )
    })?;
    let library_s = t_lib.elapsed().as_secs_f64();
    log(&format!(
        "{}: {nfiles} files, {size} bytes, {:.0} s",
        img.display(),
        t_start.elapsed().as_secs_f64()
    ));

    if !o.keep_work {
        store.remove_stage();
        rmtree(&tmp);
        rmtree(&mesh_dir);
    }
    let failures: Vec<ShaderResult> = results
        .iter()
        .filter(|r| r.status != "ok")
        .take(1000)
        .cloned()
        .collect();
    let st = &counting.stats;
    let memo_n = |f: fn(&toolmemo::MemoStats) -> &AtomicU64| {
        memo.as_ref()
            .map_or(0, |m| f(&m.stats).load(Ordering::Relaxed))
    };
    if let Some(m) = &memo {
        log(&m.stats.line());
    }
    Ok(BuildReport {
        image: img,
        shaders: results.len(),
        ok,
        failed,
        failures,
        cache_hit: hits.len(),
        cache_miss: misses.len(),
        files: nfiles,
        bytes: size,
        c1_unproven: c1_n,
        cb_shaders: cb_n,
        inputs_digest: inputs,
        mesh_built: mesh_stats.built,
        mesh_missing: mesh_stats.missing,
        mesh_not_composed: mesh_stats.not_composed,
        rt_modules,
        translate_calls: st.xlate_calls.load(Ordering::Relaxed),
        translate_s: st.xlate_ns.load(Ordering::Relaxed) as f64 / 1e9,
        spirv_cross_calls: st.spirv_calls.load(Ordering::Relaxed),
        spirv_cross_s: st.spirv_ns.load(Ordering::Relaxed) as f64 / 1e9,
        wall_s: t_start.elapsed().as_secs_f64(),
        digest_s,
        library_s,
        memo_xlate_hits: memo_n(|s| &s.xlate_hits),
        memo_xlate_misses: memo_n(|s| &s.xlate_misses),
        memo_cross_hits: memo_n(|s| &s.cross_hits),
        memo_cross_misses: memo_n(|s| &s.cross_misses),
    })
}
