//! Mods in the chain (`gameapp_pipeline.py`'s `mods_module()`, `apply_mods` and the hooks in `do_rewrite` / `rewrite_is_cached`): the enabled mods' exefs patches are applied to the
//! NSO modules BEFORE stage 2 (`plan`, `apply_module`), the rewrite cache key includes their digest, and RomFS files and cheats are handed to the engine at each boot (`apply_mods`).
//! The mods themselves are `ibuilder-mods` behind the [`crate::adapters::ModsHost`] trait; this file is only what the chain does with them.

use crate::adapters::{ModPlanEntry, ModsHost};
use crate::ctx::Ctx;
use crate::error::{PipelineError, Result};
use crate::util::is_dir;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// `mods.plan(...)` through the host: `[(module, entry)]`; empty without a title id or a host. A failure is what the Python lets through (an uncaught exception in `plan`):
/// stage 2 fails with it.
pub fn mods_plan(
    ctx: &Ctx,
    exe_dir: &Path,
    modules: &[String],
    quiet: bool,
) -> Result<Vec<(String, ModPlanEntry)>> {
    let Some(tid) = ctx.title_id() else {
        return Ok(vec![]);
    };
    let Some(host) = host_or_say(ctx, &tid, quiet) else {
        return Ok(vec![]);
    };
    host.plan(ctx.docs.root(), &tid, exe_dir, modules, &*ctx.log, quiet)
        .map_err(|f| PipelineError::failed(format!("mods: {f}")))
}

/// The host, or `None` after saying (once per call site, unless `quiet`) that this build has none while the game has a mods folder.
fn host_or_say(ctx: &Ctx, tid: &str, quiet: bool) -> Option<Arc<dyn ModsHost>> {
    if let Some(h) = ctx.adapters.mods.clone() {
        return Some(h);
    }
    let store = ctx.docs.game_dir(tid).join("mods");
    if !quiet
        && is_dir(&store)
        && std::fs::read_dir(&store)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
        ctx.log("mods: this build has no mods support: the mods in Documents/games/<id>/mods are not applied");
    }
    None
}

/// `apply_mods`: the enabled mods' RomFS files and cheats, for the engine: `DM_ROMFS_OVERLAY` and `DM_CHEATS`. Read at each boot, so changing them needs no rebuild. A failure here
/// never stops the boot.
pub fn apply_mods(ctx: &Ctx) -> BTreeMap<String, String> {
    let Some(tid) = ctx.title_id() else {
        return BTreeMap::new();
    };
    let Some(host) = host_or_say(ctx, &tid, false) else {
        return BTreeMap::new();
    };
    match host.apply_boot(ctx.docs.root(), &tid, &*ctx.log, &*ctx.env) {
        Ok(set) => set,
        Err(f) => {
            // a mod must not be the reason a game does not start
            ctx.log(&format!(
                "mods: not applied ({}: {}) -- the game starts without them",
                f.exc, f.msg
            ));
            BTreeMap::new()
        }
    }
}

/// What stage 2 does after the last module: the patched copies go, and what this rewrite saw is remembered (the sheet's "Rebuild to apply" compares with it).
pub fn finish_rewrite(ctx: &Ctx) -> Result<()> {
    let Some(tid) = ctx.title_id() else {
        return Ok(());
    };
    let Some(host) = ctx.adapters.mods.clone() else {
        return Ok(());
    };
    host.remove_patched(ctx.docs.root(), &tid);
    host.mark_applied(ctx.docs.root(), &tid, &*ctx.log)
        .map_err(|f| PipelineError::failed(format!("mods: {f}")))
}
