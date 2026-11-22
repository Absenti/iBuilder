//! `mallocprobe`: what the macOS allocator does with freed memory (docs/chain/MEMORY-RUST.md, "why freed memory stays"). Not part of the product (`unsafe`, Mac only).
//!
//! Patterns that stand for the chain's stages: many small blocks (a record per shader), medium blocks (a few KB .. 100 KB), large buffers (MBs), freed fully or partly, with and without
//! `malloc_zone_pressure_relief`. Each line is `(resident, phys_footprint)` from `task_info`, the two numbers the chain reports.

unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    fn malloc_default_zone() -> *mut std::ffi::c_void;
    fn malloc_get_all_zones(
        task: u32,
        reader: *mut std::ffi::c_void,
        addresses: *mut *mut usize,
        count: *mut u32,
    ) -> i32;
}

fn mem() -> (f64, f64) {
    let (mut res, mut fp) = (0u64, 0u64);
    // SAFETY: as in ibuilder-pipeline's sys/mem.rs.
    unsafe {
        let mut basic = [0u64; 6];
        let mut c1: u32 = 12;
        if task_info(mach_task_self_, 20, basic.as_mut_ptr() as *mut i32, &mut c1) == 0 {
            res = basic[1];
        }
        let mut vm = [0u64; 48];
        let mut c2: u32 = 93;
        if task_info(mach_task_self_, 22, vm.as_mut_ptr() as *mut i32, &mut c2) == 0 && c2 >= 42 {
            fp = vm[18];
        }
    }
    (res as f64 / 1048576.0, fp as f64 / 1048576.0)
}

fn show(what: &str) {
    let (r, f) = mem();
    // SAFETY: documented to accept NULL (all zones) and 0 (everything).
    let relieved = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    let (r2, f2) = mem();
    println!(
        "{what:<58} resident {r:7.1} footprint {f:7.1} | after relief ({:5.1} MiB reported) resident {r2:7.1} footprint {f2:7.1}",
        relieved as f64 / 1048576.0
    );
}

fn fill(v: &mut [u8], seed: u8) {
    for (i, b) in v.iter_mut().enumerate().step_by(512) {
        *b = seed.wrapping_add(i as u8);
    }
    if let Some(l) = v.last_mut() {
        *l = seed;
    }
}

fn blocks(n: usize, size: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            let mut v = vec![0u8; size];
            fill(&mut v, i as u8);
            v
        })
        .collect()
}

fn zones() {
    // every zone and what each one's relief says, with a goal of 1 GiB and with 0
    // SAFETY: malloc_get_all_zones with a NULL reader reads this process's own zone table (documented for in-process use); the pointers it returns stay valid.
    unsafe {
        let mut addrs: *mut usize = std::ptr::null_mut();
        let mut n: u32 = 0;
        let rc = malloc_get_all_zones(mach_task_self_, std::ptr::null_mut(), &mut addrs, &mut n);
        println!(
            "malloc_get_all_zones rc {rc}, {n} zones, default zone {:p}",
            malloc_default_zone()
        );
        for i in 0..n as usize {
            let z = *addrs.add(i) as *mut std::ffi::c_void;
            let r0 = malloc_zone_pressure_relief(z, 0);
            let r1 = malloc_zone_pressure_relief(z, 1 << 30);
            println!("  zone {i} at {z:p}: relief(0) {r0}, relief(1 GiB) {r1}");
        }
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("zones") {
        let all = blocks(40_000, 6 * 1024);
        let big = blocks(31, 8 << 20);
        show("held: 6 KB x 40k + 8 MB x 31");
        drop(all);
        drop(big);
        show("all freed");
        zones();
        show("after per-zone relief");
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("delay") {
        // does the footprint fall by itself some time after a free? (6 KB blocks, then 8 MB blocks)
        for (name, size, n) in [
            ("6 KB x 40k", 6 * 1024usize, 40_000usize),
            ("8 MB x 31", 8 << 20, 31),
            ("200 KB x 1.2k", 200 << 10, 1250),
        ] {
            let all = blocks(n, size);
            show(&format!("{name}: allocated"));
            drop(all);
            for ms in [0u64, 100, 500, 1000, 3000, 6000] {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                let (r, f) = mem();
                println!("{name}: freed, +{ms:5} ms  resident {r:7.1} footprint {f:7.1}");
            }
            show(&format!("{name}: after the waits"));
        }
        return;
    }
    let which = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let sizes: [(&str, usize, usize); 6] = [
        ("100 B x 2.5M (250 MB)", 100, 2_500_000),
        ("1 KB x 250k (250 MB)", 1024, 250_000),
        ("6 KB x 40k (250 MB)", 6 * 1024, 40_000),
        ("30 KB x 8k (250 MB)", 30 * 1024, 8_000),
        ("200 KB x 1.2k (250 MB)", 200 * 1024, 1_250),
        ("8 MB x 31 (250 MB)", 8 << 20, 31),
    ];
    show("start");
    for (name, size, n) in sizes {
        if which != "all" && !name.starts_with(&which) {
            continue;
        }
        let all = blocks(n, size);
        show(&format!("{name}: allocated"));
        drop(all);
        show(&format!("{name}: all freed"));
        // half freed, interleaved with survivors
        let mut all = blocks(n, size);
        let keep: Vec<Vec<u8>> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 10 == 0)
            .map(|(_, v)| v.clone())
            .collect();
        all.clear();
        drop(all);
        show(&format!("{name}: 10% kept (new copies), 100% freed"));
        drop(keep);
        show(&format!("{name}: after the kept ones are freed"));
    }
}
