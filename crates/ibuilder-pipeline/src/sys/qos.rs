//! Quality of service of the BUILD's worker threads.
//!
//! The chain runs in the app's process: the stages' rayon pools and scoped threads compete with the UI thread for the same cores. On Apple targets a build worker therefore
//! lowers its own QoS to `QOS_CLASS_UTILITY` at thread start, so the scheduler keeps the main thread's priority. This changes timing only, never a result.
//!
//! Only build workers call [`lower_worker_qos`], and only on their own thread (`pthread_set_qos_class_self_np`): the thread that calls `ibuilder_run_chain` and every thread
//! of the boot path (stage 5, the game's own threads) are never touched, so the game does not inherit a lowered class. `DM_BUILD_QOS=off` in the process environment turns it off.

use std::sync::OnceLock;

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

/// Set the calling thread's QoS class to utility (Apple targets, unless switched off); a no-op elsewhere. Call it as the first thing a build worker thread does.
pub fn lower_worker_qos() {
    #[cfg(target_vendor = "apple")]
    if enabled() {
        // `qos_class_t` is an unsigned int; QOS_CLASS_UTILITY = 0x11 (<sys/qos.h>). The call affects only the calling thread; a failure leaves the class as it was.
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        // SAFETY: plain libSystem call with two integers, no pointers.
        unsafe {
            pthread_set_qos_class_self_np(0x11, 0);
        }
    }
}

/// The one log line per stage (`None` when off or not an Apple target), for the stage that starts the worker threads.
pub fn log_line(stage: u32) -> Option<String> {
    (cfg!(target_vendor = "apple") && enabled()).then(|| {
        format!("stage {stage}: worker threads at utility QoS (the UI keeps its priority)")
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
}
