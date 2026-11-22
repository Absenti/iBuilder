//! `main()` of `shader_split.py` and `shader_device.split`: read every file of the game's RomFS (an image or a folder, the update's and the DLC's too) and its `main`, find the shaders,
//! merge them by key, write `pkg/<stage>_<group>.bin` and `tables/{shader_map,shader_facts,shader_pairs,pso_pairs}.tsv`.
//!
//! Threads: the files are scanned in jobs of 16 files by `jobs` worker threads (the Python's process pool, or the phone's thread pool); a job's result is memoised on disk
//! ([`JobMemo`]) and every job goes through the caller's [`JobGate`] (the memory gate). The results are merged **in job order**, whatever order the threads finish in, so the
//! output is the same for 1, 2 or 8 threads and for a split resumed from its memo.

use crate::archive::zs_dicts_from;
use crate::ctl::*;
use crate::error::{Failure, SplitError};
use crate::game::{Game, Profile};
use crate::image::{Entry, Loc, walk_root};
use crate::memo::{JobMemo, write_atomic};
use crate::scan::*;
use crate::scan_types::*;
use crate::source::Source;
use crate::spill::{At, Spill};
use crate::unity::{self, Trees};
use crate::zstd::Zstd;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Bump when a change of this crate can change the output of a split (the pipeline's work-folder signature and `stale_split` use [`split_version`]).
/// 2: the rebased engine (Snowdrop bare zlib caches, NVN2 ray tracing, CIVBSF 0x1118, bnd2 v2, Phyre, KIT, `main`'s `uniq_keys`: docs/shaders/SHADERSPLIT-RUST.md section 10)
/// 3: Vector Unit APF packages (`FPUV`, `apf.rs`): their entries are decoded and scanned (Beach Buggy Racing 2: 0 shaders before, 496 after)
pub const SPLIT_REV: u32 = 3;

/// The shader split this build runs, as a short digest (`ShaderBackend::split_version`)
pub fn split_version() -> String {
    let d = JobMemo::key(&[
        "ibuilder-shadersplit",
        env!("CARGO_PKG_VERSION"),
        &SPLIT_REV.to_string(),
    ]);
    format!("rs{}", &d[..10])
}

/// `shader_split.py`'s `SINGLE_STAGE`: sources whose every shader is a program of one stage
const SINGLE_STAGE: [&str; 17] = [
    "#GLSLC@",
    "#VIPER",
    "#CIVBSF@",
    "#UE",
    "#MTSPK@",
    "#MGFX@",
    "#NLG",
    "#PGSO",
    "#RAW@",
    "#TFXO@",
    "#NEEDLE@",
    "#SNOWDROP@",
    "#PHYRE@",
    "#SUMO@",
    "#NVNSH@",
    "#D3@",
    "#SHAD@",
];

/// Lets a job through when the memory allows (the pipeline's `ShaderMemoryGate::run`)
pub trait JobGate: Sync {
    fn run(&self, f: &mut dyn FnMut());
}

pub struct NoGate;
impl JobGate for NoGate {
    fn run(&self, f: &mut dyn FnMut()) {
        f()
    }
}

/// The files of every root, by (root, path below it)
pub struct FileIndex {
    map: HashMap<(usize, String), Loc>,
}

impl FileIndex {
    /// no files at all (what the readers that look for a sibling file see in a unit test)
    pub fn empty() -> FileIndex {
        FileIndex {
            map: HashMap::new(),
        }
    }
    pub fn exists(&self, root: usize, rel: &str) -> bool {
        self.map.contains_key(&(root, rel.to_string()))
    }
    pub fn get(&self, root: usize, rel: &str) -> Option<&Loc> {
        self.map.get(&(root, rel.to_string()))
    }
}

pub struct SplitOptions<'a> {
    /// `(RomFS image or folder, main NSO)`: the base game first, then its updates (the output is the union of the versions)
    pub versions: Vec<(&'a Path, &'a Path)>,
    /// add-on content roots (`aoc/<index>.img` images or folders)
    pub dlc: Vec<&'a Path>,
    pub pkg_dir: &'a Path,
    pub tables_dir: &'a Path,
    /// the game's own profile folder (it holds `game.tsv`) or the folder that holds `<game_id>/`
    pub profile_dir: &'a Path,
    pub game_id: &'a str,
    /// reading threads
    pub jobs: usize,
    /// where finished jobs are kept: an interrupted split resumed gives the same bytes as one that was not
    pub memo_dir: Option<&'a Path>,
    /// `DM_GLSLC_CAPTURE` / `<version dir>/glslc_capture`: GLSLC outputs the game compiles at run time, captured on the Mac (None: the folder beside the RomFS)
    pub glslc_capture: Option<&'a Path>,
    /// `DM_NRO_DIR`: NROs the game decodes at run time, scanned like main
    pub nro_dir: Option<&'a Path>,
}

impl<'a> SplitOptions<'a> {
    pub fn single(
        romfs: &'a Path,
        main: &'a Path,
        pkg_dir: &'a Path,
        tables_dir: &'a Path,
        profile_dir: &'a Path,
        game_id: &'a str,
    ) -> SplitOptions<'a> {
        SplitOptions {
            versions: vec![(romfs, main)],
            dlc: vec![],
            pkg_dir,
            tables_dir,
            profile_dir,
            game_id,
            jobs: 8,
            memo_dir: None,
            glslc_capture: None,
            nro_dir: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SplitStats {
    /// rows of the shader map
    pub shaders: usize,
    pub jobs_reused: usize,
    pub jobs_ran: usize,
    /// files no reader could read (left out, as in the Python)
    pub unreadable: usize,
    /// per stage name (`vertex`, ...)
    pub by_stage: Vec<(String, usize)>,
}

fn io(e: std::io::Error) -> SplitError {
    SplitError::Io(e.to_string())
}

fn red(msg: impl Into<String>) -> SplitError {
    SplitError::Red(msg.into())
}

/// Python's `f'{n:,}'`
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            o.push(',');
        }
        o.push(c);
    }
    o
}

