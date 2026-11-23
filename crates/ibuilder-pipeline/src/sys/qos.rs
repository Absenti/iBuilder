//! Quality of service of the BUILD's worker threads.
//!
//! The chain runs in the app's process: the stages' rayon pools and scoped threads compete with the UI thread for the same cores. On Apple targets a build worker therefore
//! lowers its own QoS to `QOS_CLASS_UTILITY` at thread start, so the scheduler keeps the main thread's priority. This changes timing only, never a result.
//!
//! Only build workers call [`lower_worker_qos`], and only on their own thread (`pthread_set_qos_class_self_np`): the thread that calls `ibuilder_run_chain` and every thread
//! of the boot path (stage 5, the game's own threads) are never touched, so the game does not inherit a lowered class. `DM_BUILD_QOS=off` in the process environment turns it off.
//!
//! **Build mode** (the build screen's "Mode": Cool / Fast; `fast_build` in the app settings, default off = Cool). Fast leaves the workers at the class of the thread that runs the chain, as
//! before this module existed: the build runs on the performance cores again, faster and hotter, and the UI may lag. The FFI sets the mode from the setting at each run
//! ([`set_fast_build`], [`record_caller_class`]) and again when the player changes it during a build. A worker that started under one mode moves to the other at its next [`follow`]: the app's
//! translator and SPIRV-Cross callbacks call it before every call, so the shader translation (most of a build) follows within a shader; the other stages' workers keep the mode they
//! started with until the next build. [`follow`] never touches a thread that did not start as a build worker (the boot path, the game's threads, a live translation).

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const QOS_CLASS_UTILITY: u32 = 0x11;
const QOS_CLASS_DEFAULT: u32 = 0x15;

static FAST_BUILD: AtomicBool = AtomicBool::new(false);
/// the class a Fast worker gets when it moves from Cool (the class of the thread that runs the chain, at least `QOS_CLASS_DEFAULT`)
static FAST_CLASS: AtomicU32 = AtomicU32::new(QOS_CLASS_DEFAULT);

thread_local! {
    /// 0: not a build worker (never touched by `follow`), 1: a worker at utility (Cool), 2: a worker at the fast class
    static APPLIED: Cell<u8> = const { Cell::new(0) };
}

/// The build mode for the workers: Fast (`true`) or Cool. Process-wide, from any thread; running workers follow it at their next [`follow`].
pub fn set_fast_build(on: bool) {
    FAST_BUILD.store(on, Ordering::Relaxed);
}

/// `true` while the build mode is Fast.
pub fn fast_build() -> bool {
    FAST_BUILD.load(Ordering::Relaxed)
}

static XLATE_OPTIMIZED: AtomicBool = AtomicBool::new(false);

/// Speed-up B9, kept here with the other per-run build switches (this file is outside the framework id): the app's "Optimized translator (experimental)" setting (`xlate_optimized`,
/// default off), set by the FFI at every run. `ibuilder_shaderbuild::jobs::XlateOpts::default()` reads it: the translator then runs without its DebugMode. Unlike the build mode it
/// changes outputs (it is in every shader's digest).
pub fn set_xlate_optimized(on: bool) {
    XLATE_OPTIMIZED.store(on, Ordering::Relaxed);
}

/// `true` while the optimized translator setting is on.
pub fn xlate_optimized() -> bool {
    XLATE_OPTIMIZED.load(Ordering::Relaxed)
}

/// Remember the calling thread's class (the thread that runs the chain) as the one a worker gets when it moves to Fast; a class below `QOS_CLASS_DEFAULT` (utility, background, none) counts
/// as `QOS_CLASS_DEFAULT`.
pub fn record_caller_class() {
    #[cfg(target_vendor = "apple")]
    {
        unsafe extern "C" {
            fn qos_class_self() -> u32;
        }
        // SAFETY: plain libSystem call, no arguments.
        let c = unsafe { qos_class_self() };
        FAST_CLASS.store(c.max(QOS_CLASS_DEFAULT), Ordering::Relaxed);
    }
}

