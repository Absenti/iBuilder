//! Stage 5: dlopen the signed framework in THIS process and call the host's `dm_host_probe_boot` (`boot_framework`, `term_after`, `py_boot`, `gameapp_quit_game`, the host-side
//! half of the live shaders and the touch pad lookups of `gameapp_shim.c`; `oneproc/chain_shim.c`'s shape). The guest runs inside this process: iOS reaps a UI-less app the moment the
//! host returns, and the guest's own SIGTERM ends the process, so `boot` may never come back -- which is fine: the evidence is already in the logs on disk.
//!
//! The host's C entry points this file uses (all looked up with `dlsym`, so an older host without one still boots, with the feature missing and a log line saying so):
//! `dm_host_probe_boot(patched, modules, profile) -> int`, `dm_front_set_layer(layer)` (the game view's CAMetalLayer: without it the engine reads "no window" and draws nothing),
//! `nvn_live_set(handler)` / `nvn_live_ready()` (live shaders, paches/0008), `dm_front_pad_create` / `dm_front_pad_layout` (the touch gamepad, looked up by the app's view).

use crate::adapters::{BootHost, BootRequest, BootResult};
use crate::ctx::View;
use crate::env::{Env, RealEnv};
use crate::live::LiveQueue;
use crate::log::Log;
use crate::sys::{self, redirect, vm};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

/// The game view's CAMetalLayer, from the managed side: handed to the host right after the framework is dlopen'd. The caller retains it (this crate has no CoreFoundation).
static GAME_LAYER: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// The dlopen handle of the host, once the framework has landed (the managed side may then ask it for the touch gamepad).
static HOST: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
/// A boot's number: its time limit is armed for it alone (a timer whose boot is no longer the current one stands down).
static BOOT_GEN: AtomicI32 = AtomicI32::new(0);
/// The low-ceiling case: the guest block is taken right after the dlopen, not at launch.
static RESERVE_LATER: AtomicBool = AtomicBool::new(false);
/// The queue the host's `live_handler` callback talks to.
static LIVE: Mutex<Option<Arc<LiveQueue>>> = Mutex::new(None);

pub fn set_game_layer(layer: *mut c_void) {
    GAME_LAYER.store(layer, Ordering::SeqCst);
}

pub fn game_layer() -> *mut c_void {
    GAME_LAYER.load(Ordering::SeqCst)
}

/// The host's dlopen handle (null before the framework is loaded).
pub fn host_handle() -> *mut c_void {
    HOST.load(Ordering::Acquire)
}

/// `dlsym` on the loaded host; null when the host is not loaded or lacks the symbol.
pub fn host_symbol(name: &str) -> *mut c_void {
    let h = host_handle();
    let Ok(c) = CString::new(name) else {
        return std::ptr::null_mut();
    };
    if h.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `h` is a live handle from dlopen (never closed); `c` is NUL-terminated.
    unsafe { libc::dlsym(h, c.as_ptr()) }
}

/// The host's callback for a shader its library lacks (`nvn_live_set(handler)`): `int handler(key, ctl, nctl, code, ncode, stage, cap)`. Returns -1 refused, 0 not yet (ask again), 1 translated by
/// this ask, 2 by an earlier one; `stage` receives the stage name for 1 and 2.
///
/// # Safety
/// The host's contract: `key` is NULL or NUL-terminated, `ctl` / `code` are NULL or valid for `nctl` / `ncode` bytes, `stage` is NULL or writable for `cap` bytes, all for the duration of the call.
/// (This function is `unsafe` because it dereferences those pointers; it is the one the host is handed, so it never unwinds into the C caller: a panic is caught and answered -1.)
pub unsafe extern "C" fn live_handler(
    key: *const c_char,
    ctl: *const c_void,
    nctl: u32,
    code: *const c_void,
    ncode: u32,
    stage: *mut c_char,
    cap: c_int,
) -> c_int {
    // SAFETY: the contract above is this function's.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        live_handler_unguarded(key, ctl, nctl, code, ncode, stage, cap)
    }));
    r.unwrap_or(-1)
}

