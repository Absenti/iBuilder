//! The CLI library on stand-in workers (fast, no game data): the layout it makes, the config it hands the chain, the per-step table, the exit semantics.

use ibuilder_cli::{
    CliArgs, check_shader_args, engine_tool_paths, exit_code, format_report, make_config,
    prepare_extract, report_tsv, run,
};
use ibuilder_pipeline::chain::ChainReport;
use ibuilder_pipeline::testutil::{TempDir, write};
use std::fs;
use std::sync::Mutex;

/// the stage hook the meter installs is process-global: two runs at once would count each other's steps
static ONE_RUN_AT_A_TIME: Mutex<()> = Mutex::new(());

fn args(d: &TempDir) -> CliArgs {
    for m in ["main", "subsdk0", "sdk"] {
        write(d.join(format!("game/ExeFS/{m}")), format!("NSO:{m}"));
    }
    write(d.join("game/RomFS.img"), "rom");
    write(d.join("game/control.nacp"), "nacp");
    write(d.join("host.dylib"), "HOST");
    CliArgs {
        exefs: d.join("game/ExeFS"),
        out: d.join("out"),
        host: Some(d.join("host.dylib")),
        mock: true,
        ..Default::default()
    }
}

#[test]
fn the_inputs_are_laid_out_as_symlinks_and_never_touched() {
    let d = TempDir::new("cli-layout");
    let a = args(&d);
    let extract = prepare_extract(&a).unwrap();
    for n in ["ExeFS", "RomFS.img", "control.nacp"] {
        assert!(
            fs::symlink_metadata(extract.join(n))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{n}"
        );
    }
    assert!(
        !extract.join("icon.jpg").exists(),
        "only what exists beside the ExeFS"
    );
    // a second call replaces the links (a real folder left by an earlier --nup run is refused: tests/stale_extract.rs)
    prepare_extract(&a).unwrap();
    let c = make_config(&a);
    assert_eq!(
        c.get("keep_exefs"),
        Some("1"),
        "the CLI never deletes its inputs"
    );
    assert_eq!(
        c.get("extract_out"),
        Some(d.join("out/extract").to_str().unwrap())
    );
    assert_eq!(
        c.get("framework_out"),
        Some(
            d.join("out/DmGameProbe.framework/DmGameProbe")
                .to_str()
                .unwrap()
        )
    );
    assert_eq!(c.get("game"), Some("game"));
}

