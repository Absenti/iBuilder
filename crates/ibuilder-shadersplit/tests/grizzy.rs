//! The whole split of Grizzy and the Lemmings against the Python's output (`<oracle>/{pkg,tables}`), byte for byte, for 1, 2 and 8 threads and for a split resumed from its memo.
//! Inputs (read-only, never copied here): the RomFS image and `main` (`SHADERSPLIT_ROMFS` / `SHADERSPLIT_MAIN` name another copy of them, e.g. when the checkout's `out/ref-img-grizzy` is gone);
//! the oracle folder is `SHADERSPLIT_ORACLE` or `$IBUILDER_SCRATCH_OLD/shader/split-out`. The test skips when one is absent.

use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use std::path::{Path, PathBuf};

/// `$IOSBUILDER/out/ref-img-grizzy/RomFS.img`
fn default_romfs() -> String {
    format!(
        "{}/out/ref-img-grizzy/RomFS.img",
        ibuilder_testkit::IOSBUILDER
    )
}
/// `$IOSBUILDER/out/ref-out-grizzy/ExeFS/main`
fn default_main() -> String {
    ibuilder_testkit::grizzy_exefs("main")
}
/// `$IBUILDER_SCRATCH_OLD/shader/split-out`
fn default_oracle() -> String {
    format!("{}/shader/split-out", ibuilder_testkit::SCRATCH_OLD)
}

const FILES: [&str; 11] = [
    "pkg/cs_data.bin",
    "pkg/cs_main.bin",
    "pkg/gs_data.bin",
    "pkg/ps_data.bin",
    "pkg/ps_main.bin",
    "pkg/vs_data.bin",
    "pkg/vs_main.bin",
    "tables/shader_map.tsv",
    "tables/shader_facts.tsv",
    "tables/shader_pairs.tsv",
    "tables/pso_pairs.tsv",
];

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// the profile text the split reads (`shader_groups`, `shader_note`): derived text, not game data
fn profile(dir: &Path) -> PathBuf {
    let p = dir.join("profile");
    std::fs::create_dir_all(p.join("0100DFA027504000")).unwrap();
    std::fs::write(p.join("0100DFA027504000/game.tsv"), "title_id\t0100DFA027504000\nshader_groups\t/Data/=data\nshader_note\tGrizzy and the Lemmings - Crazy Party shaders\n").unwrap();
    p
}

fn run_once(
    name: &str,
    jobs: usize,
    memo: Option<&Path>,
) -> Option<(PathBuf, ibuilder_shadersplit::SplitStats)> {
    let oracle = std::env::var("SHADERSPLIT_ORACLE").unwrap_or_else(|_| default_oracle());
    let (romfs, main_nso) = (
        std::env::var("SHADERSPLIT_ROMFS").unwrap_or_else(|_| default_romfs()),
        std::env::var("SHADERSPLIT_MAIN").unwrap_or_else(|_| default_main()),
    );
    for p in [&romfs, &main_nso, &oracle] {
        if !Path::new(p).exists() {
            ibuilder_testkit::skip(&format!("{p} is not there"));
            return None;
        }
    }
    let out = temp(name);
    let prof = profile(&out);
    let (pkg, tables) = (out.join("pkg"), out.join("tables"));
    let mut o = SplitOptions::single(
        Path::new(&romfs),
        Path::new(&main_nso),
        &pkg,
        &tables,
        &prof,
        "0100DFA027504000",
    );
    o.jobs = jobs;
    o.memo_dir = memo;
    let stats = split(&o, &Ruzstd, &NoGate, &|_| {}).expect("the split of Grizzy");
    Some((out, stats))
}

fn compare(out: &Path) {
    let oracle = std::env::var("SHADERSPLIT_ORACLE").unwrap_or_else(|_| default_oracle());
    for f in FILES {
        ibuilder_testkit::compare_files(out.join(f), Path::new(&oracle).join(f))
            .unwrap_or_else(|e| panic!("{f}: {e}"));
    }
    let mut mine: Vec<String> = std::fs::read_dir(out.join("pkg"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    mine.sort();
    assert_eq!(
        mine,
        [
            "cs_data.bin",
            "cs_main.bin",
            "gs_data.bin",
            "ps_data.bin",
            "ps_main.bin",
            "vs_data.bin",
            "vs_main.bin"
        ],
        "no other package file"
    );
}

#[test]
fn grizzy_is_byte_identical_for_1_2_and_8_threads() {
    for j in [1usize, 2, 8] {
        if let Some((out, stats)) = run_once(&format!("j{j}"), j, None) {
            assert_eq!(stats.shaders, 21990);
            assert_eq!(stats.unreadable, 0);
            compare(&out);
            let _ = std::fs::remove_dir_all(&out);
        }
    }
}

#[test]
fn grizzy_resumed_from_its_memo_is_byte_identical() {
    let memo = temp("memo");
    let Some((out1, s1)) = run_once("m1", 4, Some(&memo)) else {
        return;
    };
    assert_eq!((s1.jobs_reused, s1.jobs_ran), (0, 44));
    compare(&out1);
    // a complete memo: nothing runs
    let (out2, s2) = run_once("m2", 8, Some(&memo)).unwrap();
    assert_eq!((s2.jobs_reused, s2.jobs_ran), (44, 0));
    compare(&out2);
    // an interrupted split: every other job's file is gone, a damaged one and a half-written one are left
    let mut names: Vec<_> = std::fs::read_dir(&memo)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    for (i, p) in names.iter().enumerate() {
        if i % 2 == 1 {
            std::fs::remove_file(p).unwrap();
        }
    }
    std::fs::write(&names[0], b"garbage").unwrap();
    std::fs::write(memo.join("x.job.tmp-1-2"), b"half").unwrap();
    let (out3, s3) = run_once("m3", 2, Some(&memo)).unwrap();
    assert_eq!(s3.jobs_reused + s3.jobs_ran, 44);
    assert!(s3.jobs_reused > 0 && s3.jobs_ran > 0);
    compare(&out3);
    assert!(!memo.join("x.job.tmp-1-2").exists());
    for d in [memo, out1, out2, out3] {
        let _ = std::fs::remove_dir_all(d);
    }
}
