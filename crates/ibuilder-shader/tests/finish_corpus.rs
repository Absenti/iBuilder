//! The per-shader `finish` against the Python's library image, file by file, on the captured Grizzy inputs (docs/shaders/SHADER-FINISH-RUST.md, section 3): runs the `shader_finish` binary
//! (the only place that drives the native tools as child processes) over a capture and requires every shader byte-identical to the image the instrumented Python build wrote.
//! The fixtures are read-only paths of the scratch folder (`FINISH_SCRATCH`, default `$IBUILDER_SCRATCH/f2`; they are written by `tools/finish/make_cap_engine.py` + `run_build.py`);
//! a missing fixture skips the test with a message.
use std::path::Path;
use std::process::Command;

fn run(name: &str, extra: &[&str]) {
    run_with(name, None, extra)
}

/// `profile`: a profile folder relative to the scratch folder (default: the Grizzy profile of the earlier scratch folder)
fn run_with(name: &str, profile: Option<&str>, extra: &[&str]) {
    let f2 = std::env::var("FINISH_SCRATCH")
        .unwrap_or_else(|_| format!("{}/f2", ibuilder_testkit::SCRATCH));
    let old =
        std::env::var("FINISH_OLD").unwrap_or_else(|_| ibuilder_testkit::SCRATCH_OLD.to_string());
    let tools = format!("{f2}/engine-cap/dm/gfx/tools");
    let need = [
        format!("{f2}/cap-{name}"),
        format!("{f2}/runs/{name}/b-b24b25240cad.img"),
        profile
            .map(|p| format!("{f2}/{p}"))
            .unwrap_or_else(|| format!("{old}/shader/profile")),
        format!("{tools}/dmsh_xlate_bin/dmsh_xlate"),
        format!("{tools}/spirv_cross_bin/spirv-cross"),
    ];
    if let Some(m) = need.iter().find(|p| !Path::new(p).exists()) {
        ibuilder_testkit::skip(&format!("fixture {m} is absent"));
        return;
    }
    let work = std::env::temp_dir().join(format!("finish-corpus-{name}-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_shader_finish"))
        .args([
            "--cap",
            &need[0],
            "--expect",
            &need[1],
            "--profile",
            &need[2],
            "--xlate",
            &need[3],
            "--spirv-cross",
            &need[4],
            "--work",
            &work.to_string_lossy(),
        ])
        .args(extra)
        .output()
        .expect("run shader_finish");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    eprintln!("{text}");
    let line = text
        .lines()
        .find(|l| l.starts_with("shaders: identical"))
        .unwrap_or_else(|| {
            panic!(
                "no summary line:\n{text}\n{}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    let nums: Vec<u64> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect();
    // shaders: identical N, differ D, unported U, failed/other F of T (Python status ...: P)
    assert_eq!(
        (nums[1], nums[2], nums[3]),
        (0, 0, 0),
        "differ / unported / failed: {line}"
    );
    assert_eq!(nums[0], nums[4], "{line}");
}

/// 401 shaders (the keys-s300 sample and the pixel shaders paired with its vertex shaders)
#[test]
fn sample_300_identical() {
    run("s300", &[]);
}

/// 2,459 shaders (keys-s1800 + pairs): 1,539 ps, 700 vs, 200 cs, 20 gs, every file of the library image (6,781)
#[test]
fn sample_1800_identical() {
    run("s1800", &[]);
}

/// the same with the pulling libraries (`<key>_bp.metal`), against an image the Python built with `pulling=True`
#[test]
fn sample_1800_with_pulling_identical() {
    run("p1800", &["--pulling"]);
}

/// every pixel shader listed in `dual_want.tsv` (`fs_dual`, `main0_d`) and `bf_want.tsv` (the readable-tile version without a rewritable sample): 1,534 `_bf`, 1 `main0_d`
#[test]
fn sample_1800_dual_and_all_bf_identical() {
    run_with("d1800", Some("profile-dual"), &[]);
}

/// every vertex shader with a transform feedback layout (`main0_tf`) and a side effect (`main0_discard`), the Python built with `DMSH_MUTATE=tf,discard` (tools/finish/make_cap_engine.py)
#[test]
fn sample_1800_tf_and_discard_identical() {
    run("m1800", &["--mutate", "tf,discard"]);
}

/// the whole Grizzy library: 21,990 shaders (10,541 ps, 10,537 vs, 767 cs, 145 gs), every one of its 56,882 shader files (about 90 s and 1.5 GB of captured inputs; `cargo test ... -- --ignored`)
#[test]
#[ignore = "21,990 shaders, about 90 s"]
fn full_grizzy_identical() {
    run("full", &[]);
}
