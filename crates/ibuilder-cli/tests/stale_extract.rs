//! `prepare_extract` links the inputs into `<out>/extract`. An earlier run in the same `--out` leaves things there: links of its own (replaced, or removed when this run has no such input) and, after
//! `--nup`, REAL files and folders. Reading a real leftover as if it were this run's input built the wrong game without a word (review finding 4.10): it is refused with the way out.

use ibuilder_cli::{CliArgs, prepare_extract};
use ibuilder_pipeline::testutil::{TempDir, write};
use std::fs;

fn args(d: &TempDir) -> CliArgs {
    for m in ["main", "subsdk0", "sdk"] {
        write(d.join(format!("game/ExeFS/{m}")), format!("NSO:{m}"));
    }
    write(d.join("game/RomFS.img"), "rom");
    CliArgs {
        exefs: d.join("game/ExeFS"),
        out: d.join("out"),
        mock: true,
        ..Default::default()
    }
}

fn is_link(p: &std::path::Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

#[test]
fn a_real_extract_folder_from_an_earlier_nup_run_is_refused_not_silently_reused() {
    let d = TempDir::new("stale-real");
    let a = args(&d);
    // what a `--nup` run left: a real ExeFS (the other package's) and RomFS image
    write(d.join("out/extract/ExeFS/main"), "OTHER GAME");
    write(d.join("out/extract/RomFS.img"), "OTHER ROM");
    let e = prepare_extract(&a).expect_err("a real folder is not this run's input");
    let msg = e.to_string();
    assert!(
        msg.contains("ExeFS")
            && msg.contains("real folder")
            && msg.contains("another --out")
            && msg.contains("game/ExeFS"),
        "{msg}"
    );
    assert_eq!(
        fs::read_to_string(d.join("out/extract/ExeFS/main")).unwrap(),
        "OTHER GAME",
        "nothing of it is touched"
    );
    assert!(!is_link(&d.join("out/extract/ExeFS")));
}

#[test]
fn a_real_leftover_is_refused_for_an_input_this_run_does_not_have_either() {
    let d = TempDir::new("stale-real-romfs");
    let mut a = args(&d);
    fs::remove_file(d.join("game/RomFS.img")).unwrap(); // this run has no RomFS beside the ExeFS
    a.romfs = None;
    write(d.join("out/extract/RomFS.img"), "OTHER ROM");
    let e =
        prepare_extract(&a).expect_err("the other package's RomFS would be read with this ExeFS");
    assert!(
        e.to_string().contains("RomFS.img") && e.to_string().contains("real file"),
        "{e}"
    );
}

#[test]
fn the_requested_input_may_be_the_folder_the_chain_extracted_before() {
    let d = TempDir::new("stale-same");
    write(d.join("out/extract/ExeFS/main"), "NSO:main");
    let a = CliArgs {
        exefs: d.join("out/extract/ExeFS"),
        out: d.join("out"),
        mock: true,
        ..Default::default()
    };
    prepare_extract(&a).expect("re-running the chain on its own extraction is fine");
    assert_eq!(
        fs::read_to_string(d.join("out/extract/ExeFS/main")).unwrap(),
        "NSO:main"
    );
}

#[test]
fn links_of_an_earlier_run_are_replaced_or_removed() {
    let d = TempDir::new("stale-links");
    let a = args(&d);
    prepare_extract(&a).unwrap();
    assert!(is_link(&d.join("out/extract/RomFS.img")));
    // the second game has a different ExeFS and no RomFS: the first game's RomFS link must not survive
    write(d.join("second/ExeFS/main"), "NSO:second");
    let b = CliArgs {
        exefs: d.join("second/ExeFS"),
        out: d.join("out"),
        mock: true,
        ..Default::default()
    };
    prepare_extract(&b).unwrap();
    assert!(is_link(&d.join("out/extract/ExeFS")));
    assert_eq!(
        fs::read_to_string(d.join("out/extract/ExeFS/main")).unwrap(),
        "NSO:second",
        "the new input"
    );
    assert!(
        fs::symlink_metadata(d.join("out/extract/RomFS.img")).is_err(),
        "the first game's RomFS link is gone"
    );
}

#[test]
fn a_nup_run_clears_the_links_a_previous_exefs_run_left_so_the_extraction_does_not_write_through_them()
 {
    let d = TempDir::new("stale-nup");
    let a = args(&d);
    prepare_extract(&a).unwrap();
    assert!(is_link(&d.join("out/extract/ExeFS")) && is_link(&d.join("out/extract/RomFS.img")));
    let n = CliArgs {
        nup: Some(d.join("game.nup")),
        out: d.join("out"),
        mock: true,
        ..Default::default()
    };
    prepare_extract(&n).unwrap();
    assert!(!is_link(&d.join("out/extract/ExeFS")) && !is_link(&d.join("out/extract/RomFS.img")));
    assert!(
        d.join("game/ExeFS/main").exists(),
        "the input the links pointed at is untouched"
    );
}
