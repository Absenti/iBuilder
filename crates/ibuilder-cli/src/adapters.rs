//! The real stage workers, behind the `ibuilder-pipeline` adapter traits. Each one is a thin mapping from the pipeline's request to the stage crate's public API
//! (nothing is re-implemented here); a stage whose cargo feature is off is `Unwired` and fails with a message naming it.
//!
//! | pipeline trait | crate | feature | what it calls |
//! |---|---|---|---|
//! | `ProfileDeriver` | `ibuilder-profile` | `profile` | `derive::derive` (stage 0/1), `game::write_profile` (render again) |
//! | `Rewriter` | `ibuilder-rewrite` | `rewrite` | `rewrite` on a rayon pool of `req.jobs` threads, the files `patch_nso.py` writes |
//! | `Assembler` | `ibuilder-macho` | `macho` | `assemble_files(host, out, Game::Dir{flat, modules})` |
//! | `Signer` | `ibuilder-sign` | `sign` | `sign_file` (ad hoc, in place) |
//! | `ModsHost` | `ibuilder-mods` | `mods` | `plan`, `apply_module`, `remove_patched`, `mark_applied` (stage 2), `apply_boot` (the boot) |
//! | `ShaderBackend` | `ibuilder-shaderbuild` (+ `ibuilder-shadersplit`) | `shaders` (`shader-finish`, `shadersplit`) | `build`, `live_translate`; `split` |
//! | `NupReader`, `LauncherBase` | none yet | -- | no crate ports `pack_nup.py`, `settings.py` |

use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::ctx::View;
use ibuilder_pipeline::error::{PipelineError, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The identifier the framework is signed with when no app identity applies (the CLI is not the app).
pub const DEFAULT_IDENTIFIER: &str = "dev.ibuilder.app";

/// Make rayon's global pool (what `par_iter` uses outside an `install`) one whose workers run at utility QoS on Apple targets, before a build stage uses it. Same thread count as the
/// default pool. Called only by the build stages' adapters (never by the boot or the live translation), so no game work ever runs on these workers; if some other code already
/// created the global pool the call changes nothing. The calling thread keeps its own priority.
pub fn build_pool() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rayon::ThreadPoolBuilder::new()
            .start_handler(|_| ibuilder_pipeline::sys::qos::lower_worker_qos())
            .build_global();
    });
}

/// Which stage workers this build of the CLI has.
pub fn wired() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "profile") {
        v.push("profile (ibuilder-profile)");
    }
    if cfg!(feature = "rewrite") {
        v.push("rewrite (ibuilder-rewrite)");
    }
    if cfg!(feature = "macho") {
        v.push("assemble (ibuilder-macho)");
    }
    if cfg!(feature = "sign") {
        v.push("sign (ibuilder-sign)");
    }
    if cfg!(feature = "mods") {
        v.push("mods (ibuilder-mods)");
    }
    if cfg!(feature = "nup") {
        v.push("Switch 2 package reader (ibuilder-nup)");
    }
    if cfg!(feature = "shaders") {
        v.push("shader build + live (ibuilder-shaderbuild)");
    }
    if cfg!(feature = "shadersplit") {
        v.push("shader split (ibuilder-shadersplit)");
    }
    if cfg!(feature = "shader-host-tools") {
        v.push("shader tools as child processes (--engine; Mac only)");
    }
    v
}

/// The adapter set for a CLI / FFI run: every worker whose feature is on; the rest `Unwired`.
pub fn real_adapters(identifier: &str) -> Adapters {
    let mut a = Adapters::unwired();
    #[cfg(feature = "profile")]
    {
        a.profile = Arc::new(RustProfile);
    }
    #[cfg(feature = "rewrite")]
    {
        a.rewriter = Arc::new(RustRewriter);
    }
    #[cfg(feature = "macho")]
    {
        a.assembler = Arc::new(RustAssembler);
    }
    #[cfg(feature = "sign")]
    {
        a.signer = Arc::new(RustSigner {
            identifier: identifier.to_string(),
        });
    }
    #[cfg(feature = "mods")]
    {
        a.mods = Some(Arc::new(RustMods));
    }
    #[cfg(feature = "nup")]
    {
        a.nup = Some(Arc::new(RustNup));
    }
    #[cfg(feature = "shaders")]
    {
        a.shaders = Some(Arc::new(shaders::RustShaders));
    }
    let _ = identifier;
    a
}

