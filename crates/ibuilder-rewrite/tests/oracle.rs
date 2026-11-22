//! The Rust rewrite of every Grizzy module compared, file by file, with the Python oracle's output
//! (`out-py/pyrewrite/out/<module>/`), at several thread counts. Skipped with a message when the fixtures are absent.

use ibuilder_rewrite::{Options, rewrite};
use ibuilder_testkit::{compare_files, grizzy_exefs, grizzy_oracle};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// every file the Python writes for a module (a module may not have all of them: only those the oracle has are compared)
const FILES: [&str; 9] = [
    "patched.bin",
    "fixups.tsv",
    "sites.tsv",
    "gates.tsv",
    "cuts.tsv",
    "obs.tsv",
    "changed.tsv",
    "match.tsv",
    "build_src.sha256",
];

/// the folder the oracle's Python ran from (the iOS-patched copy of the engine's tools): `build_src.sha256` hashes it
fn py_tools() -> PathBuf {
    PathBuf::from(format!(
        "{}/out-py/pyrewrite/dm/tools",
        ibuilder_testkit::IOSBUILDER
    ))
}

fn fixtures(module: &str) -> bool {
    let ok = Path::new(&grizzy_exefs(module)).is_file()
        && Path::new(&grizzy_oracle(module, &format!("{module}.patched.bin"))).is_file();
    if !ok {
        ibuilder_testkit::skip(&format!(
            "{module}: Grizzy ExeFS / oracle fixtures are not on this machine"
        ));
    }
    ok
}

fn out_dir(tag: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("rewrite-{tag}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Rewrite `module` on `threads` threads, write the files the CLI writes, compare each with the oracle's
fn check(module: &str, threads: usize) {
    let file = fs::File::open(grizzy_exefs(module)).unwrap();
    // SAFETY: read-only map of a fixture that nothing writes
    let map = unsafe { memmap2::Mmap::map(&file) }.unwrap();
    let have_tools = py_tools().is_dir();
    let opts = Options {
        workers: threads,
        py_tools: have_tools.then(py_tools),
        ..Default::default()
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let r = pool
        .install(|| rewrite(&map, module, module, &opts))
        .unwrap_or_else(|e| panic!("{module}: {e}"));
    let dir = out_dir(&format!("{module}-{threads}"));
    {
        let mut f = std::io::BufWriter::new(
            fs::File::create(dir.join(format!("{module}.patched.bin"))).unwrap(),
        );
        r.write_patched(&mut f).unwrap();
        f.flush().unwrap();
    }
    let texts: [(&str, &str); 6] = [
        ("fixups.tsv", &r.fixups),
        ("sites.tsv", &r.sites),
        ("gates.tsv", &r.gates),
        ("cuts.tsv", &r.cuts),
        ("obs.tsv", &r.obs),
        ("changed.tsv", &r.changed),
    ];
    for (ext, text) in texts {
        fs::write(dir.join(format!("{module}.{ext}")), text).unwrap();
    }
    if let Some(m) = &r.match_tsv {
        fs::write(dir.join(format!("{module}.match.tsv")), m).unwrap();
    }
    if let Some(b) = &r.build_src {
        fs::write(dir.join("build_src.sha256"), b).unwrap();
    }
    let mut compared = 0;
    for ext in FILES {
        let name = if ext == "build_src.sha256" {
            ext.to_string()
        } else {
            format!("{module}.{ext}")
        };
        let oracle = grizzy_oracle(module, &name);
        let have_oracle = Path::new(&oracle).is_file();
        let have_ours = dir.join(&name).is_file();
        if ext == "build_src.sha256" && !have_tools {
            // a PARTIAL skip: the other files are still compared below, but this hash is not
            ibuilder_testkit::skip(&format!(
                "{module}: build_src.sha256 not compared (the Python sources are not on this machine)"
            ));
            continue;
        }
        assert_eq!(
            have_oracle, have_ours,
            "{module}: {name} exists in the oracle: {have_oracle}, in ours: {have_ours}"
        );
        if have_oracle {
            compare_files(dir.join(&name), &oracle)
                .unwrap_or_else(|e| panic!("{module} ({threads} threads): {e}"));
            compared += 1;
        }
    }
    // the lines the Python printed (stats line, [RED] lines, skipped patches) are part of its behaviour too
    let log = format!("{}\n", r.log.join("\n"));
    let py_log = fs::read_to_string(format!(
        "{}/out-py/pyrewrite/rewrite_{module}.log",
        ibuilder_testkit::IOSBUILDER
    ))
    .unwrap();
    assert_eq!(
        log, py_log,
        "{module}: stdout differs from the Python's log"
    );
    eprintln!("{module}: {compared} files + log identical to the oracle ({threads} threads)");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn oracle_sdk() {
    if fixtures("sdk") {
        check(
            "sdk",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        );
    }
}

#[test]
fn oracle_subsdk0() {
    if fixtures("subsdk0") {
        check(
            "subsdk0",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        );
    }
}

#[test]
fn oracle_main() {
    if fixtures("main") {
        check(
            "main",
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        );
    }
}

/// The output does not depend on the thread count: sdk and subsdk0 on 1, 2 and 8 threads
#[test]
fn thread_counts_small_modules() {
    for module in ["sdk", "subsdk0"] {
        if fixtures(module) {
            for t in [1, 2, 8] {
                check(module, t);
            }
        }
    }
}

/// ... and main (the large one) on 1, 2 and 8 threads
#[test]
fn thread_counts_main() {
    if fixtures("main") {
        for t in [1, 2, 8] {
            check("main", t);
        }
    }
}
