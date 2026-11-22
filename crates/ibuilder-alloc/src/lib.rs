//! # ibuilder-alloc
//!
//! `BigBlocks`: a `#[global_allocator]` that keeps **big blocks** out of malloc's hands. On macOS and iOS a block of 128 KiB or more that Rust frees stays in the process's
//! `phys_footprint` (the number jetsam counts) because libmalloc keeps it for reuse, and `malloc_zone_pressure_relief` does not give it back (measured, `docs/chain/MEMORY-RUST.md`
//! section 3: it reports 0 MiB in every case tried). A chain that reads a 100 MB file, decodes it, drops it and goes on to the next stage therefore carries those buffers for
//! the rest of the run. `BigBlocks` serves every allocation of at least [`BIG_BLOCK`] bytes with an anonymous `mmap` and frees it with `munmap`, so the pages are gone the
//! moment the `Vec` is dropped; everything smaller goes to the system allocator unchanged.
//!
//! ```ignore
//! #[global_allocator]
//! static GLOBAL: ibuilder_alloc::BigBlocks = ibuilder_alloc::BigBlocks;
//! ```
//!
//! * **Where it applies**: `target_vendor = "apple"` (macOS, iOS, the simulators). Everywhere else `BigBlocks` is the system allocator, byte for byte (glibc and Android's
//!   allocator already hand big blocks back with `munmap`), so a Linux CI run is unaffected.
//! * **Correctness**: the size class of a block is a function of its `Layout` alone, so `dealloc` and `realloc` always know which allocator made it. A block whose alignment is
//!   larger than a page is never mapped (it falls back to the system allocator). A mapping is zero-filled, so `alloc_zeroed` needs no `memset`.
//! * **Cost**: one `mmap` and one `munmap` per big block (a few microseconds each) and the page faults of fresh memory, which malloc's cache of freed blocks would sometimes
//!   have saved. `realloc` of a block that is, or becomes, big copies it (there is no `mremap` on Darwin); `Vec` growth is geometric, so the copies add up to about twice the final size.
//! * **Not a leak detector**: [`stats`] only counts the mapped bytes, for tests and for the chain's memory line.

#![deny(unsafe_code)]

#[cfg(feature = "profile")]
pub mod prof;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Blocks of at least this many bytes are served by `mmap`. 128 KiB is about where libmalloc's own large-block path starts: below it blocks live in size-class regions, which are returned
/// to the system only when a whole region is empty (and which the chain does not churn the way it churns big buffers); measured thresholds from 8 KiB to 1 MiB: docs/chain/MEMORY-RUST.md section 3.
pub const BIG_BLOCK: usize = 128 << 10;

/// The granule a mapping is rounded up to (the page size of Apple's arm64; `mmap` itself rounds to the real page size, this keeps `len` the same at `munmap`).
const GRANULE: usize = 16 << 10;

/// The largest alignment a mapping gives: the page size of the CPU (16 KiB on Apple's arm64, 4 KiB on an Intel Mac)
const MAX_ALIGN: usize = if cfg!(target_arch = "aarch64") {
    16 << 10
} else {
    4 << 10
};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static MAPS: AtomicUsize = AtomicUsize::new(0);

/// What `BigBlocks` has mapped: bytes in live big blocks now, their peak, and how many mappings were made so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BigStats {
    pub live: usize,
    pub peak: usize,
    pub maps: usize,
}

pub fn stats() -> BigStats {
    BigStats {
        live: LIVE.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
        maps: MAPS.load(Ordering::Relaxed),
    }
}

/// Is a block of this layout served by `mmap`?
#[inline]
fn is_big(l: &Layout) -> bool {
    cfg!(target_vendor = "apple") && l.size() >= BIG_BLOCK && l.align() <= MAX_ALIGN
}

#[inline]
fn mapped_len(size: usize) -> usize {
    (size + GRANULE - 1) & !(GRANULE - 1)
}

/// The global allocator: see the crate documentation.
pub struct BigBlocks;

#[allow(unsafe_code)]
mod map {
    use super::*;

    /// A fresh zero-filled private mapping of `mapped_len(size)` bytes, or null.
    pub unsafe fn alloc(size: usize) -> *mut u8 {
        let len = mapped_len(size);
        // SAFETY: an anonymous private mapping with no address hint: it cannot overlap or replace anything; MAP_FAILED is turned into a null pointer, which `GlobalAlloc` reports as out of memory.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return std::ptr::null_mut();
        }
        let live = LIVE.fetch_add(len, Ordering::Relaxed) + len;
        PEAK.fetch_max(live, Ordering::Relaxed);
        MAPS.fetch_add(1, Ordering::Relaxed);
        p as *mut u8
    }

    /// Unmaps a block `alloc` made for `size` bytes.
    pub unsafe fn free(p: *mut u8, size: usize) {
        let len = mapped_len(size);
        // SAFETY: `p` and `size` are the pair `alloc` was called with (the layout is the same at free time), so this is exactly one whole mapping of this process.
        unsafe { libc::munmap(p as *mut libc::c_void, len) };
        LIVE.fetch_sub(len, Ordering::Relaxed);
    }
}

