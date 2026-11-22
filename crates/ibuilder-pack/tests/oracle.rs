//! Oracle tests against the Python `pack_nxgame.py` / `compare_nxgame.py` (and `json.dumps`). Every case runs both
//! implementations on the same inputs and requires identical trees (byte for byte), stdout and exit status.
//! A missing oracle or fixture skips the case with a message.
use ibuilder_pack::json::{Dump, dumps, loads};
use ibuilder_pack::{PackArgs, compare_files_cli, pack};
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

fn tool(name: &str) -> Option<PathBuf> {
    let a = PathBuf::from(format!("{SCRATCH}/oracle/tools/{name}"));
    if a.exists() {
        return Some(a);
    }
    let b = PathBuf::from(format!("{IOSBUILDER}/tools/{name}"));
    b.exists().then_some(b)
}

fn work(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("pack-oracle")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

struct Rng(u64);
impl Rng {
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n)
            .map(|_| {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0 as u8
            })
            .collect()
    }
}

/// The synthetic input tree (same shape as the one used for the manual runs).
fn make_inputs(root: &Path) {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for d in [
        "dm/sub",
        "aoc/1",
        "extra/Headers",
        "msl",
        "fw/Headers",
        "fw/dm",
    ] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("exe"), r.bytes(300_000)).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(r.bytes(5000));
    fs::write(root.join("icon.png"), png).unwrap();
    fs::write(root.join("msl/b-0c434f1b68df.img"), r.bytes(4097)).unwrap();
    for n in [
        "c1_full.tsv",
        "cb_extent.tsv",
        "shader_map.tsv",
        "pso_manifest.tsv",
    ] {
        fs::write(root.join("dm").join(n), format!("# {n}\nrow\t1\n")).unwrap();
    }
    fs::write(root.join("dm/launcher.json"), "{\"schema\": 1}\n").unwrap();
    fs::write(root.join("dm/sub/deep.tsv"), "deep\n").unwrap();
    fs::write(root.join("dm.txt"), "sorts after dm/ component-wise\n").unwrap();
    fs::write(root.join("dm-x"), "x\n").unwrap();
    fs::write(root.join("extra/Headers/H-Swift.h"), "// h\n").unwrap();
    fs::write(root.join("extra/.DS_Store"), b"junk").unwrap();
    fs::write(root.join("extra/._res"), b"junk").unwrap();
    fs::write(root.join("aoc/1/1.img"), vec![b'a'; 1000]).unwrap();
    fs::write(root.join("aoc/.DS_Store"), vec![b'z'; 1000]).unwrap();
    fs::write(
        root.join("nro.tsv"),
        "# header\nrefused\tx\na.nro\t1\n\nb.nro\n#c\n",
    )
    .unwrap();
    let p = "# Runtime game profile of 0100DFA027504000 (Grizzy\u{2122} Party), written by ...\ntitle_id\t0100dfa027504000\nname\tGrizzy\u{2122} & \"Lemmings\" \\ \u{e9}\nromfs_marker\tData/x.resS\nmodules\tmain subsdk0 sdk\n  \n#comment\ndisplay_version\tabc\t1.0.0\ndisplay_version\tdef\t1.1.0\n";
    fs::write(root.join("profile.tsv"), p).unwrap();
    fs::write(
        root.join("profile-sw2.tsv"),
        p.replace("Party)", "Party) (Switch 2, 12 GB)"),
    )
    .unwrap();
    // fw/: a framework folder to copy as a whole
    fs::write(root.join("fw/DmGame0100DFA027504000"), r.bytes(1234)).unwrap();
    fs::write(root.join("fw/Headers/a.h"), "hdr\n").unwrap();
    fs::write(root.join("fw/game.manifest.json"), "old\n").unwrap();
    fs::write(root.join("fw/Info.plist"), "plist").unwrap();
    fs::write(root.join("fw/dm/t.tsv"), "t\n").unwrap();
    // a synthetic DMROMFS1 image: header + data + an index of odd length (partial last word)
    let data = r.bytes(777);
    let index = r.bytes(24 * 3 + 31);
    let mut img = Vec::new();
    img.extend_from_slice(b"DMROMFS1");
    img.extend_from_slice(&(32 + data.len() as u64).to_le_bytes());
    img.extend_from_slice(&(index.len() as u64).to_le_bytes());
    img.extend_from_slice(&3u64.to_le_bytes());
    img.extend(data);
    img.extend(index);
    fs::write(root.join("RomFS.img"), img).unwrap();
    fs::write(root.join("badromfs.img"), b"DMROMFS1\0\0\0\0\0\0\0\0\0\0").unwrap();
}

