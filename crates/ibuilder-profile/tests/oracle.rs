//! Oracle tests: the Python `gameapp/profile_derive.py` (with the engine's `profile_guess.py`, `game.py`, `nacp.py`,
//! `nso.py` it runs) and the stored 2026-11-17 outputs of the Python on the same game are the judges.
//! A missing oracle or fixture skips the case with a message (it never fails).
//!
//! * `grizzy_*`: the reference game (Grizzy, `0100DFA027504000`): Rust vs. Python byte for byte, vs. the stored
//!   Python output (`out-prof/derived-*.tsv`), and vs. the Mac orchestrator's profile (`out-prof/harness-profile.tsv`,
//!   the "37 data rows identical" claim of `docs/PROFILE-DERIVED.md`).
//! * `registered_games_*`: every `dm/games/<id>/game.tsv` of the engine rendered by Python and Rust.
//! * `synthetic_variants_*`: generated ExeFS / RomFS / NACP variants that exercise every rule.
use ibuilder_profile::HookSet;
use ibuilder_profile::compare::{compare, rows_of};
use ibuilder_profile::derive::{DeriveConfig, derive};
use ibuilder_profile::facts::facts_of;
use ibuilder_profile::game::{Game, render_profile};
use ibuilder_profile::guess::{guess, report};
use ibuilder_testkit::{IOSBUILDER, SCRATCH_OLD as SCRATCH};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn python() -> Option<PathBuf> {
    let p = std::env::var("IBUILDER_ORACLE_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| format!("{SCRATCH}/ref/venv/bin/python").into());
    p.exists().then_some(p)
}

/// The engine layout (`dm/games`, `dm/host`, `dm/gfx`, `dm/tools`): the copy under `$IBUILDER_SCRATCH_OLD`, else the third-party engine tree (`$IBUILDER_NX2APPLE_ENGINE`).
fn bundle() -> Option<PathBuf> {
    for c in [
        std::env::var("IBUILDER_ENGINE_BUNDLE").ok(),
        Some(format!("{SCRATCH}/ref/engine")),
        ibuilder_testkit::local_env("IBUILDER_NX2APPLE_ENGINE"),
    ]
    .into_iter()
    .flatten()
    {
        let p = PathBuf::from(c);
        if p.join("dm/games/game.py").exists()
            && p.join("dm/host").exists()
            && p.join("dm/tools/nso.py").exists()
        {
            return Some(p);
        }
    }
    None
}

fn gameapp() -> Option<PathBuf> {
    [
        format!("{SCRATCH}/oracle/gameapp"),
        format!("{IOSBUILDER}/gameapp"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.join("profile_derive.py").exists())
}

fn work(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("profile-oracle")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

struct Oracle {
    py: PathBuf,
    bundle: PathBuf,
    gameapp: PathBuf,
}

fn oracle(what: &str) -> Option<Oracle> {
    match (python(), bundle(), gameapp()) {
        (Some(py), Some(bundle), Some(gameapp)) => Some(Oracle {
            py,
            bundle,
            gameapp,
        }),
        _ => {
            ibuilder_testkit::skip(&format!(
                "{what}: Python oracle (ref/venv + engine bundle + gameapp/profile_derive.py) not found"
            ));
            None
        }
    }
}

impl Oracle {
    fn run(&self, script: &Path, args: &[&str]) -> std::process::Output {
        Command::new(&self.py)
            .arg("-B")
            .arg(script)
            .args(args)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("GAMEAPP_BUNDLE", &self.bundle)
            .env("IBUILDER_ORACLE_GAMEAPP", &self.gameapp)
            .output()
            .expect("run python")
    }
}

fn helper(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/oracle")
        .join(name)
}

fn grizzy_inputs() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let img = PathBuf::from(format!("{IOSBUILDER}/out/ref-img-grizzy"));
    let man = PathBuf::from(format!("{IOSBUILDER}/out/ref-out-grizzy"));
    let r = (img.join("RomFS.img").exists()
        && img.join("ExeFS/main").exists()
        && man.join("romfs_manifest.tsv").exists())
    .then(|| (img.clone(), man.clone(), man.join("romfs_manifest.tsv")));
    if r.is_none() {
        ibuilder_testkit::skip(
            "the Grizzy inputs (out/ref-img-grizzy, out/ref-out-grizzy) are not on this machine",
        );
    }
    r
}

fn lines_after(s: &str, n: usize) -> Vec<&str> {
    s.lines().skip(n).collect()
}

#[test]
fn grizzy_derive_matches_the_stored_python_output_and_the_orchestrator() {
    let (Some((img, man, manifest)), Some(b)) = (grizzy_inputs(), bundle()) else {
        ibuilder_testkit::skip(
            "grizzy golden: fixtures (out/ref-img-grizzy, out/ref-out-grizzy) or engine bundle absent",
        );
        return;
    };
    let w = work("grizzy-golden");
    // the image path (what the app does) and the manifest path (what the Mac survey does)
    for (tag, extract, romfs) in [
        ("img", img.clone(), None),
        ("manifest", man.clone(), Some(manifest.clone())),
    ] {
        let pdir = w.join(tag).join("0100DFA027504000");
        let mut cfg = DeriveConfig::new(&extract, &b);
        cfg.romfs = romfs;
        cfg.profile = Some(w.join(tag).join("profile.tsv"));
        cfg.profile_out = Some(pdir.clone());
        let mut log = Vec::new();
        let out = derive(&cfg, &mut |m| log.push(m.to_string())).unwrap();
        assert!(
            log[0].contains("no engine table for 0100DFA027504000"),
            "{log:?}"
        );
        let game = fs::read_to_string(pdir.join("game.tsv")).unwrap();
        let prof = fs::read_to_string(&out).unwrap();
        // stored Python output of 2026-11-17 (lines 1-3 hold the container paths it ran from)
        let g_old = fs::read_to_string(format!("{IOSBUILDER}/out-prof/derived-game.tsv")).unwrap();
        assert_eq!(
            lines_after(&game, 3),
            lines_after(&g_old, 3),
            "{tag}: game.tsv differs from the stored Python output"
        );
        let p_old =
            fs::read_to_string(format!("{IOSBUILDER}/out-prof/derived-profile.tsv")).unwrap();
        assert_eq!(
            lines_after(&prof, 1),
            lines_after(&p_old, 1),
            "{tag}: profile.tsv differs from the stored Python output"
        );
        // the orchestrator's rendered profile: the runtime keys must be identical (docs/PROFILE-DERIVED.md section 3)
        let harness =
            fs::read_to_string(format!("{IOSBUILDER}/out-prof/harness-profile.tsv")).unwrap();
        let c = compare("profile.tsv", &rows_of(&prof), &rows_of(&harness));
        assert!(
            c.different.is_empty() && c.not_derived.is_empty() && c.extra.is_empty(),
            "{tag}: {}",
            c.text
        );
        assert_eq!(c.equal.len(), 37, "{tag}");
        eprintln!(
            "OK   grizzy {tag}: game.tsv + profile.tsv equal the stored Python output; 37/37 runtime keys equal the orchestrator's"
        );
    }
}

#[test]
fn grizzy_derive_is_byte_identical_to_python() {
    let (Some(o), Some((img, man, manifest))) = (oracle("grizzy vs python"), grizzy_inputs())
    else {
        return;
    };
    let w = work("grizzy-py");
    for (tag, extract, romfs) in [("img", img, None), ("manifest", man, Some(manifest))] {
        // Python: the dry-run harness (`__main__`) for the image; a driver call for a manifest RomFS
        let (ppdir, ppath) = (
            w.join(format!("py-{tag}")).join("0100DFA027504000"),
            w.join(format!("py-{tag}")).join("profile.tsv"),
        );
        let rpdir = w.join(format!("rs-{tag}")).join("0100DFA027504000");
        let rpath = w.join(format!("rs-{tag}")).join("profile.tsv");
        let out = match &romfs {
            None => o.run(
                &o.gameapp.join("profile_derive.py"),
                &[
                    extract.to_str().unwrap(),
                    ppath.to_str().unwrap(),
                    ppdir.to_str().unwrap(),
                ],
            ),
            Some(m) => o.run(
                &helper("derive_driver.py"),
                &[
                    "provisional",
                    extract.to_str().unwrap(),
                    extract.join("ExeFS").to_str().unwrap(),
                    m.to_str().unwrap(),
                    ppath.to_str().unwrap(),
                    ppdir.to_str().unwrap(),
                ],
            ),
        };
        assert!(
            out.status.success(),
            "python failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut cfg = DeriveConfig::new(&extract, &o.bundle);
        cfg.romfs = romfs;
        cfg.profile = Some(rpath.clone());
        cfg.profile_out = Some(rpdir.clone());
        let mut log = Vec::new();
        derive(&cfg, &mut |m| log.push(m.to_string())).unwrap();
        for f in [
            "game.tsv",
            "bf_want.tsv",
            "pso_manifest.tsv",
            "shader_facts.tsv",
            "shader_map.tsv",
            "shader_map.tsv.conflicts.tsv",
            "shader_pairs.tsv",
        ] {
            assert_eq!(
                fs::read(ppdir.join(f)).unwrap(),
                fs::read(rpdir.join(f)).unwrap(),
                "{tag}: {f} differs"
            );
        }
        let norm = |p: &Path, d: &Path| {
            fs::read_to_string(p)
                .unwrap()
                .replace(&d.display().to_string(), "PDIR")
        };
        assert_eq!(
            norm(&ppath, &ppdir),
            norm(&rpath, &rpdir),
            "{tag}: profile.tsv differs"
        );
        eprintln!(
            "OK   grizzy {tag}: game.tsv, profile.tsv and the 6 empty tables are byte-identical to the Python's"
        );
    }
}

#[test]
fn facts_report_matches_profile_guess_cli() {
    let (Some(o), Some((_, man, manifest))) = (oracle("profile_guess CLI"), grizzy_inputs()) else {
        return;
    };
    let py = o.run(
        &o.bundle.join("dm/games/profile_guess.py"),
        &[
            man.join("ExeFS").to_str().unwrap(),
            manifest.to_str().unwrap(),
        ],
    );
    assert!(
        py.status.success(),
        "{}",
        String::from_utf8_lossy(&py.stderr)
    );
    let hooks = HookSet::from_host_dir(&o.bundle.join("dm/host"), false).unwrap();
    let f = facts_of(&man.join("ExeFS"), &manifest).unwrap();
    let g = guess(&f, &hooks).unwrap();
    assert_eq!(String::from_utf8(py.stdout).unwrap(), report(&f, &g));
}

#[test]
fn registered_games_render_identically_to_python() {
    let Some(o) = oracle("registered games") else {
        return;
    };
    let w = work("registered");
    let (fake, pyout) = (w.join("fake"), w.join("py"));
    let out = o.run(
        &helper("derive_driver.py"),
        &[
            "registered",
            fake.to_str().unwrap(),
            pyout.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let games = o.bundle.join("dm/games");
    let mut n = 0;
    for e in fs::read_dir(&games).unwrap().flatten() {
        let p = e.path();
        if !p.join("game.tsv").exists() {
            continue;
        }
        let tid = e.file_name().to_string_lossy().into_owned();
        let want = fs::read_to_string(pyout.join(format!("{tid}.profile.tsv"))).unwrap();
        let got = render_profile(&Game::load_dir(&p).unwrap()).unwrap().text;
        assert_eq!(want, got, "{tid}: profile.tsv differs");
        // and through derive() with a RomFS.img.id, as the app does
        let ex = fake.join(&tid).join("extract");
        let mut cfg = DeriveConfig::new(&ex, &o.bundle);
        cfg.profile = Some(w.join("rs").join(format!("{tid}.tsv")));
        let mut log = Vec::new();
        derive(&cfg, &mut |m| log.push(m.to_string())).unwrap();
        assert_eq!(
            want,
            fs::read_to_string(w.join("rs").join(format!("{tid}.tsv"))).unwrap()
        );
        n += 1;
    }
    assert!(n > 100, "only {n} registered games found");
    eprintln!("OK   {n} registered games: profile.tsv byte-identical to the Python's");
}

#[test]
fn synthetic_variants_match_python() {
    let Some(o) = oracle("synthetic variants") else {
        return;
    };
    let w = work("variants");
    let (src, pyd, rsd) = (w.join("src"), w.join("py"), w.join("rs"));
    fs::create_dir_all(&src).unwrap();
    let gen_out = o.run(
        &helper("gen_variants.py"),
        &[
            src.to_str().unwrap(),
            "60",
            o.bundle.join("dm/host").to_str().unwrap(),
            o.bundle.join("dm/tools").to_str().unwrap(),
        ],
    );
    assert!(
        gen_out.status.success(),
        "generator failed (needs the lz4 module of ref/venv): {}",
        String::from_utf8_lossy(&gen_out.stderr)
    );
    for d in [&pyd, &rsd] {
        let _ = Command::new("cp")
            .arg("-R")
            .arg(&src)
            .arg(d)
            .status()
            .unwrap();
    }
    let drv = o.run(
        &helper("derive_driver.py"),
        &["variants", pyd.to_str().unwrap()],
    );
    assert!(
        drv.status.success(),
        "{}",
        String::from_utf8_lossy(&drv.stderr)
    );
    let hooks = HookSet::from_host_dir(&o.bundle.join("dm/host"), false).unwrap();
    let (mut same, mut refused) = (0, 0);
    let mut kinds = std::collections::BTreeSet::new();
    for e in fs::read_dir(&pyd).unwrap().flatten() {
        let v = e.file_name().to_string_lossy().into_owned();
        let (pv, rv) = (pyd.join(&v), rsd.join(&v));
        let ex = rv.join("extract");
        let mut cfg = DeriveConfig::new(&ex, &o.bundle);
        cfg.romfs = Some(ex.join("romfs_manifest.tsv"));
        cfg.profile = Some(rv.join("profile.tsv"));
        cfg.hooks = Some(hooks.clone());
        let r = derive(&cfg, &mut |_| {});
        let py_refused = !pv.join("profile.tsv").exists();
        assert_eq!(
            py_refused,
            r.is_err(),
            "{v}: python refused={py_refused}, rust {:?}",
            r.as_ref().err()
        );
        if py_refused {
            refused += 1;
            continue;
        }
        let norm = |p: PathBuf, root: &Path| {
            fs::read_to_string(p)
                .unwrap()
                .replace(&root.display().to_string(), "ROOT")
        };
        assert_eq!(
            norm(pv.join("profile.tsv"), &pv),
            norm(rv.join("profile.tsv"), &rv),
            "{v}: profile.tsv differs"
        );
        let (pd, rd) = (
            fs::read_dir(pv.join("profile-derived"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
            fs::read_dir(rv.join("profile-derived"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        );
        for f in fs::read_dir(&pd).unwrap().flatten() {
            let n = f.file_name();
            assert_eq!(
                norm(pd.join(&n), &pv),
                norm(rd.join(&n), &rv),
                "{v}: {} differs",
                n.to_string_lossy()
            );
        }
        kinds.insert(
            fs::read_to_string(pd.join("game.tsv"))
                .unwrap()
                .lines()
                .filter(|l| l.starts_with("# ") && l.len() > 40)
                .count(),
        );
        same += 1;
    }
    assert!(same >= 40, "only {same} variants compared");
    eprintln!(
        "OK   {same} synthetic variants byte-identical to the Python's ({refused} refused by both: no title ID)"
    );
}

#[test]
fn compare_profile_text_matches_python() {
    let Some(o) = oracle("compare_profile.py") else {
        return;
    };
    let w = work("compare");
    let old = PathBuf::from(format!("{IOSBUILDER}/out-prof"));
    if !old.join("derived-game.tsv").exists() {
        ibuilder_testkit::skip("compare_profile: out-prof fixtures absent");
        return;
    }
    // an "orchestrator" game.tsv made from the derived one: a changed value, a key the app cannot derive, a repeated key
    let derived = fs::read_to_string(old.join("derived-game.tsv")).unwrap();
    let orch = derived.replace("package\tv0", "package\tv1")
        + "# ---- Address region check ----\ndepth_mode\tzero\nbuild\tv9\t2.0\tmain=aa\n";
    fs::write(w.join("orch.tsv"), orch).unwrap();
    let a = |n: &str| old.join(n).display().to_string();
    let cases: Vec<Vec<String>> = vec![
        vec![
            a("derived-game.tsv"),
            w.join("orch.tsv").display().to_string(),
        ],
        vec![
            a("derived-game.tsv"),
            w.join("orch.tsv").display().to_string(),
            a("derived-profile.tsv"),
            a("harness-profile.tsv"),
        ],
        vec![
            a("derived-game.tsv"),
            a("derived-game.tsv"),
            a("derived-profile.tsv"),
            w.join("orch.tsv").display().to_string(),
        ],
    ];
    for args in cases {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let py = o.run(&o.gameapp.join("compare_profile.py"), &argv);
        assert!(
            py.status.success(),
            "{}",
            String::from_utf8_lossy(&py.stderr)
        );
        let rs = ibuilder_profile::compare::main_text(&args).unwrap();
        assert_eq!(
            String::from_utf8(py.stdout).unwrap(),
            rs,
            "compare output differs for {args:?}"
        );
    }
    eprintln!("OK   compare_profile: 3 comparisons print the Python's text byte for byte");
}

#[test]
fn host_hooks_match_python() {
    let Some(o) = oracle("host_hooks") else {
        return;
    };
    let host = o.bundle.join("dm/host");
    let script = "import sys; sys.dont_write_bytecode = True; sys.path.insert(0, sys.argv[1]); import nso_syms\nfor n in sorted(nso_syms.host_hooks(sys.argv[2], forced=sys.argv[3] == '1')): print(n)\n";
    for forced in [false, true] {
        let out = Command::new(&o.py)
            .arg("-B")
            .arg("-c")
            .arg(script)
            .arg(o.bundle.join("dm/tools"))
            .arg(&host)
            .arg(if forced { "1" } else { "0" })
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        let want: std::collections::BTreeSet<String> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let got = HookSet::from_host_dir(&host, forced).unwrap().0;
        assert_eq!(want, got, "host hooks (forced={forced}) differ");
        eprintln!(
            "OK   host_hooks(forced={forced}): {} names identical",
            got.len()
        );
    }
}
