//! `shadersplit`: the Mac CLI of the split (the Python's `shader_split.py <RomFS> <main> <pkg> <map> <facts> <pairs> <pso_pairs> [dlc]` with the profile folder).
//!
//! ```text
//! shadersplit --romfs RomFS.img --main ExeFS/main --pkg out/pkg --tables out/tables --profile <folder with <title id>/game.tsv> --game <title id>
//!             [--jobs 8] [--memo <folder>] [--dlc a.img,b.img] [--update-romfs RomFS2 --update-main main2] [--glslc-capture dir] [--nro-dir dir]
//!             [--registered shader_map.tsv] [--record split.record]
//! ```

use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use std::path::PathBuf;

/// The same allocator the `ibuilder` CLI runs with: big blocks go back to the kernel when freed (docs/chain/MEMORY-RUST.md). `--features system-alloc` turns it off, to measure what it saves.
#[cfg(not(feature = "system-alloc"))]
#[global_allocator]
static GLOBAL: ibuilder_alloc::BigBlocks = ibuilder_alloc::BigBlocks;

fn main() {
    let mut a = std::env::args().skip(1);
    let (mut romfs, mut main_nso, mut pkg, mut tables, mut profile, mut game) =
        (None, None, None, None, None, None);
    let (mut jobs, mut memo, mut dlc, mut cap, mut nro) = (
        8usize,
        None::<PathBuf>,
        Vec::<PathBuf>::new(),
        None::<PathBuf>,
        None::<PathBuf>,
    );
    let (mut up_romfs, mut up_main) = (Vec::<PathBuf>::new(), Vec::<PathBuf>::new());
    let (mut registered, mut record) = (None::<PathBuf>, None::<PathBuf>);
    while let Some(k) = a.next() {
        let mut v = || {
            a.next()
                .unwrap_or_else(|| usage(&format!("{k} needs a value")))
        };
        match k.as_str() {
            "--romfs" => romfs = Some(PathBuf::from(v())),
            "--main" => main_nso = Some(PathBuf::from(v())),
            "--pkg" => pkg = Some(PathBuf::from(v())),
            "--tables" => tables = Some(PathBuf::from(v())),
            "--profile" => profile = Some(PathBuf::from(v())),
            "--game" => game = Some(v()),
            "--jobs" => {
                jobs = v()
                    .parse()
                    .unwrap_or_else(|_| usage("--jobs takes a number"))
            }
            "--memo" => memo = Some(PathBuf::from(v())),
            "--dlc" => {
                dlc = v()
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .collect()
            }
            "--update-romfs" => up_romfs.push(PathBuf::from(v())),
            "--update-main" => up_main.push(PathBuf::from(v())),
            "--glslc-capture" => cap = Some(PathBuf::from(v())),
            "--nro-dir" => nro = Some(PathBuf::from(v())),
            "--registered" => registered = Some(PathBuf::from(v())),
            "--record" => record = Some(PathBuf::from(v())),
            _ => usage(&format!("unknown argument {k}")),
        }
    }
    let need =
        |o: Option<PathBuf>, n: &str| o.unwrap_or_else(|| usage(&format!("{n} is required")));
    let (romfs, main_nso, pkg, tables, profile) = (
        need(romfs, "--romfs"),
        need(main_nso, "--main"),
        need(pkg, "--pkg"),
        need(tables, "--tables"),
        need(profile, "--profile"),
    );
    let game = game.unwrap_or_else(|| usage("--game is required"));
    if up_romfs.len() != up_main.len() {
        usage("--update-romfs and --update-main pair one to one");
    }
    let mut o = SplitOptions::single(&romfs, &main_nso, &pkg, &tables, &profile, &game);
    for (r, m) in up_romfs.iter().zip(&up_main) {
        o.versions.push((r, m));
    }
    o.dlc = dlc.iter().map(|p| p.as_path()).collect();
    o.jobs = jobs;
    o.memo_dir = memo.as_deref();
    o.glslc_capture = cap.as_deref();
    o.nro_dir = nro.as_deref();
    o.registered = registered.as_deref();
    o.record = record.as_deref();
    let t0 = std::time::Instant::now();
    let log = |l: &str| println!("{l}");
    match split(&o, &Ruzstd, &NoGate, &log) {
        Ok(s) => {
            println!(
                "shaders {} wall {:.1} s ({} jobs reused, {} ran, {} unreadable files)",
                s.shaders,
                t0.elapsed().as_secs_f64(),
                s.jobs_reused,
                s.jobs_ran,
                s.unreadable
            );
        }
        Err(e) => {
            eprintln!("[RED] {e}");
            std::process::exit(1);
        }
    }
}

fn usage(why: &str) -> ! {
    eprintln!(
        "shadersplit: {why}\nusage: shadersplit --romfs <RomFS.img|folder> --main <main NSO> --pkg <dir> --tables <dir> --profile <dir> --game <title id> [--jobs N] [--memo dir] [--dlc a,b] [--update-romfs R --update-main M] [--glslc-capture dir] [--nro-dir dir] [--registered shader_map.tsv] [--record split.record]"
    );
    std::process::exit(2);
}
