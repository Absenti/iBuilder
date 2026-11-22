//! The split's reader record (`record.rs`) and the pipeline's verdict on it (`ibuilder_pipeline::shaders::split_verdict`) with this build's real reader table: which of the synthetic
//! games would be split again on the phone after this change, for a library split before records existed (the owner's libraries: SPLIT_REV 3, no record) and for one with a record.

use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::log::Log;
use ibuilder_pipeline::shaders::{SplitVerdict, split_verdict};
use ibuilder_shadersplit::record::{self, LEGACY, READERS};
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split, split_version};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Real;
impl ShaderBackend for Real {
    fn split_version(&self) -> String {
        split_version()
    }
    fn split_revisions(&self) -> Option<SplitRevisions> {
        Some(SplitRevisions {
            readers: READERS
                .iter()
                .map(|r| SplitReader {
                    name: r.name.into(),
                    rev: r.rev,
                    claims: r.claims.iter().map(|c| c.to_string()).collect(),
                })
                .collect(),
            legacy: LEGACY.iter().map(|(n, r)| (n.to_string(), *r)).collect(),
        })
    }
    fn split_probe(&self, romfs: &Path, dlc: &[PathBuf], map: Option<&Path>) -> Option<String> {
        let d: Vec<&Path> = dlc.iter().map(|p| p.as_path()).collect();
        record::probe_text(romfs, &d, map).ok()
    }
    fn split(&self, _: &SplitRequest<'_>, _: &dyn Log) -> ibuilder_pipeline::Result<SplitResult> {
        unreachable!()
    }
    fn build(
        &self,
        _: &BuildRequest<'_>,
        _: &mut dyn FnMut(&str),
    ) -> ibuilder_pipeline::Result<PathBuf> {
        unreachable!()
    }
    fn live_translate(
        &self,
        _: &LiveRequest<'_>,
        _: &dyn Log,
    ) -> ibuilder_pipeline::Result<LiveResult> {
        unreachable!()
    }
}

fn games() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("expected.json").is_file())
        .collect();
    v.sort();
    v
}

fn dlc_of(g: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(g.join("dlc"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// split the game with a record, and its shader map
fn split_with_record(g: &Path) -> (String, PathBuf) {
    let out = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-record-{}-{}",
        g.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    let (romfs, main, prof) = (g.join("RomFS"), g.join("main"), g.join("profile"));
    let (pkg, tables, rec) = (
        out.join("pkg"),
        out.join("tables"),
        out.join("split.record"),
    );
    let dlc = dlc_of(g);
    let mut o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, "0100TESTTEST0000");
    o.dlc = dlc.iter().map(|p| p.as_path()).collect();
    o.jobs = 2;
    o.record = Some(&rec);
    let r = split(&o, &Ruzstd, &NoGate, &|_| {});
    if r.is_err() {
        return (String::new(), tables.join("shader_map.tsv"));
    }
    (
        std::fs::read_to_string(&rec).unwrap(),
        tables.join("shader_map.tsv"),
    )
}

fn rows_of(record: &str) -> BTreeMap<String, String> {
    let mut m: BTreeMap<String, String> = BTreeMap::new();
    m.insert("split".into(), split_version());
    for (k, v) in ibuilder_pipeline::shaders::record_rows(record) {
        m.insert(k, v);
    }
    m
}

#[test]
fn which_synthetic_games_a_new_app_splits_again() {
    let b = Real;
    let mut legacy_stale = Vec::new();
    for g in games() {
        let name = g.file_name().unwrap().to_string_lossy().into_owned();
        let (rec, map) = split_with_record(&g);
        if rec.is_empty() {
            continue; // a game whose split refuses (an unsupported container) has no library
        }
        // a library recorded by this build: current
        let v = split_verdict(
            Some(&b),
            &rows_of(&rec),
            &split_version(),
            &g.join("RomFS"),
            &dlc_of(&g),
            Some(&map),
        );
        assert_eq!(v, SplitVerdict::Current, "{name}: {rec}");
        // the same library without a record (split by the app before this change, SPLIT_REV 3): its files are classified again
        let mut legacy = BTreeMap::new();
        legacy.insert("split".to_string(), split_version());
        let v = split_verdict(
            Some(&b),
            &legacy,
            &split_version(),
            &g.join("RomFS"),
            &dlc_of(&g),
            Some(&map),
        );
        match v {
            SplitVerdict::Kept(rows) => {
                // what the probe says the game uses is what the split recorded
                let r = rows_of(&rec);
                let kept: BTreeMap<String, String> = rows.into_iter().collect();
                assert_eq!(kept["split-uses"], r["split-uses"], "{name}");
            }
            SplitVerdict::Stale(why) => legacy_stale.push((name, why)),
            SplitVerdict::Current => panic!("{name}: the legacy revisions are not this build's"),
        }
    }
    let names: Vec<&str> = legacy_stale.iter().map(|(n, _)| n.as_str()).collect();
    eprintln!("split again from a library without a record: {legacy_stale:#?}");
    // the readers that changed (falcom, mvgl, nxsd, unreal-pak are new; phyre finds more; snowdrop checks its block sizes) concern only the games that hold their files
    for n in &names {
        assert!(
            n.starts_with("sync-falcom")
                || n.starts_with("sync-mvgl")
                || n.starts_with("sync-nxsd")
                || n.starts_with("ue-")
                || n.starts_with("rebase")
                || n.starts_with("snowdrop")
                || n.contains("berg"),
            "{n} is split again: {legacy_stale:?}"
        );
    }
    for must in ["sync-falcom", "sync-mvgl", "sync-nxsd", "ue-v8-kakarot"] {
        assert!(names.contains(&must), "{must}: {legacy_stale:?}");
    }
    for keep in ["plain", "sync-registered", "kpka", "linkdata"] {
        assert!(!names.contains(&keep), "{keep}: {legacy_stale:?}");
    }
}

/// A real game on this machine, read only: `RECORD_ROMFS=<RomFS image or folder> [RECORD_MAP=<its library's shader_map.tsv>] cargo test --release --test record -- --ignored`.
/// Prints what the probe finds and the verdict for a library split before records existed.
#[test]
#[ignore]
fn the_verdict_for_a_real_game_split_before_records() {
    let Ok(romfs) = std::env::var("RECORD_ROMFS") else {
        return;
    };
    let map = std::env::var("RECORD_MAP").ok().map(PathBuf::from);
    let t0 = std::time::Instant::now();
    let n = ibuilder_shadersplit::image::walk_root(Path::new(&romfs))
        .unwrap()
        .len();
    eprintln!("walk: {n} files in {:.1} s", t0.elapsed().as_secs_f64());
    let t0 = std::time::Instant::now();
    let probe = record::probe_text(Path::new(&romfs), &[], map.as_deref()).unwrap();
    let probe_s = t0.elapsed().as_secs_f64();
    let mut legacy = BTreeMap::new();
    legacy.insert("split".to_string(), split_version());
    let v = split_verdict(
        Some(&Real),
        &legacy,
        &split_version(),
        Path::new(&romfs),
        &[],
        map.as_deref(),
    );
    eprintln!("probe ({probe_s:.1} s):\n{probe}verdict: {v:?}");
}
