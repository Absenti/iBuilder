//! The files that decide how the next launch resumes (`gameapp.running`, `gameapp.framework`, `gameapp.boot-only`, `gameapp.continue.run`, `gameapp.timing.partial`) are replaced with
//! `util::write_atomic` (a `.tmp` beside, then a rename), not written in place: a process killed in the middle of an in-place write leaves a half file that the next launch reads as an answer
//! (review finding 4.3). The test makes the folder read only, so no `.tmp` can be made beside the file, while the real file stays writable: an atomic writer fails and leaves the old content; an
//! in-place one would overwrite it. (The temporary name is each writer's own since review r3 §5.5, so a folder in the place of `<file>.tmp` no longer blocks it.)

mod common;

use common::Fx;
use ibuilder_pipeline::layout::Docs;
use ibuilder_pipeline::markers::{boot_only_mark_attempted, write_framework_stamp};
use ibuilder_pipeline::state::{prepare_continued_build, write_run_marker};
use ibuilder_pipeline::timings::Timings;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// No temporary file of `p` is left beside it (`<name>.tmp`, or a writer's own `<name>.<pid>.<n>.tmp`).
fn no_tmp_beside(p: &Path) -> bool {
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    fs::read_dir(p.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .all(|n| !(n.starts_with(&name) && n.ends_with(".tmp")))
}

/// The folder of a blocked file is read only until this is dropped.
struct Blocked(PathBuf);
impl Drop for Blocked {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

/// `p` holds "OLD" and nothing can be written beside it (its folder is read only; the file itself stays writable). `None` (and a skip) where folder permissions do not bind (root).
fn seeded_and_blocked(p: &Path) -> Option<Blocked> {
    let dir = p.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap(); // a file blocked earlier in the test is in the same folder
    fs::write(p, "OLD").unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let b = Blocked(dir.clone());
    if fs::write(dir.join(".probe"), "x").is_ok() {
        let _ = fs::remove_file(dir.join(".probe"));
        ibuilder_testkit::skip(
            "a read-only folder does not stop this user from writing in it (root)",
        );
        return None;
    }
    Some(b)
}

#[test]
fn the_run_marker_is_replaced_whole_or_not_at_all() {
    let fx = Fx::new();
    let docs = Docs::new(fx.docs());
    let Some(_blocked1) = seeded_and_blocked(&docs.running()) else {
        return;
    };
    assert!(
        write_run_marker(&docs, "game.nsp", true, 4242, 1_700_000_000_000_000).is_err(),
        "the write goes through a .tmp"
    );
    assert_eq!(
        fs::read_to_string(docs.running()).unwrap(),
        "OLD",
        "the old marker is intact"
    );
    // and when it can be written it reads whole, with no .tmp left
    let fx2 = Fx::new();
    let docs2 = Docs::new(fx2.docs());
    write_run_marker(&docs2, "game.nsp", true, 4242, 1_700_000_000_000_000).unwrap();
    let text = fs::read_to_string(docs2.running()).unwrap();
    assert!(
        text.contains("pid 4242") && text.contains("game=game.nsp") && text.contains("bootonly=1"),
        "{text:?}"
    );
    assert!(no_tmp_beside(&docs2.running()));
}

#[test]
fn the_framework_stamp_and_the_attempted_boot_request_are_replaced_whole_or_not_at_all() {
    let mut fx = Fx::new();
    fx.set_cfg("app_build", "build-1");
    fx.set_cfg("game", "game.nsp");
    let stamp = fx.ctx.docs.framework_stamp();
    let Some(_blocked2) = seeded_and_blocked(&stamp) else {
        return;
    };
    write_framework_stamp(&fx.ctx);
    assert_eq!(fs::read_to_string(&stamp).unwrap(), "OLD");
    assert!(
        fx.log.lines().iter().any(|l| l.contains("could not write")),
        "{:?}",
        fx.log.lines()
    );

    let req = fx.ctx.docs.boot_only();
    let Some(_blocked3) = seeded_and_blocked(&req) else {
        return;
    };
    boot_only_mark_attempted(&fx.ctx);
    assert_eq!(fs::read_to_string(&req).unwrap(), "OLD");
    assert!(
        fx.log
            .lines()
            .iter()
            .any(|l| l.contains("could not mark the request attempted")),
        "{:?}",
        fx.log.lines()
    );
}

#[test]
fn the_continue_request_and_the_timing_partial_are_replaced_whole_or_not_at_all() {
    let fx = Fx::new();
    let docs = Docs::new(fx.docs());
    let Some(_blocked4) = seeded_and_blocked(&docs.continue_run()) else {
        return;
    };
    assert!(prepare_continued_build(&docs, "game.nsp", 3, "memory").is_err());
    assert_eq!(fs::read_to_string(docs.continue_run()).unwrap(), "OLD");

    let partial = fx.p("gameapp.timing.partial");
    let Some(_blocked5) = seeded_and_blocked(&partial) else {
        return;
    };
    let t = Timings::new(&fx.p("build_times.tsv"), &partial, "iPhone");
    t.save_partial();
    assert_eq!(
        fs::read_to_string(&partial).unwrap(),
        "OLD",
        "a partial is written through a .tmp too"
    );
}

// ---- review r2, N7: four more files that another step (or the UI) reads while they are written ------------------------------------------------------------------------------------

#[test]
fn the_stage_line_the_ui_polls_is_replaced_whole_or_not_at_all() {
    let fx = Fx::new();
    let docs = Docs::new(fx.docs());
    let Some(_blocked6) = seeded_and_blocked(&docs.stage_file()) else {
        return;
    };
    let line =
        ibuilder_pipeline::stage::write_stage(&docs, 2, 5, "rewriting the modules", Some(&*fx.log));
    assert_eq!(
        line, "stage 2/5: rewriting the modules",
        "the line is still returned and logged"
    );
    assert_eq!(
        fs::read_to_string(docs.stage_file()).unwrap(),
        "OLD",
        "the UI never reads a half line: the old one stays"
    );
    assert!(
        fx.log.lines().iter().any(|l| l.contains("could not write")),
        "{:?}",
        fx.log.lines()
    );
    // and when it can be written it reads whole, with no .tmp left
    let fx2 = Fx::new();
    let docs2 = Docs::new(fx2.docs());
    ibuilder_pipeline::stage::write_stage(&docs2, 3, 5, "translating the shaders", None);
    assert_eq!(
        fs::read_to_string(docs2.stage_file()).unwrap(),
        "stage 3/5: translating the shaders\n"
    );
    assert!(no_tmp_beside(&docs2.stage_file()));
}

#[test]
fn the_log_marks_are_replaced_whole_or_not_at_all() {
    let fx = Fx::new();
    let docs = Docs::new(fx.docs());
    fs::write(fx.p("gameapp.chain.log"), "a line\n").unwrap(); // a log the marks describe
    let Some(_blocked7) = seeded_and_blocked(&docs.logmarks()) else {
        return;
    };
    ibuilder_pipeline::logwindow::mark(&docs, 1_700_000_000);
    assert_eq!(fs::read_to_string(docs.logmarks()).unwrap(), "OLD");
}

#[test]
fn the_load_blocked_marker_and_pipeline_boot_are_replaced_whole_or_not_at_all() {
    let fx = Fx::new();
    // a refused dlopen: the guest log says so
    fs::write(
        fx.p("gameapp.guest.log"),
        "dlopen: file system sandbox blocked mmap()\n",
    )
    .unwrap();
    let Some(_blocked8) = seeded_and_blocked(&fx.ctx.docs.load_blocked()) else {
        return;
    };
    assert!(
        ibuilder_pipeline::markers::note_boot_result(&fx.ctx, 3),
        "the refusal is still recognised"
    );
    assert_eq!(
        fs::read_to_string(fx.ctx.docs.load_blocked()).unwrap(),
        "OLD"
    );
    assert!(
        fx.log.lines().iter().any(|l| l.contains("could not write")),
        "{:?}",
        fx.log.lines()
    );

    let Some(_blocked9) = seeded_and_blocked(&fx.ctx.docs.pipeline_boot()) else {
        return;
    };
    ibuilder_pipeline::markers::write_pipeline_boot(&fx.ctx, "/f", "/flat", "main sdk", "/p", "");
    assert_eq!(
        fs::read_to_string(fx.ctx.docs.pipeline_boot()).unwrap(),
        "OLD"
    );
    assert!(
        fx.log
            .lines()
            .iter()
            .filter(|l| l.contains("could not write"))
            .count()
            >= 2,
        "{:?}",
        fx.log.lines()
    );
}

// ---- review r2, N7 (the swallowed failures): the two writes that said nothing when they failed ----------------------------------------------------------------------------------

#[test]
fn log_marks_that_cannot_be_written_are_said_in_the_diagnostics() {
    // `logwindow` has no log handle (the app calls `mark` / `trim_to_window` with a folder and a time), so the failure goes where the chain's own lines go when no `Ctx` is at hand: the
    // hidden diagnostics (`Documents/.diag/pipeline-<session>.jsonl`, started by the chain). This binary starts them for itself; the lines of the other tests that land in the file do not matter.
    let fx = Fx::new();
    ibuilder_pipeline::diag::start(&fx.ctx);
    let docs = Docs::new(fx.docs());
    fs::write(fx.p("gameapp.chain.log"), "a line\n").unwrap();
    let Some(_blocked10) = seeded_and_blocked(&docs.logmarks()) else {
        return;
    };
    ibuilder_pipeline::logwindow::mark(&docs, 1_700_000_000);
    assert_eq!(
        fs::read_to_string(docs.logmarks()).unwrap(),
        "OLD",
        "the old marks stay"
    );
    let said: Vec<String> = fs::read_dir(ibuilder_pipeline::diag::dir(fx.docs()))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("pipeline-"))
        .flat_map(|e| {
            fs::read_to_string(e.path())
                .unwrap()
                .lines()
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .filter(|l| {
            l.contains("log marks: could not write")
                && l.contains(&fx.p("gameapp.logmarks").display().to_string())
        }) // this test's folder (the writer is one per process: the other test of this file that blocks the marks lands its line here too)
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
}

#[test]
fn a_boot_request_that_cannot_be_written_is_not_reported_as_written() {
    // the chain refuses the in-process boot (little memory left) and hands it to the next launch through `Documents/gameapp.boot-only`; when that file cannot be written the log must say so
    // instead of "wrote Documents/gameapp.boot-only: close the app and reopen it" (nothing would start by itself, and the player was told it would)
    let mut fx = Fx::new();
    fx.set_cfg("boot_after_build", "1");
    fx.mem.set_available(1095 * (1 << 20));
    let Some(_blocked11) = seeded_and_blocked(&fx.ctx.docs.boot_only()) else {
        return;
    };
    ibuilder_pipeline::boot::boot_after_build(
        &mut fx.ctx,
        Path::new("/f/framework"),
        Path::new("/f/flat"),
    );
    let lines = fx.log.lines();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("boot NOT STARTED IN THIS PROCESS")),
        "the refusal itself is still said: {lines:?}"
    );
    assert_eq!(fs::read_to_string(fx.ctx.docs.boot_only()).unwrap(), "OLD");
    assert!(
        lines
            .iter()
            .any(|l| l.contains("could not write") && l.contains("gameapp.boot-only")),
        "{lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("wrote Documents/gameapp.boot-only")),
        "it must not claim a file it could not write: {lines:?}"
    );
}

#[test]
fn a_valid_run_writes_the_markers_with_the_same_bytes_as_before() {
    // what the next launch (and the UI) reads must not change with the way it is written: file names and bytes, and no `.tmp` left
    let fx = Fx::new();
    let docs = Docs::new(fx.docs());
    ibuilder_pipeline::markers::write_pipeline_boot(
        &fx.ctx,
        "/f/framework",
        "/f/flat",
        "main subsdk0 sdk",
        "/p/profile.tsv",
        "romfs-dir",
    );
    assert_eq!(
        fs::read_to_string(docs.pipeline_boot()).unwrap(),
        "/f/framework\n/f/flat\nmain subsdk0 sdk\n/p/profile.tsv\nromfs-dir\n"
    );

    fs::write(fx.p("gameapp.chain.log"), "a line\n").unwrap();
    ibuilder_pipeline::logwindow::mark(&docs, 1_700_000_000);
    assert_eq!(
        fs::read_to_string(docs.logmarks()).unwrap(),
        "1700000000\tgameapp.chain.log\t7\n"
    );

    fs::write(
        fx.p("gameapp.guest.log"),
        "dlopen: file system sandbox blocked mmap()\n",
    )
    .unwrap();
    assert!(ibuilder_pipeline::markers::note_boot_result(&fx.ctx, 3));
    let blocked = fs::read_to_string(docs.load_blocked()).unwrap();
    assert!(
        blocked.starts_with(
            "the dlopen of the built framework in Documents was refused by the sandbox ("
        ),
        "{blocked:?}"
    );
    assert!(blocked.ends_with("): this install's certificate does not let the app run code from Documents; export the game as an .ipa\n"), "{blocked:?}");
    assert_eq!(blocked.matches('\n').count(), 1, "one line");

    for p in [docs.pipeline_boot(), docs.logmarks(), docs.load_blocked()] {
        assert!(no_tmp_beside(&p), "{}", p.display());
    }
}
