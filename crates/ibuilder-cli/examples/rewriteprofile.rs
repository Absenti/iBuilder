//! `rewriteprofile`: the allocation profile of one module's rewrite as the CLI runs it (`adapters::RustRewriter`): where the live heap is at its peak and what the kernel
//! counts (docs/chain/MEMORY-RUST.md). The profiler is `ibuilder_alloc::prof`.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo run --release -p ibuilder-cli --example rewriteprofile -- \
//!     --src ExeFS/main --out <dir> --module main --game 0100DFA027504000 --games <derived profiles dir> --games <engine>/dm/games [--threads 4] [--top 30] [--depth 6] [--bigmmap BYTES] [--timeline] [--mmap]
//! ```
//! `--mmap` reads the module through a file mapping instead of `fs::read` (what a change of the adapter would do); `--bigmmap BYTES` serves blocks of at least that size by `mmap`.

use ibuilder_alloc::prof::{self, Prof, Run};
use std::io::Write;
use std::path::PathBuf;

#[global_allocator]
static A: Prof = Prof;

fn main() {
    let mut a = std::env::args().skip(1);
    let (mut src, mut out, mut module, mut game) =
        (None, None, String::from("main"), None::<String>);
    let (mut games, mut threads, mut top, mut depth) =
        (Vec::<PathBuf>::new(), 4usize, 30usize, 6usize);
    let (mut timeline, mut mapped) = (false, false);
    while let Some(k) = a.next() {
        let mut v = || a.next().expect("a value");
        match k.as_str() {
            "--src" => src = Some(PathBuf::from(v())),
            "--out" => out = Some(PathBuf::from(v())),
            "--module" => module = v(),
            "--game" => game = Some(v()),
            "--games" => games.push(PathBuf::from(v())),
            "--threads" => threads = v().parse().unwrap(),
            "--top" => top = v().parse().unwrap(),
            "--depth" => depth = v().parse().unwrap(),
            "--bigmmap" => prof::set_big_threshold(v().parse().unwrap()),
            "--timeline" => timeline = true,
            "--mmap" => mapped = true,
            _ => panic!("unknown argument {k}"),
        }
    }
    let (src, out) = (src.unwrap(), out.unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let run = Run::start();
    let opts = ibuilder_rewrite::Options {
        workers: threads.max(1),
        game,
        games_dirs: games,
        ..Default::default()
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .unwrap();
    let name = src.to_string_lossy().into_owned();
    run.mark("start");
    let r = if mapped {
        let f = std::fs::File::open(&src).unwrap();
        // SAFETY: a read-only mapping of the module, which nothing writes while this tool runs (the same as `ibuilder_nso::read`).
        let m = unsafe { memmap2::Mmap::map(&f) }.unwrap();
        pool.install(|| ibuilder_rewrite::rewrite(&m, &name, &module, &opts))
            .map_err(|e| e.msg)
            .unwrap()
    } else {
        let data = std::fs::read(&src).unwrap();
        let r = pool
            .install(|| ibuilder_rewrite::rewrite(&data, &name, &module, &opts))
            .map_err(|e| e.msg)
            .unwrap();
        drop(data);
        r
    };
    run.mark("rewritten");
    {
        let f = std::fs::File::create(out.join(format!("{module}.patched.bin"))).unwrap();
        let mut bw = std::io::BufWriter::with_capacity(1 << 20, f);
        r.write_patched(&mut bw).unwrap();
        bw.flush().unwrap();
    }
    run.mark("written");
    drop(r);
    println!(
        "rewrite {module}: {threads} threads, {}",
        if mapped {
            "mapped input"
        } else {
            "fs::read input"
        }
    );
    print!("{}", run.finish(top, depth, timeline));
}
