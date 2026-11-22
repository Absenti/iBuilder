//! The chain driver (`gameapp_pipeline.py`'s `main` / `_chain`): stage 0/1 derive the profile, stage 2 rewrite, stage 3 shaders, stage 4 assemble and sign, stage 5 boot, with
//! the settings, the memory lines, the interrupted-run markers, the resume, the timings row and the logs around them. Workers are `adapters`; each step here is a `Stage`.
//!
//! Result codes (what `gameapp_run_chain` returned): 0 the chain ran and every stage that ran succeeded (or had nothing to build), 1 a stage that must succeed failed (NUP read, profile,
//! rewrite) or a boot-only launch whose boot did not run, 2 no `extract_out` in the config, 3 boot-only had nothing to boot, 4 the game has no shader library: stage 3 had to build one and
//! could not (`shaders=failed (no library: ...)`; the chain still went on to the assembly, as in Python, but the game is neither booted nor stamped), or a boot-only launch whose stage 3
//! could not build it (the boot is refused), 5 no framework: stage 4 (the assembly or the signature) failed (`assemble=failed`; the chain does not stop before it, as in Python, but there
//! is nothing to boot or to stamp; it wins over 4 when both failed), 75 the chain ended on purpose to be restarted (memory between the rewrite and the shaders), 130 cancelled, 131 paused
//! by the user (both stopped at the next safe point; `stop`). `shaders=none (...)` (nothing to build: no chain in the config, `shader_build=0`, a game whose split finds no shader) is not
//! a failure and keeps 0. `ChainReport::status` says which stage failed in every case.

use crate::boot::{boot_after_build, boot_only};
use crate::ctx::Ctx;
use crate::error::{PipelineError, Result};
use crate::markers::{note_killed_run, restart_for_shaders, take_continue, write_pipeline_boot};
use crate::memory::mem_line;
use crate::profile::{drop_exefs, invalidate_previous_build, profile_modules, save_game_icon};
use crate::rewrite::{do_rewrite, patched_dir};
use crate::settings::write_launcher_settings;
use crate::shaderpack::use_config_paths;
use crate::shaders::do_shaders;
use crate::stage::{Stage, StageId, StageOutcome, write_stage};
use crate::stop;
use crate::util::{is_file, utc_now_iso};
use crate::{assemble::do_assemble, config::Config};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const RC_OK: i32 = 0;
pub const RC_FAILED: i32 = 1;
pub const RC_NO_EXTRACT_OUT: i32 = 2;
pub const RC_NOTHING_TO_BOOT: i32 = 3;
/// The game has no shader library (review r4 §4.1): stage 3 had to build one and could not (`shaders=failed (no library: ...)`). The chain went on to the assembly (a failed stage 3 does not
/// stop it, as in Python) but it boots nothing, writes no framework stamp (the app reads the build as not ready) and returns this instead of 0; a boot-only launch whose stage 3 cannot
/// build the missing library refuses the boot and returns it too. A game with no library draws nothing. `none (...)` (nothing to build) is not this.
pub const RC_NO_SHADER_LIBRARY: i32 = 4;
/// No framework (review r4 follow-up): stage 4 failed, the assembly or the signature (`assemble=failed`: the signer's rc 10-14 for a missing or unreadable certificate, a module the
/// assembler refused, ...). Nothing was stamped or booted (a framework that was not signed is not one to boot); the chain returned 0 here, as Python did, and the app said "Built: ready
/// to play.". When stage 3 failed too, this is the code (the game has neither), and the status names both.
pub const RC_NO_FRAMEWORK: i32 = 5;
pub const RC_RESTART: i32 = crate::state::RESTART_RC;
pub const RC_CANCELLED: i32 = 130;
/// The run was paused by the user (`stop::request(stop::PAUSE)`, the app's Pause): it stopped at the next safe point and kept everything (the shader cache, the rewrite cache, the split's jobs), so a
/// later run continues from there. 131 is next to 130 (cancelled) and unused by anything else the chain returns (0, 1, 2, 3, 4, 5, 75, 130).
pub const RC_PAUSED: i32 = 131;

