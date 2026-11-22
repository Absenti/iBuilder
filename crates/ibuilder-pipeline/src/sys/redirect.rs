//! The guest's own output: nowhere to look for it otherwise (DECISIONS trap 20). The guest (the host dylib, then the game's `nnMain`) writes to fd 1 and 2. In the app those are the
//! managed runtime's console, not a file, so a boot that died left no line saying how far it got. `dup2` the guest's stdout and stderr onto `Documents/gameapp.guest.log` for the duration of
//! the boot, so the next "it quit" arrives with the guest's own last words (the host's banner, `>>> nnMain`, an abort message). The originals are restored if the boot returns.

use std::ffi::CString;
use std::path::Path;
use std::sync::Mutex;

static SAVED: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// Redirect fd 1 and 2 onto `path` (append, with a banner). False when the file cannot be opened.
pub fn redirect_guest_output(path: &Path) -> bool {
    let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // SAFETY: plain POSIX calls on descriptors this function opens or duplicates itself; the banner is written with `write` from a live buffer.
    unsafe {
        let fd = libc::open(
            c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
            0o644 as libc::c_uint,
        );
        if fd < 0 {
            return false;
        }
        let banner = format!(
            "\n{}{} (what the host and the game wrote to fd 1/2) ====\n",
            crate::markers::GUEST_RUN_BANNER,
            std::process::id()
        );
        let _ = libc::write(fd, banner.as_ptr() as *const libc::c_void, banner.len());
        let out = libc::dup(1);
        let err = libc::dup(2);
        libc::dup2(fd, 1);
        libc::dup2(fd, 2);
        libc::close(fd);
        *SAVED.lock().unwrap() = Some((out, err));
    }
    true
}

/// One line (and its newline) on fd 2, in one `write(2)`: while the guest's output is redirected, that is `Documents/gameapp.guest.log`.
pub fn write_fd2(line: &str) {
    let mut b = Vec::with_capacity(line.len() + 1);
    b.extend_from_slice(line.as_bytes());
    b.push(b'\n');
    // SAFETY: `write` from a live buffer of `b.len()` bytes to a descriptor this process owns (or a closed one: the call then fails, which is ignored).
    unsafe {
        let _ = libc::write(2, b.as_ptr() as *const libc::c_void, b.len());
    }
}

/// Put fd 1 and 2 back.
pub fn restore_output() {
    if let Some((out, err)) = SAVED.lock().unwrap().take() {
        // SAFETY: `out`/`err` are the duplicates `redirect_guest_output` made; each is closed once, here.
        unsafe {
            if out >= 0 {
                libc::dup2(out, 1);
                libc::close(out);
            }
            if err >= 0 {
                libc::dup2(err, 2);
                libc::close(err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn stdout_goes_to_the_log_and_comes_back() {
        let d = TempDir::new("redirect");
        let p = d.join("gameapp.guest.log");
        assert!(redirect_guest_output(&p));
        // write straight to fd 1 (the Rust test harness captures `println!`, but the fd is what the C guest uses)
        let msg = b"[dm] >>> nnMain\n";
        // SAFETY: fd 1 is open (just redirected) and `msg` is a live buffer.
        unsafe { libc::write(1, msg.as_ptr() as *const libc::c_void, msg.len()) };
        // the boot's dlopen failure goes to fd 2 as well (review r5 P1): it lands in this boot's section, where `note_boot_result` reads it
        write_fd2(
            "[gameapp] dlopen FAILED: tried: '/x' (file system sandbox blocked mmap() of '/x')",
        );
        restore_output();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("==== guest output, pid ") && text.contains("[dm] >>> nnMain"),
            "{text}"
        );
        assert!(crate::markers::last_guest_run_refused(&p), "{text}");
        assert!(
            !redirect_guest_output(&d.join("no/such/dir/x.log")),
            "an unopenable log leaves the descriptors alone"
        );
    }
}
