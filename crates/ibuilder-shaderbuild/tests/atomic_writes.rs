//! The tables and packages this crate writes for a LATER step (review r2, N7): `live_package`'s `<st>_live.bin`, `live_map.tsv` and `live_facts.tsv` (the partial build reads them next), and the build's
//! `c1_full.tsv` / `cb_extent.tsv` (stage 3 of the pipeline copies them into the shader pack after the build returns). They are replaced whole (`cache::write_atomic`: a temporary name beside, then a
//! rename), not truncated in place: when the last step cannot be done, the file that was there stays and nothing is left beside it. A valid run writes the same bytes as before.
//!
//! The blocked step: the temporary name is taken by a folder (`<file>.tmp-<pid>-ThreadId<n>`, the name `cache::write_atomic` picks for the calling thread), so the temporary file cannot be created
//! while the target itself stays writable: an atomic writer fails and leaves the old content, an in-place one would overwrite it. (If the naming ever changes, the folder blocks nothing, the write
//! succeeds, and the "must fail" assertions below fail loudly.) No fixture is needed: the profile and the packages are made here.

use ibuilder_shader::romfs::Image;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use ibuilder_shaderbuild::finish::{Finished, Finisher};
use ibuilder_shaderbuild::game::Game;
use ibuilder_shaderbuild::live::{LiveEntry, ids, live_package, live_translate};
use ibuilder_shaderbuild::tools::FnTools;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, build};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ibsb-atomic-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// `path` with the temporary suffix `cache::write_atomic` gives the calling thread
fn tmp_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(
        format!(
            ".tmp-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )
        .replace(['(', ')'], ""),
    );
    PathBuf::from(s)
}

/// `path` holds "OLD" and cannot be replaced through a temporary file beside it
fn seeded_and_blocked(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "OLD").unwrap();
    fs::create_dir(tmp_of(path)).unwrap();
}

