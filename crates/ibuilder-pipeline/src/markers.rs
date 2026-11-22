//! The files the chain writes to talk to the next launch: the framework stamp, the boot-only request, the killed note, the continue request, the load-blocked
//! record (see `state` for the table and the launch-side readers).
//!
//! The boot-only request is consumed when it is HONOURED -- when the boot has come back -- and not when it is read: a request removed before its effect exists
//! is a request that evaporates (docs/DECISIONS.md trap 28; the boot died in its first second, the file was already gone, and the next launch re-ran the whole
//! 2.5-minute chain). Two states, told apart by the first word of the file: ARMED (anything else: nobody has tried it, a launch starts it by itself) and ATTEMPTED
//! (`attempted ...`: a boot began from it and never came back; a launch does NOT start it again -- that would be a retry loop -- but the file stays, so a tap
//! boots once more for the price of a dlopen instead of a rebuild).

use crate::assemble::EXPORT_ONLY_SIGNATURE;
use crate::ctx::Ctx;
use crate::state::BOOT_ONLY_ATTEMPTED;
use crate::timings::read_kv;
use crate::util::{MIB, utc_now_iso};
use std::collections::BTreeMap;
use std::fs;

/// `read_framework_stamp`: the stamp as a map (app_build, game, built, content); empty when there is none.
pub fn read_framework_stamp(ctx: &Ctx) -> crate::config::Config {
    crate::config::Config::read(&ctx.docs.framework_stamp())
}

/// `write_framework_stamp`: record that THIS build of the app built and signed the framework, for which game. `app_build` is the build script's id (handed over in the
/// config): a framework another build made would run an older host and rewrite, which is what an app update must not boot by accident.
pub fn write_framework_stamp(ctx: &Ctx) {
    let build = ctx.cfg.get_or("app_build", "");
    if build.is_empty() {
        ctx.log("framework stamp: no app_build in the config, so the framework is left unstamped (it will read as stale)");
        return;
    }
    // `game` is the dump the extraction ran on; a chain-only run has none, and keeps the one already stamped.
    let game = match ctx.cfg.get_nonempty("game") {
        Some(g) => g.to_string(),
        None => read_framework_stamp(ctx).get_or("game", "").to_string(),
    };
    // content=: the update and DLC this build took (the app's GameAppUi.ContentSignature, handed over in the config): one added later shows as "rebuild to include it"
    let text = framework_stamp_text(
        build,
        &game,
        &utc_now_iso(),
        ctx.cfg.get_or("content", ""),
        ctx.export_only,
    );
    let path = ctx.docs.framework_stamp();
    match crate::util::write_atomic(&path, text.as_bytes()) {
        Ok(()) => ctx.log(&format!(
            "framework stamp: {} (app_build={build}, game={}){}",
            path.display(),
            if game.is_empty() { "-" } else { &game },
            if ctx.export_only {
                format!(", signature={EXPORT_ONLY_SIGNATURE}: signed ad hoc, for export only")
            } else {
                String::new()
            }
        )),
        Err(e) => ctx.log(&format!(
            "framework stamp: could not write {}: {e} -- the framework will read as stale",
            path.display()
        )),
    }
}

/// The stamp's text. `export_only` (stage 4b signed the framework ad hoc: no identity on this install) adds `signature=adhoc-export`; a framework signed with the identity has
/// exactly the four lines it always had.
pub fn framework_stamp_text(
    build: &str,
    game: &str,
    built: &str,
    content: &str,
    export_only: bool,
) -> String {
    let mut text = format!("app_build={build}\ngame={game}\nbuilt={built}\ncontent={content}\n");
    if export_only {
        text.push_str(&format!("signature={EXPORT_ONLY_SIGNATURE}\n"));
    }
    text
}

/// The stamp of the framework on disk says it was signed ad hoc for export only (`signature=adhoc-export`).
pub fn framework_is_export_only(ctx: &Ctx) -> bool {
    read_framework_stamp(ctx).get_or("signature", "") == EXPORT_ONLY_SIGNATURE
}

/// `boot_only_mark_attempted`: the boot is about to start: rewrite the request, in place, as `attempted`. It is not removed.
pub fn boot_only_mark_attempted(ctx: &Ctx) {
    let path = ctx.docs.boot_only();
    let text = format!(
        "{BOOT_ONLY_ATTEMPTED} pid {}: a boot started from this request and has not returned. A tap boots again; a launch does not\n",
        std::process::id()
    );
    if let Err(e) = crate::util::write_atomic(&path, text.as_bytes()) {
        ctx.log(&format!(
            "boot-only: could not mark the request attempted ({}): {e} -- it stays armed",
            path.display()
        ));
    }
}

