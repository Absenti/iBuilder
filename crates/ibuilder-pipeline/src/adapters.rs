//! The seams where the stage workers plug in. Each trait is one thing the Python chain called out to (`patch_nso.py` through
//! `pyrewrite_driver.run_once`, `assemble.py`, `_dmgameapp.sign`, `shader_device.py`, `profile_derive.py`, `pack_nup.py`,
//! `settings.py`, `_dmgameapp.boot`). The other crates (`ibuilder-rewrite`, `ibuilder-macho`, `ibuilder-sign`, `ibuilder-shader`,
//! `ibuilder-profile`, `ibuilder-pack`) are wired in by `ibuilder-cli`'s adapters (and, for the app, by the FFI crate); this crate
//! depends on none of them, so it builds and tests alone. `Unwired` adapters fail with a message that names the missing crate.

use crate::config::Config;
use crate::ctx::View;
use crate::error::{PipelineError, Result};
use crate::gate::ShaderMemoryGate;
use crate::log::Log;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---- stage 1: the Switch 2 container ---------------------------------------------------------------------------------

/// `extract_nup`: a `.nup` (cfg `nup`) into the layout an NSP extraction leaves (`extract/ExeFS`, `extract/RomFS.img`, `control.nacp`,
/// `icon.jpg`; DLC into `Documents/games/<id>/aoc`). Returns the title id. The adapter reads `v.cfg`, `v.docs` and logs through `v.log`.
pub trait NupReader: Send + Sync {
    fn extract_nup(&self, v: &View<'_>) -> Result<String>;
}

// ---- stage 0/1: the profile ----------------------------------------------------------------------------------------------

pub trait ProfileDeriver: Send + Sync {
    /// `do_profile`: derive the runtime profile from the game in Documents; returns the path of the profile.tsv it wrote.
    fn derive(&self, v: &View<'_>) -> Result<PathBuf>;
    /// `pd.render(tid, profile)`: render `profile.tsv` again from the (corrected) game table in `Documents/profile-derived/<tid>`.
    fn render(&self, v: &View<'_>, tid: &str, profile: &Path) -> Result<()>;
}

// ---- stage 2: the rewrite ----------------------------------------------------------------------------------------------------

pub struct RewriteRequest<'a> {
    /// the NSO (`ExeFS/<module>`)
    pub src: &'a Path,
    /// where the rewrite writes `<module>.patched.bin`, `.fixups.tsv`, `.gates.tsv`, `.sites.tsv`, ... (`rewrite/<module>`)
    pub out_dir: &'a Path,
    pub module: &'a str,
    /// DM_REWRITE_JOBS: threads for the big module's scans; the output is identical for any count
    pub jobs: usize,
}

pub trait Rewriter: Send + Sync {
    fn rewrite(&self, req: &RewriteRequest<'_>, v: &View<'_>) -> Result<()>;
}

// ---- stage 2 and the boot: mods ------------------------------------------------------------------------------------------------------------

/// What `gameapp/mods.py` raised (the Python lets some exceptions through that its own handlers do not catch: see `ibuilder-mods`'s `ModsError`): `exc` is
/// `type(error).__name__`, `msg` is `str(error)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModsFailure {
    pub exc: String,
    pub msg: String,
}

impl std::fmt::Display for ModsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.exc, self.msg)
    }
}

/// What `mods.plan` found for one module: the digest of the exefs patches of the enabled mods that name it (an input of the rewrite cache key) and the
/// patches themselves, which only the host understands (`token`: handed back to [`ModsHost::apply_module`]).
#[derive(Clone)]
pub struct ModPlanEntry {
    pub digest: String,
    /// `<mod>/<file> [<block>]` of each patch (logs and tests)
    pub labels: Vec<String>,
    pub token: Arc<dyn std::any::Any + Send + Sync>,
}

