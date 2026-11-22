//! `--extra SRC=DEST` copies SRC to `<output>/<framework>/DEST`. A DEST that is absolute or climbs out with `..` wrote outside the output folder (review finding 4.8); it is refused, and nothing
//! is created or removed before the refusal.

use ibuilder_pack::pack::{PackArgs, pack};
use std::fs;
use std::path::PathBuf;

fn dir(tag: &str) -> PathBuf {
    let base = std::env::var("IBUILDER_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!("ibuilder-pack-extra-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn args(d: &std::path::Path, extra: &str) -> PackArgs {
    let fw = d.join("DmGameProbe");
    fs::write(&fw, b"exe").unwrap();
    fs::write(d.join("extra.txt"), b"payload").unwrap();
    let mut a = PackArgs::new(d.join("out"));
    a.framework = Some(fw);
    a.game = Some("0100DFA027504000".into());
    a.name = Some("Test Game".into());
    a.modules = Some("main sdk".into());
    a.chain = Some("b-0123456789ab".into());
    a.package_version = Some("65536".into());
    a.display_version = Some("1.0.0".into());
    a.extra = vec![extra.replace("SRC", &d.join("extra.txt").to_string_lossy())];
    a
}

#[test]
fn an_extra_that_leaves_the_framework_folder_is_refused_before_anything_is_written() {
    let d = dir("escape");
    let escape = d.join("escaped.txt");
    for dest in [
        escape.to_string_lossy().into_owned(),
        "../../escaped2.txt".into(),
        "a/../../escaped3.txt".into(),
        "..".into(),
        "/".into(),
        String::new(),
    ] {
        let a = args(&d, &format!("SRC={dest}"));
        let e = pack(&a)
            .err()
            .unwrap_or_else(|| panic!("DEST {dest:?} must be refused"));
        assert!(
            e.to_string()
                .contains("DEST must be a relative path inside the framework folder"),
            "{dest:?}: {e}"
        );
        assert!(
            !d.join("out").exists(),
            "nothing is created before the refusal ({dest:?})"
        );
    }
    assert!(!escape.exists() && !d.parent().unwrap().join("escaped2.txt").exists());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn an_extra_inside_the_framework_folder_is_copied() {
    let d = dir("inside");
    let a = args(&d, "SRC=extras/deep/extra.txt");
    let r = pack(&a);
    match r {
        Ok(_) => {}
        Err(e) => panic!("a relative DEST inside the folder packs: {e}"),
    }
    let found: Vec<PathBuf> = walk(&d.join("out"))
        .into_iter()
        .filter(|p| p.ends_with("extras/deep/extra.txt"))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(fs::read(&found[0]).unwrap(), b"payload");
    let _ = fs::remove_dir_all(&d);
}

fn walk(p: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let q = e.path();
            if q.is_dir() {
                out.extend(walk(&q))
            } else {
                out.push(q)
            }
        }
    }
    out
}

// ---- review 4.7: a failed forced pack leaves the previous package ---------------------------------------------------------------------------------------------------------------------

fn tree(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = walk(root)
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_forced_pack_that_fails_leaves_the_previous_package_as_it_was() {
    let d = dir("force-fails");
    let a = args(&d, "SRC=extras/extra.txt");
    pack(&a).expect("the first pack");
    let before = tree(&d.join("out"));
    assert!(before.iter().any(|(p, _)| p == "package.json"));
    // each of these fails AFTER the old folder used to be removed
    let mut bad = args(&d, "SRC=extras/extra.txt");
    bad.force = true;
    bad.framework = Some(d.join("no-such-framework"));
    let e = pack(&bad).expect_err("a framework that does not exist");
    assert!(e.to_string().contains("does not exist"), "{e}");
    assert_eq!(
        tree(&d.join("out")),
        before,
        "--framework missing: the old package is still there, byte for byte"
    );
    let mut bad = args(&d, "SRC=extras/extra.txt");
    bad.force = true;
    bad.extra = vec![format!(
        "{}=extras/x.txt",
        d.join("no-such-source.txt").display()
    )];
    assert!(pack(&bad).is_err());
    assert_eq!(
        tree(&d.join("out")),
        before,
        "--extra source missing: the old package is still there"
    );
    assert!(
        !d.join("out.new").exists(),
        "no staging folder is left behind"
    );
    // and a forced pack that works replaces it, with the same files a first pack makes
    let mut ok = args(&d, "SRC=extras/extra.txt");
    ok.force = true;
    pack(&ok).expect("a forced pack that works");
    assert_eq!(tree(&d.join("out")), before);
    assert!(leftovers(&d).is_empty(), "{:?}", leftovers(&d));
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn an_output_folder_given_with_a_trailing_slash_is_replaced_not_nested() {
    let d = dir("slash");
    let mut a = args(&d, "SRC=extras/extra.txt");
    a.out = d.join("out/"); // `out/` is `out`: the staging folder is a sibling, never `out/.new`
    pack(&a).expect("the first pack");
    let before = tree(&d.join("out"));
    a.force = true;
    pack(&a).expect("a forced pack over it");
    assert_eq!(tree(&d.join("out")), before);
    assert!(
        leftovers(&d).is_empty()
            && fs::read_dir(d.join("out"))
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with('.')),
        "{:?}",
        leftovers(&d)
    );
    let _ = fs::remove_dir_all(&d);
}

// ---- N4 (review 4.7), completed: the staged forced pack ---------------------------------------------------------------------------------------------------------------------------------------
// The two tests above cover a missing `--framework` and a missing `--extra` source. These cover the rest of the contract: what the tree of a forced pack is (directories and permission bits too, not only file
// bytes), the leftover of a killed run, the refusal without `--force`, and the two things the staging itself must not destroy (a folder of the user's named `<out>.old`, a plain file at `<out>`).

/// Every entry under `root`, directories included: (relative path, permission bits, bytes of a file). What a tree hash covers.
fn snapshot(root: &std::path::Path) -> Vec<(String, u32, Option<Vec<u8>>)> {
    use std::os::unix::fs::PermissionsExt;
    fn go(
        root: &std::path::Path,
        p: &std::path::Path,
        v: &mut Vec<(String, u32, Option<Vec<u8>>)>,
    ) {
        for e in fs::read_dir(p).unwrap().flatten() {
            let q = e.path();
            let md = fs::symlink_metadata(&q).unwrap();
            let rel = q.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if md.is_dir() {
                v.push((rel, md.permissions().mode() & 0o7777, None));
                go(root, &q, v);
            } else {
                v.push((
                    rel,
                    md.permissions().mode() & 0o7777,
                    Some(fs::read(&q).unwrap()),
                ));
            }
        }
    }
    let mut v = Vec::new();
    go(root, root, &mut v);
    v.sort();
    v
}

/// What is in `d` next to `out` that pack made and did not clean up: `out.new`, `out.old-<pid>`.
fn leftovers(d: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("out."))
        .collect();
    v.sort();
    v
}

/// The leftovers of a killed run: a half-built package in `<out>.new`.
fn stale_staging(d: &std::path::Path) {
    fs::create_dir_all(d.join("out.new/DmGame0100DFA027504000.framework/half")).unwrap();
    fs::write(d.join("out.new/half-written.bin"), b"junk").unwrap();
}

#[test]
fn a_forced_pack_over_an_existing_package_has_the_tree_of_a_fresh_pack() {
    // the reference: the same arguments packed into an output folder that did not exist
    let fresh_dir = dir("force-tree-fresh");
    pack(&args(&fresh_dir, "SRC=extras/extra.txt")).expect("the fresh pack");
    let fresh = snapshot(&fresh_dir.join("out"));
    assert!(
        fresh.iter().any(|(p, m, _)| p
            == "DmGame0100DFA027504000.framework/DmGame0100DFA027504000"
            && *m == 0o755),
        "{fresh:?}"
    );
    // an existing package that differs (another name, another extra): nothing of it may survive or leak into the new one
    let d = dir("force-tree-forced");
    let mut old = args(&d, "SRC=old-only/leftover.txt");
    old.name = Some("The Old Name".into());
    pack(&old).expect("the old package");
    assert_ne!(snapshot(&d.join("out")), fresh);
    let mut forced = args(&d, "SRC=extras/extra.txt");
    forced.force = true;
    let r = pack(&forced).expect("the forced pack");
    assert_eq!(
        r.out,
        d.join("out"),
        "the outcome names the final folder, not the staging one"
    );
    assert_eq!(
        snapshot(&d.join("out")),
        fresh,
        "a forced pack over another package is the fresh package, entry for entry (bytes and permission bits)"
    );
    assert!(leftovers(&d).is_empty(), "{:?}", leftovers(&d));
    let _ = fs::remove_dir_all(&fresh_dir);
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn a_staging_folder_left_by_a_killed_run_is_cleared() {
    let d = dir("stale-new");
    pack(&args(&d, "SRC=extras/extra.txt")).expect("the first pack");
    let fresh = snapshot(&d.join("out"));
    // a run that fails, with a leftover beside it: the old package stays, the leftover is gone
    stale_staging(&d);
    let mut bad = args(&d, "SRC=extras/extra.txt");
    bad.force = true;
    bad.framework = Some(d.join("no-such-framework"));
    assert!(pack(&bad).is_err());
    assert_eq!(snapshot(&d.join("out")), fresh);
    assert!(
        !d.join("out.new").exists(),
        "the leftover of the killed run, and no staging of this one"
    );
    // a run that works: nothing of the leftover is in the package
    stale_staging(&d);
    let mut ok = args(&d, "SRC=extras/extra.txt");
    ok.force = true;
    pack(&ok).expect("a forced pack that works");
    assert_eq!(snapshot(&d.join("out")), fresh);
    assert!(!d.join("out.new").exists());
    // and a first pack (no output folder yet) is not polluted either
    fs::remove_dir_all(d.join("out")).unwrap();
    stale_staging(&d);
    pack(&args(&d, "SRC=extras/extra.txt")).expect("a first pack beside a leftover");
    assert_eq!(snapshot(&d.join("out")), fresh);
    assert!(!d.join("out.new").exists());
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn without_force_an_existing_output_is_refused_with_the_same_message_and_left_alone() {
    let d = dir("no-force");
    let a = args(&d, "SRC=extras/extra.txt");
    pack(&a).expect("the first pack");
    let before = snapshot(&d.join("out"));
    let e = pack(&a).expect_err("an existing output without --force is refused");
    assert_eq!(
        e.to_string(),
        format!(
            "[RED] pack_nxgame: {} exists (pass --force to replace it)",
            d.join("out").display()
        )
    );
    assert_eq!(snapshot(&d.join("out")), before);
    assert!(!d.join("out.new").exists(), "a refusal creates nothing");
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn a_forced_pack_does_not_delete_a_folder_of_the_user_named_like_the_aside_folder() {
    let d = dir("user-old");
    pack(&args(&d, "SRC=extras/extra.txt")).expect("the first pack");
    // not ours: a backup the user keeps next to the package
    fs::create_dir_all(d.join("out.old")).unwrap();
    fs::write(d.join("out.old/backup.txt"), b"mine").unwrap();
    let mut a = args(&d, "SRC=extras/extra.txt");
    a.force = true;
    pack(&a).expect("a forced pack");
    assert_eq!(
        fs::read(d.join("out.old/backup.txt")).unwrap(),
        b"mine",
        "pack only removes what it made itself"
    );
    assert_eq!(
        leftovers(&d),
        ["out.old"],
        "nothing of pack's own is left beside the package"
    );
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn a_forced_pack_over_a_plain_file_is_refused_and_leaves_the_file() {
    let d = dir("out-is-file");
    fs::write(d.join("out"), b"not a package").unwrap();
    let mut a = args(&d, "SRC=extras/extra.txt");
    a.force = true; // `--force` replaces a package folder (it used to fail on a file: rmtree)
    let e = pack(&a).expect_err("a file where the package folder goes");
    assert_eq!(
        e.to_string(),
        format!(
            "pack_nxgame: rmtree {}: Not a directory (os error 20)",
            d.join("out").display()
        ),
        "the message the old `rmtree` gave"
    );
    assert_eq!(fs::read(d.join("out")).unwrap(), b"not a package");
    assert!(
        leftovers(&d).is_empty(),
        "no staging or aside folder is left behind: {:?}",
        leftovers(&d)
    );
    let _ = fs::remove_dir_all(&d);
}