/// What a chain run produced.
#[derive(Debug, Clone)]
pub struct ChainReport {
    pub rc: i32,
    /// per stage: `ok`, `none (...)` (stage 3 with nothing to build: no chain in the config, `shader_build=0`, a game whose split finds no shader), `failed`, `failed (no library: ...)`
    /// (stage 3 that was to build a library and could not: it does not stop the chain, but it is a failure)
    pub status: BTreeMap<String, String>,
    /// the row's result: `ok`, `failed (stage, ...)`, `paused: ...`, `failed`
    pub result: String,
    pub framework: Option<PathBuf>,
    pub flat: Option<PathBuf>,
}

/// The status of a stage 3 that had to build the game's shader library and could not (`failed (no library: ...)`), or `None`. `none (...)` (nothing to build) is not a failure.
pub fn shader_library_failure(status: &BTreeMap<String, String>) -> Option<&str> {
    status
        .get("shaders")
        .map(String::as_str)
        .filter(|v| v.starts_with("failed"))
}

impl ChainReport {
    /// True when every stage that ran succeeded (or reports "none": nothing to build). A `failed (...)` stage 3 is not ok.
    pub fn all_ok(&self) -> bool {
        self.rc == 0
            && self
                .status
                .values()
                .all(|v| v == "ok" || v.starts_with("none"))
    }
}

// ---- the stages ----------------------------------------------------------------------------------------------------------------------------------

/// Stage 1 for a Switch 2 game: its `.nup` container read in this process (`extract_nup`).
pub struct NupStage;
impl Stage for NupStage {
    fn id(&self) -> StageId {
        StageId::Nup
    }
    fn run(&self, ctx: &mut Ctx) -> Result<StageOutcome> {
        let nup = ctx.adapters.nup.clone().ok_or_else(|| {
            PipelineError::failed("reading a Switch 2 package: no NUP reader in this build")
        })?;
        let extract = ctx
            .cfg
            .get("extract_out")
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.docs.extract());
        write_stage(
            &ctx.docs,
            1,
            5,
            &format!(
                "reading the Switch 2 package ({})",
                Path::new(ctx.cfg.get_or("nup", ""))
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            Some(&*ctx.log),
        );
        let t0 = Instant::now();
        nup.extract_nup(&ctx.view())?;
        ctx.log(&format!(
            "nup: read in {:.1}s -> {}",
            t0.elapsed().as_secs_f64(),
            extract.display()
        ));
        ctx.cfg.set(
            "exe_dir",
            extract.join("ExeFS").to_string_lossy().into_owned(),
        );
        Ok(StageOutcome::Done)
    }
}

/// Stage 0/1: the runtime profile is DERIVED here, from the game in Documents -- not supplied (docs/PROFILE-DERIVED.md). The config's `profile` path (`Documents/profile.tsv`) is overwritten
/// with what this process derives, so the boot reads the profile the app produced.
pub struct ProfileStage;
impl Stage for ProfileStage {
    fn id(&self) -> StageId {
        StageId::Profile
    }
    fn run(&self, ctx: &mut Ctx) -> Result<StageOutcome> {
        ctx.env.set("GAMEAPP_BUNDLE", &ctx.bundle.to_string_lossy());
        write_stage(
            &ctx.docs,
            1,
            5,
            "deriving the profile from the game in Documents",
            Some(&*ctx.log),
        );
        let t0 = Instant::now();
        let p = ctx.adapters.profile.clone();
        let out = p.derive(&ctx.view())?;
        ctx.log(&format!(
            "stage 1/5 profile: derived in {:.1}s -> {}",
            t0.elapsed().as_secs_f64(),
            out.display()
        ));
        ctx.cfg.set("profile", out.to_string_lossy().into_owned());
        Ok(StageOutcome::Done)
    }
}

pub struct RewriteStage;
impl Stage for RewriteStage {
    fn id(&self) -> StageId {
        StageId::Rewrite
    }
    fn run(&self, ctx: &mut Ctx) -> Result<StageOutcome> {
        let patched = do_rewrite(ctx)?;
        write_launcher_settings(ctx, &patched.join("main"));
        Ok(StageOutcome::Done)
    }
}

pub struct ShadersStage;
impl Stage for ShadersStage {
    fn id(&self) -> StageId {
        StageId::Shaders
    }
    fn run(&self, ctx: &mut Ctx) -> Result<StageOutcome> {
        Ok(match do_shaders(ctx)? {
            Some(_) => StageOutcome::Done,
            // a stop that ended the stage early is not "no library": the run ends paused or cancelled
            None if ctx.cancelled() => return Err(PipelineError::Cancelled),
            // nothing to build (a failure to build is an `Err` of `do_shaders`: `failed (no library: ...)`)
            None => StageOutcome::Status(
                "none (the game will draw nothing until it has a library)".into(),
            ),
        })
    }
}

