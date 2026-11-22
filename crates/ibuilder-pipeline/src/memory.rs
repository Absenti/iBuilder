//! Memory instrumentation and the thread-count decisions made from it.
//!
//! The Python chain printed "resident / peak / footprint / available" around every heavy step, from the kernel (`task_info`,
//! `getrusage`, `os_proc_available_memory`) through `_dmgameapp.mem()`: jetsam on a real iPhone is the number that decides whether
//! the build works at all (docs/MEMORY.md). `MemSource` is that module; `sys::mem::SystemMem` is the real one (macOS and iOS),
//! tests use `FakeMem`.

use crate::config::Config;
use crate::log::Log;
use crate::thermal::ThermalGate;
use crate::util::MIB;
use std::sync::atomic::{AtomicU64, Ordering};

/// What `_dmgameapp.mem()` returned. `available` is `os_proc_available_memory()`: the ceiling iOS will still grant this process; 0 where
/// there is none (a simulator, a Mac).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub resident: u64,
    pub peak: u64,
    pub footprint: u64,
    pub available: u64,
    /// free space on the volume holding Documents
    pub free: u64,
}

/// What `_dmgameapp.address_space()` returned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddressSpace {
    pub free: u64,
    pub largest: u64,
    /// Extended Virtual Addressing is in this app's signature
    pub extended: bool,
    /// the increased-memory-limit entitlement: 1 yes, 0 no, -1 unreadable
    pub increased_memory: i32,
}

/// The `_dmgameapp` memory half.
pub trait MemSource: Send + Sync {
    fn mem(&self) -> MemInfo;
    /// `mem().available` alone: the memory gate asks only this, every 250 ms per waiting worker, and a source that can read it by itself skips the rest (`SystemMem`: two `task_info`, a
    /// `getrusage` and a `statvfs` that the decision does not use; review r4 §6.3).
    fn available(&self) -> u64 {
        self.mem().available
    }
    /// `malloc_zone_pressure_relief`: bytes of freed-but-retained memory handed back to the OS (0 where there is nothing to do).
    fn trim(&self) -> u64 {
        0
    }
    /// `None` where it cannot be read (the Python `except (ImportError, AttributeError)` branches).
    fn address_space(&self) -> Option<AddressSpace> {
        None
    }
    /// `_dmgameapp.release_mimalloc()`: the Python app's *patched CPython* (paches/python/0001-mimalloc-release.patch, DECISIONS 82) could hand back what the free-threaded runtime's
    /// allocator keeps after the rewrite's threads ended. That is specific to CPython's mimalloc: **Rust has no such allocator, so the default is the explicit, documented no-op**
    /// -- `None` is "this runtime has no such function" (the Python branch `getattr(_dmgameapp, 'release_mimalloc', None) is None`). A host whose allocator can do it
    /// (a build that links mimalloc, say) returns `Some(rc)` with `rc == 0` for success; [`release_mimalloc`] logs the footprint around it exactly as the Python did.
    fn release_mimalloc(&self) -> Option<i32> {
        None
    }
}

/// A memory source with fixed numbers, for tests and for hosts that have no ceiling to report.
#[derive(Debug, Default)]
pub struct FakeMem {
    pub available: AtomicU64,
    pub peak: AtomicU64,
    pub resident: AtomicU64,
    pub footprint: AtomicU64,
    pub free: AtomicU64,
    pub space: std::sync::Mutex<Option<AddressSpace>>,
}

impl FakeMem {
    pub fn new(available: u64) -> FakeMem {
        FakeMem {
            available: AtomicU64::new(available),
            peak: AtomicU64::new(1500 * MIB),
            resident: AtomicU64::new(1 << 30),
            footprint: AtomicU64::new(1 << 30),
            free: AtomicU64::new(1 << 40),
            space: std::sync::Mutex::new(None),
        }
    }
    pub fn set_available(&self, v: u64) {
        self.available.store(v, Ordering::SeqCst);
    }
    pub fn set_address_space(&self, a: Option<AddressSpace>) {
        *self.space.lock().unwrap() = a;
    }
}

impl MemSource for FakeMem {
    fn mem(&self) -> MemInfo {
        MemInfo {
            resident: self.resident.load(Ordering::SeqCst),
            peak: self.peak.load(Ordering::SeqCst),
            footprint: self.footprint.load(Ordering::SeqCst),
            available: self.available.load(Ordering::SeqCst),
            free: self.free.load(Ordering::SeqCst),
        }
    }
    fn address_space(&self) -> Option<AddressSpace> {
        *self.space.lock().unwrap()
    }
}

fn mib(n: u64) -> f64 {
    n as f64 / 1_048_576.0
}

/// `mem_line(tag, log)`: the one line every heavy step prints. The text is the contract with the app (it greps `[mem:`) and with
/// the post-mortem of a killed run.
pub fn mem_line(tag: &str, log: &dyn Log, mem: &dyn MemSource) {
    let m = mem.mem();
    crate::diag::mem_event(tag, &m);
    log.line(&format!(
        "  [mem:{tag}] resident {:.0} MiB  peak {:.0} MiB  footprint {:.0} MiB  available {:.0} MiB  | free {:.2} GiB",
        mib(m.resident),
        mib(m.peak),
        mib(m.footprint),
        mib(m.available),
        m.free as f64 / 1_073_741_824.0
    ));
}