/// `live_handler` without the `catch_unwind`: for a caller that has its own guard AND wants the panic's message (the C ABI's `ibuilder_live_handler` runs it under `guard`, which records the
/// message for `ibuilder_last_error()`; the guarded form above swallows the panic and leaves no trace).
///
/// # Safety
/// As [`live_handler`].
pub unsafe fn live_handler_unguarded(
    key: *const c_char,
    ctl: *const c_void,
    nctl: u32,
    code: *const c_void,
    ncode: u32,
    stage: *mut c_char,
    cap: c_int,
) -> c_int {
    if key.is_null() {
        return -1;
    }
    let Some(q) = LIVE.lock().ok().and_then(|g| g.clone()) else {
        return -1;
    };
    // SAFETY: the host passes a NUL-terminated key and buffers of the stated lengths that stay valid for the call.
    let (key, ctl, code) = unsafe {
        (
            CStr::from_ptr(key).to_string_lossy().into_owned(),
            if ctl.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(ctl as *const u8, nctl as usize)
            },
            if code.is_null() {
                &[][..]
            } else {
                std::slice::from_raw_parts(code as *const u8, ncode as usize)
            },
        )
    };
    let (r, st) = q.handler(&key, ctl, code);
    if r > 0
        && !stage.is_null()
        && cap > 0
        && let Some(st) = st
    {
        let b = st.as_bytes();
        let n = b.len().min(cap as usize - 1);
        // SAFETY: `stage` has room for `cap` bytes; n + the terminator fit.
        unsafe {
            std::ptr::copy_nonoverlapping(b.as_ptr(), stage as *mut u8, n);
            *stage.add(n) = 0;
        }
    }
    r
}

/// Install (or clear) the queue the host's `live_handler` talks to: the boot does it, and a test of the C entry point does.
pub fn set_live(q: Option<Arc<LiveQueue>>) {
    *LIVE.lock().unwrap() = q;
}

/// `consume_boot_request`: reaching the time limit IS the designed end of a boot, and the player's Quit is another: the request is honoured, so it goes.
fn consume_boot_request(docs: &Path, chain_log: &dyn Log, why: &str) {
    if std::fs::remove_file(docs.join(crate::layout::BOOT_ONLY)).is_ok() {
        chain_log.line(&format!(
            "[gameapp] Documents/gameapp.boot-only consumed: {why}, the designed end"
        ));
    }
}

/// `gameapp_quit_game`: the player's own way out of a game that has no time limit: SIGTERM to self, which the host answers with its graceful exit -- the same end as the time limit's, and the same
/// honoured boot, but its own line, so the next launch can tell a player who left from a process that died (`classify_previous_run` reads "the player ended the game").
pub fn quit_game(docs: &Path, chain_log: &dyn Log) {
    chain_log.line("[gameapp] the player ended the game: sending SIGTERM to self (the CAUSE of this exit, not a crash)");
    consume_boot_request(docs, chain_log, "the player ended the game");
    // SAFETY: signalling this process.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGTERM);
    }
}

/// The boot's time limit. It belongs to ONE boot (`BOOT_GEN`): a boot that returned first is not followed, `seconds` later, by a SIGTERM into an app that has nothing left to end. It wakes every
/// second to look instead of sleeping the limit in one call.
fn arm_time_limit(
    seconds: i64,
    generation: i32,
    docs: std::path::PathBuf,
    chain_log: Arc<dyn Log>,
) {
    arm_time_limit_with(seconds, generation, docs, chain_log, |body| {
        std::thread::Builder::new()
            .name("boot-time-limit".into())
            .spawn(body)
            .map(|_| ())
    });
}

/// [`arm_time_limit`] with the thread start as a parameter (a test cannot make a real spawn fail). When the timer thread cannot be started the guest runs with NO time limit and the log says so
/// (it used to be a comment: "the caller's log is not reachable here"; the same family as the iOSBuilder review's unchecked `pthread_create`, review r2 N6).
fn arm_time_limit_with(
    seconds: i64,
    generation: i32,
    docs: std::path::PathBuf,
    chain_log: Arc<dyn Log>,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send + 'static>) -> std::io::Result<()>,
) {
    if seconds <= 0 {
        return;
    }
    let log = chain_log.clone(); // the thread owns `chain_log`; a failed start still has to be able to say so
    let body: Box<dyn FnOnce() + Send + 'static> = Box::new(move || {
        for s in 1..=seconds {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if BOOT_GEN.load(Ordering::SeqCst) != generation {
                chain_log.line(&format!("[gameapp] boot time limit of {seconds} s disarmed at {s} s: the boot returned first, no SIGTERM is sent"));
                return;
            }
        }
        chain_log.line(&format!("[gameapp] boot time limit {seconds} s reached: sending SIGTERM to self (the CAUSE of this exit, not a crash)"));
        consume_boot_request(&docs, &*chain_log, "the boot ran to its time limit");
        // SAFETY: signalling this process.
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }
    });
    if let Err(e) = spawn(body) {
        log.line(&format!("[gameapp] boot time limit of {seconds} s NOT ARMED: the timer thread could not be started ({e}); the guest runs with no time limit"));
    }
}