/// Relative path -> bytes for every file under `root`, sorted.
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut es: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        es.sort();
        for p in es {
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                out.push((
                    p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                    fs::read(&p).unwrap(),
                ));
            }
        }
    }
    let mut v = Vec::new();
    if root.exists() {
        walk(root, root, &mut v);
    }
    v
}

fn run_case(name: &str, inputs: &Path, args: &[&str], json_flag: bool) {
    let (Some(py), Some(script)) = (python(), tool("pack_nxgame.py")) else {
        ibuilder_testkit::skip(&format!("{name}: Python oracle not found"));
        return;
    };
    let w = work(name);
    let (po, ro) = (w.join("py").join("o.nxgame"), w.join("rs").join("o.nxgame"));
    // --- python
    let mut c = Command::new(py);
    c.arg("-B")
        .arg(script)
        .arg("--out")
        .arg(&po)
        .current_dir(inputs)
        .env("PYTHONDONTWRITEBYTECODE", "1");
    c.args(args);
    if json_flag {
        c.arg("--json");
    }
    let pout = c.output().unwrap();
    // --- rust (same relative argument spelling, same cwd)
    let mut pa = PackArgs::new(&ro);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut v = || it.next().unwrap().to_string();
        match *a {
            "--profile" => pa.profile = Some(inputs.join(v())),
            "--framework" => pa.framework = Some(inputs.join(v())),
            "--framework-name" => pa.framework_name = Some(v()),
            "--dm-dir" => pa.dm_dir = Some(inputs.join(v())),
            "--msl" => pa.msl = Some(inputs.join(v())),
            "--icon" => pa.icon = Some(inputs.join(v())),
            "--extra" => {
                let s = v();
                let (a, b) = s.split_once('=').unwrap_or((&s, ""));
                pa.extra.push(if s.contains('=') {
                    format!("{}={b}", inputs.join(a).display())
                } else {
                    s.clone()
                });
            }
            "--romfs-img" => pa.romfs_img = Some(inputs.join(v())),
            "--aoc" => pa.aoc = Some(inputs.join(v())),
            "--nro-table" => pa.nro_table = Some(inputs.join(v())),
            "--game" => pa.game = Some(v()),
            "--name" => pa.name = Some(v()),
            "--display-version" => pa.display_version = Some(v()),
            "--package-version" => pa.package_version = Some(v()),
            "--modules" => pa.modules = Some(v()),
            "--romfs-marker" => pa.romfs_marker = Some(v()),
            "--romfs-identity" => pa.romfs_identity = Some(v()),
            "--chain" => pa.chain = Some(v()),
            "--min-os" => pa.min_os = v(),
            "--loader" => pa.loader = v(),
            "--created-by" => pa.created_by = v(),
            "--no-console" => pa.no_console = true,
            "--manifest-only" => pa.manifest_only = true,
            "--force" => pa.force = true,
            "--clone" => pa.clone = true,
            other => panic!("test bug: {other}"),
        }
    }
    let r = pack(&pa);
    let py_ok = pout.status.success();
    assert_eq!(
        py_ok,
        r.is_ok(),
        "{name}: exit status differs: python {:?} / rust {:?}\npython stderr: {}",
        pout.status.code(),
        r.as_ref().err(),
        String::from_utf8_lossy(&pout.stderr)
    );
    // the Rust side is handed absolute input paths, Python relative ones (cwd = inputs): strip the prefix
    let prefix = format!("{}/", inputs.display());
    let norm = |s: &str, d: &Path| {
        s.replace(&d.display().to_string(), "OUT")
            .replace(&prefix, "")
    };
    match r {
        Ok(o) => {
            let mut lines = o.stdout_lines();
            if json_flag {
                lines.push(o.json_summary());
            }
            let rs_out = norm(&(lines.join("\n") + "\n"), &ro);
            let py_out = norm(&String::from_utf8_lossy(&pout.stdout), &po);
            assert_eq!(py_out, rs_out, "{name}: stdout differs");
            assert_eq!(
                tree(&w.join("py")),
                tree(&w.join("rs")),
                "{name}: output trees differ"
            );
        }
        Err(e) => {
            let py_err = norm(&String::from_utf8_lossy(&pout.stderr), &po);
            let rs_err = norm(&format!("{e}\n"), &ro);
            assert_eq!(py_err, rs_err, "{name}: error message differs");
        }
    }
    eprintln!("OK   {name}");
}