// SAFETY: `GlobalAlloc` requires that the returned block be valid for `layout` until freed with the same layout, that a null return means failure, and that `realloc` keeps
// the contents. Big blocks are whole `mmap`ed regions (page aligned, at least `layout.size()` bytes, zero-filled), small blocks are the system allocator's; which of the two a
// block is, is decided from its `Layout` only (`is_big`), and every method decides the same way for the same layout, so a block is always freed by the allocator that made it.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for BigBlocks {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if is_big(&l) {
            // SAFETY: see `map::alloc`.
            unsafe { map::alloc(l.size()) }
        } else {
            // SAFETY: forwarded unchanged.
            unsafe { System.alloc(l) }
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if is_big(&l) {
            // SAFETY: a fresh mapping is already zero.
            unsafe { map::alloc(l.size()) }
        } else {
            // SAFETY: forwarded unchanged.
            unsafe { System.alloc_zeroed(l) }
        }
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if is_big(&l) {
            // SAFETY: `p` was returned by `alloc` / `alloc_zeroed` / `realloc` for this layout, i.e. it is a mapping of `mapped_len(l.size())` bytes.
            unsafe { map::free(p, l.size()) }
        } else {
            // SAFETY: forwarded unchanged.
            unsafe { System.dealloc(p, l) }
        }
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller guarantees `new_size` is non-zero and does not overflow when rounded to `l.align()`, so this layout is valid.
        let nl = unsafe { Layout::from_size_align_unchecked(new_size, l.align()) };
        if !is_big(&l) && !is_big(&nl) {
            // SAFETY: forwarded unchanged: both sizes are the system allocator's.
            return unsafe { System.realloc(p, l, new_size) };
        }
        if is_big(&l) && is_big(&nl) && mapped_len(l.size()) == mapped_len(new_size) {
            return p; // the same mapping already holds the new size
        }
        // SAFETY: a new block for the new layout (either allocator), the common prefix copied between two distinct live blocks, then the old block freed by the allocator that made it.
        unsafe {
            let q = self.alloc(nl);
            if !q.is_null() {
                std::ptr::copy_nonoverlapping(p, q, l.size().min(new_size));
                self.dealloc(p, l);
            }
            q
        }
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;

    #[test]
    fn sizes_map_to_the_two_allocators_by_layout_alone() {
        let l = |n: usize, a: usize| Layout::from_size_align(n, a).unwrap();
        assert!(!is_big(&l(BIG_BLOCK - 1, 8)));
        assert_eq!(is_big(&l(BIG_BLOCK, 8)), cfg!(target_vendor = "apple"));
        assert!(
            !is_big(&l(1 << 20, MAX_ALIGN * 2)),
            "a block aligned beyond a page is the system allocator's"
        );
        assert_eq!(mapped_len(1), GRANULE);
        assert_eq!(mapped_len(GRANULE), GRANULE);
        assert_eq!(mapped_len(GRANULE + 1), 2 * GRANULE);
    }

    /// The allocator used directly (not as the test binary's global one): every way a block can be made, grown, shrunk and freed keeps its contents and its zeros.
    #[test]
    fn alloc_zeroed_realloc_and_dealloc_keep_contents() {
        let a = BigBlocks;
        // SAFETY: each pointer is used only with the layout it was made with (or the layout passed to `realloc`) and freed once.
        unsafe {
            for size in [
                100usize,
                BIG_BLOCK - 1,
                BIG_BLOCK,
                BIG_BLOCK + 1,
                3 * BIG_BLOCK,
                5 << 20,
            ] {
                let l = Layout::from_size_align(size, 16).unwrap();
                let p = a.alloc_zeroed(l);
                assert!(!p.is_null());
                assert!(
                    (0..size).step_by(4099).all(|i| *p.add(i) == 0),
                    "zero-filled: {size}"
                );
                for i in (0..size).step_by(4099) {
                    *p.add(i) = (i % 251) as u8;
                }
                // grow across the boundary, then shrink back below it, then free
                let bigger = size * 3 + 7;
                let q = a.realloc(p, l, bigger);
                assert!(!q.is_null());
                assert!(
                    (0..size)
                        .step_by(4099)
                        .all(|i| *q.add(i) == (i % 251) as u8),
                    "grown: {size}"
                );
                let l2 = Layout::from_size_align(bigger, 16).unwrap();
                let r = a.realloc(q, l2, 64);
                assert!(!r.is_null());
                assert_eq!(*r, 0);
                a.dealloc(r, Layout::from_size_align(64, 16).unwrap());
            }
        }
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn a_big_block_is_counted_while_it_lives_and_not_after() {
        let a = BigBlocks;
        let before = stats();
        let l = Layout::from_size_align(4 << 20, 8).unwrap();
        // SAFETY: allocated and freed once with the same layout.
        unsafe {
            let p = a.alloc(l);
            assert!(!p.is_null());
            let during = stats();
            assert!(
                during.live >= before.live + (4 << 20),
                "{before:?} {during:?}"
            );
            assert!(during.maps > before.maps);
            a.dealloc(p, l);
        }
        // other tests run in this process: only the direction can be asserted, not the absolute value
        assert!(stats().maps > before.maps);
    }
}