/// `gameapp_as_reserve`: at the earliest point the app controls (a Mach-O initializer, before the managed runtime), decide whether this launch will boot and take the guest block now, later or
/// never. `docs` is `Documents`. Only on iOS unless `force`. Lines go to `Documents/gameapp.chain.log`, which the chain appends to later, so one file holds the whole launch.
pub fn early_init(docs: &Path, force: bool) {
    if !(force || cfg!(target_os = "ios")) {
        return;
    }
    let may_boot = docs.join(crate::layout::BOOT_ONLY).exists()
        || docs.join(crate::layout::FRAMEWORK_STAMP).exists();
    if !may_boot {
        return; // nothing to boot: leave the address space alone
    }
    let say = |s: &str| append_chain_log(docs, s);
    if let Some(c) = vm::census("at launch, before the .NET runtime") {
        say(&c);
    }
    let ceiling = sys::mem::available_bytes();
    match vm::reserve_decision(true, ceiling) {
        vm::ReserveWhen::SkippedHighCeiling => {
            say(&format!(
                "[gameapp] as-reserve: skipped -- this process may use {:.2} GB, so the engine keeps the guest pool in sparse files (measured at a 6132 MiB ceiling: nnMain) and a 5 GB block would only take the room the framework needs to dlopen",
                ceiling as f64 / 1_073_741_824.0
            ));
            if let Some(c) = vm::census("at launch, nothing reserved") {
                say(&c);
            }
        }
        vm::ReserveWhen::AfterDlopen => {
            RESERVE_LATER.store(true, Ordering::SeqCst);
            say(&format!(
                "[gameapp] as-reserve: deferred -- this process may use only {:.2} GB, so the 5 GB block is taken right AFTER the framework is dlopen'd: at launch first it left the 137 MB framework nowhere to land (vm_allocate(size=0x827CA000) failed with result=3, 1.50/1.00 GB holes)",
                ceiling as f64 / 1_073_741_824.0
            ));
        }
        vm::ReserveWhen::Never => {}
    }
}

