//! The only module of this crate with `unsafe`: the places where the Python chain reached the OS through `gameapp_shim.c`.
//!
//! Why `unsafe` is justified here and nowhere else: each item is a thin call into libc or the Mach kernel interface that has no safe std equivalent --
//! `task_info` (resident / peak / phys footprint: the numbers jetsam is expressed in), `os_proc_available_memory` (the ceiling iOS enforces), `vm_region_recurse_64`
//! (the address-space census and the guest block's reservation), `malloc_zone_pressure_relief`, `dlopen` / `dlsym` and a call through the host's `dm_host_probe_boot`
//! (stage 5: the guest runs in this process), `dup2` of fd 1/2 onto the guest log, the fatal-signal logger, `setenv` (edition 2024 makes `std::env::set_var` unsafe
//! too: it races with `getenv` on other threads; it is called once, before the guest starts, with nothing else reading the environment).
//! Nothing here starts a process: no fork, exec, posix_spawn or `Command`.
//!
//! Platform gating: Mach calls exist on `target_vendor = "apple"` (macOS and iOS); on other unixes `mem` falls back to `getrusage` and the vm functions report "unavailable".
//! `os_proc_available_memory` is only looked up on iOS (it is looked up with `dlsym`, so the iOS simulator, which links the host's libSystem where the symbol is absent, reports
//! "no ceiling" instead of failing to link).

pub mod boot;
pub mod entitlements;
pub mod mem;
pub mod qos;
pub mod redirect;
pub mod signals;
pub mod vm;

use std::ffi::CString;
use std::path::Path;

/// `setenv(name, value, 1)` on the real process environment.
pub fn setenv(name: &str, value: &str) {
    if let (Ok(n), Ok(v)) = (CString::new(name), CString::new(value)) {
        // SAFETY: both strings are NUL-terminated and live for the call; callers run before the guest starts (see the module docs).
        unsafe {
            libc::setenv(n.as_ptr(), v.as_ptr(), 1);
        }
    }
}

pub fn unsetenv(name: &str) {
    if let Ok(n) = CString::new(name) {
        // SAFETY: NUL-terminated string that lives for the call.
        unsafe {
            libc::unsetenv(n.as_ptr());
        }
    }
}

/// `free_bytes_strict(path)`: bytes `statvfs` reports available on the volume holding `path` (`f_bavail * f_frsize`; purgeable space is NOT in it), -1 when it cannot be read.
pub fn free_bytes_strict(path: &Path) -> i64 {
    let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) else {
        return -1;
    };
    // SAFETY: `st` is a zeroed plain-old-data struct that statvfs fills; `c` is NUL-terminated.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return -1;
        }
        (st.f_bavail as u64).saturating_mul(st.f_frsize as u64) as i64
    }
}

/// The volume's available capacity for important usage (free + purgeable), or `None` (not an Apple target, the call failed, or it gave 0).
pub fn free_bytes_important(path: &Path) -> Option<i64> {
    #[cfg(target_vendor = "apple")]
    {
        important_usage_bytes(path)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        let _ = path;
        None
    }
}

/// `free_bytes(path)`: the space the gates count on: the "important usage" capacity where the OS has one (iOS purges on demand for it), else the `statvfs` value; -1 when neither can be read.
/// Never below the strict value.
pub fn free_bytes(path: &Path) -> i64 {
    let strict = free_bytes_strict(path);
    match free_bytes_important(path) {
        Some(i) => i.max(strict),
        None => strict,
    }
}