/// `gameapp/mods.py`: Atmosphere-style mods (IPS / IPS32 / pchtxt exefs patches applied to the NSO before stage 2, RomFS overlay, cheats). `ibuilder-cli`
/// implements it with `ibuilder-mods`; without one the chain builds and boots as it did before mods existed, and says so when a game has mods.
pub trait ModsHost: Send + Sync {
    /// `plan(docs, tid, exe_dir, modules, log, quiet)`: `[(module, entry)]` for the modules some patch names by build id, in the order of `modules`. `quiet`: no log
    /// lines and no `build_ids.txt` (the cache key's own question).
    fn plan(
        &self,
        docs: &Path,
        tid: &str,
        exe_dir: &Path,
        modules: &[String],
        log: &dyn Log,
        quiet: bool,
    ) -> std::result::Result<Vec<(String, ModPlanEntry)>, ModsFailure>;
    /// `apply_module`: the patched copy of the module (`Documents/games/<id>/mods-exefs/<m>`), or `None` when it cannot be made (said in the log).
    fn apply_module(
        &self,
        docs: &Path,
        tid: &str,
        exe_dir: &Path,
        module: &str,
        entry: &ModPlanEntry,
        log: &dyn Log,
    ) -> std::result::Result<Option<PathBuf>, ModsFailure>;
    /// `remove_patched`: the copies go.
    fn remove_patched(&self, docs: &Path, tid: &str);
    /// `mark_applied`: what this rewrite saw, for the sheet's "Rebuild to apply".
    fn mark_applied(
        &self,
        docs: &Path,
        tid: &str,
        log: &dyn Log,
    ) -> std::result::Result<(), ModsFailure>;
    /// `apply_boot`: `DM_ROMFS_OVERLAY` and `DM_CHEATS` into `env` (a switch the environment already names wins). Returns what it set.
    fn apply_boot(
        &self,
        docs: &Path,
        tid: &str,
        log: &dyn Log,
        env: &dyn crate::env::Env,
    ) -> std::result::Result<BTreeMap<String, String>, ModsFailure>;
}

// ---- stage 3: the shaders ---------------------------------------------------------------------------------------------------------

/// A native tool run in process: `argv` as `subprocess` would get it, program name first; the result is `(returncode, stdout, stderr)`.
#[derive(Clone, Debug, Default)]
pub struct ToolOutput {
    pub rc: i32,
    pub stdout: String,
    pub stderr: String,
}
pub type ToolFn = Arc<dyn Fn(&[String]) -> ToolOutput + Send + Sync>;

/// The tools the shader chain calls (all third-party and staying so: the engine's `dmsh_xlate` over Ryujinx.Graphics.Shader, C#, SPIRV-Cross, C, and for OpenGL games
/// glslang, C++: `glslangValidator`'s command line, in the app `gameapp_glslang` of `app/c/glslang_cli.cpp`). `glslang: None`: an OpenGL game's GLSL cannot be compiled
/// (`ibuilder_glvk::tools::NO_GLSLANG` per shader), every other game is unaffected.
#[derive(Clone)]
pub struct ShaderTools {
    pub xlate: ToolFn,
    pub spirv_cross: ToolFn,
    pub glslang: Option<ToolFn>,
}

impl ShaderTools {
    /// Every call goes through the memory/thermal gate (`gate.wrap` in Python).
    pub fn gated(&self, gate: &Arc<ShaderMemoryGate>) -> ShaderTools {
        let wrap = |f: &ToolFn| -> ToolFn {
            let f = f.clone();
            let gate = gate.clone();
            Arc::new(move |argv: &[String]| gate.run(|| f(argv)))
        };
        ShaderTools {
            xlate: wrap(&self.xlate),
            spirv_cross: wrap(&self.spirv_cross),
            glslang: self.glslang.as_ref().map(wrap),
        }
    }
}

/// Where a backend that knows which readers its split used leaves that record (`ibuilder_shadersplit::record`: `revs`, `uses` and `unknown` rows, tab-separated): beside the split's
/// `tables/` folder, `<work>/split.record`. Stage 3 copies its rows into the library's `pack.txt` (`split-revs`, `split-uses`, `split-unknown`) for a targeted re-split.
pub fn split_record_path(tables_dir: &Path) -> PathBuf {
    tables_dir.with_file_name("split.record")
}

/// One reader family of the split and its revision now (`ibuilder_shadersplit::record::READERS`): a library whose record names another revision of a reader it used is split again
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplitReader {
    pub name: String,
    pub rev: u32,
    /// the unknown kinds the reader would read: `.ext`, or `#<hex prefix of the first bytes>`
    pub claims: Vec<String>,
}

