//! The kernel's memory numbers: `task_info` (MACH_TASK_BASIC_INFO: resident and its high-water mark; TASK_VM_INFO: `phys_footprint`), `getrusage` (peak RSS),
//! `os_proc_available_memory()` (iOS only) and `malloc_zone_pressure_relief`. All three numbers are the kernel's own, not ours (docs/MEMORY.md).
//! Port of `mem_now`, `gameapp_available`, `gameapp_memline`, `py_mem`, `py_trim`, `py_address_space` of `gameapp_shim.c`.

use crate::memory::{AddressSpace, MemInfo, MemSource};
use crate::util::MIB;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// The memory source of a real process. `documents` is the volume whose free space is reported.
pub struct SystemMem {
    documents: PathBuf,
}

/// Read and raised from several threads at once (the UI thread's memory line, the chain, the translation threads): a plain read-compare-write could lower it.
static PEAK: AtomicU64 = AtomicU64::new(0);

fn peak_raise(v: u64) {
    let mut cur = PEAK.load(Ordering::SeqCst);
    while v > cur {
        match PEAK.compare_exchange_weak(cur, v, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(c) => cur = c,
        }
    }
}

#[cfg(target_vendor = "apple")]
mod mach {
    pub const MACH_TASK_BASIC_INFO: u32 = 20;
    pub const TASK_VM_INFO: u32 = 22;
    unsafe extern "C" {
        pub static mach_task_self_: u32;
        pub fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        pub fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
    }
}

/// `(resident, resident peak, phys_footprint)` in bytes.
pub fn mem_now() -> (u64, u64, u64) {
    let (mut res, mut resmax, mut fp) = (0u64, 0u64, 0u64);
    #[cfg(target_vendor = "apple")]
    // SAFETY: `info` buffers are large enough for the flavors requested (mach_task_basic_info is 48 bytes = 12 ints, task_vm_info is 372 bytes = 93 ints; the
    // buffers below are 64-bit aligned arrays covering both), `count` is the buffer's size in ints as the kernel expects, and the task port is this task's own.
    unsafe {
        let mut basic = [0u64; 6];
        let mut c1: u32 = 12;
        if mach::task_info(
            mach::mach_task_self_,
            mach::MACH_TASK_BASIC_INFO,
            basic.as_mut_ptr() as *mut i32,
            &mut c1,
        ) == 0
        {
            res = basic[1];
            resmax = basic[2];
        }
        let mut vm = [0u64; 48];
        let mut c2: u32 = 93;
        if mach::task_info(
            mach::mach_task_self_,
            mach::TASK_VM_INFO,
            vm.as_mut_ptr() as *mut i32,
            &mut c2,
        ) == 0
            && c2 >= 42
        {
            fp = vm[18]; // phys_footprint at offset 144
        }
    }
    // SAFETY: `ru` is a zeroed rusage getrusage fills.
    let rss = unsafe {
        let mut ru: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut ru) == 0 {
            // macOS reports bytes, Linux KiB
            if cfg!(target_vendor = "apple") {
                ru.ru_maxrss as u64
            } else {
                ru.ru_maxrss as u64 * 1024
            }
        } else {
            0
        }
    };
    if res == 0 {
        res = rss; // no Mach (or task_info refused): the best the kernel offers
    }
    peak_raise(res);
    peak_raise(resmax);
    peak_raise(rss);
    (res, PEAK.load(Ordering::SeqCst), fp)
}

/// The kernel's own high-water mark of this task's `phys_footprint` (`TASK_VM_INFO`'s `ledger_phys_footprint_peak`, revision 3 and later): the largest footprint the process ever had, exact
/// however rarely it is sampled (review r4 B4: the CLI meter samples every 100 ms). `None` where the kernel does not give it (an older revision, or not an Apple system).
pub fn footprint_peak() -> Option<u64> {
    #[cfg(target_vendor = "apple")]
    // SAFETY: as in `mem_now`: a 64-bit aligned buffer of 93 ints (task_vm_info), its size in ints as the count, this task's own port.
    unsafe {
        let mut vm = [0u64; 48];
        let mut count: u32 = 93;
        if mach::task_info(
            mach::mach_task_self_,
            mach::TASK_VM_INFO,
            vm.as_mut_ptr() as *mut i32,
            &mut count,
        ) == 0
            && count >= 44
        {
            let v = vm[21] as i64; // ledger_phys_footprint_peak at offset 168 (int64_t)
            return (v > 0).then_some(v as u64);
        }
    }
    None
}