#[test]
fn pack_matches_python_on_synthetic_tree() {
    if python().is_none() || tool("pack_nxgame.py").is_none() {
        ibuilder_testkit::skip("Python oracle absent");
        return;
    }
    let inp = work("inputs");
    make_inputs(&inp);
    let full = [
        "--profile",
        "profile.tsv",
        "--framework",
        "exe",
        "--dm-dir",
        "dm",
        "--msl",
        "msl/b-0c434f1b68df.img",
        "--icon",
        "icon.png",
        "--romfs-img",
        "RomFS.img",
        "--aoc",
        "aoc",
        "--nro-table",
        "nro.tsv",
        "--package-version",
        "v65536",
        "--extra",
        "dm.txt=dm.txt",
        "--extra",
        "dm-x=dm-x",
        "--extra",
        "extra=extra",
    ];
    run_case("full", &inp, &full, true);
    run_case(
        "manifest_only",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--package-version",
            "v0",
            "--chain",
            "b-1234",
            "--manifest-only",
        ],
        true,
    );
    run_case(
        "manifest_only_romfs",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--package-version",
            "v0",
            "--chain",
            "b-1234",
            "--manifest-only",
            "--romfs-img",
            "RomFS.img",
        ],
        false,
    );
    run_case(
        "switch2",
        &inp,
        &[
            "--profile",
            "profile-sw2.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--msl",
            "msl/b-0c434f1b68df.img",
        ],
        false,
    );
    run_case(
        "no_console",
        &inp,
        &[
            "--profile",
            "profile-sw2.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
            "--no-console",
        ],
        false,
    );
    run_case(
        "no_profile",
        &inp,
        &[
            "--framework",
            "exe",
            "--package-version",
            "v7",
            "--chain",
            "b-1",
            "--game",
            "0100aa0000000000",
            "--name",
            "My Game",
            "--modules",
            "main sdk",
            "--display-version",
            "2.0",
            "--romfs-marker",
            "x/y",
            "--romfs-identity",
            "DMROMFSID1 size=5 index=ab",
            "--min-os",
            "18.0",
            "--loader",
            "FR",
            "--created-by",
            "me",
        ],
        false,
    );
    run_case(
        "fw_dir",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "fw",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
            "--dm-dir",
            "dm",
        ],
        true,
    );
    run_case(
        "framework_name",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
            "--framework-name",
            "MyFw",
            "--game",
            "0100DFA027504000",
        ],
        false,
    );
    run_case(
        "aoc_only_junk",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
            "--aoc",
            "extra",
        ],
        false,
    );
    run_case(
        "clone",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
            "--romfs-img",
            "RomFS.img",
            "--clone",
        ],
        false,
    );
}

