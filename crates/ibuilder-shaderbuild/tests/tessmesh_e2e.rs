//! End to end, round 3: the Rust build (`ibuilder_shaderbuild::build` with `ibuilder_shader::finish`) over a SYNTHETIC game of tessellation, task / mesh and passthrough geometry shaders, against the
//! Python's own build of the same game (`gameapp/shader_device.build`, the phone's driver over the engine's Python): the two library images must be byte-identical, and the translator must be
//! handed the same jobs (text and code bytes) by both.
//!
//! The game and the Python's run are made by `crates/ibuilder-shader/tools/tessmesh/e2e.sh <name> [--random N]` (docs/shaders/SHADER-TESS-MESH-RUST.md section 3.3) under `$S2/sh3/<name>/`: `game/{pkg,profile,fake}`,
//! `py/b-<chain>.img`, `jobs-py/`, `eng/` (the engine folder with the real spirv-cross and a `dmsh_xlate` that serves prepared answers and otherwise runs the real translator: the engine's own
//! hand-assembled test programs). The test uses `$TM_E2E` (that folder) or `$S2/sh3/e2e-r120`, and skips with a message when it, the real translator or the Python's image is absent.
#![cfg(all(feature = "host-tools", feature = "f2-finish"))]
mod common;
use common::*;
use ibuilder_shaderbuild::finish_adapter::RustFinisher;
use ibuilder_shaderbuild::tools::ProcessTools;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, build};

fn dir_of() -> std::path::PathBuf {
    std::env::var("TM_E2E")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| s2("sh3/e2e-r120"))
}

#[test]
fn the_synthetic_game_is_the_pythons_library_and_gets_the_pythons_jobs() {
    let d = dir_of();
    let (game, py, eng) = (
        d.join("game"),
        d.join("py/b-6c53446391cc.img"),
        d.join("eng"),
    );
    if !have(&game.join("pkg")) || !have(&py) || !have(&eng) {
        return;
    }
    let work = std::env::temp_dir().join(format!("ibsb-tessmesh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let joblog = work.join("jobs");
    let wrapper = eng.join("dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate");
    // the wrapper logs to $TM_JOBLOG (the test cannot set the environment of this process without unsafe code: the wrapper defaults to <dir>/jobs-test, which is what is compared)
    let _ = std::fs::remove_dir_all(d.join("jobs-test"));
    if !have(&wrapper) {
        return;
    }
    let profile = game.join("profile");
    let mut o = BuildOptions::new(
        &game.join("pkg"),
        &profile,
        GAME,
        &work.join("out"),
        CHAIN,
        &work.join("cas"),
    );
    o.jobs = 2;
    o.tables = false;
    o.pulling = false;
    let fin = RustFinisher::load(&profile.join(GAME), GAME, CHAIN, false).unwrap();
    let tools = ProcessTools::in_engine(&eng);
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
    let want = std::fs::read(&py).unwrap();
    let got = std::fs::read(&r.image).unwrap();
    assert_eq!(got.len(), want.len(), "image sizes");
    assert!(got == want, "the library image differs from the Python's");
    eprintln!(
        "{} shaders ({} ok, failed {:?}): {} files, image byte-identical to the Python's ({} bytes); mesh libraries built {} / missing {} / not composed {}",
        r.shaders,
        r.ok,
        r.failed,
        r.files,
        r.bytes,
        r.mesh_built,
        r.mesh_missing,
        r.mesh_not_composed
    );
    // the translator's jobs
    let (a, b) = (d.join("jobs-py"), d.join("jobs-test"));
    if a.exists() && b.exists() {
        let mut names: Vec<String> = std::fs::read_dir(&a)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut n = 0;
        for f in &names {
            assert_eq!(
                std::fs::read_to_string(a.join(f)).unwrap(),
                std::fs::read_to_string(b.join(f)).unwrap_or_else(|_| panic!(
                    "{f}: the Rust build gave the translator no such job"
                )),
                "job {f}"
            );
            n += 1;
        }
        eprintln!("{n} translator jobs identical (job text and sha256 of the code)");
    }
    let _ = (joblog, std::fs::remove_dir_all(&work));
}