/// `os_proc_available_memory()`: the ceiling iOS will still grant this process (the number jetsam is expressed in, and the only way to tell whether the increased-memory-limit entitlement took
/// effect). 0 where there is none: a Mac, the simulator (the symbol is absent there), or a non-Apple host. Looked up with `dlsym` so nothing fails to link.
pub fn available_bytes() -> u64 {
    #[cfg(target_os = "ios")]
    {
        use std::sync::OnceLock;
        static F: OnceLock<Option<unsafe extern "C" fn() -> usize>> = OnceLock::new();
        let f = F.get_or_init(|| {
            let name = std::ffi::CString::new("os_proc_available_memory").unwrap();
            // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name; the symbol, when present, has this C signature (<os/proc.h>).
            let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
            if p.is_null() {
                None
            } else {
                Some(unsafe {
                    std::mem::transmute::<*mut libc::c_void, unsafe extern "C" fn() -> usize>(p)
                })
            }
        });
        if let Some(f) = f {
            // SAFETY: see above.
            return unsafe { f() } as u64;
        }
    }
    0
}

/// `malloc_zone_pressure_relief(NULL, 0)`: ask every malloc zone to give its free pages back; bytes relieved.
pub fn trim() -> u64 {
    #[cfg(target_vendor = "apple")]
    // SAFETY: documented to accept a NULL zone (all zones) and a goal of 0 (everything).
    unsafe {
        return mach::malloc_zone_pressure_relief(std::ptr::null_mut(), 0) as u64;
    }
    #[allow(unreachable_code)]
    0
}

impl SystemMem {
    pub fn new(documents: impl Into<PathBuf>) -> SystemMem {
        SystemMem {
            documents: documents.into(),
        }
    }

    /// The text `gameapp_memline` returned for the managed side's log.
    pub fn memline(&self) -> String {
        let m = self.mem();
        format!(
            "resident {:.0} MiB, peak {:.0} MiB, footprint {:.0} MiB, available {:.0} MiB | free {:.2} GiB",
            m.resident as f64 / MIB as f64,
            m.peak as f64 / MIB as f64,
            m.footprint as f64 / MIB as f64,
            m.available as f64 / MIB as f64,
            m.free as f64 / (1u64 << 30) as f64
        )
    }
}

impl MemSource for SystemMem {
    fn mem(&self) -> MemInfo {
        let (resident, peak, footprint) = mem_now();
        MemInfo {
            resident,
            peak,
            footprint,
            available: available_bytes(),
            free: crate::sys::free_bytes(&self.documents).max(0) as u64,
        }
    }

    /// `os_proc_available_memory()` only (the peak `mem_now` tracks needs no sample here: `resident_size_max` and `ru_maxrss` are the kernel's own maxima).
    fn available(&self) -> u64 {
        available_bytes()
    }

    fn trim(&self) -> u64 {
        trim()
    }

    fn address_space(&self) -> Option<AddressSpace> {
        let (free, largest) = crate::sys::vm::address_space()?;
        let ent = crate::sys::entitlements::own_entitlements();
        Some(AddressSpace {
            free,
            largest,
            extended: ent.as_ref().map(|e| e.extended_va).unwrap_or(false),
            increased_memory: ent
                .as_ref()
                .map(|e| e.increased_memory as i32)
                .unwrap_or(-1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_has_resident_memory_and_a_peak_that_does_not_fall() {
        let m = SystemMem::new(".");
        let a = m.mem();
        assert!(a.resident > 1 << 20, "{a:?}");
        assert!(a.peak >= a.resident, "{a:?}");
        let _hold: Vec<u8> = vec![1u8; 64 << 20];
        let b = m.mem();
        assert!(b.peak >= a.peak, "the peak never falls: {a:?} then {b:?}");
        assert!(b.free > 0);
        #[cfg(target_vendor = "apple")]
        assert!(b.footprint > 0, "phys_footprint from TASK_VM_INFO: {b:?}");
        assert!(m.memline().contains("footprint"));
        // a Mac has no jetsam ceiling
        #[cfg(target_os = "macos")]
        assert_eq!(b.available, 0);
    }

    /// Review r4 B4: the kernel's footprint peak is there on this system, never below the footprint now, and does not fall when the memory is given back.
    #[test]
    fn the_kernels_footprint_peak_is_a_high_water_mark() {
        #[cfg(target_vendor = "apple")]
        {
            let before = footprint_peak().expect("TASK_VM_INFO rev 3+ on this system");
            let (_, _, fp) = mem_now();
            assert!(before >= fp, "peak {before} < footprint {fp}");
            let mut big = vec![0u8; 96 << 20];
            for i in (0..big.len()).step_by(4096) {
                big[i] = 1; // touched: in the footprint
            }
            std::hint::black_box(&big);
            let during = footprint_peak().unwrap();
            drop(big);
            let after = footprint_peak().unwrap();
            assert!(
                during >= before && after >= during,
                "{before} {during} {after}"
            );
            assert!(
                during >= fp + (64 << 20),
                "the touched 96 MiB raised the peak: {fp} -> {during}"
            );
        }
    }

    #[test]
    fn trim_does_not_fail() {
        let _ = trim();
    }
}
