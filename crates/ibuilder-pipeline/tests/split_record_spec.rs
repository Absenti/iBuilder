//! The targeted re-split: a library made on this device is split again only when the global split version differs or a reader its game needs changed (`split_verdict`, the split's
//! record kept in `pack.txt` / the no-shader marker), not on every change of any reader. Mock backend only (no game data).

mod common;

use common::*;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::mock::MockShaders;
use ibuilder_pipeline::shaders::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const STID: &str = "0100AAAA00000000";
const CHAIN: &str = "cc0f2129e1de";

fn stage3_fixture() -> Fx {
    let mut fx = Fx::new();
    write(
        fx.p(&format!("profile-derived/{STID}/game.tsv")),
        format!("title_id\t{STID}\n"),
    );
    fx.profile(&format!("title_id\t{STID}\n"));
    write(fx.p("extract/RomFS.img"), "rom");
    write(fx.p("extract/ExeFS/main"), "main");
    for (k, v) in [
        ("shader_chain", CHAIN),
        ("shader_free_gib", "0"),
        ("shader_tables", "0"),
    ] {
        fx.set_cfg(k, v);
    }
    fx.set_cfg("romfs", &fx.p("extract/RomFS.img").to_string_lossy());
    fx.set_cfg("exe_dir", &fx.p("extract/ExeFS").to_string_lossy());
    fx.ctx.adapters.tools = Some(ibuilder_pipeline::mock::mock_tools());
    fx
}

/// readers `(name, rev, claims)`; the legacy revisions: every reader at 1 except those named in `new`
fn revs(readers: &[(&str, u32, &[&str])], new: &[&str]) -> SplitRevisions {
    SplitRevisions {
        readers: readers
            .iter()
            .map(|(n, r, c)| SplitReader {
                name: n.to_string(),
                rev: *r,
                claims: c.iter().map(|x| x.to_string()).collect(),
            })
            .collect(),
        legacy: readers
            .iter()
            .filter(|(n, _, _)| !new.contains(n))
            .map(|(n, _, _)| (n.to_string(), 1))
            .collect(),
    }
}

const V1: &[(&str, u32, &[&str])] = &[
    ("apf", 1, &["#46505556"]),
    ("generic", 1, &[]),
    ("phyre", 1, &["#52594850"]),
];
/// a newer app: phyre moved, falcom is new
const V2: &[(&str, u32, &[&str])] = &[
    ("apf", 1, &["#46505556"]),
    ("falcom", 1, &[".pkg", "#0dcff77f"]),
    ("generic", 1, &[]),
    ("phyre", 2, &["#52594850"]),
];

fn backend(r: SplitRevisions, record: Option<&str>, probe: Option<&str>) -> Arc<MockShaders> {
    Arc::new(MockShaders {
        shaders: 3,
        revisions: Some(r),
        record: record.map(String::from),
        probe: probe.map(String::from),
        ..Default::default()
    })
}