/// What `stale` compares a library's record with: this build's reader revisions, and the revisions a library without a record is taken to have
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SplitRevisions {
    pub readers: Vec<SplitReader>,
    pub legacy: BTreeMap<String, u32>,
}

pub struct SplitRequest<'a> {
    pub romfs: &'a Path,
    pub main_nso: &'a Path,
    pub pkg_dir: &'a Path,
    pub tables_dir: &'a Path,
    /// the folder holding the game's table folder (`dirname(derived)`)
    pub profile_dir: &'a Path,
    pub game_id: &'a str,
    pub dlc: &'a [PathBuf],
    /// reading threads
    pub jobs: usize,
    /// where finished jobs are kept (`<work>/splitjobs`): an interrupted split resumed gives the same bytes as one that was not
    pub memo_dir: &'a Path,
    /// every job the split submits goes through the gate
    pub gate: &'a Arc<ShaderMemoryGate>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SplitResult {
    pub shaders: usize,
    pub jobs_reused: usize,
    pub jobs_ran: usize,
}

pub struct BuildRequest<'a> {
    pub pkg_dir: &'a Path,
    pub profile_dir: &'a Path,
    pub game_id: &'a str,
    pub out_dir: &'a Path,
    pub chain: &'a str,
    pub jobs: usize,
    /// write the c1_full / cb_extent tables (cfg `shader_tables` != 0)
    pub tables: bool,
    /// build the "pulling" libraries (Metal 4 pipelines)
    pub pulling: bool,
    /// the translation cache folder (shared `Documents/shadercache`, or the work folder's `cas`)
    pub cas: &'a Path,
    pub tools: &'a ShaderTools,
}

pub struct LiveEntry {
    pub key: String,
    pub ctl: Vec<u8>,
    pub code: Vec<u8>,
}

pub struct LiveRequest<'a> {
    pub entries: &'a [LiveEntry],
    pub pack: &'a Path,
    pub derived: &'a Path,
    pub game_id: &'a str,
    pub work: &'a Path,
    pub chain: &'a str,
    pub tools: &'a ShaderTools,
    pub cas: Option<&'a Path>,
}

#[derive(Default)]
pub struct LiveResult {
    /// key -> the stage it was translated for
    pub ok: BTreeMap<String, String>,
    /// key -> why it was refused
    pub bad: BTreeMap<String, String>,
    /// the files the host reads, by name
    pub files: BTreeMap<String, Vec<u8>>,
}

