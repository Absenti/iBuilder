//! APFS `clonefile(2)`: a copy that shares its blocks with the source until one side changes (what the Python's
//! `--clone` gets from `cp -c`, and what the Mac app's `PackageWriter` relies on: "2 GB in 1 ms").
//!
//! This is the only `unsafe` in the crate: one libc call with two NUL-terminated paths we own. The library never
//! spawns `cp`.
#![allow(unsafe_code)]
use std::io;
use std::path::Path;

/// Clone `src` to `dst` (which must not exist). `Err` when the volume cannot clone or on other platforms: the
/// caller falls back to a real copy.
pub fn clone_file(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let s = CString::new(src.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        let d = CString::new(dst.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))?;
        // SAFETY: both pointers are valid NUL-terminated C strings that outlive the call; flags 0 is defined.
        let rc = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = (src, dst);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "clonefile is Apple-only",
        ))
    }
}
