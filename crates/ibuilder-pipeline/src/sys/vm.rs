//! The address space, and the guest block taken before the app's own loading fragments it (DECISIONS traps 22, 24, 25, 27). Port of `as_census`, `as_profile_hex`,
//! `gameapp_as_reserve`, `as_reserve_block`, `gameapp_address_space` of `gameapp_shim.c`.
//!
//! An iPhone gives a process under 450 GB of address space and the system itself reserves almost all of it before `main()` runs (measured 2026-11-17: 445.94 GB in use, of which 385.0 GB is
//! one VM_MEMORY_GUARD region and 54.8 GB an untagged one; 4.02 GB free in gaps of 2.96, 0.58 and 0.28 GB). The engine asks for ONE contiguous block (the profile's `mem_reserve`, 5 GB) when
//! the guest reaches `nnosInitialize`, and by then this app has loaded the runtime and the 195 MB framework, so no 5 GB hole is left: `die("reserve guest AS")`. So the block is taken at the
//! earliest point this app controls (a Mach-O initializer: `ibuilder-ffi`'s `__mod_init_func`) and the engine is told where it is: `DM_AS_AT`, mapped `MAP_FIXED` over this reservation
//! (paches/0006). Nothing is committed -- PROT_NONE costs address space only, no memory, no jetsam.
//!
//! Only a launch that BOOTS takes it, and only below a ceiling of 5 GiB (above it the engine keeps the guest pool in sparse files and a 5 GB block would only take the room the framework
//! needs to dlopen). At a low ceiling the block goes right AFTER the framework is dlopen'd (trap 25: framework first, block immediately after).

use crate::env::Env;
use crate::tsv::profile_hex;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_vendor = "apple")]
mod mach {
    unsafe extern "C" {
        pub static mach_task_self_: u32;
        pub fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
        pub fn vm_region_recurse_64(
            task: u32,
            address: *mut usize,
            size: *mut usize,
            depth: *mut u32,
            info: *mut i32,
            count: *mut u32,
        ) -> i32;
    }
    pub const TASK_VM_INFO: u32 = 22;
}

/// One region of the map.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub start: u64,
    pub size: u64,
    pub tag: u32,
}

/// `(min_address, max_address)` of this task's map.
#[cfg(target_vendor = "apple")]
fn bounds() -> (u64, u64) {
    // SAFETY: a 64-bit aligned buffer large enough for task_vm_info (372 bytes = 93 ints); count is in ints; the port is this task's own.
    unsafe {
        let mut vm = [0u64; 48];
        let mut n: u32 = 93;
        if mach::task_info(
            mach::mach_task_self_,
            mach::TASK_VM_INFO,
            vm.as_mut_ptr() as *mut i32,
            &mut n,
        ) == 0
            && n >= 42
        {
            return (vm[19], vm[20]); // min_address at 152, max_address at 160
        }
    }
    (0, 0)
}

/// Walk the map with `vm_region_recurse_64`, calling `f(region)` for every leaf region in address order.
#[cfg(target_vendor = "apple")]
fn walk(f: impl FnMut(Region)) {
    walk_regions(
        |addr, size, depth| {
            let mut info = [0u32; 21]; // vm_region_submap_info_64, packed(4): 84 bytes
            let mut cnt: u32 = 21;
            // SAFETY: `info` is VM_REGION_SUBMAP_INFO_COUNT_64 ints; the other out-params are valid for writes; the task port is this task's own.
            let kr = unsafe {
                mach::vm_region_recurse_64(
                    mach::mach_task_self_,
                    addr,
                    size,
                    depth,
                    info.as_mut_ptr() as *mut i32,
                    &mut cnt,
                )
            };
            // is_submap at offset 48, user_tag at offset 20
            (kr == 0).then(|| (info[12] != 0, info[5] & 255))
        },
        f,
    );
}

/// The loop of `walk`, over `region_at(addr, size, depth)`: the region at or after `addr` (the call updates the three), `Some((is_submap, user_tag))`, or `None` at the end of the map.
#[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
fn walk_regions(
    mut region_at: impl FnMut(&mut usize, &mut usize, &mut u32) -> Option<(bool, u32)>,
    mut f: impl FnMut(Region),
) {
    let (mut addr, mut size, mut depth) = (0usize, 0usize, 0u32);
    while let Some((is_submap, tag)) = region_at(&mut addr, &mut size, &mut depth) {
        if is_submap {
            depth += 1;
            continue;
        }
        f(Region {
            start: addr as u64,
            size: size as u64,
            tag,
        });
        // a region of size 0 would be asked for again forever, and one that ends at the top of the address space has nothing after it (review r3 §5.6)
        match addr.checked_add(size) {
            Some(next) if size > 0 => addr = next,
            _ => break,
        }
    }
}

