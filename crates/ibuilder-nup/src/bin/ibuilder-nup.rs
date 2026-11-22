//! `ibuilder-nup`: the Mac tool over the NUP reader. Subcommands:
//!
//! ```text
//! ibuilder-nup info FILE                       the packages, versions, DLC, sizes, counts
//! ibuilder-nup extract FILE --out DIR [--aoc DIR] [--version N] [--program N] [--threads N] [--exefs-only] [--ruzstd]
//! ibuilder-nup verify FILE [--threads N] [--ruzstd]    every file of the container against its SHA-256, nothing written
//! ibuilder-nup bench-zstd FILE [N]              the C libzstd and ruzstd on the first N zstd files of the container
//! ibuilder-nup formats RomFS.img                the image's .zs files that do not start with a zstd frame (romfs_image.format_mismatches)
//! ```

use ibuilder_nup::{
    Nup, Options, Progress, Ruzstd, ZstdBackend, extract_application, pack_exefs_only, scan,
    verify_container,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "usage: ibuilder-nup info FILE | extract FILE --out DIR [--aoc DIR] [--version N] [--program N] [--threads N] [--exefs-only] [--ruzstd] | verify FILE [--threads N] [--ruzstd] | bench-zstd FILE [N]"
    );
    std::process::exit(2);
}

fn gib(b: u64) -> f64 {
    b as f64 / (1u64 << 30) as f64
}

fn open(path: &Path, ruzstd: bool) -> Nup {
    let n = match Nup::open(path) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("[RED] {e}");
            std::process::exit(1);
        }
    };
    if ruzstd {
        n.with_backend(Arc::new(Ruzstd))
    } else {
        n
    }
}

fn options(threads: usize) -> Options {
    let t0 = Instant::now();
    let last = Arc::new(Mutex::new((String::new(), 0u64)));
    let mut o = Options {
        threads,
        ..Options::default()
    };
    o.say = Some(Arc::new(|s| println!("{s}")));
    o.progress = Some(Arc::new(move |p: &Progress| {
        let mut l = last.lock().unwrap();
        let now = t0.elapsed().as_secs();
        if l.0 == p.step && now == l.1 && p.done_bytes < p.total_bytes {
            return;
        }
        *l = (p.step.clone(), now);
        let rate = if now > 0 {
            p.done_bytes as f64 / now as f64 / 1e6
        } else {
            0.0
        };
        eprintln!(
            "  [{:>5}s] {}: {:.2}/{:.2} GiB, {}/{} files ({:.0} MB/s)",
            now,
            p.step,
            gib(p.done_bytes),
            gib(p.total_bytes),
            p.done_files,
            p.total_files,
            rate
        );
    }));
    o
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage();
    }
    let cmd = args[0].as_str();
    let file = PathBuf::from(&args[1]);
    let mut out: Option<PathBuf> = None;
    let mut aoc: Option<PathBuf> = None;
    let (mut version, mut program, mut threads) = (None::<u32>, None::<u32>, 0usize);
    let (mut exefs_only, mut ruzstd) = (false, false);
    let mut rest: Vec<String> = vec![];
    let mut i = 2;
    while i < args.len() {
        let a = args[i].as_str();
        let val = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| usage())
        };
        match a {
            "--out" => out = Some(PathBuf::from(val(&mut i))),
            "--aoc" => aoc = Some(PathBuf::from(val(&mut i))),
            "--version" => version = Some(val(&mut i).parse().unwrap_or_else(|_| usage())),
            "--program" => program = Some(val(&mut i).parse().unwrap_or_else(|_| usage())),
            "--threads" => threads = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--exefs-only" => exefs_only = true,
            "--ruzstd" => ruzstd = true,
            _ => rest.push(a.to_string()),
        }
        i += 1;
    }
    let t0 = Instant::now();
    let code = match cmd {
        "info" => info(&open(&file, ruzstd)),
        "extract" => {
            let Some(out) = out else { usage() };
            let n = open(&file, ruzstd);
            println!("zstd backend: {}", n.backend_name());
            let opts = options(threads);
            let r = if exefs_only {
                scan(&n, program)
                    .and_then(|sc| {
                        let ver = version.unwrap_or_else(|| {
                            sc.versions.keys().next_back().copied().unwrap_or(0)
                        });
                        let ki = if ver != 0 {
                            sc.versions.get(&ver).copied()
                        } else {
                            sc.base
                        };
                        let Some(ki) = ki else {
                            return Err(ibuilder_nup::Error::Msg(format!("no version {ver}")));
                        };
                        pack_exefs_only(
                            &n,
                            &sc,
                            &n.packages[ki],
                            sc.base.map(|b| &n.packages[b]),
                            &out,
                            &opts,
                        )
                    })
                    .map(|_| println!("ExeFS and control data written to {}", out.display()))
            } else {
                extract_application(
                    &file,
                    &out,
                    aoc.as_ref()
                        .map(|a| {
                            Box::new({
                                let a = a.clone();
                                move |_: &str| a.clone()
                            }) as Box<dyn Fn(&str) -> PathBuf>
                        })
                        .as_deref(),
                    program,
                    version,
                    Some(n),
                    &opts,
                )
                .map(|x| {
                    println!(
                        "{}: version {} ({}), image {}, {} DLC",
                        x.title_id,
                        x.version,
                        x.package,
                        x.how.as_str(),
                        x.dlc
                    );
                })
            };
            match r {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("[RED] {e}");
                    1
                }
            }
        }
        "verify" => {
            let n = open(&file, ruzstd);
            println!("zstd backend: {}", n.backend_name());
            match verify_container(&n, &options(threads)) {
                Ok((files, bytes)) => {
                    println!(
                        "{files} files, {:.2} GiB: every file matches its size and SHA-256",
                        gib(bytes)
                    );
                    0
                }
                Err(e) => {
                    eprintln!("[RED] {e}");
                    1
                }
            }
        }
        "formats" => match ibuilder_nup::romfs::format_mismatches(&file) {
            Ok((looked, bad)) => {
                println!(
                    "{looked} files whose names promise a format looked at, {} that do not start as promised",
                    bad.len()
                );
                for (p, sz, b) in bad {
                    println!("  {p}: {sz} bytes, first bytes {:02x?}", b);
                }
                0
            }
            Err(e) => {
                eprintln!("[RED] {e}");
                1
            }
        },
        "bench-zstd" => {
            let nmax: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(200);
            bench(&open(&file, false), nmax)
        }
        _ => usage(),
    };
    eprintln!("wall {:.1} s", t0.elapsed().as_secs_f64());
    std::process::exit(code);
}