pub trait ShaderBackend: Send + Sync {
    /// The shader split this build runs, as a short digest ("-" when unknown): a split that finds more shaders is not a chain change, so the
    /// library's chain alone cannot say it is out of date (`split_version`).
    fn split_version(&self) -> String;
    /// The split's per-reader revisions (None: only `split_version` says whether a library is current). With them, a library is split again only when a reader its game needs changed.
    fn split_revisions(&self) -> Option<SplitRevisions> {
        None
    }
    /// For a library split before records existed: which readers its game's files need (the record's text: `uses` and `unknown` rows), from the RomFS, the DLC and the library's shader
    /// map; None when the backend cannot tell (the library is then split again when any reader changed)
    fn split_probe(
        &self,
        _romfs: &Path,
        _dlc: &[PathBuf],
        _shader_map: Option<&Path>,
    ) -> Option<String> {
        None
    }
    fn split(&self, req: &SplitRequest<'_>, log: &dyn Log) -> Result<SplitResult>;
    /// Translate and write the library image; returns its path (`b-<chain>.img`).
    fn build(&self, req: &BuildRequest<'_>, progress: &mut dyn FnMut(&str)) -> Result<PathBuf>;
    /// Shaders the running game lacks, translated while it plays (paches/0008).
    fn live_translate(&self, req: &LiveRequest<'_>, log: &dyn Log) -> Result<LiveResult>;
    /// The engine's `finish()` rewrite sequence (`tools/shader_post.py`) over a captured manifest; returns its rc.
    fn post_rewrite(&self, _manifest: &Path, _out: &Path, _log: &dyn Log) -> Result<i32> {
        Err(PipelineError::failed(
            "shader post-rewrites are not available in this build",
        ))
    }
    // ---- games without NVN shaders: the host's GL / Vulkan layers (ibuilder-glvk; crate::layers) ----
    /// The version of the layer libraries this backend writes (`ibuilder_glvk::layer_version`); `-`: it writes none.
    fn layer_version(&self) -> String {
        "-".to_string()
    }
    /// A game whose NVN split found no shader: `Ok(None)` when it draws with NVN (or this backend has no layer support), `Ok(Some)` when its GL / Vulkan layer
    /// library was written into `req.out_dir` (`vk/` or `gl/`), `Err` when it is a GL / Vulkan game whose library could not be made.
    fn build_layer(&self, _req: &LayerRequest<'_>, _log: &dyn Log) -> Result<Option<LayerResult>> {
        Ok(None)
    }
}

/// [`ShaderBackend::build_layer`]'s inputs: the roots the engine's NSP script gives `vk_shaders.py` / `gl_shaders.py`
pub struct LayerRequest<'a> {
    pub romfs: &'a Path,
    pub main_nso: &'a Path,
    pub dlc: &'a [PathBuf],
    /// the pack folder being made: the library goes to `<out_dir>/vk/` or `<out_dir>/gl/`
    pub out_dir: &'a Path,
    /// scratch files of the tools
    pub work: &'a Path,
    pub jobs: usize,
    /// SPIRV-Cross (gated); the GL layer's glslang is the backend's own
    pub tools: &'a ShaderTools,
}

#[derive(Clone, Debug, Default)]
pub struct LayerResult {
    /// `vk` or `gl`
    pub api: String,
    pub shaders: usize,
    /// what the tools printed (failed shaders, skipped stages)
    pub lines: Vec<String>,
}

// ---- stage 4: assemble and sign ----------------------------------------------------------------------------------------------

pub struct AssembleRequest<'a> {
    /// the prebuilt host dylib the assembler grafts the game onto
    pub host: &'a Path,
    /// the framework executable to write (`DmGameProbe.framework/DmGameProbe`)
    pub out: &'a Path,
    /// `<name>.patched.bin` + `.fixups.tsv` (+ `.gates.tsv` ...) per module
    pub flat: &'a Path,
    pub modules: &'a [String],
}

#[derive(Clone, Debug, Default)]
pub struct AssembleInfo {
    /// load commands written, when the assembler says
    pub ncmds: Option<u32>,
}

pub trait Assembler: Send + Sync {
    fn assemble(&self, req: &AssembleRequest<'_>, v: &View<'_>) -> Result<AssembleInfo>;
}

/// What `_dmgameapp.sign` returned: `(rc, mode, identifier, err)`. rc: 0 signed; 10 no embedded.mobileprovision, 11 unreadable, 12 no certificate
/// in this app's Keychain, 13 the item carries no .p12, 14 the .p12's passphrase does not fit the shim's 511-byte buffer, 20 the signer itself refused (`err` then holds its message).
#[derive(Clone, Debug, Default)]
pub struct SignOutcome {
    pub rc: i32,
    pub mode: String,
    pub identifier: String,
    pub err: String,
}

pub trait Signer: Send + Sync {
    /// Sign `<framework_dir>/<exe>` in process (adhoc, or with the device identity when the app carries a provisioning profile).
    fn sign(&self, framework_dir: &Path, exe: &str, v: &View<'_>) -> SignOutcome;
    /// Sign `<framework_dir>/<exe>` AD HOC, with no identity: what stage 4b falls back to when [`Signer::sign`] found no identity on this install (rc 10, 12, 13:
    /// `assemble::adhoc_fallback`). Such a framework cannot be played inside the app on a device, but an installer re-signs it in an exported IPA (`Export as its own app`).
    /// `None`: this signer has no ad hoc mode, and the build fails as it did before the fallback existed.
    fn sign_adhoc(&self, _framework_dir: &Path, _exe: &str, _v: &View<'_>) -> Option<SignOutcome> {
        None
    }
    /// This app's own CodeDirectory identifier (the framework is signed *as this app*, docs/SIGN-IN-APP.md)
    fn identifier(&self) -> String {
        "dev.ibuilder.app".to_string()
    }
}

// ---- stage 5: the boot -------------------------------------------------------------------------------------------------------------

pub struct BootRequest<'a> {
    pub framework: &'a Path,
    /// the folder of the rewritten modules (`flat/`)
    pub patched: &'a Path,
    /// `main subsdk0 sdk`
    pub modules: &'a str,
    pub profile: &'a Path,
    pub romfs: &'a str,
    /// time limit in seconds; 0 = none
    pub seconds: i64,
}

/// `(stage, guest)`: stage 0 = the guest was called and came back, 3 = dlopen failed, 4 = dm_host_probe_boot not found; guest = its own
/// return value, -1 while it was never called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootResult {
    pub stage: i32,
    pub guest: i32,
}

