//! The two OS calls this crate makes beyond `std`: free space of a volume (`statvfs`, the Python's `free_bytes`) and a file's identity
//! (`romfs_image.file_identity`: inode, size, mtime and ctime in ns; not the device number, which macOS renumbers when it mounts a volume).

use std::path::Path;

/// Bytes `statvfs` reports available to this process on the volume holding `path` (None when the call fails; purgeable space is not in it). The one `unsafe` of the crate: `statvfs` on a
/// NUL-terminated path with a zeroed out-struct.
#[allow(unsafe_code)]
pub fn free_bytes_strict(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid C string for the call's duration; `st` is plain data the call fills in
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    }
}

/// The space the extraction gate counts on: on Apple targets the volume's "available capacity for important usage" (free + purgeable, what Settings shows; iOS purges on demand),
/// else (or when that call fails) the `statvfs` value. Never below the strict value.
pub fn free_bytes(path: &Path) -> Option<u64> {
    let strict = free_bytes_strict(path);
    #[cfg(target_vendor = "apple")]
    if let Some(i) = important_usage_bytes(path) {
        return Some(strict.unwrap_or(0).max(i as u64));
    }
    strict
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

/// `[inode, size, mtime_ns, ctime_ns]` of `path`
pub fn file_identity(path: &Path) -> std::io::Result<[u64; 4]> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path)?;
    let ns = |s: i64, n: i64| (s as i128 * 1_000_000_000 + n as i128) as u64;
    Ok([
        m.ino(),
        m.size(),
        ns(m.mtime(), m.mtime_nsec()),
        ns(m.ctime(), m.ctime_nsec()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_bytes_is_at_least_the_strict_value_and_a_bad_path_is_none() {
        let here = Path::new(".");
        assert!(free_bytes(here).unwrap() >= free_bytes_strict(here).unwrap());
        assert_eq!(free_bytes(Path::new("/no/such/place/at/all")), None);
    }
}