// ---- profile ----------------------------------------------------------------------------------------------------------------------------------------

#[cfg(feature = "profile")]
#[derive(Default)]
pub struct RustProfile;

#[cfg(feature = "profile")]
impl ProfileDeriver for RustProfile {
    fn derive(&self, v: &View<'_>) -> Result<std::path::PathBuf> {
        use ibuilder_profile::derive::{DeriveConfig, derive};
        build_pool();
        let extract = v
            .cfg
            .get("extract_out")
            .ok_or_else(|| PipelineError::missing("extract_out is not in the config"))?;
        let mut cfg = DeriveConfig::new(extract, v.bundle);
        cfg.exe_dir = v.cfg.get("exe_dir").map(Into::into);
        cfg.romfs = v.cfg.get_nonempty("romfs").map(Into::into);
        cfg.profile = v.cfg.get_nonempty("profile").map(Into::into);
        cfg.profile_out = None; // <extract>/../profile-derived/<id>: Documents/profile-derived/<id>
        let log = v.log;
        derive(&cfg, &mut |l| log.line(l)).map_err(|e| PipelineError::failed(e.to_string()))
    }

    fn render(&self, v: &View<'_>, tid: &str, profile: &Path) -> Result<()> {
        use ibuilder_profile::game::{Game, find_dir, write_profile};
        build_pool();
        let games = v.bundle.join("dm").join("games");
        let dir = find_dir(tid, &games, &[v.docs.profile_derived()], true);
        let g = Game::load_dir(&dir).map_err(|e| PipelineError::failed(e.to_string()))?;
        write_profile(&g, profile).map_err(|e| PipelineError::failed(e.to_string()))?;
        Ok(())
    }
}

// ---- rewrite ----------------------------------------------------------------------------------------------------------------------------------------

/// A module's bytes for the rewrite: a read-only file mapping when the file can be mapped, else the file read into memory. A mapping's pages are clean file-backed pages: they
/// count in the resident size but not in the footprint jetsam is expressed in (the 72 MB of `main` stood in the footprint for the whole rewrite when it was `fs::read`;
/// docs/chain/MEMORY-RUST.md section 5). Only this process reads the file while it is mapped (the rewrite writes to its own folder; a mods copy is a different file).
#[cfg(feature = "rewrite")]
enum ModuleBytes {
    Mapped(memmap2::Mmap),
    Read(Vec<u8>),
}

#[cfg(feature = "rewrite")]
impl ModuleBytes {
    fn open(path: &Path) -> std::io::Result<ModuleBytes> {
        if let Ok(f) = std::fs::File::open(path) {
            // SAFETY: a read-only private mapping of a file no one writes while the rewrite runs (see the type's documentation); the same as `ibuilder_nso::read`.
            #[allow(unsafe_code)]
            if let Ok(m) = unsafe { memmap2::Mmap::map(&f) } {
                return Ok(ModuleBytes::Mapped(m));
            }
        }
        std::fs::read(path).map(ModuleBytes::Read) // empty file, odd file system: the plain read, with its own error
    }
}

#[cfg(feature = "rewrite")]
impl std::ops::Deref for ModuleBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            ModuleBytes::Mapped(m) => m,
            ModuleBytes::Read(v) => v,
        }
    }
}

/// `pyrewrite_driver.run_once(patch_nso.py, [script, src, sub, m], env)`: rewrite one module and write what `patch_nso.py` writes into `req.out_dir`.
#[cfg(feature = "rewrite")]
pub struct RustRewriter;