/// `release_memory(tag, log)`: ask the allocator to hand its free pages back before the next heavy step, and print what came back
/// and the memory line. (The CPython part -- `gc.collect()` -- is gone with the interpreter: Rust frees when it drops.)
/// The honest result, measured on the phone with CPython: the footprint does not follow the live set down; that is why the boot
/// has its own launch (docs/DECISIONS.md trap 17).
pub fn release_memory(tag: &str, log: &dyn Log, mem: &dyn MemSource) {
    let relieved = mem.trim();
    log.line(&format!(
        "  [mem:{tag}] release: the allocator handed back {:.0} MiB",
        mib(relieved)
    ));
    release_mimalloc(tag, log, mem);
    mem_line(tag, log, mem);
}

/// `release_mimalloc(tag, log)`: hands back what the free-threaded runtime's allocator keeps after the rewrite's threads ended (trap 71). Logs the footprint before and after. Returns the
/// MiB the footprint fell by, or `None` when the runtime has no such function ([`MemSource::release_mimalloc`] is `None`: the case for every Rust build, where this is a no-op that logs
/// nothing) or the call did not succeed (logged).
pub fn release_mimalloc(tag: &str, log: &dyn Log, mem: &dyn MemSource) -> Option<i64> {
    let before = mem.mem().footprint;
    let t0 = std::time::Instant::now();
    let rc = mem.release_mimalloc()?;
    let after = mem.mem().footprint;
    if rc != 0 {
        log.line(&format!(
            "  [mem:{tag}] mimalloc release: not in this runtime (rc {rc})"
        ));
        return None;
    }
    let freed = (before as i64 - after as i64).div_euclid(MIB as i64);
    log.line(&format!(
        "  [mem:{tag}] mimalloc release: footprint {} -> {} MiB ({:+} MiB, {:.1} s)",
        before / MIB,
        after / MIB,
        -freed,
        t0.elapsed().as_secs_f64()
    ));
    Some(freed)
}

/// `shader_jobs_for_memory`: the translation's threads for the memory left. Each running translation holds a few hundred MB of its own
/// (the translator's heap, SPIRV-Cross, the MSL). About `per_job` a thread above 1 GB kept free; at least 1. `available == 0` means no
/// ceiling to read (a simulator, a Mac): unchanged.
pub fn shader_jobs_for_memory(
    jobs: usize,
    mem: &dyn MemSource,
    log: &dyn Log,
    per_job: u64,
    what: &str,
) -> usize {
    let left = mem.mem().available as i128;
    if left <= 0 {
        return jobs;
    }
    // `per_job` is a divisor and may come from a hand-edited config: 0 would divide by zero
    let fit = std::cmp::max(
        1,
        (left - 1024 * MIB as i128).div_euclid(per_job.max(1) as i128),
    ) as usize;
    if fit < jobs {
        log.line(&format!(
            "stage 3 shaders: {} MiB left under the ceiling: {} {} thread{} instead of {}",
            left / MIB as i128,
            fit,
            what,
            if fit == 1 { "" } else { "s" },
            jobs
        ));
        return fit;
    }
    jobs
}

/// Default `per_job` of `shader_jobs_for_memory`.
pub const SHADER_PER_JOB: u64 = 450 * MIB;

/// `rewrite_jobs`: threads for the rewrite of the big module (paches/engine/0301, DECISIONS 80). Rust has no GIL, so the thread count
/// is what the machine and the phone allow: 4 is the measured sweet spot (the serial rest dominates beyond), about 150 MiB a thread
/// above one from the memory left, 2 in Low performance mode, 1 when hot (state serious or worse). `rewrite_jobs` in the config overrides.
pub fn rewrite_jobs(
    cfg: &Config,
    log: Option<&dyn Log>,
    mem: &dyn MemSource,
    cpus: usize,
    thermal: (u8, bool),
) -> usize {
    if let Some(v) = cfg.get_nonempty("rewrite_jobs")
        && let Ok(n) = v.trim().parse::<i64>()
    {
        return std::cmp::max(1, n) as usize;
    }
    let mut jobs = std::cmp::min(4, cpus);
    let (state, lowperf) = thermal;
    if state >= 2 {
        jobs = 1;
    } else if lowperf {
        jobs = jobs.min(2);
    }
    let left = mem.mem().available as i128;
    if left > 0 {
        // the serial rewrite itself peaks near 3 GiB
        let fit = std::cmp::max(
            1,
            1 + (left - 3200 * MIB as i128).div_euclid(150 * MIB as i128),
        ) as usize;
        if fit < jobs {
            if let Some(log) = log {
                log.line(&format!(
                    "stage 2 rewrite: {} MiB left under the ceiling: {} thread{} instead of {}",
                    left / MIB as i128,
                    fit,
                    if fit == 1 { "" } else { "s" },
                    jobs
                ));
            }
            jobs = fit;
        }
    }
    jobs.max(1)
}

/// `rewrite_jobs` reading the gate's current thermal state (what `do_rewrite` calls).
pub fn rewrite_jobs_now(
    cfg: &Config,
    log: &dyn Log,
    mem: &dyn MemSource,
    thermal: &ThermalGate,
) -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    rewrite_jobs(cfg, Some(log), mem, cpus, thermal.read())
}