#[cfg(target_vendor = "apple")]
fn set_self_class(class: u32) {
    // `qos_class_t` is an unsigned int (<sys/qos.h>). The call affects only the calling thread; a failure leaves the class as it was.
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }
    // SAFETY: plain libSystem call with two integers, no pointers.
    unsafe {
        pthread_set_qos_class_self_np(class, 0);
    }
}

/// A build worker moves to the mode in effect if it changed since it last looked: called before each translator and SPIRV-Cross call (the app's callbacks, through the FFI). Cheap: one atomic
/// load and a thread-local compare when nothing changed. A thread that is not a build worker is left alone.
pub fn follow() {
    if !enabled() {
        return;
    }
    let want = if fast_build() { 2 } else { 1 };
    APPLIED.with(|a| {
        let have = a.get();
        if have == 0 || have == want {
            return;
        }
        #[cfg(target_vendor = "apple")]
        set_self_class(if want == 2 {
            FAST_CLASS.load(Ordering::Relaxed)
        } else {
            QOS_CLASS_UTILITY
        });
        a.set(want);
    });
}

/// `false` when `DM_BUILD_QOS` is `off` / `0` / `false` / `no`; read once.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        !matches!(
            std::env::var("DM_BUILD_QOS")
                .map(|v| v.trim().to_ascii_lowercase())
                .as_deref(),
            Ok("off" | "0" | "false" | "no")
        )
    })
}

/// A build worker's first call (on its own thread): Cool lowers the class to utility, Fast leaves the class it inherited (that of the thread that runs the chain). Apple targets only, unless
/// switched off (`DM_BUILD_QOS=off`); a no-op elsewhere. The thread is a build worker from then on: it follows the mode ([`follow`]).
pub fn lower_worker_qos() {
    if !enabled() {
        return;
    }
    let fast = fast_build();
    #[cfg(target_vendor = "apple")]
    if !fast {
        set_self_class(QOS_CLASS_UTILITY);
    }
    APPLIED.with(|a| a.set(if fast { 2 } else { 1 }));
}

/// The one log line per stage (`None` when off or not an Apple target), for the stage that starts the worker threads.
pub fn log_line(stage: u32) -> Option<String> {
    (cfg!(target_vendor = "apple") && enabled()).then(|| {
        if fast_build() {
            format!("stage {stage}: worker threads at the caller's QoS (build mode Fast: faster and hotter, the UI may lag)")
        } else {
            format!("stage {stage}: worker threads at utility QoS (build mode Cool: the UI keeps its priority)")
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn lowering_the_class_never_panics_and_is_repeatable() {
        super::lower_worker_qos();
        super::lower_worker_qos();
        assert_eq!(
            super::log_line(3).is_some(),
            cfg!(target_vendor = "apple") && super::enabled()
        );
    }

    #[test]
    fn a_worker_follows_the_mode_and_other_threads_are_left_alone() {
        use super::*;
        // a thread that never started as a build worker: follow() leaves it (the boot path, a live translation)
        std::thread::spawn(|| {
            set_fast_build(true);
            follow();
            assert_eq!(APPLIED.with(|a| a.get()), 0);
        })
        .join()
        .unwrap();
        record_caller_class();
        assert!(FAST_CLASS.load(Ordering::Relaxed) >= QOS_CLASS_DEFAULT);
        std::thread::spawn(|| {
            set_fast_build(false);
            lower_worker_qos();
            let on = enabled();
            assert_eq!(APPLIED.with(|a| a.get()), if on { 1 } else { 0 });
            set_fast_build(true);
            follow();
            assert_eq!(APPLIED.with(|a| a.get()), if on { 2 } else { 0 });
            if let Some(l) = log_line(3) {
                assert!(l.contains("Fast"), "{l}");
            }
            set_fast_build(false);
            follow();
            assert_eq!(APPLIED.with(|a| a.get()), if on { 1 } else { 0 });
            if let Some(l) = log_line(3) {
                assert!(l.contains("Cool"), "{l}");
            }
        })
        .join()
        .unwrap();
    }
}
