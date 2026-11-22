//! The whole stage with the real finish port (`ibuilder_shader::finish`, package F2) and the engine's real tools, against the Python's library images. Needs `--features host-tools,f2-finish`
//! and the fixtures; the 2,459-shader sample (13 s) is `#[ignore]`d with the full Grizzy build (2 min, 3.4 GB of disk, set `IBSB_FULL=1`), run them with `-- --ignored`.
#![cfg(all(feature = "host-tools", feature = "f2-finish"))]
mod common;
use common::*;
use ibuilder_shader::romfs::Image;
use ibuilder_shaderbuild::finish_adapter::RustFinisher;
use ibuilder_shaderbuild::tools::ProcessTools;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, build};

fn run(
    keys: Option<Vec<String>>,
    name: &str,
    jobs: usize,
    tables: bool,
) -> Option<(ibuilder_shaderbuild::BuildReport, std::path::PathBuf)> {
    let engine = s2("B/py/engine-jobs");
    if !have(&engine) || !have(&pkg()) || !have(&profile()) {
        return None;
    }
    let work = std::env::temp_dir().join(format!("ibsb-real-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let mut o = BuildOptions::new(
        &pkg(),
        &profile(),
        GAME,
        &work.join("out"),
        CHAIN,
        &work.join("cas"),
    );
    o.keys = keys;
    o.jobs = jobs;
    o.tables = tables;
    o.pulling = false;
    let fin = RustFinisher::load(&profile().join(GAME), GAME, CHAIN, false).unwrap();
    let tools = ProcessTools::in_engine(&engine);
    let log = |l: &str| {
        if !l.starts_with("compiling") && !l.starts_with("cache check") {
            eprintln!("{l}");
        }
    };
    let r = build(
        &o,
        &BuildEnv {
            tools: &tools,
            finisher: &fin,
            log: &log,
            cancel: None,
        },
    )
    .unwrap();
    Some((r, work))
}

fn same_files(a: &[u8], b: &[u8], tolerate_first_line: &[&str]) -> (usize, usize) {
    let (ia, ib) = (Image::open(a).unwrap(), Image::open(b).unwrap());
    assert_eq!(ia.files.len(), ib.files.len(), "file counts");
    let (mut same, mut stamp_only) = (0, 0);
    for f in &ia.files {
        let g = ib
            .find(&f.name)
            .unwrap_or_else(|| panic!("{} is not in the Python's image", f.name));
        let (x, y) = (ia.bytes(f), ib.bytes(g));
        if x == y {
            same += 1;
        } else if tolerate_first_line.contains(&f.name.as_str()) {
            let (i, j) = (
                x.iter().position(|c| *c == b'\n').unwrap(),
                y.iter().position(|c| *c == b'\n').unwrap(),
            );
            assert_eq!(&x[i..], &y[j..], "{} after its first line", f.name);
            stamp_only += 1;
        } else {
            panic!("{} differs from the Python's", f.name);
        }
    }
    (same, stamp_only)
}

#[test]
fn the_300_shader_sample_is_the_pythons_library() {
    let Some(keys) = keys_file("keys-s300.txt") else {
        return;
    };
    let Some((r, work)) = run(Some(keys), "s300", 4, false) else {
        return;
    };
    assert_eq!((r.shaders, r.ok), (401, 401), "{:?}", r.failed);
    let want = std::fs::read(old("shader/runs/s300-j1/b-b24b25240cad.img")).unwrap();
    assert_eq!(std::fs::read(&r.image).unwrap(), want, "the library image");
    eprintln!(
        "401 shaders: image byte-identical to the Python's ({} bytes, {:.1} s)",
        r.bytes, r.wall_s
    );
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
#[ignore]
fn the_1800_shader_sample_is_the_pythons_library() {
    let Some(keys) = keys_file("keys-s1800.txt") else {
        return;
    };
    let Some((r, work)) = run(Some(keys), "s1800", 6, false) else {
        return;
    };
    assert_eq!((r.shaders, r.ok), (2459, 2459), "{:?}", r.failed);
    let want = std::fs::read(old("shader/runs/s1800-cap/b-b24b25240cad.img")).unwrap();
    assert_eq!(std::fs::read(&r.image).unwrap(), want, "the library image");
    eprintln!(
        "2459 shaders: image byte-identical to the Python's ({} bytes, {:.1} s wall, {} translator calls {:.0} s, {} spirv-cross calls {:.0} s)",
        r.bytes, r.wall_s, r.translate_calls, r.translate_s, r.spirv_cross_calls, r.spirv_cross_s
    );
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
#[ignore]
fn the_whole_grizzy_library_is_the_pythons() {
    if std::env::var("IBSB_FULL").is_err() {
        ibuilder_testkit::skip("set IBSB_FULL=1 (2 min, 3.4 GB of disk)");
        return;
    }
    let Some((r, work)) = run(None, "full", 6, true) else {
        return;
    };
    assert_eq!((r.shaders, r.ok), (21990, 21990), "{:?}", r.failed);
    let map = |p: &std::path::Path| {
        let f = std::fs::File::open(p).unwrap();
        // SAFETY: a read-only map of a finished image that nothing writes while the test runs
        unsafe { memmap2::Mmap::map(&f) }.unwrap()
    };
    let (a, b) = (
        map(&r.image),
        map(&old("shader/runs/full-gil-j6/b-b24b25240cad.img")),
    );
    let (same, stamp) = same_files(&a[..], &b[..], &["c1_full.tsv", "cb_extent.tsv"]);
    eprintln!(
        "{same} files identical + {stamp} tables identical but for the time stamp line; c1 {} rows, cb {} rows; wall {:.1} s",
        r.c1_unproven, r.cb_shaders, r.wall_s
    );
    assert_eq!((r.c1_unproven, r.cb_shaders), (6679, 21522));
    let _ = std::fs::remove_dir_all(&work);
}