/// `as_say`: a line straight into `Documents/gameapp.chain.log` (append, one `open` per line: usable before anything is initialised).
pub fn append_chain_log(docs: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(docs.join(crate::layout::CHAIN_LOG))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// stage 5 by `dlopen`.
pub struct DlopenBoot {
    live: Arc<LiveQueue>,
}

impl DlopenBoot {
    pub fn new(live: Arc<LiveQueue>) -> DlopenBoot {
        DlopenBoot { live }
    }
}

type ProbeBoot = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> c_int;
type SetLayer = unsafe extern "C" fn(*mut c_void);
type LiveHandler = unsafe extern "C" fn(
    *const c_char,
    *const c_void,
    u32,
    *const c_void,
    u32,
    *mut c_char,
    c_int,
) -> c_int;
type LiveSet = unsafe extern "C" fn(LiveHandler);
type LiveReady = unsafe extern "C" fn();

impl BootHost for DlopenBoot {
    fn boot(&self, req: &BootRequest<'_>, v: &View<'_>) -> BootResult {
        let generation = BOOT_GEN.fetch_add(1, Ordering::SeqCst) + 1; // this boot's number: its timer is armed for it alone
        arm_time_limit(
            req.seconds,
            generation,
            v.docs.root().to_path_buf(),
            v.chain_log.clone(),
        );
        let (stage, guest) = self.boot_framework(req, v);
        BOOT_GEN.fetch_add(1, Ordering::SeqCst); // the boot is over: its timer stands down
        BootResult { stage, guest }
    }
}

impl DlopenBoot {
    fn boot_framework(&self, req: &BootRequest<'_>, v: &View<'_>) -> (i32, i32) {
        let log: &dyn Log = &**v.chain_log;
        if redirect::redirect_guest_output(&v.docs.guest_log()) {
            log.line("[gameapp] the guest's fd 1/2 now go to Documents/gameapp.guest.log (not the console's)");
        } else {
            log.line("[gameapp] could not open Documents/gameapp.guest.log: the guest's output has nowhere to go");
        }
        // The host reads its runtime profile lazily, at its first `dm_profile()` call, from DM_PROFILE; a call that comes before the variable is set loads the compiled generic profile for good
        // (reserve 0x100000000, alias 0, stack 0x1000000: the guest then died right after nnosInitialize). Anything between the dlopen and the boot can make that call (`dm_front_set_layer`, the
        // touch pad's lookups of the host), so the host's variables are set BEFORE the framework is loaded, not after, as the Python shim did.
        sys::setenv("PROBE_MODE", "boot");
        sys::setenv("DM_PROFILE", &req.profile.to_string_lossy());
        if !req.romfs.is_empty() {
            sys::setenv("DM_ROMFS", req.romfs);
        }
        log.line(&format!(
            "[gameapp] stage 5 boot: dlopen {}",
            req.framework.display()
        ));
        if cfg!(target_os = "ios") {
            // the framework is one 137 MB image with vmaddr 0: if it does not fit, the error is `vm_allocate(size=0x827CA000) failed with result=3` and nothing says how much room was left
            if let Some(c) = vm::census("before the dlopen") {
                log.line(&c);
            }
        }
        let Ok(path) = CString::new(req.framework.as_os_str().as_encoded_bytes()) else {
            log.line("[gameapp] dlopen FAILED: the framework path has a NUL");
            redirect::restore_output();
            return (3, -1);
        };
        // SAFETY: dlopen of a NUL-terminated path.
        let h = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if h.is_null() {
            let line = format!("[gameapp] dlopen FAILED: {}", dlerror_text());
            log.line(&line);
            // The same line on fd 2, which is the guest log until `restore_output`: `markers::note_boot_result` reads the refusal ("file system sandbox blocked mmap()") from THERE, as the
            // C shim's `logln` (chain log + stderr) put it there. The chain log alone never reached it (review r5 P1 follow-up: in the Rust build the load-blocked record was never written).
            redirect::write_fd2(&line);
            redirect::restore_output();
            return (3, -1);
        }
        log.line("[gameapp] dlopen OK");
        HOST.store(h, Ordering::Release); // the managed side may now ask it for the touch gamepad
        if RESERVE_LATER.swap(false, Ordering::SeqCst) {
            // the low-ceiling case: the block goes in now that the framework has landed
            let env = RealEnv;
            let docs = v.docs.root().to_path_buf();
            vm::reserve_block(&v.docs.join("profile.tsv"), &env, &|s| {
                append_chain_log(&docs, s)
            });
        }
        // The window. The engine presents into front_metal_layer(), which the host answers from what dm_front_set_layer was given; with none, the engine reads "no window" and draws nothing, which is
        // how a game that ran and played its audio was never seen (device, 2026-11-17).
        let set_layer = host_symbol("dm_front_set_layer");
        let layer = game_layer();
        if !set_layer.is_null() && !layer.is_null() {
            // SAFETY: the host's `void dm_front_set_layer(void *layer)`.
            unsafe { std::mem::transmute::<*mut c_void, SetLayer>(set_layer)(layer) };
            log.line(&format!("[gameapp] the game layer {layer:p} was handed to the host: the engine has a window and presents into it"));
        } else {
            log.line(&format!(
                "[gameapp] no game layer handed to the host ({}): the guest runs with no window and nothing is drawn",
                if set_layer.is_null() { "this host has no dm_front_set_layer: an older host-prebuilt.dylib" } else { "the app created none" }
            ));
        }
        // Shaders the game's library lacks, translated while it runs (paches/0008). A host without it draws without them, as before.
        let live_set = host_symbol("nvn_live_set");
        let live_ready = host_symbol("nvn_live_ready");
        set_live(Some(self.live.clone()));
        if !live_ready.is_null() {
            // SAFETY: the host's `void nvn_live_ready(void)`.
            let ready: LiveReady =
                unsafe { std::mem::transmute::<*mut c_void, LiveReady>(live_ready) };
            self.live.set_ready(Arc::new(move || unsafe { ready() }));
        }
        if !live_set.is_null()
            && !live_ready.is_null()
            && std::env::var_os("GAMEAPP_LIVE_THREAD").is_some()
        {
            // SAFETY: the host's `void nvn_live_set(handler)`; `live_handler` has the C signature it expects and lives for the whole process.
            unsafe { std::mem::transmute::<*mut c_void, LiveSet>(live_set)(live_handler) };
            log.line(&format!("[gameapp] shaders missing from the library: translated while the game runs (DM_LIVE_SHADERS={})", std::env::var("DM_LIVE_SHADERS").unwrap_or_else(|_| "pause".into())));
        } else {
            log.line(&format!(
                "[gameapp] shaders missing from the library are drawn without ({})",
                if live_set.is_null() {
                    "this host has no nvn_live_set: built without paches/0008"
                } else {
                    "the chain started no live translation thread"
                }
            ));
        }
        let boot = host_symbol("dm_host_probe_boot");
        if boot.is_null() {
            log.line(&format!(
                "[gameapp] dlsym dm_host_probe_boot FAILED: {}",
                dlerror_text()
            ));
            redirect::restore_output();
            return (4, -1);
        }
        log.line(&format!(
            "[gameapp] dm_host_probe_boot(patched=\"{}\", modules=\"{}\", profile=\"{}\")",
            req.patched.display(),
            req.modules,
            req.profile.display()
        ));
        let (Ok(p), Ok(m), Ok(pr)) = (
            CString::new(req.patched.as_os_str().as_encoded_bytes()),
            CString::new(req.modules),
            CString::new(req.profile.as_os_str().as_encoded_bytes()),
        ) else {
            log.line("[gameapp] dm_host_probe_boot: an argument has a NUL");
            redirect::restore_output();
            return (4, -1);
        };
        // SAFETY: the host's `int dm_host_probe_boot(const char *patched, const char *modules, const char *profile)`, called with NUL-terminated strings that outlive the call. The guest runs in this thread.
        let r = unsafe {
            std::mem::transmute::<*mut c_void, ProbeBoot>(boot)(p.as_ptr(), m.as_ptr(), pr.as_ptr())
        };
        log.line(&format!("[gameapp] dm_host_probe_boot returned {r}"));
        redirect::restore_output();
        (0, r)
    }
}

fn dlerror_text() -> String {
    // SAFETY: dlerror returns NULL or a NUL-terminated string valid until the next dl* call on this thread.
    unsafe {
        let e = libc::dlerror();
        if e.is_null() {
            "(no message)".to_string()
        } else {
            CStr::from_ptr(e).to_string_lossy().into_owned()
        }
    }
}

#[allow(dead_code)]
fn _assert_env_is_object_safe(_: &dyn Env) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::MemLog;

    #[test]
    fn a_time_limit_whose_thread_cannot_start_says_so() {
        let log = MemLog::new();
        arm_time_limit_with(45, 7, std::env::temp_dir(), log.clone(), |_body| {
            Err(std::io::Error::other("no threads left"))
        });
        let text = log.text();
        assert!(
            text.contains("boot time limit of 45 s NOT ARMED")
                && text.contains("no threads left")
                && text.contains("no time limit"),
            "{text}"
        );
    }

    #[test]
    fn a_time_limit_that_starts_says_nothing_and_none_is_armed_for_zero_seconds() {
        let log = MemLog::new();
        let started = std::cell::Cell::new(false);
        arm_time_limit_with(0, 1, std::env::temp_dir(), log.clone(), |_body| {
            started.set(true);
            Ok(())
        });
        assert!(
            !started.get() && log.lines().is_empty(),
            "no limit asked for: no thread"
        );
        arm_time_limit_with(30, 1, std::env::temp_dir(), log.clone(), |_body| {
            started.set(true);
            Ok(()) // the body is dropped, not run: nothing is signalled
        });
        assert!(started.get() && log.lines().is_empty(), "{:?}", log.lines());
    }
}