#[cfg(feature = "rewrite")]
impl Rewriter for RustRewriter {
    fn rewrite(&self, req: &RewriteRequest<'_>, v: &View<'_>) -> Result<()> {
        use std::io::Write;
        // paches/engine/0301: DM_REWRITE_JOBS below 1 is refused by `patch_nso.py` with `sys.exit`, which `pyrewrite_driver.run_once` turns into an ordinary error
        if req.jobs == 0 {
            return Err(PipelineError::failed(
                "patch_nso.py stopped: [RED] DM_REWRITE_JOBS must be a positive integer",
            ));
        }
        let data = ModuleBytes::open(req.src)
            .map_err(|e| PipelineError::io(format!("read {}", req.src.display()), e))?;
        let tid = v
            .cfg
            .get_nonempty("profile")
            .and_then(|p| ibuilder_pipeline::tsv::profile_title_id(Path::new(p)));
        let opts = ibuilder_rewrite::Options {
            workers: req.jobs.max(1),
            game: tid,
            games_dirs: vec![v.docs.profile_derived(), v.bundle.join("dm").join("games")],
            ..Default::default()
        };
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(req.jobs.max(1))
            .start_handler(|_| ibuilder_pipeline::sys::qos::lower_worker_qos())
            .build()
            .map_err(|e| PipelineError::failed(e.to_string()))?;
        let name = req.src.to_string_lossy().into_owned();
        let r = pool
            .install(|| ibuilder_rewrite::rewrite(&data, &name, req.module, &opts))
            .map_err(|e| PipelineError::failed(e.msg))?;
        drop(data); // the module is not needed once the rewrite holds its own image
        for l in &r.log {
            v.log.line(&format!("  {}: {l}", req.module));
        }
        let p = |ext: &str| req.out_dir.join(format!("{}.{ext}", req.module));
        let io = |what: &str, e: std::io::Error| PipelineError::io(what.to_string(), e);
        {
            let f =
                std::fs::File::create(p("patched.bin")).map_err(|e| io("create patched.bin", e))?;
            let mut bw = std::io::BufWriter::with_capacity(1 << 20, f);
            r.write_patched(&mut bw)
                .map_err(|e| io("write patched.bin", e))?;
            bw.flush().map_err(|e| io("flush patched.bin", e))?;
        }
        for (ext, text) in [
            ("fixups.tsv", &r.fixups),
            ("gates.tsv", &r.gates),
            ("sites.tsv", &r.sites),
            ("cuts.tsv", &r.cuts),
            ("changed.tsv", &r.changed),
            ("obs.tsv", &r.obs),
        ] {
            std::fs::write(p(ext), text).map_err(|e| io(ext, e))?;
        }
        if let Some(m) = &r.match_tsv {
            std::fs::write(p("match.tsv"), m).map_err(|e| io("match.tsv", e))?;
        }
        if let Some(b) = &r.build_src {
            std::fs::write(req.out_dir.join("build_src.sha256"), b)
                .map_err(|e| io("build_src.sha256", e))?;
        }
        Ok(())
    }
}

// ---- assemble ---------------------------------------------------------------------------------------------------------------------------------------

/// `assemble.build(host, out, flat, modules)`
#[cfg(feature = "macho")]
pub struct RustAssembler;

#[cfg(feature = "macho")]
impl Assembler for RustAssembler {
    fn assemble(&self, req: &AssembleRequest<'_>, _v: &View<'_>) -> Result<AssembleInfo> {
        use ibuilder_macho::assemble::{Game, assemble_files};
        build_pool();
        let game = Game::Dir {
            dir: req.flat.to_path_buf(),
            modules: req.modules.to_vec(),
        };
        let r = assemble_files(req.host, req.out, &game, "")
            .map_err(|e| PipelineError::failed(e.to_string()))?;
        Ok(AssembleInfo {
            ncmds: Some(r.info.n_load_commands as u32),
        })
    }
}