/// the temporary names left in `dir` apart from the folders a test made on purpose
fn stray_tmp_files(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-") && !e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// a pixel shader the way the host hands it over: a graphics control section (not NVN2), a code area with the GFX header and an SPH word that says "pixel"
fn ps_entry() -> LiveEntry {
    let mut ctl = vec![0u8; 0x100];
    ctl[..4].copy_from_slice(&ids::CTL_MAGIC);
    ctl[4] = 1;
    let mut code = vec![0u8; 0x40];
    code[..4].copy_from_slice(&ids::GFX_MAGIC);
    put32(&mut code, 0x30, 5 << 10);
    LiveEntry {
        key: ids::skey(&ctl, &code, (0, 0)),
        ctl,
        code,
    }
}

/// a compute shader: the CS header, a thread group of 8 x 8 x 1 and 100 bytes of shared memory
fn cs_entry() -> LiveEntry {
    let mut ctl = vec![0u8; 0x800];
    ctl[..4].copy_from_slice(&ids::CTL_MAGIC);
    ctl[4] = 1;
    for (o, v) in [(0x748, 8), (0x74c, 8), (0x750, 1), (0x754, 100)] {
        put32(&mut ctl, o, v);
    }
    let mut code = vec![0u8; 0x20];
    code[..4].copy_from_slice(&ids::CS_MAGIC);
    LiveEntry {
        key: ids::skey(&ctl, &code, (0, 0)),
        ctl,
        code,
    }
}

/// the split's record format (`BTCDBTCD`, the key, the two lengths at +0x28 / +0x2c, the code area, the control section), written out here from its description
fn record(e: &LiveEntry) -> Vec<u8> {
    let mut r = b"BTCDBTCD".to_vec();
    r.extend((0..8).map(|i| u8::from_str_radix(&e.key[2 * i..2 * i + 2], 16).unwrap()));
    r.extend([0u8; 0x28]);
    r[0x28..0x2c].copy_from_slice(&(e.ctl.len() as u32).to_le_bytes());
    r[0x2c..0x30].copy_from_slice(&(e.code.len() as u32).to_le_bytes());
    r.extend_from_slice(&e.code);
    r.extend_from_slice(&e.ctl);
    r
}

fn game(dir: &Path) -> Game {
    Game {
        dir: dir.to_path_buf(),
        id: "G".into(),
        rows: Vec::new(),
    }
}

#[test]
fn live_package_writes_the_same_files_as_before() {
    let d = scratch("live-same");
    let (pkg, tables) = (d.join("pkg"), d.join("tables"));
    let entries = vec![ps_entry(), cs_entry()];
    let (ok, bad) = live_package(&entries, &game(&d), &pkg, &tables).unwrap();
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(ok.values().map(String::as_str).collect::<Vec<_>>().len(), 2);
    assert_eq!(
        fs::read(pkg.join("ps_live.bin")).unwrap(),
        record(&entries[0])
    );
    assert_eq!(
        fs::read(pkg.join("cs_live.bin")).unwrap(),
        record(&entries[1])
    );
    assert_eq!(
        fs::read_to_string(tables.join("live_map.tsv")).unwrap(),
        format!(
            "ps_live.bin\t0\tpixel\t{}\tlive\ncs_live.bin\t0\tcompute\t{}\tlive\n",
            entries[0].key, entries[1].key
        )
    );
    assert_eq!(
        fs::read_to_string(tables.join("live_facts.tsv")).unwrap(),
        format!("{}\t{{\"local\":[8,8,1],\"shared\":112}}\n", entries[1].key)
    );
    assert!(
        stray_tmp_files(&pkg).is_empty() && stray_tmp_files(&tables).is_empty(),
        "{:?} {:?}",
        stray_tmp_files(&pkg),
        stray_tmp_files(&tables)
    );
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn live_package_files_are_replaced_whole_or_not_at_all() {
    let entries = vec![ps_entry(), cs_entry()];
    let mut wrong = Vec::new(); // every file is tried, so a failure lists every site that still writes in place
    for (name, rel) in [
        ("bin", "pkg/ps_live.bin"),
        ("map", "tables/live_map.tsv"),
        ("facts", "tables/live_facts.tsv"),
    ] {
        let d = scratch(&format!("live-blocked-{name}"));
        let target = d.join(rel);
        seeded_and_blocked(&target);
        fs::create_dir_all(d.join("pkg")).unwrap();
        fs::create_dir_all(d.join("tables")).unwrap();
        let r = live_package(&entries, &game(&d), &d.join("pkg"), &d.join("tables"));
        if r.is_ok() {
            wrong.push(format!("{rel}: the write goes through a temporary file, which cannot be made here: the call must fail"));
        }
        if fs::read(&target).ok().as_deref() != Some(&b"OLD"[..]) {
            wrong.push(format!("{rel}: the file that was there is not intact"));
        }
        if !stray_tmp_files(target.parent().unwrap()).is_empty() {
            wrong.push(format!(
                "{rel}: left beside it: {:?}",
                stray_tmp_files(target.parent().unwrap())
            ));
        }
        let _ = fs::remove_dir_all(&d);
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// `live_translate` merges the live rows into the tables of its work folder (the shader map and the pair tables are rewritten, the facts appended) before the partial build reads them; the
/// finisher factory stops it right after that, so nothing of the build itself is needed. What it writes there is what the rows say, and no temporary file is left.
#[test]
fn live_translate_writes_the_merged_tables_as_before() {
    let d = scratch("live-merge");
    let tables = d.join("tables");
    fs::create_dir_all(&tables).unwrap();
    let entries = vec![ps_entry(), cs_entry()];
    let (k1, k2) = (&entries[0].key, &entries[1].key);
    fs::write(tables.join("game.tsv"), "title_id\tG\n").unwrap();
    fs::write(tables.join("shader_map.tsv"), format!("package\tindex\tstage\tdm_ctl_sha16\tsource\nps_a.bin\t0\tpixel\t1111111111111111\tx\nps_old.bin\t3\tpixel\t{k1}\told\n")).unwrap();
    fs::write(
        tables.join("shader_pairs.tsv"),
        format!("# a header\nvvvvvvvvvvvvvvvv pppppppppppppppp\n{k1} qqqqqqqqqqqqqqqq\n"),
    )
    .unwrap();
    fs::write(
        tables.join("pso_pairs.tsv"),
        format!("{k2} rrrrrrrrrrrrrrrr\n# kept\n"),
    )
    .unwrap();
    fs::write(tables.join("shader_facts.tsv"), "FACTS-OLD\n").unwrap();
    let work = d.join("work");
    let tools = FnTools::new(
        Arc::new(|_: &[String]| ToolRun::ok()),
        Arc::new(|_: &[String]| ToolRun::ok()),
    );
    let make =
        |_: &Path, _: &str| -> Result<Box<dyn Finisher + '_>, String> { Err("stop here".into()) };
    let log = |_: &str| {};
    let t: &dyn ShaderTools = &tools;
    let r = live_translate(
        &entries, &tables, &tables, "G", &work, "c0ffee", t, &make, &log, None,
    );
    assert_eq!(r.err().as_deref(), Some("stop here"));
    let g = work.join("profile/G");
    assert_eq!(
        fs::read_to_string(g.join("shader_map.tsv")).unwrap(),
        format!(
            "package\tindex\tstage\tdm_ctl_sha16\tsource\nps_a.bin\t0\tpixel\t1111111111111111\tx\nps_live.bin\t0\tpixel\t{k1}\tlive\ncs_live.bin\t0\tcompute\t{k2}\tlive\n"
        ),
        "the old row of a live key goes, the live rows follow"
    );
    assert_eq!(
        fs::read_to_string(g.join("shader_pairs.tsv")).unwrap(),
        "# a header\nvvvvvvvvvvvvvvvv pppppppppppppppp\n"
    );
    assert_eq!(
        fs::read_to_string(g.join("pso_pairs.tsv")).unwrap(),
        "# kept\n"
    );
    assert_eq!(
        fs::read_to_string(g.join("shader_facts.tsv")).unwrap(),
        format!("FACTS-OLD\n{k2}\t{{\"local\":[8,8,1],\"shared\":112}}\n")
    );
    assert!(stray_tmp_files(&g).is_empty(), "{:?}", stray_tmp_files(&g));
    let _ = fs::remove_dir_all(&d);
}

/// a finisher for a build of no shader: the driver only asks it for its fingerprint
struct NoShaders;
impl Finisher for NoShaders {
    fn fingerprint(&self) -> String {
        "test: no shaders".into()
    }
    fn finish(
        &self,
        _key: &str,
        _work: &Path,
        _job: &mut Value,
        _c1: &[u8],
        _tools: &dyn ShaderTools,
        _paired_locs: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        Finished::failed("unported: this test builds no shader", "")
    }
}

/// a profile of a game whose shader map has no rows, and what a build of it needs around it
fn empty_profile(root: &Path) -> PathBuf {
    let g = root.join("profile").join("G");
    fs::create_dir_all(&g).unwrap();
    fs::write(g.join("game.tsv"), "title_id\tG\n").unwrap();
    fs::write(
        g.join("shader_map.tsv"),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\n",
    )
    .unwrap();
    fs::write(g.join("shader_facts.tsv"), "").unwrap();
    fs::write(g.join("shader_pairs.tsv"), "").unwrap();
    fs::create_dir_all(root.join("pkg")).unwrap();
    g
}

fn run_empty_build(root: &Path) -> Result<ibuilder_shaderbuild::BuildReport, String> {
    let o = BuildOptions::new(
        &root.join("pkg"),
        &root.join("profile"),
        "G",
        &root.join("out"),
        "c0ffee",
        &root.join("cas"),
    );
    let tools = FnTools::new(
        Arc::new(|_: &[String]| ToolRun::ok()),
        Arc::new(|_: &[String]| ToolRun::ok()),
    );
    let log = |_: &str| {};
    build(
        &o,
        &BuildEnv {
            tools: &tools,
            finisher: &NoShaders,
            log: &log,
            cancel: None,
        },
    )
}

#[test]
fn the_build_writes_its_tables_whole_and_they_are_the_ones_in_the_image() {
    let root = scratch("build-same");
    empty_profile(&root);
    let rep = run_empty_build(&root).unwrap();
    let built = root.join("out/_work/profile/G");
    let img_bytes = fs::read(&rep.image).unwrap();
    let img = Image::open(&img_bytes).unwrap();
    for name in ["c1_full.tsv", "cb_extent.tsv"] {
        let on_disk = fs::read(built.join(name)).unwrap();
        let f = img
            .files
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the image"));
        assert_eq!(
            img.bytes(f),
            &on_disk[..],
            "{name}: the table stage 3 copies is the one in the library"
        );
        assert!(
            String::from_utf8(on_disk)
                .unwrap()
                .starts_with("# chain b-c0ffee inputs "),
            "{name}"
        );
    }
    assert!(
        stray_tmp_files(&built).is_empty(),
        "{:?}",
        stray_tmp_files(&built)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn the_builds_tables_are_replaced_whole_or_not_at_all() {
    let mut wrong = Vec::new();
    for name in ["c1_full.tsv", "cb_extent.tsv"] {
        let root = scratch(&format!("build-blocked-{}", &name[..2]));
        let g = empty_profile(&root);
        // the build works on a copy of the profile folder: the copy carries the old table and the folder that takes the temporary name
        seeded_and_blocked(&g.join(name));
        let kept = root.join("out/_work/profile/G").join(name);
        let r = run_empty_build(&root);
        if r.is_ok() {
            wrong.push(format!("{name}: the table goes through a temporary file, which cannot be made here: the build must fail, not go on with a truncated table"));
        }
        if fs::read(&kept).ok().as_deref() != Some(&b"OLD"[..]) {
            wrong.push(format!("{name}: the table that was there is not intact"));
        }
        if !stray_tmp_files(kept.parent().unwrap()).is_empty() {
            wrong.push(format!(
                "{name}: left beside it: {:?}",
                stray_tmp_files(kept.parent().unwrap())
            ));
        }
        let _ = fs::remove_dir_all(&root);
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
