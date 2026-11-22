//! `memprofile`: the allocation profile of the Grizzy split (docs/chain/MEMORY-RUST.md): where the live heap is at its peak, the live heap against the kernel's resident size and
//! `phys_footprint` over time. The profiler is `ibuilder_alloc::prof` (a counting global allocator with sampled backtraces).
//!
//! ```text
//! CARGO_TARGET_DIR=<dir> CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo run --release -p ibuilder-shadersplit --example memprofile --features profile -- \
//!     --romfs RomFS.img --main main --pkg out/pkg --tables out/tables --profile <profile> --game 0100DFA027504000 [--jobs 6] [--top 30] [--depth 6] [--memo dir] [--timeline] [--bigmmap BYTES]
//! ```
//! `--bigmmap BYTES` serves blocks of at least that size by `mmap` (what `ibuilder_alloc::BigBlocks` does at 131072); without it the system allocator runs, as in the first measurements.

use ibuilder_alloc::prof::{self, Prof, Run};
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use std::path::PathBuf;

#[global_allocator]
static A: Prof = Prof;

fn main() {
    let mut a = std::env::args().skip(1);
    let (mut romfs, mut main_nso, mut pkg, mut tables, mut profile, mut game) =
        (None, None, None, None, None, None);
    let (mut jobs, mut top, mut depth) = (6usize, 30usize, 5usize);
    let mut memo: Option<PathBuf> = None;
    let mut timeline = false;
    while let Some(k) = a.next() {
        let mut v = || a.next().expect("a value");
        match k.as_str() {
            "--romfs" => romfs = Some(PathBuf::from(v())),
            "--main" => main_nso = Some(PathBuf::from(v())),
            "--pkg" => pkg = Some(PathBuf::from(v())),
            "--tables" => tables = Some(PathBuf::from(v())),
            "--profile" => profile = Some(PathBuf::from(v())),
            "--game" => game = Some(v()),
            "--jobs" => jobs = v().parse().unwrap(),
            "--top" => top = v().parse().unwrap(),
            "--depth" => depth = v().parse().unwrap(),
            "--memo" => memo = Some(PathBuf::from(v())),
            "--timeline" => timeline = true,
            "--bigmmap" => prof::set_big_threshold(v().parse().unwrap()),
            _ => panic!("unknown argument {k}"),
        }
    }
    let (romfs, main_nso, pkg, tables, profile, game) = (
        romfs.unwrap(),
        main_nso.unwrap(),
        pkg.unwrap(),
        tables.unwrap(),
        profile.unwrap(),
        game.unwrap(),
    );
    let mut o = SplitOptions::single(&romfs, &main_nso, &pkg, &tables, &profile, &game);
    o.jobs = jobs;
    o.memo_dir = memo.as_deref();
    let run = Run::start();
    let log = |l: &str| run.mark(l);
    let stats = split(&o, &Ruzstd, &NoGate, &log).expect("the split");
    println!("split: {} shaders, jobs {jobs}", stats.shaders);
    print!("{}", run.finish(top, depth, timeline));
}
