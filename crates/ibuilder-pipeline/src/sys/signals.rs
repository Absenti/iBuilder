//! A signal-driven exit leaves a cause in the log. iOS reclaims a process in ways that all look like "the app closed by itself". A handler for the fatal signals the host does NOT own (it installs
//! its own SIGTERM handler for the graceful shutdown, docs/PIPELINE-IOS.md 6.4) writes one final line through `write(2)`, which is async-signal-safe, and then re-raises so the system still sees
//! the crash as a crash. So the next "it closed by itself" arrives with a signal name instead of a guess; `state::classify_previous_run` reads this line (`received SIG`).

use std::sync::atomic::{AtomicI32, Ordering};

static LOG_FD: AtomicI32 = AtomicI32::new(-1);

const SIGS: [libc::c_int; 6] = [
    libc::SIGABRT,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGILL,
    libc::SIGSEGV,
    libc::SIGTRAP,
];

pub fn signal_name(sig: libc::c_int) -> &'static str {
    match sig {
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGILL => "SIGILL",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGTRAP => "SIGTRAP",
        _ => "SIG?",
    }
}

extern "C" fn fatal_signal(sig: libc::c_int) {
    const PRE: &[u8] = b"\n[gameapp] received ";
    const MID: &[u8] =
        b": the process is terminating here (this line is the cause the log carries)\n";
    let fd = LOG_FD.load(Ordering::Relaxed);
    let name = signal_name(sig).as_bytes();
    // SAFETY: only async-signal-safe calls (write, signal, raise) on static buffers; the fd is the log's, kept open for the process's life.
    unsafe {
        if fd >= 0 {
            let _ = libc::write(fd, PRE.as_ptr() as *const libc::c_void, PRE.len());
            let _ = libc::write(fd, name.as_ptr() as *const libc::c_void, name.len());
            let _ = libc::write(fd, MID.as_ptr() as *const libc::c_void, MID.len());
        }
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// Install the handler. `log_fd` is the chain log's descriptor; the handler keeps its OWN duplicate for the life of the process (the shim wrote through the caller's stream, which it closed at
/// the end of a run: a late signal would then have written into whatever reused that descriptor).
pub fn install_signal_logger(log_fd: i32) {
    // SAFETY: dup of a descriptor the caller owns; the duplicate is ours, the previous one (if any) is closed once.
    let own = unsafe { libc::dup(log_fd) };
    let old = LOG_FD.swap(own, Ordering::Relaxed);
    if old >= 0 {
        // SAFETY: the previous duplicate this function made.
        unsafe { libc::close(old) };
    }
    for s in SIGS {
        // SAFETY: registers an `extern "C" fn(c_int)` as the handler.
        unsafe {
            libc::signal(
                s,
                fatal_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(signal_name(libc::SIGSEGV), "SIGSEGV");
        assert_eq!(signal_name(libc::SIGABRT), "SIGABRT");
        assert_eq!(signal_name(libc::SIGUSR1), "SIG?");
    }
}