#[cfg(not(target_vendor = "apple"))]
fn bounds() -> (u64, u64) {
    (0, 0)
}
#[cfg(not(target_vendor = "apple"))]
fn walk(_f: impl FnMut(Region)) {}

/// `gameapp_address_space`: the free address space now: the sum of the holes between regions and the largest one, in bytes. `None` off Apple platforms.
pub fn address_space() -> Option<(u64, u64)> {
    if !cfg!(target_vendor = "apple") {
        return None;
    }
    let (lo, hi) = bounds();
    let (mut prev, mut total, mut big) = (lo, 0u64, 0u64);
    walk(|r| {
        let e = r.start + r.size;
        if r.start > prev {
            let g = r.start - prev;
            total += g;
            big = big.max(g);
        }
        prev = prev.max(e);
    });
    if hi > prev {
        let g = hi - prev;
        total += g;
        big = big.max(g);
    }
    Some((total, big))
}

/// `as_census`: how full the address space is, the way the engine's crash handler measures it (`dm/host/memstat.m dm_vm_census`): bytes in regions, the largest free gaps, and the three largest
/// regions with their VM tag (31 is VM_MEMORY_GUARD, 0 untagged, 30 stacks, 21 IOKit). The line is the measurement trap 22 rests on.
pub fn census(when: &str) -> Option<String> {
    if !cfg!(target_vendor = "apple") {
        return None;
    }
    let (lo, hi) = bounds();
    let (mut used, mut prev, mut nreg) = (0u64, lo, 0u32);
    let (mut gap0, mut gap1, mut gap_at) = (0u64, 0u64, 0u64);
    let mut top: [Option<Region>; 3] = [None; 3];
    let mut note_gap = |g: u64, at: u64| {
        if g > gap0 {
            gap1 = gap0;
            gap0 = g;
            gap_at = at;
        } else if g > gap1 {
            gap1 = g;
        }
    };
    walk(|r| {
        if r.start > prev {
            note_gap(r.start - prev, prev);
        }
        prev = prev.max(r.start + r.size);
        used += r.size;
        nreg += 1;
        for k in 0..3 {
            if top[k].map(|t| r.size > t.size).unwrap_or(true) {
                for j in (k + 1..3).rev() {
                    top[j] = top[j - 1];
                }
                top[k] = Some(r);
                break;
            }
        }
    });
    if hi > prev {
        note_gap(hi - prev, prev);
    }
    let g = |v: u64| v as f64 / 1_073_741_824.0;
    let t = |k: usize| {
        top[k]
            .map(|r| format!("{:#x} {:.2} GB tag {}", r.start, g(r.size), r.tag))
            .unwrap_or_else(|| "0x0 0.00 GB tag -1".into())
    };
    Some(format!(
        "[gameapp] as-census {when}: {:.2} GB in {nreg} regions between {lo:#x} and {hi:#x} ({:.2} GB); free {:.2} GB at {gap_at:#x} then {:.2} GB; largest {}, {}, {}",
        g(used),
        g(hi - lo),
        g(gap0),
        g(gap1),
        t(0),
        t(1),
        t(2)
    ))
}

// ---- the reservation ------------------------------------------------------------------------------------------------------------------------

/// The block this launch holds for the boot (0 when it took none).
static EARLY: AtomicUsize = AtomicUsize::new(0);
static EARLY_SIZE: AtomicUsize = AtomicUsize::new(0);

pub fn early_block() -> Option<(usize, usize)> {
    match EARLY.load(Ordering::SeqCst) {
        0 => None,
        p => Some((p, EARLY_SIZE.load(Ordering::SeqCst))),
    }
}

/// How the block is sized, from the profile and the environment (pure: the part of `as_reserve_block` worth testing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReservePlan {
    /// the size the engine would ask for (or `DM_AS_SIZE`)
    pub want: usize,
    /// the least it accepts: the engine refuses an alias region under 256 MB
    pub floor: usize,
    /// the part of the block that is not alias region (256 MB front + heap + stacks)
    pub base_rest: usize,
    pub engine_alias: usize,
    /// no usable profile: sized as the 5 GB / 4.375 GB alias profile the phone-proven ones carry
    pub default_profile: bool,
}

