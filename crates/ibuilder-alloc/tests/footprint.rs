//! `BigBlocks` as the real global allocator of a process: a big `Vec` that is dropped leaves the process's `phys_footprint` at once (the property the chain's stage boundaries rely on;
//! libmalloc keeps such a block, `docs/chain/MEMORY-RUST.md` section 3). The kernel's number is read with `task_info`, as `ibuilder-pipeline`'s `sys/mem.rs` does. Mac / iOS only.

#![cfg(target_vendor = "apple")]
#![allow(unsafe_code)]

use ibuilder_alloc::BigBlocks;

#[global_allocator]
static GLOBAL: BigBlocks = BigBlocks;

unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
}

/// `phys_footprint` in bytes
fn footprint() -> u64 {
    let mut vm = [0u64; 48];
    let mut count: u32 = 93;
    // SAFETY: TASK_VM_INFO (22) into a buffer of 48 u64 (384 bytes, more than the 372 the kernel fills), `count` in 32-bit words as the kernel expects, this task's own port.
    let rc = unsafe { task_info(mach_task_self_, 22, vm.as_mut_ptr() as *mut i32, &mut count) };
    assert_eq!(rc, 0, "task_info");
    vm[18]
}

fn touch(v: &mut [u8]) {
    for i in (0..v.len()).step_by(4096) {
        v[i] = 1;
    }
}

const MIB: u64 = 1 << 20;

#[test]
fn a_dropped_big_buffer_leaves_the_footprint_and_a_small_one_is_malloc_s() {
    let before = footprint();
    let mut big = vec![0u8; 256 << 20];
    touch(&mut big);
    let held = footprint();
    assert!(
        held >= before + 200 * MIB,
        "the touched buffer is in the footprint: {before} -> {held}"
    );
    assert!(ibuilder_alloc::stats().live >= 256 << 20);
    drop(big);
    let after = footprint();
    assert!(
        after + 200 * MIB <= held,
        "the buffer left the footprint when it was dropped: {held} -> {after}"
    );
    // growth through realloc across the boundary keeps the data, and the final drop returns everything again
    let mut v: Vec<u8> = Vec::new();
    for i in 0..(40 << 20) {
        v.push((i % 251) as u8);
    }
    assert!(
        v.iter()
            .enumerate()
            .step_by(99991)
            .all(|(i, b)| *b == (i % 251) as u8)
    );
    drop(v);
    let end = footprint();
    assert!(
        end < before + 64 * MIB,
        "everything is back: {before} -> {end}"
    );
}