/// the base name of the directory a path is in (`os.path.basename(os.path.dirname(os.path.abspath(p)))`)
fn parent_name(p: &Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    abs.parent()
        .and_then(|d| d.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What `uepak.unreal_paks`, `ue_legacy_jobs` and the other `*_jobs` of the Python look for in the file list. A container this port does not read is an explicit error:
/// leaving it out would give the game fewer shaders than it has.
fn detect_unsupported(files: &[FileRef], index: &FileIndex) -> Result<(), SplitError> {
    let head_of = |f: &FileRef, n: usize| -> Vec<u8> {
        f.loc
            .open()
            .ok()
            .and_then(|s| s.read(0, n).ok())
            .unwrap_or_default()
    };
    let tail_of = |f: &FileRef, n: usize| -> Vec<u8> {
        f.loc
            .open()
            .ok()
            .and_then(|s| {
                let len = s.len();
                s.read(len.saturating_sub(n as u64), n).ok()
            })
            .unwrap_or_default()
    };
    let unsup = |what: &str, f: &FileRef| -> Result<(), SplitError> {
        Err(SplitError::Unsupported(format!("{what} (/{})", f.rel)))
    };
    for f in files {
        let base = f.rel.rsplit('/').next().unwrap_or(&f.rel);
        let lower = f.rel.to_ascii_lowercase();
        if f.rel.ends_with(".pak") {
            if head_of(f, 4) == b"KPKA" {
                continue; // read by jobs::plan
            }
            let t = tail_of(f, 8);
            if t.len() == 8 && &t[4..8] == b"LSPK" {
                return unsup("Larian LSPK package", f);
            }
            let t = tail_of(f, 256);
            if t.windows(4).any(|w| w == [0xe1, 0x12, 0x6f, 0x5a]) {
                return unsup("Unreal Engine pak", f);
            }
        }
        if f.rel.ends_with(".utoc") {
            return unsup("Unreal Engine IoStore container", f);
        }
        if f.rel.ends_with(".trpfs")
            && index.exists(f.root, &format!("{}d", &f.rel[..f.rel.len() - 1]))
            && head_of(f, 8) == b"ONEPACK\0"
        {
            return unsup("Game Freak Trinity pack (data.trpfs)", f);
        }
        if f.rel.ends_with(".arc") && head_of(f, 8) == 0xABCDEF9876543210u64.to_le_bytes() {
            return unsup("Super Smash Bros. Ultimate data.arc", f);
        }
        if lower.ends_with(".cpk") {
            let h = head_of(f, 8);
            if h.starts_with(b"CPK ") {
                return unsup("CRI CPK archive", f);
            }
            // d3cpk.is_d3cpk: the magic 0xa1b2c3d4 and version 6 or 7
            if h.len() >= 8
                && h[..4] == [0xd4, 0xc3, 0xb2, 0xa1]
                && matches!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 6 | 7)
            {
                return unsup("Diablo III package (d3cpk.py)", f);
            }
        }
        if base == "build_key.txt" {
            return unsup("Blizzard CASC archive set", f);
        }
        let h = head_of(f, 8);
        if h.starts_with(b"CRA.") {
            return unsup("Camelot CRA archive", f);
        }
        if h.starts_with(b"PDAT") {
            return unsup("phx PDAT archive (Kirby Air Riders)", f);
        }
        if f.rel.ends_with("Data/layout.toc") {
            return unsup("Frostbite bundle set (Data/layout.toc)", f);
        }
        if h.len() >= 16 && h.starts_with(b"RPF6") {
            return unsup("Rockstar RPF6 archive", f);
        }
    }
    Ok(())
}

fn nro_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "nro"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// One merged shader: where it first appeared, its stage, and where its bytes are in the spill file
struct Held {
    label: String,
    st: St,
    at: At,
}

/// The merged shaders of the whole split: `key → first copy`, and where each came from. The bytes are in the spill file (`spill.rs`), the map holds only what the merge and the
/// package order need.
struct Merged {
    keys: HashMap<String, Held>,
    pairs: BTreeSet<Pair>,
    per_src: Vec<(String, i64)>,
    spill: Spill,
    /// scratch for the first copy of a key being compared with a later one
    old: Vec<u8>,
}

impl Merged {
    fn count(&mut self, g: &str, d: i64) {
        match self.per_src.iter_mut().find(|(k, _)| k == g) {
            Some(e) => e.1 += d,
            None => self.per_src.push((g.to_string(), d)),
        }
    }

    /// `merge_keys` for one file's result: a key found more than once keeps its first copy, except that the game's own copy takes the place of an add-on's (`/aoc/<index>/`)
    fn add(
        &mut self,
        cx: &Ctx<'_>,
        prof: &Profile,
        uniq: Vec<(String, Shader)>,
        pairs: Vec<Pair>,
    ) -> Result<(), SplitError> {
        self.pairs.extend(pairs);
        for (key, v) in uniq {
            if let Some(old) = self.keys.get(&key) {
                let (old_at, old_label) = (old.at, old.label.clone());
                self.spill.read(old_at, &mut self.old).map_err(io)?;
                let (octl, ocode) = self.old.split_at(old_at.nctl as usize);
                if !same_shader(cx.rules, (octl, ocode), (&v.ctl, &v.code))
                    .map_err(|e| red(e.to_string()))?
                {
                    return Err(red(format!(
                        "key {key} is the same but the control section or code area differs ({} / {})",
                        v.label, old_label
                    )));
                }
                if old_label.starts_with("/aoc/") && !v.label.starts_with("/aoc/") {
                    let (go, gn) = (
                        prof.group(&old_label).to_string(),
                        prof.group(&v.label).to_string(),
                    );
                    self.count(&go, -1);
                    self.count(&gn, 1);
                    let at = self.spill.append(&v.ctl, &v.code).map_err(io)?;
                    self.keys.insert(
                        key,
                        Held {
                            label: v.label,
                            st: v.st,
                            at,
                        },
                    );
                }
                continue;
            }
            let g = prof.group(&v.label).to_string();
            self.count(&g, 1);
            let at = self.spill.append(&v.ctl, &v.code).map_err(io)?;
            self.keys.insert(
                key,
                Held {
                    label: v.label,
                    st: v.st,
                    at,
                },
            );
        }
        Ok(())
    }
}

fn make_cx<'a>(
    prof: &'a Profile,
    zstd: &'a dyn Zstd,
    common: Option<&'a HashMap<u32, String>>,
    borrowed: &'a Trees,
    zdicts: &'a HashMap<u32, Vec<u8>>,
    log: &'a (dyn Fn(&str) + Sync),
    files: &'a FileIndex,
) -> Ctx<'a> {
    Ctx {
        rules: &prof.rules,
        zstd,
        common,
        borrowed,
        zdicts,
        log,
        files,
        bahs_var: RefCell::new(HashMap::new()),
    }
}