/// The volume's "available capacity for important usage" (what iPhone Settings shows: free space PLUS purgeable space iOS clears on demand), through CoreFoundation
/// (`kCFURLVolumeAvailableCapacityForImportantUsageKey`). `None` when it cannot be read or is not positive: callers fall back to `statvfs`. Apple targets only.
#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
fn important_usage_bytes(path: &Path) -> Option<i64> {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFURLVolumeAvailableCapacityForImportantUsageKey: *const c_void;
        fn CFURLCreateFromFileSystemRepresentation(
            alloc: *const c_void,
            buf: *const u8,
            len: isize,
            is_dir: u8,
        ) -> *const c_void;
        fn CFURLCopyResourcePropertyForKey(
            url: *const c_void,
            key: *const c_void,
            value: *mut *const c_void,
            err: *mut *const c_void,
        ) -> u8;
        fn CFNumberGetValue(n: *const c_void, kind: isize, out: *mut c_void) -> u8;
        fn CFRelease(o: *const c_void);
    }
    const SINT64: isize = 4; // kCFNumberSInt64Type
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.is_empty() {
        return None;
    }
    // SAFETY: every CF object created here is released once; `bytes` outlives the call; out-pointers are valid locals; a CFNumber is read with its own type.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        );
        if url.is_null() {
            return None;
        }
        let mut value: *const c_void = std::ptr::null();
        let mut err: *const c_void = std::ptr::null();
        let ok = CFURLCopyResourcePropertyForKey(
            url,
            kCFURLVolumeAvailableCapacityForImportantUsageKey,
            &mut value,
            &mut err,
        );
        let mut out: i64 = 0;
        let got = ok != 0
            && !value.is_null()
            && CFNumberGetValue(value, SINT64, &mut out as *mut i64 as *mut c_void) != 0;
        if !value.is_null() {
            CFRelease(value);
        }
        if !err.is_null() {
            CFRelease(err);
        }
        CFRelease(url);
        (got && out > 0).then_some(out)
    }
}

/// The device's model identifier ("iPhone16,1" on iOS, "arm64" elsewhere): `hw.machine` on iOS, `uname -m` otherwise.
pub fn machine_model() -> Option<String> {
    #[cfg(target_os = "ios")]
    {
        let mut buf = [0u8; 64];
        let mut len = buf.len();
        let name = CString::new("hw.machine").ok()?;
        // SAFETY: `buf`/`len` describe a writable buffer; sysctlbyname writes at most `len` bytes and updates `len`.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buf.as_mut_ptr() as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 && len > 0 {
            let s = String::from_utf8_lossy(&buf[..len])
                .trim_end_matches('\0')
                .to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    // SAFETY: `u` is a zeroed utsname that uname fills with NUL-terminated strings.
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) == 0 {
            let s = std::ffi::CStr::from_ptr(u.machine.as_ptr())
                .to_string_lossy()
                .into_owned();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// A string `sysctlbyname` value (`kern.osversion`, `hw.machine`, ...), `None` where the name does not exist or the value is not text.
pub fn sysctl_text(name: &str) -> Option<String> {
    let cname = CString::new(name).ok()?;
    let mut len: libc::size_t = 0;
    // SAFETY: a size query (null buffer) on a NUL-terminated name.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 || len > 4096 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is `len` writable bytes; sysctlbyname writes at most `len` of them and updates `len`.
    let rc = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf8(buf).ok().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysctl_text_reads_a_text_value_and_refuses_a_missing_name() {
        assert!(sysctl_text("hw.machine").is_some());
        assert_eq!(sysctl_text("no.such.name.at.all"), None);
    }

    #[test]
    fn free_bytes_reads_this_volume_and_rejects_nonsense() {
        assert!(free_bytes(Path::new(".")) > 0);
        assert_eq!(free_bytes(Path::new("/no/such/place/at/all")), -1);
        assert_eq!(
            free_bytes_important(Path::new("/no/such/place/at/all")),
            None
        );
        assert_eq!(free_bytes_important(Path::new("")), None);
        assert!(free_bytes_important(Path::new(".")).is_none_or(|i| i >= 0));
        assert!(free_bytes(Path::new(".")) >= free_bytes_strict(Path::new(".")));
    }

    #[test]
    fn machine_model_is_never_empty_here() {
        assert!(machine_model().map(|m| !m.is_empty()).unwrap_or(false));
    }
}