// ---- sign -------------------------------------------------------------------------------------------------------------------------------------------

/// `_dmgameapp.sign` for a Mac / CLI run: the in-process ad-hoc signature (no certificate, no provisioning profile).
#[cfg(feature = "sign")]
pub struct RustSigner {
    pub identifier: String,
}

#[cfg(feature = "sign")]
impl Signer for RustSigner {
    fn sign(&self, framework_dir: &Path, exe: &str, _v: &View<'_>) -> SignOutcome {
        build_pool();
        let path = framework_dir.join(exe);
        match ibuilder_sign::sign_file(&path, Some(&self.identifier)) {
            Ok(info) => SignOutcome {
                rc: 0,
                mode: "adhoc(ibuilder-sign, no certificate)".into(),
                identifier: info.identifier,
                err: String::new(),
            },
            Err(e) => SignOutcome {
                rc: 20,
                mode: "adhoc(ibuilder-sign, no certificate)".into(),
                identifier: self.identifier.clone(),
                err: e.to_string(),
            },
        }
    }
    fn identifier(&self) -> String {
        self.identifier.clone()
    }
}

// ---- the Switch 2 package ---------------------------------------------------------------------------------------------------------------------------

/// `gameapp_pipeline.extract_nup` through `ibuilder-nup` (the Rust port of `pack_nup.py`): the config's `nup` container into `extract_out` (`ExeFS/`, `RomFS.img`, `control.nacp`,
/// `icon.jpg`, `romfs_programs/`) and its DLC into `Documents/games/<title id>/aoc`. The newest version of the highest program is read; every file is checked against the container's
/// SHA-256; the RomFS image is written by `threads` workers straight from the container (never unpacked into loose files). The stage's progress goes to the stage file and the log
/// (a line every 5 s), the pipeline's cancel flag stops it between chunks.
#[cfg(feature = "nup")]
pub struct RustNup;

#[cfg(feature = "nup")]
impl NupReader for RustNup {
    fn extract_nup(&self, v: &View<'_>) -> Result<String> {
        use ibuilder_nup::{Nup, Options, Progress, extract_application};
        build_pool();
        use ibuilder_pipeline::stage::write_stage;
        use std::sync::atomic::{AtomicBool, Ordering};
        let path = PathBuf::from(
            v.cfg
                .get_nonempty("nup")
                .ok_or_else(|| PipelineError::missing("nup is not in the config"))?,
        );
        let extract = v
            .cfg
            .get_nonempty("extract_out")
            .map(PathBuf::from)
            .unwrap_or_else(|| v.docs.extract());
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let nup = Nup::open(&path).map_err(|e| PipelineError::failed(e.to_string()))?;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut opts = Options {
            threads: v
                .cfg
                .get_nonempty("nup_jobs")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            cancel: cancel.clone(),
            progress_ms: 5000,
            ..Options::default()
        };
        let (log, docs) = (v.log, v.docs);
        let lines: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        {
            let lines = lines.clone();
            opts.say = Some(Arc::new(move |l: &str| {
                lines.lock().unwrap().push(l.to_string())
            }));
        }
        let (tx, rx) = std::sync::mpsc::channel::<Progress>();
        let (tx, rx) = (std::sync::Mutex::new(tx), std::sync::Mutex::new(rx));
        opts.progress = Some(Arc::new(move |p: &Progress| {
            let _ = tx.lock().unwrap().send(p.clone());
        }));
        let games = v.docs.clone();
        let aoc = move |tid: &str| games.game_dir(tid).join("aoc");
        let done = AtomicBool::new(false);
        let result = std::thread::scope(|s| {
            // the pipeline's cancel flag (a borrow) to the reader's (an Arc), and the reader's progress and lines to the pipeline's stage file and log
            s.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    if v.cancelled() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                    while let Ok(p) = rx.lock().unwrap().try_recv() {
                        let text = format!(
                            "reading the Switch 2 package ({name}): {} {:.1} of {:.1} GB",
                            p.step,
                            p.done_bytes as f64 / 1e9,
                            p.total_bytes as f64 / 1e9
                        );
                        write_stage(docs, 1, 5, &text, Some(log));
                    }
                    flush_lines(&lines, log);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
            let r = extract_application(&path, &extract, Some(&aoc), None, None, Some(nup), &opts);
            done.store(true, Ordering::Relaxed);
            r
        });
        flush_lines(&lines, log);
        match result {
            Ok(x) => {
                log.line(&format!("nup: image {} ({} DLC)", x.how.as_str(), x.dlc));
                Ok(x.title_id)
            }
            Err(ibuilder_nup::Error::Cancelled) => Err(PipelineError::failed(
                "reading the Switch 2 package was cancelled",
            )),
            Err(e) => Err(PipelineError::failed(e.to_string())),
        }
    }
}

