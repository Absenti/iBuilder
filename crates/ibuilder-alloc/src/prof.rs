//! `prof`: the allocation profiler the memory work of `docs/chain/MEMORY-RUST.md` was done with (feature `profile`, for tools in `examples/`, never part of a product build).
//!
//! A counting `GlobalAlloc` (`Prof`) that
//!
//!   * keeps the live heap bytes and their peak (what the program asked for, as opposed to what the allocator keeps resident),
//!   * samples allocations by size (every allocation of 64 KiB or more, a smaller one with the probability size / 64 KiB: tcmalloc's scheme) and keeps the backtrace of the
//!     live ones, so that at the moment of the peak it can say *which call chains hold the live bytes* (the weights are unbiased estimates),
//!   * keeps a histogram of the live bytes by allocation size class, and the total of what was ever allocated,
//!   * can serve big blocks by `mmap` like [`crate::BigBlocks`] ([`set_big_threshold`]), to compare the two,
//!   * and [`Run`] samples `(time, live heap, resident, phys_footprint)` every 2 ms on a thread of its own.
//!
//! ```ignore
//! #[global_allocator] static A: ibuilder_alloc::prof::Prof = ibuilder_alloc::prof::Prof;
//! let run = ibuilder_alloc::prof::Run::start();      // ... the work (run.mark("text") puts a line on the timeline) ...
//! print!("{}", run.finish(30, 6, true));             // top 30 call chains of 6 frames, with the timeline
//! ```
//! Build with `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` to get source lines in the call chains.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const SAMPLE: usize = 64 << 10;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_SNAP: AtomicUsize = AtomicUsize::new(0);
static HIST: [AtomicUsize; 40] = [const { AtomicUsize::new(0) }; 40];

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
    static RNG: Cell<u64> = const { Cell::new(0x9E3779B97F4A7C15) };
}

struct Sampled {
    weight: usize,
    bt: usize,
}

struct Tracker {
    live: HashMap<usize, Sampled>,
    traces: Vec<Backtrace>,
    snap: Option<Snapshot>,
}

struct Snapshot {
    live_bytes: usize,
    at: f64,
    samples: Vec<(usize, usize)>, // (backtrace id, weight)
    hist: Vec<usize>,
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
static T0: Mutex<Option<Instant>> = Mutex::new(None);

fn class(size: usize) -> usize {
    (usize::BITS - size.max(1).leading_zeros()) as usize
}

fn rng() -> u64 {
    RNG.with(|r| {
        let mut x = r.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        r.set(x);
        x
    })
}

/// allocations of at least this size bypass malloc and use `mmap` / `munmap` (usize::MAX: none)
static BIG: AtomicUsize = AtomicUsize::new(usize::MAX);

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

fn is_big(l: &Layout) -> bool {
    l.size() >= BIG.load(Relaxed) && l.align() <= 16384
}

fn page_round(n: usize) -> usize {
    (n + 16383) & !16383
}

/// The profiling allocator: install it with `#[global_allocator]`
pub struct Prof;

impl Prof {
    fn on_alloc(ptr: *mut u8, size: usize) {
        let live = LIVE.fetch_add(size, Relaxed) + size;
        TOTAL.fetch_add(size, Relaxed);
        COUNT.fetch_add(1, Relaxed);
        HIST[class(size).min(39)].fetch_add(size, Relaxed);
        let peak = PEAK.load(Relaxed);
        let mut want_snap = false;
        if live > peak {
            PEAK.store(live, Relaxed);
            let last = LAST_SNAP.load(Relaxed);
            if live > last + (2 << 20) {
                want_snap = true;
            }
        }
        let sample = size >= SAMPLE || (rng() % SAMPLE as u64) < size as u64;
        if !sample && !want_snap {
            return;
        }
        IN_HOOK.with(|h| {
            if h.get() {
                return;
            }
            h.set(true);
            if let Ok(mut g) = TRACKER.try_lock()
                && let Some(t) = g.as_mut()
            {
                if sample {
                    let bt = Backtrace::force_capture();
                    t.traces.push(bt);
                    let id = t.traces.len() - 1;
                    t.live.insert(
                        ptr as usize,
                        Sampled {
                            weight: size.max(SAMPLE),
                            bt: id,
                        },
                    );
                }
                if want_snap {
                    let at = T0
                        .lock()
                        .ok()
                        .and_then(|x| *x)
                        .map(|i| i.elapsed().as_secs_f64())
                        .unwrap_or(0.0);
                    let samples = t.live.values().map(|s| (s.bt, s.weight)).collect();
                    let hist = HIST.iter().map(|h| h.load(Relaxed)).collect();
                    t.snap = Some(Snapshot {
                        live_bytes: live,
                        at,
                        samples,
                        hist,
                    });
                    LAST_SNAP.store(live, Relaxed);
                }
            }
            h.set(false);
        });
    }