/// `boot_only_consume`: the request is honoured (or can never be): remove it, and say why.
pub fn boot_only_consume(ctx: &Ctx, why: &str) {
    let path = ctx.docs.boot_only();
    match fs::remove_file(&path) {
        Ok(()) => ctx.log(&format!("Documents/gameapp.boot-only consumed: {why}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => ctx.log(&format!(
            "boot-only: could not remove the request ({}): {e}",
            path.display()
        )),
    }
}

/// `note_boot_result`: after a boot: a dlopen the sandbox refused ("file system sandbox blocked mmap()") means this install may not run code from Documents at all --
/// a distribution certificate (no get-task-allow; FlareStore's, 2026-11-18). That is not a boot to try again: `Documents/gameapp.load-blocked` records it (the app's
/// screen then offers Export as .ipa instead of Play) and the boot request is withdrawn, which ends the "GameApp starts again" loop. A boot that ran removes a marker
/// left by an earlier install. Returns true when the load was refused that way.
pub fn note_boot_result(ctx: &Ctx, rc: i32) -> bool {
    if rc == 0 {
        let _ = fs::remove_file(ctx.docs.load_blocked());
        return false;
    }
    if rc != 3 {
        return false;
    }
    if !last_guest_run_refused(&ctx.docs.guest_log()) {
        return false;
    }
    let text = format!(
        "the dlopen of the built framework in Documents was refused by the sandbox ({}): this install's certificate does not let the app run code from Documents; export the game as an .ipa\n",
        utc_now_iso()
    );
    if let Err(e) = crate::util::write_atomic(&ctx.docs.load_blocked(), text.as_bytes()) {
        ctx.log(&format!("stage 5: could not write {}: {e} -- the app will not know this install cannot run a game from Documents", ctx.docs.load_blocked().display()));
    }
    boot_only_consume(
        ctx,
        "the sandbox refused the dlopen: this certificate cannot run a game from Documents",
    );
    ctx.log("stage 5: [RED] this install cannot run a game from Documents: the sandbox refused the dlopen (\"file system sandbox blocked mmap()\"). A distribution certificate (no get-task-allow) only runs code inside an app: the game is built and signed; use Export as .ipa (game settings) and install it as its own app");
    true
}

/// The line the guest log's every boot starts with (`sys::redirect::redirect_guest_output`).
pub const GUEST_RUN_BANNER: &str = "==== guest output, pid ";
/// What dyld says when the sandbox refuses to map the framework from Documents (a distribution certificate).
pub const SANDBOX_REFUSAL: &str = "sandbox blocked mmap";

/// Whether the LAST boot's section of the guest log (from its last `GUEST_RUN_BANNER` line to the end; the whole file when it has none) says the sandbox refused the dlopen. The log is opened
/// `O_APPEND` and holds every boot of every game: it is read from its end (`util::rfind_line`), line by line, until the banner, so a refusal is seen however far from the end it is (review r5
/// P1: a 16 KiB tail missed one written earlier in the run), and a refusal of an EARLIER boot is not this one's. False when the log cannot be read.
pub fn last_guest_run_refused(guest_log: &std::path::Path) -> bool {
    let found = crate::util::rfind_line(guest_log, |l| {
        if l.contains(SANDBOX_REFUSAL) {
            Some(true)
        } else if l.starts_with(GUEST_RUN_BANNER) {
            Some(false)
        } else {
            None
        }
    });
    matches!(found, Ok(Some((true, _))))
}

/// `note_killed_run`: the app found at launch that iOS ended the last build (`Documents/gameapp.killed`: the game, the last stage and the last memory line the killed
/// run left). Says so in this run's log, with those two lines, and takes the note away: this run is the answer to it. Returns the note.
pub fn note_killed_run(ctx: &Ctx) -> BTreeMap<String, String> {
    let note = read_kv(&ctx.docs.killed());
    if note.is_empty() {
        return note;
    }
    ctx.log(&format!(
        "the previous build{} was ended by iOS (the app was killed: no signal, nothing logged -- a memory kill)",
        note.get("game").filter(|g| !g.is_empty()).map(|g| format!(" of {g}")).unwrap_or_default()
    ));
    ctx.log(&format!(
        "  last stage reached: {}",
        note.get("stage")
            .filter(|s| !s.is_empty())
            .map(String::as_str)
            .unwrap_or("(unknown)")
    ));
    ctx.log(&format!(
        "  last memory line:   {}",
        note.get("mem")
            .filter(|s| !s.is_empty())
            .map(String::as_str)
            .unwrap_or("(none)")
    ));
    let _ = fs::remove_file(ctx.docs.killed());
    note
}

/// `take_continue`: the build this one continues (`Documents/gameapp.continue.run`: the app renamed gameapp.continue, or gameapp.killed's Resume wrote it), taken away so
/// it is used once. Empty for a build that continues nothing.
pub fn take_continue(ctx: &Ctx) -> BTreeMap<String, String> {
    let mut note = BTreeMap::new();
    for path in [ctx.docs.continue_run(), ctx.docs.continue_file()] {
        let got = read_kv(&path);
        if !got.is_empty() && note.is_empty() {
            note = got;
        }
        let _ = fs::remove_file(&path);
    }
    if !note.is_empty() {
        ctx.log(&format!(
            "this build continues one that stopped at stage {} ({}): {}",
            note.get("stage").map(String::as_str).unwrap_or("?"),
            note.get("why").map(String::as_str).unwrap_or("resume"),
            note.get("game").map(String::as_str).unwrap_or("")
        ));
    }
    note
}