#[test]
fn pack_refusals_match_python() {
    if python().is_none() || tool("pack_nxgame.py").is_none() {
        ibuilder_testkit::skip("Python oracle absent");
        return;
    }
    let inp = work("inputs-refuse");
    make_inputs(&inp);
    let ok = [
        "--profile",
        "profile.tsv",
        "--framework",
        "exe",
        "--package-version",
        "v0",
        "--chain",
        "b-1",
    ];
    let with = |extra: &[&'static str]| -> Vec<&'static str> {
        let mut v: Vec<&'static str> = vec![
            "--profile",
            "profile.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
        ];
        v.extend_from_slice(extra);
        v
    };
    let _ = ok;
    run_case(
        "err_no_pkgver",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "exe",
            "--chain",
            "b-1",
        ],
        false,
    );
    run_case(
        "err_no_chain",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "exe",
            "--package-version",
            "v0",
        ],
        false,
    );
    run_case("err_bad_chain", &inp, &with(&["--chain", "b-"])[..], false);
    run_case("err_bad_game", &inp, &with(&["--game", "12G"]), false);
    run_case(
        "err_no_name",
        &inp,
        &[
            "--framework",
            "exe",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
            "--game",
            "0100AA0000000000",
            "--modules",
            "main",
        ],
        false,
    );
    run_case(
        "err_bad_modules",
        &inp,
        &with(&["--modules", "main sub_sdk"]),
        false,
    );
    run_case(
        "err_no_framework",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
        ],
        false,
    );
    run_case(
        "err_missing_framework",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "nope",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
        ],
        false,
    );
    run_case(
        "err_bad_romfs",
        &inp,
        &with(&["--romfs-img", "badromfs.img"]),
        false,
    );
    run_case(
        "err_bad_extra",
        &inp,
        &with(&["--extra", "noequals"]),
        false,
    );
    run_case(
        "err_fw_dir_no_exec",
        &inp,
        &[
            "--profile",
            "profile.tsv",
            "--framework",
            "dm",
            "--package-version",
            "v0",
            "--chain",
            "b-1",
        ],
        false,
    );
}