fn info(n: &Nup) -> i32 {
    println!(
        "{}: {} bytes, {} packages, {} folders, {} files, toc sha {}",
        n.label,
        n.fsize,
        n.packages.len(),
        n.dir_count(),
        n.file_count(),
        n.toc_sha
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    for k in &n.packages {
        println!(
            "  package {}: {} title {:016X} version {} kind {} flags {} root {} exefs {:#x} romfs {:#x} meta {} B",
            k.index,
            k.name,
            k.title_id,
            k.version,
            k.kind,
            k.flags,
            k.root,
            k.exefs,
            k.romfs,
            k.meta_size
        );
        if let Ok(m) = n.meta(k) {
            let list: Vec<String> = m
                .iter()
                .map(|(t, d)| format!("{t:#x}:{}B", d.len()))
                .collect();
            println!("    meta records: {}", list.join(" "));
        }
        println!(
            "    tree: {:.2} GiB; romfs: {:.2} GiB",
            gib(n.tree_bytes(k.root)),
            if k.romfs != ibuilder_nup::NO_DIR {
                gib(n.tree_bytes(k.romfs))
            } else {
                0.0
            }
        );
        if k.exefs != ibuilder_nup::NO_DIR {
            for c in n.children(k.exefs) {
                if !c.is_dir {
                    let (eo, es, codec) = n.file_extent(c.index);
                    println!(
                        "    exefs/{} {} B (stored {} B at {:#x}, codec {})",
                        String::from_utf8_lossy(c.name),
                        n.file_size(c.index),
                        es,
                        eo,
                        codec
                    );
                }
            }
        }
    }
    let (mut raw, mut z, mut rb, mut zb, mut zs) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for i in 0..n.file_count() {
        let (_, es, codec) = n.file_extent(i);
        if codec == 0 {
            raw += 1;
            rb += n.file_size(i);
        } else {
            z += 1;
            zb += n.file_size(i);
            zs += es;
        }
    }
    println!(
        "files: {raw} raw ({:.2} GiB), {z} zstd ({:.2} GiB stored as {:.2} GiB)",
        gib(rb),
        gib(zb),
        gib(zs)
    );
    match scan(n, None) {
        Ok(sc) => println!(
            "scan: program {:016X} of application {:016X}, base {}, versions {:?}, {} DLC, aoc base {:016X}, siblings {}",
            sc.program_id,
            sc.application,
            sc.base.is_some(),
            sc.versions.keys().collect::<Vec<_>>(),
            sc.dlc.len(),
            sc.aoc_base,
            sc.siblings.len()
        ),
        Err(e) => println!("scan: {e}"),
    }
    0
}

fn bench(n: &Nup, nmax: usize) -> i32 {
    use std::sync::atomic::AtomicBool;
    // the zstd files of the container by decreasing size, the first nmax
    let mut zs: Vec<usize> = (0..n.file_count())
        .filter(|&i| n.file_extent(i).2 == 1 && n.file_size(i) > 0)
        .collect();
    zs.sort_by_key(|&i| std::cmp::Reverse(n.file_size(i)));
    zs.truncate(nmax);
    let bytes: u64 = zs.iter().map(|&i| n.file_size(i)).sum();
    let stored: u64 = zs.iter().map(|&i| n.file_extent(i).1).sum();
    println!(
        "{} zstd files, {:.2} GiB uncompressed ({:.2} GiB stored)",
        zs.len(),
        gib(bytes),
        gib(stored)
    );
    let no = AtomicBool::new(false);
    for (name, rz) in [("libzstd (C)", false), ("ruzstd (pure Rust)", true)] {
        let backend: Arc<dyn ZstdBackend> = if rz {
            Arc::new(Ruzstd)
        } else {
            ibuilder_nup::default_backend()
        };
        let m = Nup::open(&n.path).unwrap().with_backend(backend);
        let t = Instant::now();
        for &i in &zs {
            if let Err(e) = m.copy(i, &mut |_| Ok(()), &no) {
                eprintln!("{name}: {e}");
                return 1;
            }
        }
        let s = t.elapsed().as_secs_f64();
        println!(
            "{name}: {:.2} s, {:.0} MB/s of output (decode + SHA-256, 1 thread)",
            s,
            bytes as f64 / s / 1e6
        );
    }
    0
}