pub fn plan_reserve(profile: &Path, env: &dyn Env) -> ReservePlan {
    let (mut reserve, mut alias) = (
        profile_hex(profile, "mem_reserve") as u64,
        profile_hex(profile, "mem_alias") as u64,
    );
    let mut default_profile = false;
    if reserve < 0x1000_0000 || alias < 0x1000_0000 || alias >= reserve {
        reserve = 0x1_4000_0000;
        alias = 0x1_1800_0000;
        default_profile = true;
    }
    // The engine reserves mem_reserve - mem_alias + alias (hle_svc.c mem_init_once), and on iOS alias is the profile's, so with no DM_ALIAS_SIZE override the block is mem_reserve itself.
    let engine_alias = env
        .get("DM_ALIAS_SIZE")
        .filter(|s| !s.is_empty())
        .and_then(|s| crate::util::parse_int0(&s))
        .map(|v| v as usize)
        .unwrap_or(alias as usize);
    let base_rest = (reserve - alias) as usize; // 256 MB front + heap + stacks: 0x28000000 here
    let mut want = base_rest + engine_alias;
    if let Some(forced) = env
        .get("DM_AS_SIZE")
        .filter(|s| !s.is_empty())
        .and_then(|s| crate::util::parse_int0(&s))
    {
        want = forced as usize;
    }
    ReservePlan {
        want,
        floor: base_rest + 0x1000_0000,
        base_rest,
        engine_alias,
        default_profile,
    }
}

/// `as_reserve_block`: take the block (shrinking in 128 MB steps down to `floor`), set `DM_AS_AT` (and `DM_ALIAS_SIZE` when it landed short) in the real process environment and say what
/// happened, with the census after. `say` receives the `[gameapp] as-...` lines. Returns `(address, size)`.
pub fn reserve_block(profile: &Path, env: &dyn Env, say: &dyn Fn(&str)) -> Option<(usize, usize)> {
    let plan = plan_reserve(profile, env);
    let g = |v: usize| v as f64 / 1_073_741_824.0;
    if plan.default_profile {
        say(&format!(
            "[gameapp] as-reserve: no usable profile ({}): sizing the block as a 5 GB / 4.375 GB alias profile",
            profile.display()
        ));
    }
    let mut got: Option<(usize, usize)> = None;
    let mut size = plan.want;
    while size >= plan.floor {
        // SAFETY: an anonymous PROT_NONE private mapping: no file, no access, costs address space only. The result is checked against MAP_FAILED.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if p != libc::MAP_FAILED {
            got = Some((p as usize, size));
            break;
        }
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        say(&format!(
            "[gameapp] as-reserve: {:.2} GB refused (errno {errno}), trying {:.2} GB",
            g(size),
            if size > plan.floor {
                g(size - 0x800_0000)
            } else {
                0.0
            }
        ));
        if size < 0x800_0000 {
            break;
        }
        size -= 0x800_0000;
    }
    let Some((addr, size)) = got else {
        say(&format!(
            "[gameapp] as-reserve: nothing fits, not even {:.2} GB -- the boot below will ask for its own block and fail the way the three runs of 2026-11-17 did",
            g(plan.floor)
        ));
        if let Some(c) = census("after the refused reservation") {
            say(&c);
        }
        return None;
    };
    EARLY.store(addr, Ordering::SeqCst);
    EARLY_SIZE.store(size, Ordering::SeqCst);
    let hex = format!("{addr:#x}");
    env.set("DM_AS_AT", &hex);
    say(&format!(
        "[gameapp] as-reserve: took {:.2} GB at {hex} for the guest ({}), DM_AS_AT set: the engine maps its block there with MAP_FIXED",
        g(size),
        if size == plan.want {
            "the profile's block"
        } else {
            "less than the profile's block"
        }
    ));
    if size != plan.want {
        let short_alias = size - plan.base_rest;
        let hexa = format!("{short_alias:#x}");
        env.set("DM_ALIAS_SIZE", &hexa);
        say(&format!(
            "[gameapp] as-reserve: the alias region follows at {:.2} GB instead of {:.2} GB (DM_ALIAS_SIZE {hexa}); the guest mapped 2.01 GB of it before its first frames on the simulator",
            g(short_alias),
            g(plan.engine_alias)
        ));
    }
    if let Some(c) = census("after the reservation") {
        say(&c);
    }
    Some((addr, size))
}

/// `gameapp_as_reserve`'s decision, before anything is taken: does this launch reserve now, later (right after the dlopen), or never?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReserveWhen {
    /// nothing to boot: leave the address space alone
    Never,
    /// the ceiling is 5 GiB or more: the engine keeps the guest pool in sparse files; a 5 GB block would only take the room the framework needs to dlopen
    SkippedHighCeiling,
    /// below that ceiling the block is taken right after the dlopen
    AfterDlopen,
}