/// The reader's lines so far, to the pipeline's log. The batch is taken under the lock and logged after it: the log is the app's callback, and the reader's threads push their lines under
/// that lock (review r3 §4.7; a `for` over `lock().drain(..)` keeps the guard for the whole loop).
#[cfg(feature = "nup")]
fn flush_lines(lines: &std::sync::Mutex<Vec<String>>, log: &dyn ibuilder_pipeline::log::Log) {
    let batch: Vec<String> = std::mem::take(&mut *lines.lock().unwrap());
    for l in batch {
        log.line(&l);
    }
}

#[cfg(all(test, feature = "nup"))]
mod nup_tests {
    use super::flush_lines;
    use std::sync::Mutex;

    /// Review r3 §4.7: the app's log callback runs with the lines' lock released (the reader's threads push their lines under it).
    #[test]
    fn the_log_is_called_with_the_lines_lock_released() {
        let lines = Mutex::new(vec!["one".to_string(), "two".to_string()]);
        let seen = Mutex::new(Vec::new());
        let log = |l: &str| {
            seen.lock()
                .unwrap()
                .push((l.to_string(), lines.try_lock().is_ok()));
        };
        flush_lines(&lines, &log);
        assert_eq!(
            *seen.lock().unwrap(),
            [("one".to_string(), true), ("two".to_string(), true)]
        );
        assert!(lines.lock().unwrap().is_empty());
    }
}

// ---- mods -------------------------------------------------------------------------------------------------------------------------------------------

/// `gameapp/mods.py` through `ibuilder-mods`: the enabled mods' exefs patches before stage 2, the RomFS overlay and the cheats at the boot.
#[cfg(feature = "mods")]
pub struct RustMods;

#[cfg(feature = "mods")]
fn failure(e: ibuilder_mods::ModsError) -> ModsFailure {
    ModsFailure {
        exc: e.exc_name().to_string(),
        msg: e.message().to_string(),
    }
}

/// `ibuilder_pipeline::env::Env` as the `os.environ` `apply_boot` writes to.
#[cfg(feature = "mods")]
struct EnvAdapter<'a>(&'a dyn ibuilder_pipeline::env::Env);

#[cfg(feature = "mods")]
impl ibuilder_mods::Environ for EnvAdapter<'_> {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get(name)
    }
    fn set(&mut self, name: &str, value: &str) {
        self.0.set(name, value);
    }
}

#[cfg(feature = "mods")]
impl ModsHost for RustMods {
    fn plan(
        &self,
        docs: &Path,
        tid: &str,
        exe_dir: &Path,
        modules: &[String],
        log: &dyn ibuilder_pipeline::log::Log,
        quiet: bool,
    ) -> std::result::Result<Vec<(String, ModPlanEntry)>, ModsFailure> {
        let plan = ibuilder_mods::plan(docs, tid, exe_dir, modules, &mut |l| log.line(l), quiet)
            .map_err(failure)?;
        Ok(plan
            .modules
            .into_iter()
            .map(|(m, e)| {
                let labels = e.patches.iter().map(|p| p.label()).collect();
                (
                    m,
                    ModPlanEntry {
                        digest: e.digest,
                        labels,
                        token: Arc::new(e.patches),
                    },
                )
            })
            .collect())
    }