    fn on_free(ptr: *mut u8, size: usize) {
        LIVE.fetch_sub(size, Relaxed);
        HIST[class(size).min(39)].fetch_sub(size, Relaxed);
        if size < 1 << 10 {
            // a tiny allocation is never sampled with a meaningful weight, but its probability was size / SAMPLE: look it up anyway (cheap miss)
        }
        IN_HOOK.with(|h| {
            if h.get() {
                return;
            }
            h.set(true);
            if let Ok(mut g) = TRACKER.try_lock()
                && let Some(t) = g.as_mut()
            {
                t.live.remove(&(ptr as usize));
            }
            h.set(false);
        });
    }
}

// SAFETY: every method forwards to `System` with the caller's layout and pointer, adding only counting around the call.
unsafe impl GlobalAlloc for Prof {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = if is_big(&l) {
            // SAFETY: an anonymous private mapping of a page-rounded length; MAP_FAILED is (-1) and is turned into a null pointer.
            let m = unsafe { mmap(std::ptr::null_mut(), page_round(l.size()), 3, 0x1002, -1, 0) };
            if m as isize == -1 {
                std::ptr::null_mut()
            } else {
                m
            }
        } else {
            unsafe { System.alloc(l) }
        };
        if !p.is_null() && !IN_HOOK.with(|h| h.get()) {
            Prof::on_alloc(p, l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if is_big(&l) {
            return unsafe { self.alloc(l) }; // a fresh mapping is zero
        }
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() && !IN_HOOK.with(|h| h.get()) {
            Prof::on_alloc(p, l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if !IN_HOOK.with(|h| h.get()) {
            Prof::on_free(p, l.size());
        }
        if is_big(&l) {
            // SAFETY: `p` came from `alloc` above with the same layout, so it is a mapping of page_round(size) bytes.
            unsafe { munmap(p, page_round(l.size())) };
        } else {
            unsafe { System.dealloc(p, l) }
        }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let hooked = !IN_HOOK.with(|h| h.get());
        let nl = Layout::from_size_align(new, l.align()).unwrap();
        let q = if is_big(&l) || is_big(&nl) {
            // a block that is (or becomes) a mapping: copy
            let q = unsafe { self.alloc_unhooked(nl) };
            if !q.is_null() {
                unsafe { std::ptr::copy_nonoverlapping(p, q, l.size().min(new)) };
                unsafe { self.dealloc_unhooked(p, l) };
            }
            q
        } else {
            unsafe { System.realloc(p, l, new) }
        };
        if !q.is_null() && hooked {
            Prof::on_free(p, l.size());
            Prof::on_alloc(q, new);
        }
        q
    }
}

impl Prof {
    unsafe fn alloc_unhooked(&self, l: Layout) -> *mut u8 {
        if is_big(&l) {
            // SAFETY: as in `alloc`.
            let m = unsafe { mmap(std::ptr::null_mut(), page_round(l.size()), 3, 0x1002, -1, 0) };
            if m as isize == -1 {
                std::ptr::null_mut()
            } else {
                m
            }
        } else {
            unsafe { System.alloc(l) }
        }
    }
    unsafe fn dealloc_unhooked(&self, p: *mut u8, l: Layout) {
        if is_big(&l) {
            // SAFETY: as in `dealloc`.
            unsafe { munmap(p, page_round(l.size())) };
        } else {
            unsafe { System.dealloc(p, l) }
        }
    }
}

#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
}

/// `(resident, phys_footprint)` from `task_info` (0, 0 where there is no Mach)
fn kernel_mem() -> (u64, u64) {
    #[allow(unused_mut)]
    let (mut res, mut fp) = (0u64, 0u64);
    #[cfg(target_vendor = "apple")]
    // SAFETY: the same calls and buffer sizes as ibuilder-pipeline's `sys/mem.rs` (documented flavors, buffers at least as large as the kernel fills, this task's own port).
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
    (res, fp)
}

fn mib(n: usize) -> f64 {
    n as f64 / 1_048_576.0
}

/// the allocation site of a backtrace: the frames that are not the allocator's or the standard library's own, each with its source line when the build has line tables
/// `{call chain: (weight, samples)}` heaviest first. The map's order is random per process, so equal weights are ordered by samples, then by the
/// chain itself: the same profile gives the same top-N on every run.
fn ranked(by: HashMap<String, (usize, usize)>) -> Vec<(String, (usize, usize))> {
    let mut rows: Vec<_> = by.into_iter().collect();
    rows.sort_by(|a, b| {
        b.1.0
            .cmp(&a.1.0)
            .then_with(|| b.1.1.cmp(&a.1.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    rows
}

fn key_of(bt: &Backtrace, depth: usize) -> String {
    let text = bt.to_string();
    let skip = [
        "alloc",
        "alloc_impl",
        "alloc_impl_runtime",
        "allocate",
        "allocate_zeroed",
        "alloc_zeroed",
        "realloc_nonnull",
        "grow_impl_runtime",
        "grow_impl",
        "grow",
        "from_elem",
        "with_capacity",
        "box_new_uninit",
        "reserve",
        "push",
        "finish_grow",
        "do_reserve_and_handle",
        "reserve_for_push",
        "try_allocate_in",
        "with_capacity_in",
        "to_vec",
        "clone",
        "extend_from_slice",
        "spec_extend",
        "extend_desugared",
        "from_iter",
        "next",
        "fold",
        "map",
        "call_once",
    ];
    let mut frames: Vec<String> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let l = line.trim();
        let Some((idx, name)) = l.split_once(": ") else {
            continue;
        };
        if !idx.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let at = match lines.peek() {
            Some(n) if n.trim_start().starts_with("at ") => lines
                .next()
                .unwrap()
                .trim()
                .trim_start_matches("at ")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        };
        let short = name
            .split('<')
            .next()
            .unwrap_or(name)
            .rsplit("::")
            .next()
            .unwrap_or(name);
        if skip.contains(&short) || name.contains("memprofile") || name.contains("__rust_") {
            continue;
        }
        frames.push(if at.is_empty() {
            short.to_string()
        } else {
            format!("{short}@{at}")
        });
        if frames.len() >= depth {
            break;
        }
    }
    frames.join(" < ")
}

/// From now on allocations of at least `bytes` bytes are served by `mmap` (the `BigBlocks` behaviour with another threshold); `usize::MAX` turns it off
pub fn set_big_threshold(bytes: usize) {
    BIG.store(bytes, Relaxed);
}

/// A profiled run: the tracker and the sampler thread
/// One sampler row: (seconds, a count, two byte figures).
type Sample = (f64, usize, u64, u64);

pub struct Run {
    t0: Instant,
    stop: Arc<AtomicBool>,
    sampler: Option<std::thread::JoinHandle<Vec<Sample>>>,
    marks: Mutex<Vec<(f64, usize, String)>>,
}

impl Run {
    pub fn start() -> Run {
        *TRACKER.lock().unwrap() = Some(Tracker {
            live: HashMap::new(),
            traces: Vec::new(),
            snap: None,
        });
        let t0 = Instant::now();
        *T0.lock().unwrap() = Some(t0);
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let sampler = std::thread::spawn(move || {
            let mut v = Vec::new();
            while !st.load(Relaxed) {
                let (r, f) = kernel_mem();
                v.push((t0.elapsed().as_secs_f64(), LIVE.load(Relaxed), r, f));
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            v
        });
        Run {
            t0,
            stop,
            sampler: Some(sampler),
            marks: Mutex::new(Vec::new()),
        }
    }

    /// A line on the timeline, with the live heap at that moment
    pub fn mark(&self, text: &str) {
        self.marks.lock().unwrap().push((
            self.t0.elapsed().as_secs_f64(),
            LIVE.load(Relaxed),
            text.chars().take(80).collect(),
        ));
    }

    /// Stops the sampler and renders the report: the heap and kernel numbers, the call chains that hold the live bytes at the peak, the size classes, and (`timeline`) the samples
    pub fn finish(mut self, top: usize, depth: usize, timeline: bool) -> String {
        use std::fmt::Write;
        let wall = self.t0.elapsed().as_secs_f64();
        self.stop.store(true, Relaxed);
        let line = self.sampler.take().unwrap().join().unwrap();
        let (rss_end, fp_end) = kernel_mem();
        let mut o = String::new();
        let _ = writeln!(o, "wall {wall:.2} s");
        let _ = writeln!(
            o,
            "heap: peak live {:.1} MiB, total ever allocated {:.1} MiB in {} allocations, live at the end {:.1} MiB",
            mib(PEAK.load(Relaxed)),
            mib(TOTAL.load(Relaxed)),
            COUNT.load(Relaxed),
            mib(LIVE.load(Relaxed))
        );
        let (pr, pf) = line
            .iter()
            .fold((0u64, 0u64), |a, s| (a.0.max(s.2), a.1.max(s.3)));
        let _ = writeln!(
            o,
            "kernel: sampled peak resident {:.1} MiB, sampled peak phys_footprint {:.1} MiB; at the end resident {:.1} MiB, footprint {:.1} MiB",
            pr as f64 / 1048576.0,
            pf as f64 / 1048576.0,
            rss_end as f64 / 1048576.0,
            fp_end as f64 / 1048576.0
        );
        if timeline {
            let _ = writeln!(
                o,
                "\ntimeline (every 25th sample): t s | live heap MiB | resident MiB | footprint MiB"
            );
            for (i, s) in line.iter().enumerate() {
                if i % 25 == 0 {
                    let _ = writeln!(
                        o,
                        "{:7.3} | {:8.1} | {:8.1} | {:8.1}",
                        s.0,
                        mib(s.1),
                        s.2 as f64 / 1048576.0,
                        s.3 as f64 / 1048576.0
                    );
                }
            }
            let _ = writeln!(o, "\nmarks: t s | live heap MiB | text");
            for (t, l, s) in self.marks.lock().unwrap().iter() {
                let _ = writeln!(o, "{t:7.3} | {:8.1} | {s}", mib(*l));
            }
        }
        let t = TRACKER.lock().unwrap().take();
        if let Some(t) = t
            && let Some(s) = &t.snap
        {
            let _ = writeln!(
                o,
                "\nat the peak (t = {:.3} s, live heap {:.1} MiB), live bytes by the call chain that allocated them (sampled, weights are estimates):",
                s.at,
                mib(s.live_bytes)
            );
            let mut by: HashMap<String, (usize, usize)> = HashMap::new();
            let mut keyed: HashMap<usize, String> = HashMap::new();
            let mut est = 0usize;
            for (bt, w) in &s.samples {
                let k = keyed
                    .entry(*bt)
                    .or_insert_with(|| key_of(&t.traces[*bt], depth))
                    .clone();
                let e = by.entry(k).or_insert((0, 0));
                e.0 += w;
                e.1 += 1;
                est += w;
            }
            let rows = ranked(by);
            let _ = writeln!(
                o,
                "(sampled total {:.1} MiB of {:.1} MiB live)",
                mib(est),
                mib(s.live_bytes)
            );
            for (k, (w, n)) in rows.iter().take(top) {
                let _ = writeln!(o, "{:8.1} MiB  {:6} samples  {k}", mib(*w), n);
            }
            let _ = writeln!(
                o,
                "\nlive bytes by allocation size class at the peak (size in [2^(c-1), 2^c)):"
            );
            for (c, b) in s.hist.iter().enumerate() {
                if *b > (1 << 20) {
                    let _ = writeln!(o, "  < {:>12} B: {:8.1} MiB", 1usize << c.min(62), mib(*b));
                }
            }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 2026-11-20 §5.18: rows of equal weight came out in the HashMap's order, which differs from one map (and one process) to the next
    #[test]
    fn equal_weights_are_ranked_the_same_whatever_the_map_order() {
        // the same 32 rows in a new map each time (each HashMap has its own random keys, so its own order): three weights, many ties,
        // the sample counts breaking some of them
        let rows = || {
            let mut by = HashMap::new();
            for i in 0..32usize {
                by.insert(format!("chain {i:02}"), ((i % 3) << 20, i % 2));
            }
            ranked(by)
        };
        let first = rows();
        for m in 0..32 {
            assert_eq!(rows(), first, "a different order for map {m}");
        }
        assert!(first.windows(2).all(|w| {
            let ((ka, (wa, na)), (kb, (wb, nb))) = (&w[0], &w[1]);
            wa > wb || (wa == wb && (na > nb || (na == nb && ka < kb)))
        }));
    }
}