/// `may_boot`: a boot request is pending OR a framework is on disk (the screen offers "Boot again (no rebuild)" whenever the framework this build made is there, and a boot that ran to its
/// time limit has consumed its request); `ceiling`: `os_proc_available_memory()` in bytes (0 = none).
pub fn reserve_decision(may_boot: bool, ceiling: u64) -> ReserveWhen {
    if !may_boot {
        ReserveWhen::Never
    } else if ceiling >= 5 * (1 << 30) {
        ReserveWhen::SkippedHighCeiling
    } else {
        ReserveWhen::AfterDlopen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::MapEnv;
    use crate::testutil::{TempDir, write};

    /// Review r3 §5.6: a region of size 0 (or one that ends at the top of the address space) ends the walk instead of asking for the same address forever.
    #[test]
    fn a_region_of_size_zero_or_at_the_top_ends_the_walk() {
        for (start, len) in [(0x1000usize, 0usize), (usize::MAX - 0xfff, 0x1000)] {
            let mut calls = 0;
            let mut seen = Vec::new();
            walk_regions(
                |addr, size, _depth| {
                    calls += 1;
                    assert!(
                        calls < 100,
                        "the walk does not end (asked {calls} times at {addr:#x})"
                    );
                    if *addr > start {
                        return None;
                    }
                    *addr = start;
                    *size = len;
                    Some((false, 7))
                },
                |r| seen.push((r.start, r.size)),
            );
            assert_eq!(seen, [(start as u64, len as u64)]);
        }
    }

    #[test]
    fn address_space_and_census_on_apple() {
        if !cfg!(target_vendor = "apple") {
            assert!(address_space().is_none());
            return;
        }
        let (free, largest) = address_space().unwrap();
        assert!(
            free >= largest && largest > 0,
            "free {free} largest {largest}"
        );
        let c = census("test").unwrap();
        assert!(
            c.starts_with("[gameapp] as-census test: ")
                && c.contains(" regions between ")
                && c.contains("tag "),
            "{c}"
        );
    }

    #[test]
    fn plan_uses_the_profile_or_the_phone_proven_default() {
        let d = TempDir::new("vmplan");
        let p = d.join("profile.tsv");
        let env = MapEnv::new();
        write(&p, "mem_reserve\t0x140000000\nmem_alias\t0x118000000\n");
        let plan = plan_reserve(&p, &env);
        assert_eq!(
            (plan.want, plan.floor, plan.base_rest, plan.default_profile),
            (0x1_4000_0000, 0x2800_0000 + 0x1000_0000, 0x2800_0000, false)
        );
        write(&p, "nothing useful\n");
        let plan = plan_reserve(&p, &env);
        assert!(plan.default_profile && plan.want == 0x1_4000_0000);
        // an alias override moves the block: base + the override
        write(&p, "mem_reserve\t0x140000000\nmem_alias\t0x118000000\n");
        env.set("DM_ALIAS_SIZE", "0xc00000000");
        assert_eq!(plan_reserve(&p, &env).want, 0x2800_0000 + 0xc_0000_0000);
        env.set("DM_AS_SIZE", "0x30000000");
        assert_eq!(
            plan_reserve(&p, &env).want,
            0x3000_0000,
            "DM_AS_SIZE forces it"
        );
    }

    #[test]
    fn reserve_decision_follows_the_ceiling() {
        assert_eq!(reserve_decision(false, 0), ReserveWhen::Never);
        assert_eq!(
            reserve_decision(true, 6132 << 20),
            ReserveWhen::SkippedHighCeiling,
            "the 6132 MiB device run: the engine used sparse files"
        );
        assert_eq!(
            reserve_decision(true, 3364 << 20),
            ReserveWhen::AfterDlopen,
            "at 3364 MiB the block follows the dlopen (trap 25)"
        );
        assert_eq!(reserve_decision(true, 0), ReserveWhen::AfterDlopen);
    }

    #[test]
    fn reserve_takes_a_prot_none_block_and_tells_the_engine_where() {
        let d = TempDir::new("vmreserve");
        let p = d.join("profile.tsv");
        // a small profile so the test maps 640 MB of PROT_NONE address space (no memory is committed)
        write(&p, "mem_reserve\t0x40000000\nmem_alias\t0x30000000\n");
        let env = MapEnv::new();
        env.set("DM_AS_SIZE", "0x28000000");
        let lines = std::sync::Mutex::new(Vec::<String>::new());
        let say = |s: &str| lines.lock().unwrap().push(s.to_string());
        let (addr, size) = reserve_block(&p, &env, &say).unwrap();
        assert_eq!(size, 0x2800_0000);
        assert_eq!(env.get("DM_AS_AT").unwrap(), format!("{addr:#x}"));
        assert!(
            lines
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("took 0.62 GB") || l.contains("took 0.63 GB")),
            "{:?}",
            lines.lock().unwrap()
        );
        assert_eq!(early_block(), Some((addr, size)));
        // the block is address space only: it is released here so the test leaves the process as it found it
        // SAFETY: (addr, size) is the mapping reserve_block just made, unmapped exactly once.
        unsafe { libc::munmap(addr as *mut libc::c_void, size) };
    }
}