#[test]
fn a_mock_run_writes_the_framework_and_the_step_table() {
    let _one = ONE_RUN_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let d = TempDir::new("cli-run");
    let mut a = args(&d);
    a.threads = Some(2);
    let r = run(&a).expect("run");
    assert_eq!(r.chain.rc, 0, "{:?}", r.chain);
    assert!(r.chain.all_ok(), "{:?}", r.chain.status);
    let fw = fs::read(d.join("out/DmGameProbe.framework/DmGameProbe")).unwrap();
    assert!(fw.starts_with(b"MOCK-FRAMEWORK\n") && fw.ends_with(b"MOCK-SIGNATURE\n"));
    assert!(
        d.join("game/ExeFS/main").exists(),
        "the input is still there"
    );
    let labels: Vec<&str> = r.segments.iter().map(|s| s.label.as_str()).collect();
    for want in [
        "1 deriving the profile",
        "2 rewrite main",
        "2 rewrite subsdk0",
        "2 rewrite sdk",
        "4 assembling the framework",
        "4 signing the framework",
    ] {
        assert!(labels.contains(&want), "{want} in {labels:?}");
    }
    assert!(r.peak_rss > 1 << 20 && r.wall_s > 0.0);
    let t = format_report(&r);
    assert!(
        t.contains("2 rewrite main") && t.contains("total") && t.contains("result: ok (rc 0)"),
        "{t}"
    );
    assert!(report_tsv(&r).lines().count() >= 8);
    // the pipeline log and the build-times row are in the output folder
    assert!(
        fs::read_to_string(d.join("out/pipeline.stages.log"))
            .unwrap()
            .contains("[mem:chain-end]")
    );
    assert_eq!(
        fs::read_to_string(d.join("out/build_times.tsv"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[test]
fn a_missing_exefs_is_a_usage_error_not_a_panic() {
    let d = TempDir::new("cli-bad");
    let mut a = args(&d);
    a.exefs = d.join("nope");
    assert!(run(&a).err().unwrap().contains("is not a folder"));
}

/// Review r3 §5.1: a `--romfs` / `--control` the user named that is not there is an error, not an input silently left out.
#[test]
fn an_explicit_romfs_or_control_that_does_not_exist_is_an_error() {
    let d = TempDir::new("cli-missing-input");
    let mut a = args(&d);
    a.romfs = Some(d.join("no/RomFS.img"));
    let e = prepare_extract(&a).unwrap_err().to_string();
    assert!(e.contains("--romfs") && e.contains("no/RomFS.img"), "{e}");
    let mut a = args(&d);
    a.control = Some(d.join("no/control.nacp"));
    let e = prepare_extract(&a).unwrap_err().to_string();
    assert!(
        e.contains("--control") && e.contains("no/control.nacp"),
        "{e}"
    );
    let _one = ONE_RUN_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let e = run(&a).err().unwrap();
    assert!(e.contains("--control"), "{e}");
    // the defaults beside the ExeFS may be missing: left out, as before
    let mut a = args(&d);
    fs::remove_file(d.join("game/control.nacp")).unwrap();
    a.control = None;
    let extract = prepare_extract(&a).unwrap();
    assert!(fs::symlink_metadata(extract.join("control.nacp")).is_err());
}

/// Review r3 §5.3 + §3.1: the exit code says how the run ended: a pause (131) and the memory restart (75) are not "a stage failed", and a stage that failed without stopping the chain
/// (stage 3 with no library) is not a success.
#[test]
fn the_exit_code_says_how_the_run_ended() {
    let rep = |rc: i32, shaders: &str| ChainReport {
        rc,
        status: [("rewrite", "ok"), ("shaders", shaders)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        result: String::new(),
        framework: None,
        flat: None,
    };
    assert_eq!(exit_code(&rep(0, "ok")), 0);
    assert_eq!(
        exit_code(&rep(
            0,
            "none (the game will draw nothing until it has a library)"
        )),
        0
    );
    assert_eq!(
        exit_code(&rep(0, "failed (no library: the translation failed: x)")),
        1
    );
    assert_eq!(
        exit_code(&rep(4, "failed (no library: the translation failed: x)")),
        1,
        "the chain's 4 (no shader library, review r4 §4.1) is a failed run here too"
    );
    assert_eq!(
        exit_code(&rep(5, "ok")),
        1,
        "the chain's 5 (no framework) too"
    );
    assert_eq!(exit_code(&rep(1, "ok")), 1);
    assert_eq!(
        exit_code(&rep(2, "ok")),
        1,
        "2 stays the usage error of the CLI"
    );
    assert_eq!(exit_code(&rep(3, "ok")), 1);
    assert_eq!(exit_code(&rep(75, "ok")), 75);
    assert_eq!(exit_code(&rep(130, "ok")), 130);
    assert_eq!(exit_code(&rep(131, "ok")), 131);
}

// ---- stage 3 (--shaders) ----------------------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_shader_options_are_checked_before_anything_is_written() {
    let d = TempDir::new("cli-shopts");
    let mut a = args(&d);
    a.mock = false;
    // the options that belong to stage 3 are refused without --shaders (a typo must not silently run without it)
    for (set, name) in [
        ("chain", "--shader-chain"),
        ("engine", "--engine"),
        ("jobs", "--shader-jobs"),
    ] {
        let mut b = a.clone();
        match set {
            "chain" => b.shader_chain = Some("6c53446391cc".into()),
            "engine" => b.engine = Some(d.join("engine")),
            _ => b.shader_jobs = Some(2),
        }
        assert_eq!(
            check_shader_args(&b).unwrap_err(),
            format!("{name} needs --shaders")
        );
        assert!(
            run(&b).is_err() && !d.join("out").exists(),
            "refused before the output folder is made"
        );
    }
    a.shaders = true;
    assert!(
        check_shader_args(&a)
            .unwrap_err()
            .contains("--shader-chain"),
        "the chain is required"
    );
    a.shader_chain = Some("6c53446391cc/..".into());
    assert!(
        check_shader_args(&a)
            .unwrap_err()
            .contains("letters and digits"),
        "the chain names files"
    );
    a.shader_chain = Some("6c53446391cc".into());
    a.shader_jobs = Some(0);
    assert!(check_shader_args(&a).unwrap_err().contains("--shader-jobs"));
    a.shader_jobs = None;
    let e = check_shader_args(&a).unwrap_err();
    if cfg!(feature = "shader-host-tools") {
        assert!(e.contains("--engine"), "{e}");
        a.engine = Some(d.join("engine"));
        let e = check_shader_args(&a).unwrap_err();
        assert!(
            e.contains("dmsh_xlate") && e.contains("is not there"),
            "the tools are looked for: {e}"
        );
        let (x, s) = engine_tool_paths(a.engine.as_ref().unwrap());
        write(&x, "x");
        write(&s, "s");
        assert!(check_shader_args(&a).is_ok());
    } else {
        assert!(
            e.contains("shader-host-tools"),
            "a build without the feature says what to build: {e}"
        );
    }
    assert!(
        !d.join("out").exists(),
        "nothing was written by any refusal above"
    );
    // the stand-in backend carries its own tools: no engine needed
    a.mock = true;
    a.engine = None;
    assert!(check_shader_args(&a).is_ok());
}

#[test]
fn engine_tool_paths_are_where_the_engine_keeps_them() {
    let (x, s) = engine_tool_paths(std::path::Path::new("/E"));
    assert_eq!(
        x,
        std::path::Path::new("/E/dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate")
    );
    assert_eq!(
        s,
        std::path::Path::new("/E/dm/gfx/tools/spirv_cross_bin/spirv-cross")
    );
}

#[test]
fn the_config_carries_the_shader_rows_only_with_shaders_and_set_wins() {
    let d = TempDir::new("cli-shcfg");
    let mut a = args(&d);
    assert_eq!(
        make_config(&a).get("shader_chain"),
        None,
        "stage 3 stays off (status none) unless asked for"
    );
    a.shaders = true;
    a.shader_chain = Some("6c53446391cc".into());
    a.shader_jobs = Some(3);
    a.set = vec![
        ("shader_cache_gib".into(), "4".into()),
        ("shader_jobs".into(), "5".into()),
    ];
    let c = make_config(&a);
    assert_eq!(c.get("shader_chain"), Some("6c53446391cc"));
    assert_eq!(c.get("shader_cache_gib"), Some("4"));
    assert_eq!(c.get("shader_jobs"), Some("5"), "--set is applied last");
    assert_eq!(
        c.get("romfs"),
        Some(d.join("out/extract/RomFS.img").to_str().unwrap()),
        "stage 3 reads the RomFS.img and exe_dir/main the CLI linked"
    );
}

#[test]
fn a_mock_run_with_shaders_runs_stage_3_between_the_rewrite_and_the_assembly() {
    let _one = ONE_RUN_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let d = TempDir::new("cli-shmock");
    let mut a = args(&d);
    a.shaders = true;
    a.shader_chain = Some("6c53446391cc".into());
    a.set = vec![("shader_free_gib".into(), "0".into())];
    let r = run(&a).expect("run");
    assert!(r.chain.all_ok(), "{:?}", r.chain.status);
    assert_eq!(
        r.chain.status.get("shaders").map(String::as_str),
        Some("ok")
    );
    let labels: Vec<&str> = r.segments.iter().map(|s| s.label.as_str()).collect();
    let pos = |l: &str| {
        labels
            .iter()
            .position(|x| *x == l)
            .unwrap_or_else(|| panic!("{l} in {labels:?}"))
    };
    assert!(
        pos("2 rewrite sdk") < pos("3 shaders split")
            && pos("3 shaders split") < pos("3 shaders build")
            && pos("3 shaders build") < pos("4 assembling the framework"),
        "{labels:?}"
    );
    assert!(
        fs::read_to_string(d.join("out/pipeline.stages.log"))
            .unwrap()
            .contains("stage 3 shaders:")
    );
    let pack = fs::read_dir(d.join("out/shaders"))
        .unwrap()
        .next()
        .expect("a pack was installed")
        .unwrap()
        .path();
    assert!(pack.join("pack.txt").is_file(), "{}", pack.display());
}

/// Review r3 §3.1: a stage 3 that could not build the library (here: too little free space) is reported as failed and the CLI does not exit 0.
#[test]
fn a_mock_run_whose_shaders_fail_is_not_a_success() {
    let _one = ONE_RUN_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let d = TempDir::new("cli-shfail");
    let mut a = args(&d);
    a.shaders = true;
    a.shader_chain = Some("6c53446391cc".into());
    a.set = vec![("shader_free_gib".into(), "1000000".into())];
    let r = run(&a).expect("run");
    let st = r.chain.status.get("shaders").cloned().unwrap_or_default();
    assert!(
        st.starts_with("failed (no library: "),
        "{:?}",
        r.chain.status
    );
    assert_eq!(
        r.chain.status.get("assemble").map(String::as_str),
        Some("ok")
    );
    assert!(!r.chain.all_ok());
    assert_eq!(r.chain.result, "failed (shaders)");
    assert_eq!(exit_code(&r.chain), 1);
    assert_eq!(
        r.chain.rc,
        ibuilder_pipeline::chain::RC_NO_SHADER_LIBRARY,
        "review r4 §4.1: not 0"
    );
    assert!(
        format_report(&r).contains("result: failed (shaders) (rc 4)"),
        "{}",
        format_report(&r)
    );
}

#[test]
fn the_binary_lists_the_shader_options_and_refuses_a_bad_combination_with_exit_2() {
    let bin = env!("CARGO_BIN_EXE_ibuilder");
    let help = std::process::Command::new(bin)
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    for opt in [
        "--shaders",
        "--shader-chain",
        "--engine",
        "--shader-jobs",
        "--set KEY=VALUE",
    ] {
        assert!(text.contains(opt), "--help lists {opt}: {text}");
    }
    let d = TempDir::new("cli-bin");
    let out = std::process::Command::new(bin)
        .args([
            "--exefs",
            d.join("x").to_str().unwrap(),
            "--out",
            d.join("o").to_str().unwrap(),
            "--shaders",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--shader-chain"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = std::process::Command::new(bin)
        .args(["--exefs", "x", "--out", "o", "--set", "novalue"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("KEY=VALUE"));
}

// ---- --nup: a Switch 2 container instead of an extracted ExeFS ----------------------------------------------------------------------------------------

#[test]
fn a_nup_is_a_config_key_and_nothing_is_linked() {
    let d = TempDir::new("cli-nup");
    write(d.join("My Game [0400486024dec000][v131072].nup"), "NUP");
    let a = CliArgs {
        nup: Some(d.join("My Game [0400486024dec000][v131072].nup")),
        out: d.join("out"),
        mock: true,
        ..Default::default()
    };
    let extract = prepare_extract(&a).unwrap();
    assert_eq!(
        fs::read_dir(&extract).unwrap().count(),
        0,
        "stage 1 reads the container into it: nothing is linked"
    );
    let c = make_config(&a);
    let nup = c.get("nup").unwrap();
    assert!(
        nup.ends_with("My Game [0400486024dec000][v131072].nup"),
        "{nup}"
    );
    assert_eq!(
        c.get("game"),
        Some("My Game [0400486024dec000][v131072].nup")
    );
    assert_eq!(
        c.get("extract_out"),
        Some(d.join("out/extract").to_str().unwrap())
    );
}

#[test]
fn nup_and_exefs_are_two_ways_to_give_the_game_and_the_file_must_exist() {
    let d = TempDir::new("cli-nup2");
    let mut a = args(&d);
    a.nup = Some(d.join("missing.nup"));
    assert!(run(&a).err().expect("refused").contains("use one"));
    let b = CliArgs {
        nup: Some(d.join("missing.nup")),
        out: d.join("out2"),
        mock: true,
        ..Default::default()
    };
    assert!(run(&b).err().expect("refused").contains("is not a file"));
}

#[test]
fn a_mock_chain_without_a_nup_reader_fails_stage_1_with_a_message_not_a_panic() {
    let _one = ONE_RUN_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let d = TempDir::new("cli-nup3");
    write(d.join("g.nup"), "NUP");
    let a = CliArgs {
        nup: Some(d.join("g.nup")),
        out: d.join("out"),
        mock: true,
        ..Default::default()
    };
    let r = run(&a).expect("run");
    assert_eq!(r.chain.rc, 1, "{:?}", r.chain);
    assert!(
        fs::read_to_string(d.join("out/pipeline.stages.log"))
            .unwrap()
            .contains("no NUP reader in this build")
    );
}