    fn apply_module(
        &self,
        docs: &Path,
        tid: &str,
        exe_dir: &Path,
        module: &str,
        entry: &ModPlanEntry,
        log: &dyn ibuilder_pipeline::log::Log,
    ) -> std::result::Result<Option<std::path::PathBuf>, ModsFailure> {
        let Some(patches) = entry.token.downcast_ref::<Vec<ibuilder_mods::Patch>>() else {
            return Err(ModsFailure {
                exc: "TypeError".into(),
                msg: "the plan entry was not made by this mods host".into(),
            });
        };
        ibuilder_mods::apply_module(docs, tid, exe_dir, module, patches, &mut |l| log.line(l))
            .map_err(failure)
    }

    fn remove_patched(&self, docs: &Path, tid: &str) {
        ibuilder_mods::remove_patched(docs, tid);
    }

    fn mark_applied(
        &self,
        docs: &Path,
        tid: &str,
        log: &dyn ibuilder_pipeline::log::Log,
    ) -> std::result::Result<(), ModsFailure> {
        ibuilder_mods::mark_applied(docs, tid, &mut |l| log.line(l)).map_err(failure)
    }

    fn apply_boot(
        &self,
        docs: &Path,
        tid: &str,
        log: &dyn ibuilder_pipeline::log::Log,
        env: &dyn ibuilder_pipeline::env::Env,
    ) -> std::result::Result<std::collections::BTreeMap<String, String>, ModsFailure> {
        let mut adapter = EnvAdapter(env);
        ibuilder_mods::apply_boot(docs, tid, &mut |l| log.line(l), &mut adapter)
            .map(|v| v.into_iter().collect())
            .map_err(failure)
    }
}

// ---- shaders (stage 3) --------------------------------------------------------------------------------------------------------------------------------

/// `gameapp/shader_device.py` (`build`, `live_translate`, `split`) through `ibuilder-shaderbuild` / `ibuilder-shadersplit`. The pipeline's tools (`Adapters::tools`: the app's C callbacks, already
/// behind the memory / thermal gate) become `ibuilder_shader::tools::ShaderTools`; their argv carries the program name first (as `gameapp_tool_fn` always did), the build's does not.
#[cfg(feature = "shaders")]
pub mod shaders {
    use super::*;
    use ibuilder_pipeline::log::Log;
    use ibuilder_shaderbuild::live::{LiveEntry as BEntry, live_translate};
    use ibuilder_shaderbuild::tools::{FnTools, ToolRun};
    use ibuilder_shaderbuild::{BuildEnv, BuildOptions, Finisher, build};
    use std::sync::mpsc;

    pub struct RustShaders;