/// Re-pack the real Grizzy package from its own pieces (read-only under iOSBuilder/out-pkg) and require the
/// Rust `package.json`, manifest and framework tree to equal the files the Python wrote on 2026-11-16.
#[test]
fn repack_real_grizzy_package_equals_the_original() {
    let pk = PathBuf::from(format!("{IOSBUILDER}/out-pkg/0100DFA027504000.nxgame"));
    let fw = pk.join("DmGame0100DFA027504000.framework");
    let exe = PathBuf::from(format!(
        "{IOSBUILDER}/out-asm/DmGameProbe.framework/DmGameProbe"
    ));
    let icon = PathBuf::from(format!("{IOSBUILDER}/out-pkg/grizzy-icon.png"));
    if !(pk.join("package.json").exists()
        && fw.exists()
        && exe.exists()
        && icon.exists()
        && pk.join("romfs.img").exists())
    {
        ibuilder_testkit::skip("real package: out-pkg/out-asm fixtures absent");
        return;
    }
    let w = work("real");
    let out = w.join("0100DFA027504000.nxgame");
    let mut a = PackArgs::new(&out);
    a.force = true;
    a.clone = true; // 2.5 GB RomFS: APFS clone
    a.framework = Some(exe);
    a.profile = Some(fw.join("profile.tsv"));
    a.dm_dir = Some(fw.join("dm"));
    a.icon = Some(icon);
    a.romfs_img = Some(pk.join("romfs.img"));
    a.chain = Some("b-0c434f1b68df".into());
    a.package_version = Some("v0".into());
    let o = pack(&a).unwrap();
    assert_eq!(
        fs::read(pk.join("package.json")).unwrap(),
        fs::read(out.join("package.json")).unwrap(),
        "package.json differs from the original"
    );
    let ofw = out.join("DmGame0100DFA027504000.framework");
    assert_eq!(
        tree(&fw),
        tree(&ofw),
        "framework tree differs from the original"
    );
    assert_eq!(o.romfs_size, 2_498_033_693);
    let m = o
        .manifest
        .get("romfs_identity")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(
        m,
        "DMROMFSID1 title=0100DFA027504000 version=1.0.0 build=v0 size=2498033693 index=16253e096bc20e18"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// compare_nxgame.py
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn compare_matches_python() {
    let (Some(py), Some(script)) = (python(), tool("compare_nxgame.py")) else {
        ibuilder_testkit::skip("Python oracle absent");
        return;
    };
    let w = work("compare");
    let base = "{\n  \"schema\": 1,\n  \"title_id\": \"0100DFA027504000\",\n  \"name\": \"Grizzy\u{2122}\",\n  \"modules\": [\"main\", \"sdk\"],\n  \"nro_count\": 0,\n  \"flag\": true,\n  \"ratio\": 1.5,\n  \"obj\": {\"a\": 1, \"b\": [1, 2]},\n  \"console\": \"switch1\"\n}\n";
    let cands: Vec<(&str, String)> = vec![
        ("same", base.to_string()),
        ("diff_value", base.replace("0100DFA027504000", "0100AAAAAAAAAAAA")),
        ("diff_list", base.replace("\"main\", \"sdk\"", "\"main\", \"sdk\", \"x\"")),
        ("missing_key", base.replace("  \"console\": \"switch1\"\n", "").replace("\"b\": [1, 2]},", "\"b\": [1, 2]}")),
        ("extra_key", base.replace("\"schema\": 1,", "\"schema\": 1,\n  \"gfx_api\": \"metal\",")),
        ("type_mismatch", base.replace("\"nro_count\": 0", "\"nro_count\": \"0\"").replace("\"flag\": true", "\"flag\": 1")),
        ("order", "{\"title_id\": \"0100DFA027504000\", \"schema\": 1, \"name\": \"Grizzy\u{2122}\", \"modules\": [\"main\", \"sdk\"], \"nro_count\": 0, \"flag\": true, \"ratio\": 1.5, \"obj\": {\"b\": [1, 2], \"a\": 1}, \"console\": \"switch1\"}".to_string()),
        ("float_int", base.replace("\"nro_count\": 0", "\"nro_count\": 0.0")),
    ];
    fs::write(w.join("ref.json"), base).unwrap();
    for (name, text) in &cands {
        fs::write(w.join(format!("{name}.json")), text).unwrap();
        for reasons in [
            vec![],
            vec![
                ("title_id", "another game"),
                ("console", "x"),
                ("gfx_api", "newer"),
                ("modules", "m"),
                ("nro_count", "n"),
                ("flag", "f"),
            ],
        ] {
            let tag = if reasons.is_empty() {
                "unnamed"
            } else {
                "named"
            };
            let (r, c) = (w.join("ref.json"), w.join(format!("{name}.json")));
            let jp = w.join(format!("{name}-{tag}-py.json"));
            let jr = w.join(format!("{name}-{tag}-rs.json"));
            let mut cmd = Command::new(&py);
            cmd.arg("-B")
                .arg(&script)
                .arg(&r)
                .arg(&c)
                .args(["--label", "t\u{e9}st"])
                .arg("--json-out")
                .arg(&jp)
                .env("PYTHONDONTWRITEBYTECODE", "1");
            for (k, t) in &reasons {
                cmd.arg("--reason").arg(format!("{k}={t}"));
            }
            let po = cmd.output().unwrap();
            let rs: Vec<(String, String)> = reasons
                .iter()
                .map(|(k, t)| (k.to_string(), t.to_string()))
                .collect();
            let mut out = String::new();
            let code = compare_files_cli(&r, &c, &rs, "t\u{e9}st", Some(&jr), &mut out).unwrap();
            let norm = |s: &str, side: &Path| s.replace(&side.display().to_string(), "J");
            assert_eq!(po.status.code(), Some(code), "{name}/{tag}: exit code");
            assert_eq!(
                norm(&String::from_utf8_lossy(&po.stdout), &jp),
                norm(&out, &jr),
                "{name}/{tag}: stdout differs"
            );
            assert_eq!(
                fs::read_to_string(&jp)
                    .unwrap()
                    .replace(&jp.display().to_string(), "J"),
                fs::read_to_string(&jr)
                    .unwrap()
                    .replace(&jr.display().to_string(), "J"),
                "{name}/{tag}: --json-out differs"
            );
        }
    }
    eprintln!("OK   compare: {} candidates x 2 reason sets", cands.len());
}

#[test]
fn compare_finds_manifests_in_folders_like_python() {
    let w = work("find");
    let fwd = w.join("a.nxgame/DmGame1.framework");
    fs::create_dir_all(&fwd).unwrap();
    fs::write(fwd.join("game.manifest.json"), "{\"schema\": 1}").unwrap();
    fs::create_dir_all(w.join("b/deep/x")).unwrap();
    fs::write(w.join("b/deep/x/game.manifest.json"), "{\"schema\": 1}").unwrap();
    assert_eq!(
        ibuilder_pack::find_manifest(&w.join("a.nxgame")).unwrap(),
        fwd.join("game.manifest.json")
    );
    assert_eq!(
        ibuilder_pack::find_manifest(&fwd).unwrap(),
        fwd.join("game.manifest.json")
    );
    assert_eq!(
        ibuilder_pack::find_manifest(&w.join("b")).unwrap(),
        w.join("b/deep/x/game.manifest.json")
    );
    assert!(
        ibuilder_pack::find_manifest(&w.join("nothing"))
            .unwrap_err()
            .starts_with("[RED] compare_nxgame: no game.manifest.json under")
    );
}

// ---------------------------------------------------------------------------------------------------------------
// json.dumps spelling
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn json_dumps_matches_python() {
    let Some(py) = python() else {
        ibuilder_testkit::skip("Python oracle absent");
        return;
    };
    let docs = [
        r#"{"a": 1, "b": [1, 2.5, -0.0, 1e16, 1e-5, 123456789.125, 3.0], "c": {"z": null, "y": [], "x": {}}, "d": "caf\u00e9 \u2122 \ud83d\ude00 \"q\" \\ \n \t \u0001 \u007f"}"#,
        r#"[true, false, null, 0, -12345678901234567890, "", "\u2028"]"#,
        r#"{"\u00e9": {"b": [], "a": [[]]}, "A": 1, "a": 2}"#,
        r#""just a string""#,
        r#"[1e300, 1.5e-300, 0.1, 0.30000000000000004, 100000.0, 1234567890123456.0, 12345678901234567.0]"#,
    ];
    let script = "import json,sys\nfor t in sys.stdin.read().split('\\x1e'):\n    v=json.loads(t)\n    print(json.dumps(v))\n    print(json.dumps(v,indent=2))\n    print(json.dumps(v,indent=2,ensure_ascii=False))\n    print(json.dumps(v,indent=2,sort_keys=True,ensure_ascii=False,separators=(',',' : ')).replace('\\n','\\\\n'))\n";
    let mut child = Command::new(py)
        .arg("-B")
        .arg("-c")
        .arg(script)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(docs.join("\u{1e}").as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    // each doc printed 3 single-line... the indented ones span several lines: compare by rebuilding the expected text
    let mut expected = String::new();
    for d in &docs {
        let v = loads(d).unwrap();
        expected.push_str(&dumps(&v, &Dump::compact()));
        expected.push('\n');
        expected.push_str(&dumps(&v, &Dump::indented(2, true)));
        expected.push('\n');
        expected.push_str(&dumps(&v, &Dump::indented(2, false)));
        expected.push('\n');
        expected.push_str(&dumps(&v, &Dump::swift_pretty()).replace('\n', "\\n"));
        expected.push('\n');
    }
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
}