/// 2400 MiB: stage 3 wants this much left under the ceiling (1 GiB kept free + 3 translations).
pub const SHADER_RESTART_MIB: i64 = 2400;

/// `restart_for_shaders`: gives the rewrite's memory back before stage 3, when stage 3 would not fit in what is left. An allocator that keeps what the rewrite's threads
/// freed (the CPython runtime did: ~3 GB, trap 71) cannot hand it back inside a process; here the same decision stays, because the rewrite's peak is still the number the
/// ceiling is measured against: only a new process has it back. iOS cannot start the app again by itself, so the chain ends on purpose (`RESTART_RC`) after leaving
/// `Documents/gameapp.continue`; the app notifies "Tap to continue the build", closes, and the next launch runs the same game's chain from stage 3 (the rewrite is in the
/// cache, so it costs seconds). Only when ALL of these hold: the rewrite ran in this process, a device ceiling exists, less than `shader_restart_mib` is left, a new launch
/// would have that much (the launch's available memory), the whole rewrite is cached (no loop: the next run rewrites nothing, so it never asks again) and stage 3 would
/// really translate shaders. Returns true when the chain should end now.
pub fn restart_for_shaders(ctx: &Ctx, rewrote_here: bool, launch_available: u64) -> bool {
    if ctx.cfg.get_or("shader_restart", "1") == "0"
        || !rewrote_here
        || ctx.cfg.get_nonempty("game").is_none()
    {
        return false;
    }
    let left = ctx.mem.mem().available;
    if left == 0 {
        return false; // no ceiling to read (a simulator, a Mac)
    }
    // Give the threads' memory back IN this process first (DECISIONS 82); restart only if that is not enough. (Rust's allocator has no such call: `release_mimalloc` is the documented
    // no-op there, `None`, and this block is skipped.)
    let need = crate::util::mib_bytes(ctx.cfg.int_or("shader_restart_mib", SHADER_RESTART_MIB));
    let mut left = left;
    if left < need
        && crate::memory::release_mimalloc("before-restart-check", &*ctx.log, &*ctx.mem).is_some()
    {
        left = ctx.mem.mem().available;
        ctx.log(&format!(
            "stage 3 shaders: {} MiB left after giving the allocator's memory back ({} needed)",
            left / MIB,
            need / MIB
        ));
    }
    if left >= need {
        ctx.log(&format!(
            "stage 3 shaders: {} MiB left after the rewrite, {} needed: no restart",
            left / MIB,
            need / MIB
        ));
        return false;
    }
    if launch_available < need {
        ctx.log(&format!(
            "stage 3 shaders: {} MiB left after the rewrite ({} needed), but a new launch would not have more ({} MiB at launch): no restart",
            left / MIB,
            need / MIB,
            launch_available / MIB
        ));
        return false;
    }
    if !crate::rewrite::rewrite_is_cached(ctx) || !crate::shaders::shaders_will_build(ctx) {
        return false;
    }
    let text = format!(
        "game={}\nstage=3\nwhy=memory between the rewrite and the shaders\nat={}\nleft_mib={}\n",
        ctx.cfg.get_or("game", ""),
        utc_now_iso(),
        left / MIB
    );
    if let Err(e) = crate::util::write_atomic(&ctx.docs.continue_file(), text.as_bytes()) {
        ctx.log(&format!("stage 3 shaders: the restart request could not be written ({e}): stage 3 runs in this process"));
        return false;
    }
    ctx.log(&format!(
        "stage 3 shaders: only {} MiB left after the rewrite ({} needed; a new launch has {}): the rewrite's memory cannot be handed back inside this process. The app closes and the next launch continues from stage 3 (Documents/gameapp.continue)",
        left / MIB,
        need / MIB,
        launch_available / MIB
    ));
    true
}

/// `Documents/pipeline.boot`: what stage 5 needs (the signed framework, the patched dir, the modules, the profile, the RomFS), one per line.
pub fn write_pipeline_boot(
    ctx: &Ctx,
    framework: &str,
    flat: &str,
    modules: &str,
    profile: &str,
    romfs: &str,
) {
    let text = [framework, flat, modules, profile, romfs].join("\n") + "\n";
    if let Err(e) = crate::util::write_atomic(&ctx.docs.pipeline_boot(), text.as_bytes()) {
        ctx.log(&format!(
            "stage 5: could not write {}: {e}",
            ctx.docs.pipeline_boot().display()
        ));
    }
}