pub struct AssembleStage;
impl Stage for AssembleStage {
    fn id(&self) -> StageId {
        StageId::Assemble
    }
    fn run(&self, ctx: &mut Ctx) -> Result<StageOutcome> {
        do_assemble(ctx)?;
        Ok(StageOutcome::Done)
    }
}

/// `nup_extracted`: extract/ExeFS/main and extract/RomFS.img are there: an earlier run of this build already read the Switch 2 package.
pub fn nup_extracted(ctx: &Ctx) -> bool {
    let extract = ctx
        .cfg
        .get("extract_out")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.docs.extract());
    is_file(&extract.join("ExeFS").join("main")) && is_file(&extract.join("RomFS.img"))
}

/// A game file that holds nothing: missing, or zero bytes (the app's placeholder for a file it removed after the extraction, patches 0068 / 0071 / 0073).
pub fn nup_is_placeholder(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() == 0)
        .unwrap_or(true)
}

// ---- the driver ------------------------------------------------------------------------------------------------------------------------------------

fn timed_stage(ctx: &mut Ctx, stage: &dyn Stage) -> Result<StageOutcome> {
    let t = Instant::now();
    let key = stage.id().key();
    crate::diag::event(
        "stage_begin",
        serde_json::json!({"stage": key, "mem": crate::diag::mem_value(&ctx.mem.mem())}),
    );
    let mut r = stage.run(ctx);
    // A stage that ends in an error while a stop is asked for (the Switch 2 reader's "cancelled", a rewrite cut short) ends because of the stop: the run is paused or cancelled, not failed.
    if let Err(e) = &r
        && !matches!(e, PipelineError::Cancelled)
        && ctx.cancelled()
    {
        ctx.log(&format!("[stop] stage {key} ended with an error while {} was asked for ({e}): counted as the stop", stop::name(ctx.stop_kind())));
        r = Err(PipelineError::Cancelled);
    }
    let secs = t.elapsed().as_secs_f64();
    ctx.timing.add(key, secs);
    let outcome = match &r {
        Ok(StageOutcome::Done) => "done".to_string(),
        Ok(StageOutcome::Status(s)) => format!("status: {s}"),
        Ok(StageOutcome::Restart) => "restart".to_string(),
        Err(e) => format!("failed: {e}"),
    };
    crate::diag::event(
        if r.is_err() { "fail" } else { "stage_end" },
        serde_json::json!({"stage": key, "seconds": (secs * 1000.0).round() / 1000.0, "outcome": outcome, "mem": crate::diag::mem_value(&ctx.mem.mem())}),
    );
    r
}

/// Run the chain on `ctx.cfg` (read the config first: `Chain::load_config`). The Python `main()` + `_chain()`.
pub fn run_chain(ctx: &mut Ctx) -> ChainReport {
    // A pause or cancel is asked of the run that is going. One that came before this run started, or after the last one ended, is not for this one (and the app's own token,
    // `ctx.cancel`, is the caller's to set and clear).
    stop::clear();
    let report = chain_inner(ctx);
    if ctx.timing.cfg.is_some() && ctx.timing.record {
        let cfg = ctx.timing.cfg.clone().unwrap();
        let result = ctx.timing.result.clone();
        let stats = ctx.stats.clone();
        let mem = ctx.mem.clone();
        let log = ctx.log.clone();
        ctx.timing.finish(&cfg, &result, &stats, &*mem, &*log);
    }
    report
}

/// Read `gameapp.config` (or `path`) into the context; the chain log line the Python wrote.
pub fn load_config(ctx: &mut Ctx, path: Option<&Path>) {
    let p = path.map(PathBuf::from).unwrap_or_else(|| ctx.docs.config());
    ctx.cfg = Config::read(&p);
    ctx.log(&format!("config {}: {}", p.display(), ctx.cfg));
}

fn report(ctx: &Ctx, rc: i32) -> ChainReport {
    ChainReport {
        rc,
        status: ctx.status.clone(),
        result: ctx.timing.result.clone(),
        framework: None,
        flat: None,
    }
}