fn rows(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn verdict(b: &MockShaders, pack: &str) -> SplitVerdict {
    split_verdict(
        Some(b),
        &rows(pack),
        "mocksplit001",
        Path::new("/nonexistent"),
        &[] as &[PathBuf],
        None,
    )
}

#[test]
fn the_verdict_follows_the_readers_the_game_needs() {
    let now = backend(revs(V2, &["falcom"]), None, None);
    // the global version moves every library
    assert!(
        matches!(verdict(&now, "split\tmockOLD\n"), SplitVerdict::Stale(w) if w.contains("another version"))
    );
    // no per-reader revisions: the split version alone
    let plain = MockShaders::default();
    assert_eq!(
        verdict(&plain, "split\tmocksplit001\n"),
        SplitVerdict::Current
    );
    // a record of this app's revisions
    let cur = "split\tmocksplit001\nsplit-revs\tapf=1 falcom=1 generic=1 phyre=2\nsplit-uses\tgeneric phyre\nsplit-unknown\t.bin#00000000\n";
    assert_eq!(verdict(&now, cur), SplitVerdict::Current);
    // phyre moved and the game used it: split again
    let used = "split\tmocksplit001\nsplit-revs\tapf=1 generic=1 phyre=1\nsplit-uses\tgeneric phyre\nsplit-unknown\t.bin#00000000\n";
    assert!(matches!(verdict(&now, used), SplitVerdict::Stale(w) if w.contains("reader phyre")));
    // only readers it does not use moved, and no new reader claims its unknown files: kept, with this app's revisions
    let apf_game = "split\tmocksplit001\nsplit-revs\tapf=1 generic=1 phyre=1\nsplit-uses\tapf generic\nsplit-unknown\t.bin#00000000 .zs#28b52ffd\n";
    match verdict(&now, apf_game) {
        SplitVerdict::Kept(r) => assert_eq!(
            r,
            vec![
                (
                    "split-revs".to_string(),
                    "apf=1 falcom=1 generic=1 phyre=2".to_string()
                ),
                ("split-uses".to_string(), "apf generic".to_string()),
                (
                    "split-unknown".to_string(),
                    ".bin#00000000 .zs#28b52ffd".to_string()
                ),
            ]
        ),
        v => panic!("{v:?}"),
    }
    // a new reader that claims a kind of file no reader read: by extension, by magic, and an extension with more kinds than the record keeps
    for kinds in [".pkg#00000000", ".dat#0dcff77f", ".dat#*"] {
        let t = format!(
            "split\tmocksplit001\nsplit-revs\tapf=1 generic=1 phyre=2\nsplit-uses\tgeneric\nsplit-unknown\t{kinds}\n"
        );
        assert!(
            matches!(verdict(&now, &t), SplitVerdict::Stale(w) if w.contains("reader falcom") && w.contains(kinds)),
            "{kinds}"
        );
    }
}

#[test]
fn a_library_from_before_records_is_probed_once() {
    // legacy: no split-revs row; the backend classifies the game's files again
    let pack = "made\ton this device, x\nsplit\tmocksplit001\n";
    let ocarina_like = backend(
        revs(V2, &["falcom"]),
        None,
        Some("revs\tx\nuses\tgeneric\nunknown\t.zs#28b52ffd\n"),
    );
    assert!(matches!(
        verdict(&ocarina_like, pack),
        SplitVerdict::Kept(_)
    ));
    assert_eq!(ocarina_like.probes.load(Ordering::SeqCst), 1);
    let phyre_game = backend(
        revs(V2, &["falcom"]),
        None,
        Some("uses\tgeneric phyre\nunknown\t\n"),
    );
    assert!(matches!(verdict(&phyre_game, pack), SplitVerdict::Stale(w) if w.contains("phyre")));
    let blind = backend(revs(V2, &["falcom"]), None, None);
    assert!(
        matches!(verdict(&blind, pack), SplitVerdict::Stale(w) if w.contains("before the split recorded")),
        "a backend that cannot tell splits again"
    );
    // legacy revisions that are this app's: nothing to ask
    let same = backend(revs(V1, &[]), None, None);
    assert_eq!(verdict(&same, pack), SplitVerdict::Current);
    assert_eq!(same.probes.load(Ordering::SeqCst), 0);
}

#[test]
fn stage_3_keeps_a_library_whose_readers_did_not_change_and_rebuilds_one_whose_did() {
    let mut fx = stage3_fixture();
    let pack_txt = fx.p(&format!("shaders/{STID}/pack.txt"));
    // the first build records what the split used
    let v1 = backend(
        revs(V1, &[]),
        Some("revs\tapf=1 generic=1 phyre=1\nuses\tapf generic\nunknown\t.bin#00000000\n"),
        None,
    );
    fx.ctx.adapters.shaders = Some(v1.clone());
    build_shaders_on_device(&mut fx.ctx).expect("a library");
    let t = std::fs::read_to_string(&pack_txt).unwrap();
    assert!(
        t.contains("split-revs\tapf=1 generic=1 phyre=1\nsplit-uses\tapf generic\nsplit-unknown\t.bin#00000000\n"),
        "{t}"
    );
    // an app update moved phyre and added falcom: neither is the game's, the library stays and its record takes the new revisions
    let v2 = backend(revs(V2, &["falcom"]), None, None);
    fx.ctx.adapters.shaders = Some(v2.clone());
    fx.log.clear();
    assert!(!shaders_will_build(&fx.ctx));
    build_shaders_on_device(&mut fx.ctx).expect("the library");
    assert_eq!(v2.splits.load(Ordering::SeqCst), 0, "{:?}", fx.log.lines());
    assert!(fx.log.contains("none its game needs -- kept"));
    let t = std::fs::read_to_string(&pack_txt).unwrap();
    assert!(
        t.contains("split-revs\tapf=1 falcom=1 generic=1 phyre=2\n")
            && t.contains("made\ton this device"),
        "{t}"
    );
    // a later app moves apf, which the game uses: built again
    let v3 = backend(
        revs(
            &[
                ("apf", 2, &["#46505556"]),
                ("falcom", 1, &[".pkg"]),
                ("generic", 1, &[]),
                ("phyre", 2, &[]),
            ],
            &["falcom"],
        ),
        None,
        None,
    );
    fx.ctx.adapters.shaders = Some(v3.clone());
    fx.log.clear();
    assert!(shaders_will_build(&fx.ctx));
    build_shaders_on_device(&mut fx.ctx).expect("a new library");
    assert_eq!(v3.splits.load(Ordering::SeqCst), 1);
    assert!(fx.log.contains("needs reader apf, which is at revision 2"));
}

#[test]
fn a_legacy_library_is_classified_once_and_its_record_kept() {
    let mut fx = stage3_fixture();
    let old = Arc::new(MockShaders {
        shaders: 3,
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(old);
    build_shaders_on_device(&mut fx.ctx).expect("a library");
    let pack_txt = fx.p(&format!("shaders/{STID}/pack.txt"));
    assert!(
        !std::fs::read_to_string(&pack_txt)
            .unwrap()
            .contains("split-revs")
    );
    let now = backend(
        revs(V2, &["falcom"]),
        None,
        Some("revs\tignored\nuses\tgeneric\nunknown\t.zs#28b52ffd\n"),
    );
    fx.ctx.adapters.shaders = Some(now.clone());
    build_shaders_on_device(&mut fx.ctx).expect("the library");
    assert_eq!(now.splits.load(Ordering::SeqCst), 0);
    assert_eq!(now.probes.load(Ordering::SeqCst), 1);
    let t = std::fs::read_to_string(&pack_txt).unwrap();
    assert!(
        t.contains("split-revs\tapf=1 falcom=1 generic=1 phyre=2\nsplit-uses\tgeneric\nsplit-unknown\t.zs#28b52ffd\n"),
        "{t}"
    );
    build_shaders_on_device(&mut fx.ctx).expect("the library");
    assert_eq!(now.probes.load(Ordering::SeqCst), 1, "not asked again");
}

#[test]
fn a_no_shader_marker_carries_the_record_and_is_kept_or_dropped_by_it() {
    let mut fx = stage3_fixture();
    let marker = fx.p(&format!("shaders/{STID}.none"));
    let v1 = Arc::new(MockShaders {
        shaders: 0,
        revisions: Some(revs(V1, &[])),
        record: Some(
            "revs\tapf=1 generic=1 phyre=1\nuses\tgeneric\nunknown\t.pkg#00000000 .bin#*\n".into(),
        ),
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(v1);
    assert!(build_shaders_on_device(&mut fx.ctx).is_none());
    let t = std::fs::read_to_string(&marker).unwrap();
    assert!(t.contains("split mocksplit001\nsplit-revs apf=1 generic=1 phyre=1\nsplit-uses generic\nsplit-unknown .pkg#00000000 .bin#*\n"), "{t}");
    // phyre moved: it claims no kind the game has, the marker stays
    let v2 = Arc::new(MockShaders {
        shaders: 0,
        revisions: Some(revs(
            &[
                ("apf", 1, &[]),
                ("generic", 1, &[]),
                ("phyre", 2, &[".phyre"]),
            ],
            &[],
        )),
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(v2.clone());
    fx.log.clear();
    assert!(build_shaders_on_device(&mut fx.ctx).is_none());
    assert_eq!(v2.splits.load(Ordering::SeqCst), 0, "{:?}", fx.log.lines());
    assert!(fx.log.contains("the marker is kept") && fx.log.contains("not tried again"));
    // a new reader claims the .pkg files no reader read: split again
    let v3 = Arc::new(MockShaders {
        shaders: 0,
        revisions: Some(revs(V2, &["falcom"])),
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(v3.clone());
    fx.log.clear();
    assert!(build_shaders_on_device(&mut fx.ctx).is_none());
    assert_eq!(v3.splits.load(Ordering::SeqCst), 1);
    assert!(fx.log.contains("reader falcom"));
}