pub trait BootHost: Send + Sync {
    /// dlopen the framework and run the guest. The process environment has been committed already (`ProcessEnv::commit`).
    fn boot(&self, req: &BootRequest<'_>, v: &View<'_>) -> BootResult;
}

// ---- the game's settings screen ------------------------------------------------------------------------------------------------------

/// The engine's `dm/games/settings.py`: the rows of `settings.tsv` whose condition holds for THIS game (its profile's macros, its rewritten
/// modules' fixups, its languages from `control.nacp`, its graphics API) as the JSON document the app's settings sheet draws. The pipeline
/// post-processes it (`settings::write_launcher_settings`).
pub trait LauncherBase: Send + Sync {
    fn base_document(
        &self,
        v: &View<'_>,
        tid: &str,
        main_dir: &Path,
        control: &Path,
        gfxapi: &str,
        derived_root: Option<&Path>,
    ) -> Result<serde_json::Value>;
}

// ---- the set ---------------------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
pub struct Adapters {
    pub nup: Option<Arc<dyn NupReader>>,
    pub profile: Arc<dyn ProfileDeriver>,
    pub rewriter: Arc<dyn Rewriter>,
    pub shaders: Option<Arc<dyn ShaderBackend>>,
    pub tools: Option<ShaderTools>,
    pub assembler: Arc<dyn Assembler>,
    pub signer: Arc<dyn Signer>,
    pub boot: Option<Arc<dyn BootHost>>,
    pub launcher: Option<Arc<dyn LauncherBase>>,
    /// `gameapp/mods.py`; `None`: no mods support in this build
    pub mods: Option<Arc<dyn ModsHost>>,
}

impl Adapters {
    /// Every worker missing: each stage fails with a message that says which crate to wire.
    pub fn unwired() -> Adapters {
        Adapters {
            nup: None,
            profile: Arc::new(Unwired("ibuilder-profile")),
            rewriter: Arc::new(Unwired("ibuilder-rewrite")),
            shaders: None,
            tools: None,
            assembler: Arc::new(Unwired("ibuilder-macho")),
            signer: Arc::new(Unwired("ibuilder-sign")),
            boot: None,
            launcher: None,
            mods: None,
        }
    }
}

/// A worker that is not wired in this build.
pub struct Unwired(pub &'static str);

impl Unwired {
    fn err<T>(&self, what: &str) -> Result<T> {
        Err(PipelineError::failed(format!(
            "{what}: the {} stage worker is not wired into this build (add the crate through the adapters)",
            self.0
        )))
    }
}

impl ProfileDeriver for Unwired {
    fn derive(&self, _v: &View<'_>) -> Result<PathBuf> {
        self.err("profile")
    }
    fn render(&self, _v: &View<'_>, _tid: &str, _profile: &Path) -> Result<()> {
        self.err("profile render")
    }
}
impl Rewriter for Unwired {
    fn rewrite(&self, _req: &RewriteRequest<'_>, _v: &View<'_>) -> Result<()> {
        self.err("rewrite")
    }
}
impl Assembler for Unwired {
    fn assemble(&self, _req: &AssembleRequest<'_>, _v: &View<'_>) -> Result<AssembleInfo> {
        self.err("assemble")
    }
}
impl Signer for Unwired {
    fn sign(&self, _d: &Path, _e: &str, _v: &View<'_>) -> SignOutcome {
        SignOutcome {
            rc: 20,
            mode: "unwired".into(),
            identifier: self.identifier(),
            err: format!("the {} stage worker is not wired into this build", self.0),
        }
    }
}

/// `cfg` helper shared by the adapters: the `host_dylib` row.
pub fn host_dylib(cfg: &Config) -> Result<PathBuf> {
    cfg.get("host_dylib")
        .map(PathBuf::from)
        .ok_or_else(|| PipelineError::missing("host_dylib is not in the config"))
}