/// One job of the first pass: a batch of 16 files
struct Job {
    files: Vec<FileRef>,
}

/// A batch of plain files, or a range of one archive's members
enum Work {
    Batch(Job),
    Special(crate::jobs::Special),
    /// a range of one KIT archive's members (Unravel TWO)
    Kit(crate::kit::KitJob),
}

type Results = Vec<FileResult>;
/// One main result: (the named shaders, the pairs).
type MainResult = (Vec<(String, Shader)>, Vec<Pair>);

/// Runs `n` jobs on `nthreads` threads and hands each result to `sink` **in job order**
#[allow(clippy::too_many_arguments)]
fn run_pool<T: Send>(
    nthreads: usize,
    n: usize,
    work: &(dyn Fn(usize) -> Result<T, String> + Sync),
    sink: &mut dyn FnMut(usize, T) -> Result<(), SplitError>,
    log: &(dyn Fn(&str) + Sync),
    progress: bool,
) -> Result<(), SplitError> {
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    // a worker does not start job `i` while the merge is more than `window` jobs behind it: the results that wait for their turn (the merge takes them in job order, and a
    // resumed split decodes them much faster than it merges them) are at most `window`, not all of them
    let window = nthreads.max(1) + 2;
    let ahead = (Mutex::new(0usize), Condvar::new());
    let (tx, rx) = mpsc::channel::<(usize, Result<T, String>)>();
    let total = n;
    let mut outcome: Result<(), SplitError> = Ok(());
    std::thread::scope(|sc| {
        for _ in 0..nthreads.max(1).min(n.max(1)) {
            let tx = tx.clone();
            let (next, stop, ahead) = (&next, &stop, &ahead);
            sc.spawn(move || {
                ibuilder_pipeline::sys::qos::lower_worker_qos();
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    if i >= window {
                        let mut merged_to = ahead.0.lock().unwrap();
                        while i >= *merged_to + window && !stop.load(Ordering::Relaxed) {
                            merged_to = ahead
                                .1
                                .wait_timeout(merged_to, Duration::from_millis(100))
                                .unwrap()
                                .0;
                        }
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    let r = work(i);
                    if r.is_err() {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if tx.send((i, r)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut pending: BTreeMap<usize, T> = BTreeMap::new();
        let (mut want, mut done) = (0usize, 0usize);
        let mut shown: Option<Instant> = None;
        while let Ok((i, r)) = rx
            .recv_timeout(Duration::from_millis(500))
            .map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => mpsc::RecvTimeoutError::Timeout,
                d => d,
            })
        {
            match r {
                Ok(v) => {
                    pending.insert(i, v);
                }
                Err(m) => {
                    if outcome.is_ok() {
                        outcome = Err(SplitError::Unsupported(m));
                    }
                    stop.store(true, Ordering::Relaxed);
                    continue;
                }
            }
            done += 1;
            while let Some(v) = pending.remove(&want) {
                if outcome.is_ok()
                    && let Err(e) = sink(want, v)
                {
                    outcome = Err(e);
                    stop.store(true, Ordering::Relaxed);
                }
                want += 1;
                *ahead.0.lock().unwrap() = want;
                ahead.1.notify_all();
            }
            if progress
                && shown.is_none_or(|t| t.elapsed() >= Duration::from_millis(500))
                && done < total
            {
                log(&format!("progress: {done}/{total}"));
                shown = Some(Instant::now());
            }
        }
        // the channel is closed (every worker ended) or timed out waiting: drain what is left
        while let Ok((i, r)) = rx.recv() {
            match r {
                Ok(v) => {
                    pending.insert(i, v);
                }
                Err(m) => {
                    if outcome.is_ok() {
                        outcome = Err(SplitError::Unsupported(m));
                    }
                    stop.store(true, Ordering::Relaxed);
                }
            }
            done += 1;
            while let Some(v) = pending.remove(&want) {
                if outcome.is_ok()
                    && let Err(e) = sink(want, v)
                {
                    outcome = Err(e);
                    stop.store(true, Ordering::Relaxed);
                }
                want += 1;
                *ahead.0.lock().unwrap() = want;
                ahead.1.notify_all();
            }
        }
        if progress && outcome.is_ok() {
            log(&format!("progress: {done}/{total}"));
        }
    });
    outcome
}

/// The whole split. `zstd` decodes the game's zstd files; `gate` lets each job through; `log` gets the Python's stdout lines (`progress: n/total`, `[RED] ...`, the summary).
pub fn split(
    opts: &SplitOptions<'_>,
    zstd: &dyn Zstd,
    gate: &dyn JobGate,
    log: &(dyn Fn(&str) + Sync),
) -> Result<SplitStats, SplitError> {
    let game = Game::load(opts.profile_dir, opts.game_id).map_err(SplitError::Input)?;
    let prof = Profile::from_game(game).map_err(SplitError::Input)?;
    if opts.versions.is_empty() {
        return Err(SplitError::Input("no RomFS and main were given".into()));
    }
    let memo = match opts.memo_dir {
        Some(d) => Some(JobMemo::open(d).map_err(io)?),
        None => None,
    };
    let empty_trees = Trees::new();
    let empty_dicts: HashMap<u32, Vec<u8>> = HashMap::new();
    let index_placeholder = FileIndex {
        map: HashMap::new(),
    };

    // ---- main(): the roots, the mains ----------------------------------------------------------------------------------------
    let mut files: Vec<FileRef> = Vec::new();
    let mut root_names: Vec<String> = Vec::new();
    let mut results0: Vec<MainResult> = Vec::new(); // the main results (`results.append(('main', ...))`)
    let mut common: Option<HashMap<u32, String>> = None;
    let mut root_paths: Vec<PathBuf> = Vec::new();
    for (i, (romfs, mainnso)) in opts.versions.iter().enumerate() {
        let tag = if i > 0 {
            parent_name(romfs)
        } else {
            String::new()
        };
        let entries: Vec<Entry> = walk_root(romfs).map_err(SplitError::Input)?;
        let root = root_paths.len();
        root_paths.push(romfs.to_path_buf());
        root_names.push(tag.clone());
        for e in entries {
            files.push(FileRef {
                root,
                rel: if tag.is_empty() {
                    e.rel
                } else {
                    format!("{tag}/{}", e.rel)
                },
                loc: e.loc,
            });
        }
        let module = ibuilder_nso::read(mainnso).map_err(|e| red(e.to_string()))?;
        let img = module.image;
        if i == 0 {
            common = unity::common_strings(&img);
        }
        let lbl = format!(
            "main{}",
            if tag.is_empty() {
                String::new()
            } else {
                format!("@{tag}")
            }
        );
        let cx = make_cx(
            &prof,
            zstd,
            common.as_ref(),
            &empty_trees,
            &empty_dicts,
            log,
            &index_placeholder,
        );
        let mut found = scan_main_image(&cx, &img, &lbl).map_err(|e| match e {
            Failure::Unsupported(m) => SplitError::Unsupported(m),
            e => red(e.to_string()),
        })?;
        // NROs the game decodes at run time: scanned like main, as no RomFS file holds them
        if i == 0
            && let Some(dir) = opts.nro_dir
        {
            for n in nro_files(dir) {
                let b = fs::read(&n).map_err(io)?;
                let name = n
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                scan_blob(&cx, &b, &format!("main/{name}"), &mut found, 0, None).map_err(|e| {
                    match e {
                        Failure::Unsupported(m) => SplitError::Unsupported(m),
                        e => red(e.to_string()),
                    }
                })?;
            }
        }
        // GLSLC outputs the game compiles at run time, captured on the Mac (hle_glslc.c)
        let cap = match opts.glslc_capture {
            Some(c) => c.to_path_buf(),
            None => romfs
                .parent()
                .map(|p| p.join("glslc_capture"))
                .unwrap_or_else(|| PathBuf::from("glslc_capture")),
        };
        if cap.is_dir() {
            let mut names: Vec<String> = fs::read_dir(&cap)
                .map_err(io)?
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".glslc") && !n.starts_with("._"))
                .collect();
            names.sort();
            for nm in names {
                let b = fs::read(cap.join(&nm)).map_err(io)?;
                found.extend(
                    crate::glslc::glslc_shaders(&b, &format!("/glslc_capture/{nm}"), true, &[])
                        .map_err(|e| red(e.to_string()))?,
                );
            }
        }
        // Unreal Engine shader code archives decoded on the Mac (uepak.py)
        let ued = romfs
            .parent()
            .map(|p| p.join("ue_shaders"))
            .unwrap_or_else(|| PathBuf::from("ue_shaders"));
        if ued.is_dir()
            && fs::read_dir(&ued)
                .map_err(io)?
                .filter_map(|e| e.ok())
                .any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .ends_with(".ushaderbytecode")
                })
        {
            return Err(SplitError::Unsupported(format!(
                "Unreal Engine shader code archives ({})",
                ued.display()
            )));
        }
        // `uniq_keys(found)` (the rebased engine; the older one built `{skey(c, k): (w, s, c, k) for ...}` and let the LAST copy of a key win, at the position of the first): the first copy of a key
        // in main's list stays, and a later copy that is not the same shader is `red()`
        let mut order: Vec<String> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        let mut keys: Vec<String> = Vec::with_capacity(found.len());
        for s in &found {
            keys.push(cx.skey(&s.ctl, &s.code).map_err(|e| red(e.to_string()))?);
        }
        let mut pairs = Vec::new();
        {
            // var_pairs(found), the same as for a file
            let mut var: HashMap<String, HashMap<St, String>> = HashMap::new();
            let bv = cx.bahs_var.borrow();
            for (s, key) in found.iter().zip(&keys) {
                let g = if s.label.contains("#var") {
                    Some(s.label.clone())
                } else {
                    bv.get(&s.label).cloned()
                };
                if let Some(g) = g {
                    var.entry(g).or_default().insert(s.st, key.clone());
                }
            }
            let mut set = BTreeSet::new();
            for (g, v) in &var {
                for st in [St::Vs, St::Ms] {
                    if let (Some(a), Some(b)) = (v.get(&st), v.get(&St::Ps)) {
                        set.insert(Pair {
                            a: a.clone(),
                            b: b.clone(),
                            prog: g.contains("#BAHS#prog"),
                        });
                    }
                }
            }
            pairs.extend(set);
        }
        let mut uniq: Vec<(String, Shader)> = Vec::new();
        for (s, key) in found.into_iter().zip(keys) {
            match at.get(&key) {
                Some(&p) => {
                    if !cx
                        .same_shader(&uniq[p].1, &s)
                        .map_err(|e| red(e.to_string()))?
                    {
                        return Err(SplitError::Red(format!(
                            "key {key} is the same but the control section or code area differs ({})",
                            s.label
                        )));
                    }
                }
                None => {
                    at.insert(key.clone(), uniq.len());
                    order.push(key.clone());
                    uniq.push((key, s));
                }
            }
        }
        results0.push((uniq, pairs));
    }
    for romfs in &opts.dlc {
        let name = romfs
            .components()
            .next_back()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let tag = format!(
            "aoc/{}",
            if crate::image::is_image(romfs) {
                name.strip_suffix(".img").unwrap_or(&name).to_string()
            } else {
                name
            }
        );
        let root = root_paths.len();
        root_paths.push(romfs.to_path_buf());
        root_names.push(tag.clone());
        for e in walk_root(romfs).map_err(SplitError::Input)? {
            files.push(FileRef {
                root,
                rel: format!("{tag}/{}", e.rel),
                loc: e.loc,
            });
        }
    }
    let findex = FileIndex {
        map: files
            .iter()
            .map(|f| ((f.root, f.rel.clone()), f.loc.clone()))
            .collect(),
    };
    // sibling lookups use the path below the root: for a version or DLC root the rels carry the tag, so the index is by those rels as well

    // Tears of the Kingdom's zstd dictionaries: Pack/ZsDic.pack.zs of every root
    let mut zdicts: HashMap<u32, Vec<u8>> = HashMap::new();
    for (ri, _) in root_paths.iter().enumerate() {
        let tag = &root_names[ri];
        let want = if tag.is_empty() {
            "Pack/ZsDic.pack.zs".to_string()
        } else {
            format!("{tag}/Pack/ZsDic.pack.zs")
        };
        if let Some(loc) = findex.get(ri, &want) {
            let b = loc.open().and_then(|s| s.read_all()).map_err(io)?;
            let m = zs_dicts_from(zstd, &b).map_err(|e| red(e.to_string()))?;
            zdicts.extend(m);
        }
    }
    detect_unsupported(&files, &findex)?;

    // ---- first pass: every file; Unity bundles carry type trees, and the Shader trees are collected on the way -------------------
    // the archives the Python plans as jobs of their own come first (`big + files`), their files are not scanned as loose files
    let plan = crate::jobs::plan(&files, log)?;
    // KIT archives (the rebased engine's `kit_jobs`, after the other planners): the path the Python's messages name a file by is the RomFS argument and the path below it
    let path_of = |f: &FileRef| -> String {
        let tag = &root_names[f.root];
        let below = if tag.is_empty() {
            f.rel.as_str()
        } else {
            f.rel.strip_prefix(&format!("{tag}/")).unwrap_or(&f.rel)
        };
        let root = root_paths[f.root].display().to_string();
        if root.ends_with('/') {
            format!("{root}{below}")
        } else {
            format!("{root}/{below}")
        }
    };
    let (kit_jobs, kit_taken) = crate::kit::plan(&files, &path_of)?;
    let plain: Vec<FileRef> = files
        .iter()
        .filter(|f| {
            !plan.taken.contains(&(f.root, f.rel.clone()))
                && !kit_taken.contains(&(f.root, f.rel.clone()))
        })
        .cloned()
        .collect();
    let mut batches: Vec<Work> = plan.jobs.iter().cloned().map(Work::Special).collect();
    batches.extend(kit_jobs.into_iter().map(Work::Kit));
    batches.extend(
        plain
            .chunks(16)
            .map(|c| Work::Batch(Job { files: c.to_vec() })),
    );
    let nj = opts.jobs.max(1);
    let mut merged = Merged {
        keys: HashMap::new(),
        pairs: BTreeSet::new(),
        per_src: Vec::new(),
        spill: Spill::create(opts.pkg_dir).map_err(io)?,
        old: Vec::new(),
    };
    let mcx = make_cx(
        &prof,
        zstd,
        common.as_ref(),
        &empty_trees,
        &empty_dicts,
        log,
        &findex,
    );
    for (uniq, pairs) in results0 {
        merged.add(&mcx, &prof, uniq, pairs)?;
    }
    let mut trees = Trees::new();
    let mut defer: Vec<usize> = Vec::new(); // indices into `files` of the files to scan again
    let mut unreadable: Vec<(String, usize)> = Vec::new();
    let file_pos: HashMap<(usize, &str), usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| ((f.root, f.rel.as_str()), i))
        .collect();
    let scan_batch =
        |job: &Work, borrowed: &Trees, zd: &HashMap<u32, Vec<u8>>| -> Result<Results, String> {
            let cx = make_cx(&prof, zstd, common.as_ref(), borrowed, zd, log, &findex);
            match job {
                Work::Batch(b) => {
                    let mut out = Vec::with_capacity(b.files.len());
                    for f in &b.files {
                        out.push(scan_file(&cx, f)?);
                    }
                    Ok(out)
                }
                Work::Special(s) => Ok(vec![crate::jobs::scan_special(&cx, s)?]),
                Work::Kit(k) => Ok(vec![crate::kit::scan(&cx, k, &files)?]),
            }
        };
    let job_key = |kind: &str, fs_: &[FileRef]| -> String {
        let mut parts: Vec<String> = vec![kind.to_string(), split_version()];
        for f in fs_ {
            parts.push(format!("{}:{}:{}", f.root, f.rel, f.loc.len()));
        }
        let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
        JobMemo::key(&refs)
    };
    {
        let work = |i: usize| -> Result<Results, String> {
            let job = &batches[i];
            let key = match job {
                Work::Batch(b) => job_key("scan_batch", &b.files),
                Work::Special(sp) => {
                    let mut parts: Vec<String> = vec!["special".to_string(), split_version()];
                    parts.extend(sp.identity());
                    let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
                    JobMemo::key(&refs)
                }
                Work::Kit(k) => {
                    let mut parts: Vec<String> = vec!["special".to_string(), split_version()];
                    parts.extend(k.identity());
                    let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
                    JobMemo::key(&refs)
                }
            };
            if let Some(m) = &memo
                && let Some(hit) = m.get(&key)
            {
                return Ok(hit);
            }
            let mut res: Option<Result<Results, String>> = None;
            gate.run(&mut || res = Some(scan_batch(job, &empty_trees, &zdicts)));
            let res = res.unwrap();
            if let (Some(m), Ok(r)) = (&memo, &res) {
                m.put(&key, r);
            }
            res
        };
        let mut sink = |i: usize, results: Results| -> Result<(), SplitError> {
            for r in results {
                match r.extra {
                    Extra::Defer => {
                        if let Work::Batch(b) = &batches[i] {
                            let p = file_pos.get(&(
                                b.files
                                    .iter()
                                    .find(|f| f.rel == r.rel)
                                    .map(|f| f.root)
                                    .unwrap_or(0),
                                r.rel.as_str(),
                            ));
                            if let Some(&p) = p {
                                defer.push(p);
                            }
                        }
                    }
                    Extra::Trees(t) => {
                        for (k, v) in t {
                            trees.insert(k, v);
                        }
                        merged.add(&mcx, &prof, r.uniq, r.pairs)?;
                    }
                    Extra::Unreadable { why, count } => {
                        unreadable.push((why, count));
                    }
                }
            }
            Ok(())
        };
        run_pool(nj, batches.len(), &work, &mut sink, log, true)?;
    }
    // ---- second pass: Unity serialized files without type trees are scanned again with the borrowed trees -------------------------
    if !defer.is_empty() {
        // a hash no bundle supplied takes the layout unityfs.py has for it (KNOWN_TREES); only here, so files a bundle lends a tree to keep their place in the tables
        for (k, node) in unity::known_trees() {
            trees.entry(k).or_insert(Some(node));
        }
        let again_files: Vec<FileRef> = defer.iter().map(|&i| files[i].clone()).collect();
        let borrowed = &trees;
        let work = |i: usize| -> Result<Results, String> {
            let f = &again_files[i];
            let key = job_key("scan_file", std::slice::from_ref(f));
            if let Some(m) = &memo
                && let Some(hit) = m.get(&key)
            {
                return Ok(hit);
            }
            let mut res: Option<Result<Results, String>> = None;
            gate.run(&mut || {
                let cx = make_cx(
                    &prof,
                    zstd,
                    common.as_ref(),
                    borrowed,
                    &zdicts,
                    log,
                    &findex,
                );
                res = Some(scan_file(&cx, f).map(|r| vec![r]));
            });
            let res = res.unwrap();
            if let (Some(m), Ok(r)) = (&memo, &res) {
                m.put(&key, r);
            }
            res
        };
        let mut sink = |_i: usize, results: Results| -> Result<(), SplitError> {
            for r in results {
                match r.extra {
                    Extra::Defer => {
                        return Err(red(format!(
                            "{}: Unity serialized file without type trees, and no bundle has a Shader type tree of the same hash to borrow",
                            r.rel
                        )));
                    }
                    Extra::Unreadable { why, count } => unreadable.push((why, count)),
                    Extra::Trees(_) => merged.add(&mcx, &prof, r.uniq, r.pairs)?,
                }
            }
            Ok(())
        };
        run_pool(nj, again_files.len(), &work, &mut sink, log, false)?;
    }
    if !unreadable.is_empty() {
        log(&format!(
            "[RED] {} file(s) could not be read, their shaders are left out (first: {})",
            unreadable.iter().map(|u| u.1).sum::<usize>(),
            unreadable[0].0
        ));
    }
    let n_unreadable = unreadable.iter().map(|u| u.1).sum::<usize>();

    // ---- pairs --------------------------------------------------------------------------------------------------------------------
    let Merged {
        keys,
        pairs: all_pairs,
        per_src,
        mut spill,
        ..
    } = merged;
    let mut prog: BTreeSet<(String, String)> = all_pairs
        .iter()
        .filter(|p| p.prog)
        .map(|p| (p.a.clone(), p.b.clone()))
        .collect();
    let mut pairs: BTreeSet<(String, String)> = all_pairs
        .iter()
        .filter(|p| !p.prog)
        .map(|p| (p.a.clone(), p.b.clone()))
        .collect();
    prog = prog.difference(&pairs).cloned().collect();
    // separable_pairs: single-stage programs have their real pairs only at draw time; every single-stage vertex shader is paired with a cover of the pixel shaders
    let single: Vec<&String> = keys
        .iter()
        .filter(|(_, s)| {
            s.st == St::Vs
                && SINGLE_STAGE.iter().any(|x| s.label.contains(x))
                && !s.label.contains("#var")
        })
        .map(|(k, _)| k)
        .collect();
    let mut cover: Vec<String> = Vec::new();
    if !single.is_empty() {
        // only the input map at the start of each pixel shader's code area is read (not the code: it is in the spill)
        let mut ins: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        let mut prefix: Vec<u8> = Vec::new();
        for (k, s) in keys.iter().filter(|(_, s)| s.st == St::Ps) {
            spill
                .read_code_prefix(s.at, crate::scan::PS_INPUTS_PREFIX, &mut prefix)
                .map_err(io)?;
            ins.insert(
                k.clone(),
                crate::scan::ps_inputs_pub(&prefix).map_err(|e| red(e.to_string()))?,
            );
        }
        cover = crate::scan::pixel_cover_of_inputs(&ins);
        for v in &single {
            for p in &cover {
                pairs.insert(((*v).clone(), p.clone()));
            }
        }
    }
    if !cover.is_empty() {
        log(&format!(
            "single-stage programs: {} vertex shaders are paired with a cover of {} pixel shader(s) ({})",
            thousands(single.len()),
            cover.len(),
            cover.join(" ")
        ));
    }

    // ---- the package: one record per key, sorted -------------------------------------------------------------------------------------
    fs::create_dir_all(opts.pkg_dir).map_err(io)?;
    for e in fs::read_dir(opts.pkg_dir).map_err(io)? {
        let e = e.map_err(io)?;
        if e.file_name().to_string_lossy().ends_with(".bin") {
            let _ = fs::remove_file(e.path());
        }
    }
    // sorted by (group, label, key): each shader's group is found once (`prof.group` is a scan of the profile's groups), not twice per comparison
    let mut order: Vec<(&str, &String, &Held)> = keys
        .iter()
        .map(|(k, h)| (prof.group(&h.label), k, h))
        .collect();
    order.sort_by(|a, b| (a.0, &a.2.label, a.1).cmp(&(b.0, &b.2.label, b.1)));
    let mut writers: HashMap<String, std::io::BufWriter<fs::File>> = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<(String, usize, &'static str, &str, &str)> = Vec::with_capacity(order.len());
    let mut bytes: Vec<u8> = Vec::new();
    let mut stat: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut facts: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let write_result = (|| -> Result<(), SplitError> {
        for (group, key, s) in &order {
            let name = format!("{}_{}.bin", s.st.short(), group);
            spill.read(s.at, &mut bytes).map_err(io)?;
            let (ctl, code) = bytes.split_at(s.at.nctl as usize);
            let rec = record(key, ctl, code).map_err(red)?;
            let idx = counts.entry(name.clone()).or_insert(0);
            rows.push((
                name.clone(),
                *idx,
                s.st.name(),
                key.as_str(),
                s.label.as_str(),
            ));
            *idx += 1;
            if !writers.contains_key(&name) {
                let f = fs::File::create(opts.pkg_dir.join(&name)).map_err(io)?;
                writers.insert(name.clone(), std::io::BufWriter::with_capacity(1 << 20, f));
            }
            writers
                .get_mut(&name)
                .unwrap()
                .write_all(&rec)
                .map_err(io)?;
            *stat.entry(s.st.short()).or_insert(0) += 1;
            if s.st == St::Cs {
                // NVN2: the thread group at +0x58; shared memory taken from +0x64
                let ([x, y, z], sh) = compute_facts(ctl)
                    .map_err(|e| red(format!("compute shader {key}: control {e} ({})", s.label)))?;
                facts.insert(
                    (*key).clone(),
                    serde_json::json!({"local": [x, y, z], "shared": sh}),
                );
            }
        }
        for (_, mut w) in writers.drain() {
            w.flush().map_err(io)?;
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        // the Python writes nothing when a record cannot be cut: do not leave half a package behind
        drop(writers);
        for n in counts.keys() {
            let _ = fs::remove_file(opts.pkg_dir.join(n));
        }
        return Err(e);
    }

    // ---- the tables ------------------------------------------------------------------------------------------------------------------
    fs::create_dir_all(opts.tables_dir).map_err(io)?;
    let any = |pred: &dyn Fn(&str) -> bool| rows.iter().any(|r| pred(r.4));
    let unity_shader = |w: &str| {
        w.contains("#var")
            && !w.contains("#HCPS@")
            && !w.contains("#PMAP@")
            && !w.contains("#BNSH@")
            && !w.contains("#MSHD#")
            && !w.contains("#SDF#")
            && !w.contains("#GLSLC@")
            && !w.contains("#SNVN@")
            && !w.contains("#BSHD#")
    };
    let kinds: Vec<&str> = [
        (
            "BNSH / BAHS",
            any(&|w| w.contains("#BNSH@") || w.contains("#BAHS@")),
        ),
        ("Monolith HCPS / SLCT", any(&|w| w.contains("#HCPS@"))),
        ("Unity Shader", any(&unity_shader)),
        ("Retro Studios SNVN", any(&|w| w.contains("#SNVN@"))),
        ("phx PMAP", any(&|w| w.contains("#PMAP@"))),
        ("MSHD", any(&|w| w.contains("#MSHD#"))),
        ("GLSLC", any(&|w| w.contains("#GLSLC@"))),
        ("RE Engine SDF", any(&|w| w.contains("#SDF#"))),
        ("MT Framework SPK", any(&|w| w.contains("#MTSPK@"))),
        ("MonoGame MGFX", any(&|w| w.contains("#MGFX@"))),
        (
            "PlatinumGames shader objects",
            any(&|w| w.contains("#PGSO")),
        ),
        (
            "Snowdrop shader objects",
            any(&|w| w.contains("#SNOWDROP@")),
        ),
        ("Larian BSHD", any(&|w| w.contains("#BSHD#"))),
        ("Needle", any(&|w| w.contains("#NEEDLE@"))),
        (
            "Unreal Engine shader code archives",
            any(&|w| w.contains("#UE")),
        ),
    ]
    .iter()
    .filter(|(_, hit)| *hit)
    .map(|(k, _)| *k)
    .collect();
    let t = |n: &str| opts.tables_dir.join(n);
    // each table is written whole or not at all (`<name>.part`, then renamed over it): the old one stays when this run cannot finish it (review r2, N7)
    write_atomic(&t("shader_map.tsv"), |f| {
        writeln!(f, "# kind: evidence  {} ({} in RomFS + main) \u{2192} control section + code area keys (dm/gfx/tools/shader_split.py)", prof.note, kinds.join(" / "))?;
        writeln!(f, "# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied")?;
        writeln!(f, "package\tindex\tstage\tdm_ctl_sha16\tsource")?;
        for r in &rows {
            writeln!(f, "{}\t{}\t{}\t{}\t{}", r.0, r.1, r.2, r.3, r.4)?;
        }
        Ok(())
    })
    .map_err(io)?;
    // Hand-written facts the machine code cannot give (the profile's shader_facts_manual.tsv, same row format) are merged over the generated ones
    if prof.manual_facts.exists() {
        let text = fs::read_to_string(&prof.manual_facts).map_err(io)?;
        for ln in text.lines() {
            if ln.trim().is_empty() || ln.starts_with('#') {
                continue;
            }
            let (k, v) = ln.split_once('\t').ok_or_else(|| {
                red(format!(
                    "{}: a facts row without a tab: {ln:?}",
                    prof.manual_facts.display()
                ))
            })?;
            let add: serde_json::Value = serde_json::from_str(v)
                .map_err(|e| red(format!("{}: {k}: {e}", prof.manual_facts.display())))?;
            let mut base = facts
                .get(k)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            if let (Some(b), Some(a)) = (base.as_object_mut(), add.as_object()) {
                for (kk, vv) in a {
                    b.insert(kk.clone(), vv.clone());
                }
            }
            facts.insert(k.to_string(), base);
        }
    }
    write_atomic(&t("shader_facts.tsv"), |f| {
        writeln!(f, "# Answer table for translation queries (generated by dm/gfx/tools/shader_split.py, do not edit): key <TAB> fact JSON. No PC shader pairing supplied: only the thread group / shared memory of compute shaders (taken from the control section); other queries get the translator defaults")?;
        for (k, v) in &facts {
            writeln!(f, "{k}\t{}", crate::pyjson::compact_sorted(v))?;
        }
        Ok(())
    })
    .map_err(io)?;
    write_atomic(&t("shader_pairs.tsv"), |f| {
        writeln!(f, "# kind: evidence  vertex / pixel shader pairs within BNSH variations (generated by dm/gfx/tools/shader_split.py, do not edit): vertex key <TAB> pixel key")?;
        for (a, b) in &pairs {
            writeln!(f, "{a}\t{b}")?;
        }
        Ok(())
    })
    .map_err(io)?;
    write_atomic(&t("pso_pairs.tsv"), |f| {
        writeln!(f, "# kind: evidence  supplemental vertex / pixel shader pairs (BAHS program variations and legacy Unity pass-input covers) for vertex output padding and the pipeline prebuild (generated by dm/gfx/tools/shader_split.py, do not edit): vertex key <TAB> pixel key")?;
        for (a, b) in prog.difference(&pairs) {
            writeln!(f, "{a}\t{b}")?;
        }
        Ok(())
    })
    .map_err(io)?;
    let mut ps: Vec<(String, i64)> = per_src.clone();
    ps.sort_by_key(|a| std::cmp::Reverse(a.1)); // Counter.most_common(): by count, ties in insertion order (a stable sort)
    log(&format!(
        "shaders: {} ({}); sources {}; compute shader answers {} rows; variation pairs {}",
        rows.len(),
        stat.iter()
            .map(|(s, n)| format!("{} {}", St::from_short(s).unwrap().name(), n))
            .collect::<Vec<_>>()
            .join(", "),
        ps.iter()
            .map(|(g, n)| format!("{g} {n}"))
            .collect::<Vec<_>>()
            .join(", "),
        facts.len(),
        pairs.len()
    ));
    let (reused, ran) = memo
        .as_ref()
        .map(|m| {
            (
                m.hits.load(Ordering::Relaxed),
                m.runs.load(Ordering::Relaxed),
            )
        })
        .unwrap_or((0, 0));
    Ok(SplitStats {
        shaders: rows.len(),
        jobs_reused: reused,
        jobs_ran: ran,
        unreadable: n_unreadable,
        by_stage: stat
            .iter()
            .map(|(s, n)| (St::from_short(s).unwrap().name().to_string(), *n))
            .collect(),
    })
}