fn chain_inner(ctx: &mut Ctx) -> ChainReport {
    use_config_paths(ctx);
    crate::diag::start(ctx);
    if !ctx.cfg.contains("extract_out") {
        ctx.log("no extract_out in the config: nothing to do");
        return report(ctx, RC_NO_EXTRACT_OUT);
    }
    let extract_out = ctx.cfg.get_or("extract_out", "").to_string();
    ctx.cfg.set_default(
        "exe_dir",
        Path::new(&extract_out)
            .join("ExeFS")
            .to_string_lossy()
            .into_owned(),
    );
    ctx.log(&format!(
        "pid {}, platform {}-{}",
        std::process::id(),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    ctx.log(&format!(
        "this app signs as its own identifier: {}",
        ctx.adapters.signer.identifier()
    ));
    mem_line("chain-start", &*ctx.log, &*ctx.mem);
    ctx.timing.cfg = Some(ctx.cfg.clone());
    ctx.timing.launch_available = ctx.mem.mem().available;
    note_killed_run(ctx);
    crate::shaders::recover_interrupted_installs(ctx); // a shader pack install a kill cut short (review r4 §4.1): before a boot reads a pack or stage 3 looks for one
    let continued = take_continue(ctx);

    // A boot-only launch: an earlier launch built and signed the framework and left this request, so nothing here runs a stage -- this process exists to hand the guest a machine that has not
    // rewritten anything. The request is NOT removed here: boot_only() marks it attempted before the boot and consumes it once the boot has come back, so a boot that dies leaves it for the next
    // tap (docs/DECISIONS.md trap 28).
    if ctx.docs.boot_only().exists() {
        ctx.log("Documents/gameapp.boot-only found: stage 5 only, no stage of the chain is re-run");
        ctx.timing.record = false; // no stage of the chain ran: no row
        let rc = boot_only(ctx);
        return report(ctx, rc);
    }

    ctx.status.clear();
    invalidate_previous_build(ctx);
    ctx.timing.result = "failed".into();
    if ctx.cancelled() {
        return stopped(ctx, "the start");
    }
    let game = ctx.cfg.get_nonempty("game").unwrap_or("-").to_string();
    ctx.timing.dump = ctx.cfg.get_or("game", "").to_string();
    ctx.timing.merge_partial(&game, !continued.is_empty());
    let cont_stage: i64 = continued
        .get("stage")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if ctx.cfg.get_nonempty("nup").is_some()
        && !continued.is_empty()
        && cont_stage >= 2
        && nup_extracted(ctx)
    {
        ctx.log("stage 1 nup: the package was read by the build this one continues (extract/ExeFS and RomFS.img are there): not read again");
    } else if let Some(nup) = ctx.cfg.get_nonempty("nup").map(String::from)
        && nup_is_placeholder(Path::new(&nup))
        && nup_extracted(ctx)
    {
        // "Optimize storage" (app patch 0073) replaces a game's file by an empty one once it is extracted: a rebuild (after an app update) works from the extraction kept on disk
        ctx.log(&format!("stage 1 nup: {nup} is an empty placeholder (the game's file was removed after its extraction) and extract/ExeFS and RomFS.img are there: built from them, not read"));
    } else if ctx.cfg.get_nonempty("nup").is_some() {
        match timed_stage(ctx, &NupStage) {
            Ok(_) => {
                ctx.status.insert("nup".into(), "ok".into());
            }
            Err(PipelineError::Cancelled) => return stopped(ctx, "stage 1 (the Switch 2 package)"),
            Err(e) => {
                ctx.log(&format!("stage 1 nup FAILED: {e}"));
                ctx.log("the chain stops: the Switch 2 package could not be read");
                return report(ctx, RC_FAILED);
            }
        }
    }
    if ctx.cancelled() {
        return stopped(ctx, "the end of stage 1");
    }
    // Stage 0: the runtime profile is DERIVED here
    match timed_stage(ctx, &ProfileStage) {
        Ok(_) => {
            ctx.status.insert("profile".into(), "ok".into());
            if let Some(tid) = ctx.title_id() {
                ctx.timing.game = tid;
            }
            save_game_icon(ctx);
        }
        Err(PipelineError::Cancelled) => return stopped(ctx, "stage 0 (the profile)"),
        Err(e) => {
            ctx.log(&format!("stage 0 profile FAILED: {e}"));
            ctx.log("the chain stops: without this game's profile every later stage would work on the previous game's");
            return report(ctx, RC_FAILED);
        }
    }
    if ctx.cancelled() {
        return stopped(ctx, "the end of stage 0 (the profile)");
    }
    if let Some(mods) = profile_modules(ctx)
        && Some(mods.as_str()) != ctx.cfg.get("modules")
    {
        ctx.log(&format!(
            "modules: {mods} (the profile's; the config said '{}')",
            ctx.cfg.get_or("modules", "")
        ));
        ctx.cfg.set("modules", mods);
    }
    match timed_stage(ctx, &RewriteStage) {
        Ok(_) => {
            ctx.status.insert("rewrite".into(), "ok".into());
        }
        Err(PipelineError::Cancelled) => return stopped(ctx, "stage 2 (the rewrite)"),
        Err(e) => {
            ctx.log(&format!("stage 2 FAILED: {e}"));
            ctx.log("the chain stops: a framework assembled from a failed rewrite would not run");
            return report(ctx, RC_FAILED);
        }
    }
    if ctx.cancelled() {
        return stopped(ctx, "the end of stage 2 (the rewrite)"); // before the restart decision: a stop is not a reason to write gameapp.continue
    }
    let rewrote_here = !ctx.rewrite_ran.is_empty();
    let launch_available = ctx.timing.launch_available;
    if restart_for_shaders(ctx, rewrote_here, launch_available) {
        ctx.timing.result =
            "paused: the app restarts to give the rewrite's memory back, stage 3 continues".into();
        return report(ctx, RC_RESTART);
    }
    match timed_stage(ctx, &ShadersStage) {
        Ok(StageOutcome::Status(s)) => {
            ctx.status.insert("shaders".into(), s);
        }
        Ok(_) => {
            ctx.status.insert("shaders".into(), "ok".into());
        }
        Err(PipelineError::Cancelled) => return stopped(ctx, "stage 3 (the shaders)"),
        Err(e) => {
            // the chain goes on (the contract above), but a game that was to get a library and has none is a failed run, not "none" (review r3 §3.1)
            ctx.log(&format!("stage 3 FAILED: {e}"));
            ctx.status
                .insert("shaders".into(), format!("failed (no library: {e})"));
        }
    }
    if ctx.cancelled() {
        return stopped(ctx, "the end of stage 3 (the shaders)");
    }
    let (mut framework, mut flat): (Option<PathBuf>, Option<PathBuf>) = (None, None);
    let mut assemble_error: Option<String> = None;
    let t = Instant::now();
    let r = do_assemble(ctx);
    ctx.timing.add("assemble", t.elapsed().as_secs_f64());
    match r {
        Ok((f, fl)) => {
            framework = Some(f);
            flat = Some(fl);
            ctx.status.insert("assemble".into(), "ok".into());
        }
        Err(PipelineError::Cancelled) => return stopped(ctx, "stage 4 (assembly)"),
        Err(e) => {
            ctx.log(&format!("stage 4 FAILED: {e}"));
            ctx.status.insert("assemble".into(), "failed".into());
            assemble_error = Some(e.to_string());
        }
    }
    if ctx.cancelled() {
        return stopped(ctx, "the end of stage 4 (assembly)"); // the game is not started: a stop asked for during the build is not followed by the boot
    }
    ctx.log(&format!("stage status: {:?}", ctx.status));
    let all_ok = ctx
        .status
        .values()
        .all(|v| v == "ok" || v.starts_with("none"));
    ctx.timing.result = if all_ok {
        "ok".into()
    } else {
        format!(
            "failed ({})",
            ctx.status
                .iter()
                .filter(|(_, v)| *v != "ok" && !v.starts_with("none"))
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if ctx.status.get("assemble").map(String::as_str) == Some("ok")
        && ctx.status.get("shaders").map(String::as_str) == Some("ok")
    {
        drop_exefs(ctx);
    }
    ctx.log("process spawns: none (ibuilder-pipeline has no process API: no fork, exec, posix_spawn or system anywhere in the chain)");
    let no_library = shader_library_failure(&ctx.status).map(String::from);

    if let Some(fw) = framework.clone().filter(|f| is_file(f)) {
        let flat_p = flat.clone().unwrap_or_else(|| patched_dir(ctx));
        write_pipeline_boot(
            ctx,
            &fw.to_string_lossy(),
            &flat_p.to_string_lossy(),
            ctx.cfg.get_or("modules", "main subsdk0 sdk"),
            ctx.cfg.get_or("profile", ""),
            ctx.cfg.get_or("romfs", ""),
        );
        ctx.log("wrote Documents/pipeline.boot for stage 5");
        if let Some(why) = &no_library {
            // review r4 §4.1: the guest would start with no shader library and draw nothing while the app read "Ready": it is not booted, here or by a later launch (no gameapp.boot-only)
            ctx.log(&format!("stage 5 NOT STARTED: [RED] the game has no shader library (stage 3: {why}): it would draw nothing, so it is not booted -- rebuild it, or send the log"));
            if ctx.boot_enabled && fs::remove_file(ctx.docs.running()).is_ok() {
                ctx.log("the chain finished (marker cleared): the run ends here, with no boot");
            }
        } else if ctx.export_only {
            // signed ad hoc (no identity on this install): iOS would refuse the dlopen from Documents; the build is for Export as its own app (the installer re-signs it)
            ctx.log("stage 5 NOT STARTED: the framework is signed ad hoc for export only (no .p12 here): Export it as its own app, or import your .p12 and sign it again to play inside iBuilder");
            if ctx.boot_enabled && fs::remove_file(ctx.docs.running()).is_ok() {
                ctx.log("the chain finished (marker cleared): the run ends here, with no boot");
            }
        } else if ctx.boot_enabled {
            // Stage 5 in THIS process: the boot is called from here, because iOS reaps a UI-less app the moment the host returns. The guest runs inside this process.
            boot_after_build(ctx, &fw, &flat_p);
        }
    }
    if let Some(e) = &assemble_error {
        // review r4 follow-up: nothing to boot or to stamp; it returned 0 and the app said "Built: ready to play."
        ctx.log(&format!("stage 5 NOT STARTED: [RED] the game could not be assembled or signed (stage 4: {e}): there is no framework to play -- see the lines above, rebuild it, or send the log"));
        if ctx.boot_enabled && fs::remove_file(ctx.docs.running()).is_ok() {
            ctx.log("the chain finished (marker cleared): the run ends here, with no boot");
        }
    }
    mem_line("chain-end", &*ctx.log, &*ctx.mem);
    let rc = if assemble_error.is_some() {
        RC_NO_FRAMEWORK
    } else if no_library.is_some() {
        RC_NO_SHADER_LIBRARY
    } else {
        RC_OK
    };
    let mut rep = report(ctx, rc);
    rep.framework = framework;
    rep.flat = flat;
    rep
}

/// The run ends because a stop was asked of it, at `at`: a pause (`RC_PAUSED`) or a cancel (`RC_CANCELLED`, also when the request was withdrawn meanwhile: the old meaning of the error).
fn stopped(ctx: &mut Ctx, at: &str) -> ChainReport {
    if ctx.stop_kind() == stop::PAUSE {
        paused(ctx, at)
    } else {
        cancelled(ctx, at)
    }
}

fn cancelled(ctx: &mut Ctx, at: &str) -> ChainReport {
    ctx.log("the chain was cancelled by the caller");
    ctx.log(&format!("[stop] the chain was cancelled at {at}: what the stages finished is kept in their caches (the shader cache, the rewrite cache); the caller removes the partial build"));
    ctx.timing.result = "cancelled".into();
    report(ctx, RC_CANCELLED)
}

/// Paused: everything stays where it is (the work folder of stage 3, the caches); `paused:` is how the timings code tells a build that goes on from a finished one (no row, the partial is kept).
fn paused(ctx: &mut Ctx, at: &str) -> ChainReport {
    ctx.log(&format!("[stop] the chain was paused by the user at {at}: nothing is thrown away (the extraction, the rewrite cache, the shaders translated so far); a later run continues from it"));
    ctx.timing.result = format!("paused: by the user at {at}");
    report(ctx, RC_PAUSED)
}

/// A one-line description of the run for logs.
pub fn banner() -> String {
    format!(
        "chain run, pid {}, ibuilder-pipeline {}, platform {}-{} ({})",
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        utc_now_iso()
    )
}