    /// The pipeline's `ShaderTools` closures as the build's tools
    pub fn fn_tools(t: &ShaderTools) -> FnTools {
        let wrap = |f: ToolFn, prog: &'static str| -> ibuilder_shaderbuild::tools::ToolFnS {
            Arc::new(move |argv: &[String]| {
                let mut full = Vec::with_capacity(argv.len() + 1);
                full.push(prog.to_string());
                full.extend_from_slice(argv);
                let o = f(&full);
                ToolRun {
                    rc: o.rc,
                    stdout: o.stdout,
                    stderr: o.stderr,
                }
            })
        };
        FnTools::new(
            wrap(t.xlate.clone(), "dmsh_xlate"),
            wrap(t.spirv_cross.clone(), "spirv-cross"),
        )
    }

    /// The engine's native tools as pipeline `ShaderTools` (Mac / CLI runs; the app passes callbacks): `xlate` and `spirv-cross` binaries as child processes. argv: program name first, ignored.
    #[cfg(feature = "shader-host-tools")]
    pub fn process_tools(xlate_bin: &Path, spirv_cross_bin: &Path) -> ShaderTools {
        use ibuilder_shaderbuild::tools::ShaderTools as BuildTools; // the trait whose `xlate` / `spirv_cross` the process tools implement
        let p = Arc::new(ibuilder_shaderbuild::tools::ProcessTools::new(
            xlate_bin,
            spirv_cross_bin,
        ));
        let (a, b) = (p.clone(), p);
        let conv = |r: ToolRun| ToolOutput {
            rc: r.rc,
            stdout: r.stdout,
            stderr: r.stderr,
        };
        ShaderTools {
            xlate: Arc::new(move |argv: &[String]| conv(a.xlate(&argv[1..]))),
            spirv_cross: Arc::new(move |argv: &[String]| conv(b.spirv_cross(&argv[1..]))),
        }
    }

    /// The per-shader finisher for a profile folder: `ibuilder_shader::finish` (feature `shader-finish`), else the build refuses (never a stand-in: its files are not a library)
    pub fn make_finisher(
        profile_dir: &Path,
        game_id: &str,
        chain: &str,
        pulling: bool,
    ) -> std::result::Result<Box<dyn Finisher>, String> {
        #[cfg(feature = "shader-finish")]
        {
            let dir = ibuilder_shaderbuild::game::game_dir(profile_dir, game_id);
            Ok(Box::new(
                ibuilder_shaderbuild::finish_adapter::RustFinisher::load(
                    &dir, game_id, chain, pulling,
                )?,
            ))
        }
        #[cfg(not(feature = "shader-finish"))]
        {
            let _ = (profile_dir, game_id, chain, pulling);
            Err("the per-shader finish (ibuilder_shader::finish) is not wired into this build (cargo feature shader-finish)".to_string())
        }
    }

    impl ShaderBackend for RustShaders {
        fn split_version(&self) -> String {
            #[cfg(feature = "shadersplit")]
            {
                ibuilder_shadersplit::split_version()
            }
            #[cfg(not(feature = "shadersplit"))]
            {
                "-".to_string()
            }
        }

        fn split_revisions(&self) -> Option<ibuilder_pipeline::adapters::SplitRevisions> {
            #[cfg(feature = "shadersplit")]
            {
                use ibuilder_shadersplit::record::{LEGACY, READERS};
                Some(ibuilder_pipeline::adapters::SplitRevisions {
                    readers: READERS
                        .iter()
                        .map(|r| ibuilder_pipeline::adapters::SplitReader {
                            name: r.name.to_string(),
                            rev: r.rev,
                            claims: r.claims.iter().map(|c| c.to_string()).collect(),
                        })
                        .collect(),
                    legacy: LEGACY.iter().map(|(n, r)| (n.to_string(), *r)).collect(),
                })
            }
            #[cfg(not(feature = "shadersplit"))]
            {
                None
            }
        }

        fn split_probe(
            &self,
            romfs: &Path,
            dlc: &[PathBuf],
            shader_map: Option<&Path>,
        ) -> Option<String> {
            #[cfg(feature = "shadersplit")]
            {
                let dlc: Vec<&Path> = dlc.iter().map(|p| p.as_path()).collect();
                ibuilder_shadersplit::record::probe_text(romfs, &dlc, shader_map).ok()
            }
            #[cfg(not(feature = "shadersplit"))]
            {
                let _ = (romfs, dlc, shader_map);
                None
            }
        }

        fn split(&self, req: &SplitRequest<'_>, log: &dyn Log) -> Result<SplitResult> {
            #[cfg(feature = "shadersplit")]
            {
                use ibuilder_shadersplit::{Ruzstd, SplitOptions, split};
                super::build_pool();
                struct Gate<'a>(&'a Arc<ibuilder_pipeline::gate::ShaderMemoryGate>);
                impl ibuilder_shadersplit::JobGate for Gate<'_> {
                    fn run(&self, f: &mut dyn FnMut()) {
                        self.0.run(f)
                    }
                }
                let dlc: Vec<&Path> = req.dlc.iter().map(|p| p.as_path()).collect();
                let mut o = SplitOptions::single(
                    req.romfs,
                    req.main_nso,
                    req.pkg_dir,
                    req.tables_dir,
                    req.profile_dir,
                    req.game_id,
                );
                o.dlc = dlc;
                o.jobs = req.jobs.max(1);
                o.memo_dir = Some(req.memo_dir);
                // which readers the game needed, for a targeted re-split (the pipeline keeps it in pack.txt)
                let record = ibuilder_pipeline::adapters::split_record_path(req.tables_dir);
                o.record = Some(&record);
                let l = |s: &str| log.line(s);
                let st = split(&o, &Ruzstd, &Gate(req.gate), &l)
                    .map_err(|e| PipelineError::failed(e.to_string()))?;
                Ok(SplitResult {
                    shaders: st.shaders,
                    jobs_reused: st.jobs_reused,
                    jobs_ran: st.jobs_ran,
                })
            }
            #[cfg(not(feature = "shadersplit"))]
            {
                let _ = (req, log);
                Err(PipelineError::failed(
                    "the shader split (ibuilder-shadersplit) is not wired into this build (cargo feature shadersplit)",
                ))
            }
        }

        fn build(&self, req: &BuildRequest<'_>, progress: &mut dyn FnMut(&str)) -> Result<PathBuf> {
            super::build_pool();
            let finisher = make_finisher(req.profile_dir, req.game_id, req.chain, req.pulling)
                .map_err(PipelineError::failed)?;
            let tools = fn_tools(req.tools);
            let mut o = BuildOptions::new(
                req.pkg_dir,
                req.profile_dir,
                req.game_id,
                req.out_dir,
                req.chain,
                req.cas,
            );
            o.jobs = req.jobs.max(1);
            o.tables = req.tables;
            o.pulling = req.pulling;
            // the build logs from its worker threads; `progress` is the caller's (not `Send`), so the lines travel to this thread
            let (tx, rx) = mpsc::channel::<String>();
            let report = std::thread::scope(|s| {
                let h = s.spawn(move || {
                    ibuilder_pipeline::sys::qos::lower_worker_qos(); // the build's own thread (never the caller's): the workers it starts follow
                    let log = move |l: &str| {
                        let _ = tx.send(l.to_string());
                    };
                    build(
                        &o,
                        &BuildEnv {
                            tools: &tools,
                            finisher: &*finisher,
                            log: &log,
                            cancel: None,
                        },
                    )
                });
                for line in rx {
                    progress(&line);
                }
                h.join()
            });
            match report {
                Ok(Ok(r)) => Ok(r.image),
                Ok(Err(e)) => Err(PipelineError::failed(e)),
                Err(_) => Err(PipelineError::failed("the shader build thread panicked")),
            }
        }

        fn live_translate(&self, req: &LiveRequest<'_>, log: &dyn Log) -> Result<LiveResult> {
            let entries: Vec<BEntry> = req
                .entries
                .iter()
                .map(|e| BEntry {
                    key: e.key.clone(),
                    ctl: e.ctl.clone(),
                    code: e.code.clone(),
                })
                .collect();
            let tools = fn_tools(req.tools);
            let chain = req.chain.to_string();
            let make = move |prof: &Path, gid: &str| make_finisher(prof, gid, &chain, false);
            let l = |s: &str| log.line(s);
            let r = live_translate(
                &entries,
                req.pack,
                req.derived,
                req.game_id,
                req.work,
                req.chain,
                &tools,
                &make,
                &l,
                req.cas,
            )
            .map_err(PipelineError::failed)?;
            Ok(LiveResult {
                ok: r.ok,
                bad: r.bad,
                files: r.files,
            })
        }
    }
}
