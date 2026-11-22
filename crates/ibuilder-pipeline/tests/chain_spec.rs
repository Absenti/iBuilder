//! The whole chain, end to end, on the stand-in stage workers: order of stages, what each leaves on disk, the cache on the second run, the restart before stage 3, failure semantics, the boot decision.

mod common;

use common::*;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::chain::*;
use ibuilder_pipeline::env::Env;
use ibuilder_pipeline::mock::*;
use ibuilder_pipeline::state::RESTART_RC;
use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const TID: &str = "0100DFA027504000";

struct Chain {
    fx: Fx,
    rw: Arc<MockRewriter>,
    boot: Arc<MockBoot>,
    shaders: Arc<MockShaders>,
}

fn chain_fixture(extra_cfg: &str) -> Chain {
    let mut fx = Fx::new();
    let docs = fx.docs().to_path_buf();
    for m in ["main", "subsdk0", "sdk"] {
        write(docs.join("extract/ExeFS").join(m), format!("NSO:{m}"));
    }
    write(docs.join("extract/RomFS.img"), "rom");
    write(docs.join("extract/control.nacp"), "nacp");
    write(docs.join("extract/icon.jpg"), "JPEG");
    write(docs.join("host-prebuilt.dylib"), "HOST");
    let cfg = format!(
        "extract_out={0}/extract\nexe_dir={0}/extract/ExeFS\nmodules=main subsdk0 sdk\npatched_dir={0}/extract/rewrite\nhost_dylib={0}/host-prebuilt.dylib\nframework_out={0}/DmGameProbe.framework/DmGameProbe\nprofile={0}/profile.tsv\nromfs={0}/extract/RomFS.img\nboot_seconds=0\napp_build=B1\nshader_chain=cc0f2129e1de\ngame=Grizzy.nsp\nshader_free_gib=0\n{1}\n",
        docs.display(),
        extra_cfg
    );
    write(docs.join("gameapp.config"), &cfg);
    let rw = Arc::new(MockRewriter::default());
    let boot = Arc::new(MockBoot::default());
    let shaders = Arc::new(MockShaders {
        shaders: 5,
        ..Default::default()
    });
    fx.ctx.adapters = Adapters {
        nup: None,
        profile: Arc::new(MockProfile::new(TID, "main subsdk0 sdk")),
        rewriter: rw.clone(),
        shaders: Some(shaders.clone()),
        tools: Some(mock_tools()),
        assembler: Arc::new(MockAssembler),
        signer: Arc::new(MockSigner { rc: 0 }),
        boot: Some(boot.clone()),
        launcher: Some(Arc::new(MockLauncher)),
        mods: None,
    };
    fx.ctx.boot_enabled = true;
    let p = fx.p("gameapp.config");
    load_config(&mut fx.ctx, Some(&p));
    write(fx.p("gameapp.running"), "marker");
    Chain {
        fx,
        rw,
        boot,
        shaders,
    }
}

#[test]
fn a_full_chain_writes_every_stages_output_and_records_the_build() {
    let mut c = chain_fixture("");
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_OK, "{:?}", c.fx.log.lines());
    assert!(rep.all_ok(), "{:?}", rep.status);
    assert_eq!(
        rep.status.keys().cloned().collect::<Vec<_>>(),
        ["assemble", "profile", "rewrite", "shaders"]
    );
    let docs = c.fx.docs().to_path_buf();
    // stage 0: the profile, the game's icon, the modules from the profile
    assert!(docs.join("profile.tsv").is_file());
    assert_eq!(
        fs::read_to_string(docs.join(format!("games/{TID}/icon.jpg"))).unwrap(),
        "JPEG"
    );
    // stage 2: the three modules, in order, main on threads from the memory/thermal decision
    let calls = c.rw.calls.lock().unwrap().clone();
    assert_eq!(
        calls.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(),
        ["main", "subsdk0", "sdk"]
    );
    assert_eq!(calls[1].1, 1, "only the big module is worth threads");
    assert!(docs.join("extract/rewrite/main/main.patched.bin").is_file());
    // launcher.json for the settings sheet
    assert!(docs.join(format!("games/{TID}/launcher.json")).is_file());
    // stage 3: the library
    assert!(
        docs.join(format!("shaders/{TID}/b-cc0f2129e1de.img"))
            .is_file()
    );
    assert_eq!(c.shaders.splits.load(Ordering::SeqCst), 1);
    assert!(!docs.join(format!("shaders/{TID}.work")).exists());
    // stage 4: flat/ (hard links), the framework, its Info.plist, the signature, the stamp
    for m in ["main", "subsdk0", "sdk"] {
        for s in [".patched.bin", ".fixups.tsv", ".gates.tsv"] {
            assert!(
                docs.join("extract/flat").join(format!("{m}{s}")).is_file(),
                "flat/{m}{s}"
            );
        }
    }
    let fw = fs::read(docs.join("DmGameProbe.framework/DmGameProbe")).unwrap();
    assert!(fw.starts_with(b"MOCK-FRAMEWORK\n") && fw.ends_with(b"MOCK-SIGNATURE\n"));
    assert!(docs.join("DmGameProbe.framework/Info.plist").is_file());
    let stamp = ibuilder_pipeline::markers::read_framework_stamp(&c.fx.ctx);
    assert_eq!(
        (stamp.get("app_build"), stamp.get("game")),
        (Some("B1"), Some("Grizzy.nsp"))
    );
    // extract/ExeFS is dropped once the framework and the shaders are there
    assert!(!docs.join("extract/ExeFS").exists());
    // pipeline.boot for stage 5
    let pb = fs::read_to_string(docs.join("pipeline.boot")).unwrap();
    assert!(pb.starts_with(&format!(
        "{}/DmGameProbe.framework/DmGameProbe\n",
        docs.display()
    )));
    // the player starts the game: the chain did not (boot_after_build=0), and the marker is cleared
    assert_eq!(c.boot.requests.lock().unwrap().len(), 0);
    assert!(!docs.join("gameapp.running").exists());
    assert!(c.fx.log.contains("stage 5 not started"));
    // the stage line the UI reads: the last step was the signature (4/5)
    assert_eq!(
        fs::read_to_string(docs.join("gameapp.stage"))
            .unwrap()
            .trim(),
        "stage 4/5: signing the framework in-process (zsign, no codesign)"
    );
    // the timings row, one line in the log, memory lines around the heavy steps
    let rows = fs::read_to_string(docs.join("build_times.tsv")).unwrap();
    assert_eq!(rows.lines().count(), 2);
    assert!(rows.lines().nth(1).unwrap().ends_with("\tok"));
    assert!(
        c.fx.log.contains("build times:")
            && c.fx.log.contains("[mem:before-rewrite]")
            && c.fx.log.contains("[mem:after-sign]")
            && c.fx.log.contains("[mem:chain-end]")
    );
    assert!(c.fx.log.contains("process spawns: none"));
    // every stage boundary hands the allocator's free pages back before the next stage starts (docs/chain/MEMORY-RUST.md): the profile's, the rewrite's, the split's, the build's
    let lines = c.fx.log.lines();
    let at = |tag: &str| {
        lines
            .iter()
            .position(|l| l.contains(&format!("[mem:{tag}] release:")))
    };
    let order: Vec<Option<usize>> = [
        "before-rewrite",
        "after-rewrite-main",
        "before-shaders",
        "after-split",
        "after-build",
        "after-sign",
    ]
    .iter()
    .map(|t| at(t))
    .collect();
    assert!(
        order.iter().all(|o| o.is_some()),
        "a release line is missing: {order:?}"
    );
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "the boundaries are not in stage order: {order:?}"
    );
}

#[test]
fn a_second_run_takes_the_rewrite_from_the_cache_and_finds_the_library() {
    let mut c = chain_fixture("");
    run_chain(&mut c.fx.ctx);
    // the app writes the extraction again at every run (ExeFS was dropped): put it back
    for m in ["main", "subsdk0", "sdk"] {
        write(c.fx.p("extract/ExeFS").join(m), format!("NSO:{m}"));
    }
    c.fx.log.clear();
    let before = c.rw.calls.lock().unwrap().len();
    let mut ctx = ibuilder_pipeline::Ctx::builder(c.fx.docs())
        .env(c.fx.env.clone())
        .log(c.fx.log.clone())
        .mem(c.fx.mem.clone())
        .adapters(c.fx.ctx.adapters.clone())
        .boot_enabled(true)
        .build();
    let p = c.fx.p("gameapp.config");
    load_config(&mut ctx, Some(&p));
    let rep = run_chain(&mut ctx);
    assert!(rep.all_ok(), "{:?}", c.fx.log.lines());
    assert_eq!(
        c.rw.calls.lock().unwrap().len(),
        before,
        "no module rewritten again: the cache key (module bytes, app build, profile tables) matches"
    );
    assert_eq!(
        c.fx.log
            .lines()
            .iter()
            .filter(|l| l.contains("rewrite taken from the cache"))
            .count(),
        3
    );
    assert_eq!(
        c.shaders.splits.load(Ordering::SeqCst),
        1,
        "the library of this chain is already there: stage 3 builds nothing"
    );
    assert!(
        c.fx.log
            .contains("already holds a library of chain cc0f2129e1de")
    );
    // the output is byte-identical to the first run's
    let first = fs::read(c.fx.p("extract/rewrite/main/main.patched.bin")).unwrap();
    assert_eq!(first, b"PATCHED:NSO:main");
    assert_eq!(
        fs::read_to_string(c.fx.p("build_times.tsv"))
            .unwrap()
            .lines()
            .count(),
        3,
        "one row a build"
    );
}

#[test]
fn the_chain_ends_on_purpose_before_stage_3_when_memory_is_short() {
    // first run: build the library cache of the rewrite, then a device with a 3364 MiB ceiling and 600 MiB left after the rewrite
    let mut c = chain_fixture("game=Grizzy.nsp\n");
    c.fx.mem.set_available(3364 * MIB);
    c.fx.ctx.timing.launch_available = 3364 * MIB;
    // the mock rewriter "runs" in this process; the restart needs the rewrite in the cache, which do_rewrite fills; memory left is read after it
    c.fx.mem.set_available(600 * MIB);
    // the chain reads the launch ceiling at its start: make it the phone's
    struct Launch(Arc<ibuilder_pipeline::memory::FakeMem>);
    impl ibuilder_pipeline::memory::MemSource for Launch {
        fn mem(&self) -> ibuilder_pipeline::memory::MemInfo {
            self.0.mem()
        }
    }
    let _ = Launch(c.fx.mem.clone());
    // run chain_start with a 3364 MiB ceiling by setting it, then lower it after stage 2 through the rewriter
    struct Squeeze {
        inner: Arc<MockRewriter>,
        mem: Arc<ibuilder_pipeline::memory::FakeMem>,
    }
    impl Rewriter for Squeeze {
        fn rewrite(
            &self,
            req: &RewriteRequest<'_>,
            v: &ibuilder_pipeline::View<'_>,
        ) -> ibuilder_pipeline::Result<()> {
            self.inner.rewrite(req, v)?;
            self.mem.set_available(600 * MIB); // what is left once the rewrite has run
            Ok(())
        }
    }
    c.fx.mem.set_available(3364 * MIB);
    c.fx.ctx.adapters.rewriter = Arc::new(Squeeze {
        inner: c.rw.clone(),
        mem: c.fx.mem.clone(),
    });
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RESTART_RC, "{:?}", c.fx.log.lines());
    let req = ibuilder_pipeline::timings::read_kv(&c.fx.p("gameapp.continue"));
    assert_eq!(
        (
            req.get("game").map(String::as_str),
            req.get("stage").map(String::as_str)
        ),
        (Some("Grizzy.nsp"), Some("3"))
    );
    assert_eq!(
        c.shaders.splits.load(Ordering::SeqCst),
        0,
        "stage 3 did not run in this process"
    );
    assert!(rep.result.starts_with("paused"));
    assert!(
        !c.fx.p("build_times.tsv").exists() && c.fx.p("gameapp.timing.partial").is_file(),
        "a paused build keeps its partial, not a row"
    );
    let partial = fs::read_to_string(c.fx.p("gameapp.timing.partial")).unwrap();
    assert!(
        partial.contains("rewrite_s=") && partial.contains("resumed=0"),
        "{partial}"
    );
}

#[test]
fn a_failed_rewrite_stops_the_chain_and_a_failed_assembly_does_not() {
    let mut c = chain_fixture("");
    c.fx.ctx.adapters.rewriter = Arc::new(Unwired("ibuilder-rewrite"));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_FAILED);
    assert!(
        c.fx.log.contains("stage 2 FAILED")
            && c.fx.log.contains(
                "the chain stops: a framework assembled from a failed rewrite would not run"
            )
    );
    assert!(!c.fx.p("DmGameProbe.framework").exists());
    assert!(
        c.fx.p("build_times.tsv").is_file(),
        "a failed build is recorded too"
    );
    assert!(
        fs::read_to_string(c.fx.p("build_times.tsv"))
            .unwrap()
            .trim_end()
            .ends_with("\tfailed")
    );

    // the signer refuses (rc 12: no certificate in the Keychain): stage 4 fails; the chain says which, and returns 5 (it returned 0, as Python did, and the app said "Built: ready to play.")
    let mut c = chain_fixture("boot_after_build=1\n");
    c.fx.ctx.adapters.signer = Arc::new(MockSigner { rc: 12 });
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_NO_FRAMEWORK, "{:?}", c.fx.log.lines());
    assert!(
        c.fx.log.contains(
            "stage 5 NOT STARTED: [RED] the game could not be assembled or signed (stage 4: "
        ),
        "{:?}",
        c.fx.log.lines()
    );
    assert_eq!(
        c.boot.requests.lock().unwrap().len(),
        0,
        "nothing is booted"
    );
    assert!(
        !c.fx.p("gameapp.boot-only").exists() && !c.fx.p("gameapp.running").exists(),
        "no request; the run ended by itself"
    );
    assert_eq!(
        rep.status.get("assemble").map(String::as_str),
        Some("failed")
    );
    assert_eq!(rep.result, "failed (assemble)");
    assert!(
        c.fx.log.contains(
            "stage 4b sign FAILED (rc=12): no signing certificate in THIS app's Keychain"
        )
    );
    assert!(!rep.all_ok());
    assert!(
        !c.fx.p("gameapp.framework").exists(),
        "a framework that was not signed is not one to boot again"
    );
    assert!(
        c.fx.p("extract/ExeFS").exists(),
        "ExeFS stays when the build did not finish"
    );
}

#[test]
fn a_failed_shader_stage_does_not_stop_the_chain_but_is_reported_as_a_failure() {
    // review r3 §3.1: a translation that fails leaves the game with no library; the chain goes on (the assembly still runs) but the run is not "ok"
    let mut c = chain_fixture("");
    c.fx.ctx.adapters.shaders = Some(Arc::new(MockShaders {
        shaders: 5,
        fail_build: true,
        ..Default::default()
    }));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(
        rep.rc, RC_NO_SHADER_LIBRARY,
        "the chain ran, but the game has no library: not 0 (review r4 §4.1: the app decides on the code)"
    );
    assert!(
        rep.status["shaders"].starts_with("failed (no library: ")
            && rep.status["shaders"].contains("mock translation failure"),
        "{:?}",
        rep.status
    );
    assert_eq!(
        rep.status["assemble"], "ok",
        "a failed stage 3 does not stop the chain"
    );
    assert_eq!(rep.result, "failed (shaders)");
    assert!(
        !rep.all_ok(),
        "a build with no library is not a successful one"
    );
    let row = fs::read_to_string(c.fx.p("build_times.tsv")).unwrap();
    assert!(
        row.lines().nth(1).unwrap().ends_with("\tfailed (shaders)"),
        "{row}"
    );
    assert!(
        c.fx.p("extract/ExeFS").exists(),
        "ExeFS is kept when the shaders are not there (the boot builds them from it)"
    );
    assert!(
        c.fx.p("DmGameProbe.framework/DmGameProbe").is_file()
            && !c.fx.p("gameapp.framework").exists(),
        "assembled, but not stamped: the app must not read the game as ready"
    );
    assert!(
        c.fx.log
            .contains("framework stamp: NOT written: stage 3 could not build the shader library")
    );
}

#[test]
fn too_little_free_space_for_the_shaders_is_a_failure_not_none() {
    // review r3 §3.1: one of the five real failures of stage 3 (disk, no backend, split, translation, pack install)
    let mut c = chain_fixture("shader_free_gib=1000000\n");
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_NO_SHADER_LIBRARY);
    assert!(
        rep.status["shaders"].starts_with("failed (no library: ")
            && rep.status["shaders"].contains("free"),
        "{:?}",
        rep.status
    );
    assert!(!rep.all_ok());
    assert_eq!(rep.result, "failed (shaders)");
}

/// A backend whose build reports an image that is not there: the pack cannot be installed.
struct LostImage(MockShaders);
impl ShaderBackend for LostImage {
    fn split_version(&self) -> String {
        self.0.split_version()
    }
    fn split(
        &self,
        req: &SplitRequest<'_>,
        log: &dyn ibuilder_pipeline::log::Log,
    ) -> ibuilder_pipeline::error::Result<SplitResult> {
        self.0.split(req, log)
    }
    fn build(
        &self,
        req: &BuildRequest<'_>,
        progress: &mut dyn FnMut(&str),
    ) -> ibuilder_pipeline::error::Result<std::path::PathBuf> {
        let img = self.0.build(req, progress)?;
        fs::remove_file(&img)?;
        Ok(img)
    }
    fn live_translate(
        &self,
        req: &LiveRequest<'_>,
        log: &dyn ibuilder_pipeline::log::Log,
    ) -> ibuilder_pipeline::error::Result<LiveResult> {
        self.0.live_translate(req, log)
    }
}

#[test]
fn a_pack_that_cannot_be_installed_is_a_failure_and_leaves_no_part_folder() {
    // review r3 §3.1 + §4.6: the install fails; the old pack is still there, no `<id>.part` is left, and the run says it failed
    let mut c = chain_fixture("");
    let pack = c.fx.p(&format!("shaders/{TID}"));
    write(pack.join("b-cc0f2129e1de.img"), "OLD-LIBRARY");
    write(
        pack.join("shader_map.tsv"),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\n",
    ); // no rows: built again
    c.fx.ctx.adapters.shaders = Some(Arc::new(LostImage(MockShaders {
        shaders: 5,
        ..Default::default()
    })));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_NO_SHADER_LIBRARY);
    assert!(
        rep.status["shaders"].starts_with("failed (no library: ")
            && rep.status["shaders"].contains("could not be installed"),
        "{:?}",
        rep.status
    );
    assert!(!rep.all_ok());
    assert!(
        !c.fx.p(&format!("shaders/{TID}.part")).exists(),
        "the half-made pack is removed"
    );
    assert_eq!(
        fs::read_to_string(pack.join("b-cc0f2129e1de.img")).unwrap(),
        "OLD-LIBRARY",
        "the old pack is untouched"
    );
}

#[test]
fn a_game_whose_split_finds_no_shader_is_none_and_the_run_is_ok() {
    // `none (...)` is kept for the honest case: the split read the game and found nothing to translate
    let mut c = chain_fixture("");
    c.fx.ctx.adapters.shaders = Some(Arc::new(MockShaders {
        shaders: 0,
        ..Default::default()
    }));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_OK);
    assert!(
        rep.status["shaders"].starts_with("none ("),
        "{:?}",
        rep.status
    );
    assert!(rep.all_ok(), "{:?}", rep.status);
    assert_eq!(rep.result, "ok");
}

#[test]
fn the_boot_after_build_goes_in_process_or_is_handed_to_the_next_launch() {
    // boot_after_build=1 with room: the guest is booted in this process, the marker cleared when it comes back
    let mut c = chain_fixture("boot_after_build=1\nboot_seconds=45\n");
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.all_ok(), "{:?}", c.fx.log.lines());
    assert_eq!(c.boot.requests.lock().unwrap().len(), 1);
    let r = c.boot.requests.lock().unwrap()[0].clone();
    assert!(
        r.ends_with(&format!(
            "|main subsdk0 sdk|{}|45",
            c.fx.p("profile.tsv").display()
        )),
        "{r}"
    );
    assert!(
        c.fx.log
            .contains("the boot came back: interrupted-run marker cleared")
    );
    assert_eq!(
        fs::read_to_string(c.fx.p("gameapp.stage")).unwrap().trim(),
        format!("stage 5/5: {}", ibuilder_pipeline::boot::BOOT_STAGE_TEXT)
    );
    assert!(
        c.fx.env.get("DM_SAVE").is_some(),
        "the save folder the engine aborts without"
    );

    // a device ceiling below the floor: the request for a fresh launch is written instead (trap 28)
    let mut c = chain_fixture("boot_after_build=1\n");
    c.fx.mem.set_available(1095 * MIB);
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.rc == RC_OK || rep.rc == RESTART_RC);
    if rep.rc == RC_OK {
        assert_eq!(c.boot.requests.lock().unwrap().len(), 0);
        let req = fs::read_to_string(c.fx.p("gameapp.boot-only")).unwrap();
        assert!(
            req.contains("in-process boot refused: 1095 MiB available < 2500 MiB"),
            "{req}"
        );
        assert!(c.fx.log.contains("boot NOT STARTED IN THIS PROCESS"));
        assert!(!c.fx.p("gameapp.running").exists());
        // and the NEXT launch boots it without running a stage of the chain
        c.fx.mem.set_available(3300 * MIB);
        let calls = c.rw.calls.lock().unwrap().len();
        let mut ctx = ibuilder_pipeline::Ctx::builder(c.fx.docs())
            .env(c.fx.env.clone())
            .log(c.fx.log.clone())
            .mem(c.fx.mem.clone())
            .adapters(c.fx.ctx.adapters.clone())
            .boot_enabled(true)
            .build();
        let p = c.fx.p("gameapp.config");
        load_config(&mut ctx, Some(&p));
        let rep2 = run_chain(&mut ctx);
        assert_eq!(rep2.rc, 0);
        assert_eq!(
            c.rw.calls.lock().unwrap().len(),
            calls,
            "boot-only runs no stage"
        );
        assert_eq!(c.boot.requests.lock().unwrap().len(), 1);
        assert!(
            !c.fx.p("gameapp.boot-only").exists(),
            "consumed when the boot came back"
        );
        assert_eq!(
            fs::read_to_string(c.fx.p("build_times.tsv"))
                .unwrap()
                .lines()
                .count(),
            2,
            "a boot-only launch records no row"
        );
    }
}

#[test]
fn a_cancelled_run_stops_between_modules() {
    let mut c = chain_fixture("");
    c.fx.ctx.cancel.store(true, Ordering::SeqCst);
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_CANCELLED, "{:?}", c.fx.log.lines());
    assert!(c.rw.calls.lock().unwrap().is_empty());
}

#[test]
fn no_extract_out_is_result_2() {
    let mut fx = Fx::new();
    fx.cfg("profile=/x");
    let rep = run_chain(&mut fx.ctx);
    assert_eq!(rep.rc, RC_NO_EXTRACT_OUT);
    assert!(
        fx.log
            .contains("no extract_out in the config: nothing to do")
    );
}

#[test]
fn a_continued_build_adds_to_the_killed_ones_timings() {
    let mut c = chain_fixture("");
    write(
        c.fx.p("gameapp.timing.partial"),
        "game=0100DFA027504000\ndump=Grizzy.nsp\nresumed=0\nrewrite_s=120.0\nprofile_s=0.5\n",
    );
    write(
        c.fx.p("gameapp.continue.run"),
        "game=Grizzy.nsp\nstage=3\nwhy=killed\n",
    );
    write(
        c.fx.p("gameapp.killed"),
        "game=Grizzy.nsp\nstage=stage 3/5: x\nmem=[mem:x]\n",
    );
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.all_ok());
    assert!(
        c.fx.log
            .contains("the previous build of Grizzy.nsp was ended by iOS")
            && c.fx
                .log
                .contains("this build continues one that stopped at stage 3 (killed)")
    );
    // the killed run's partial carries the title id as `game=` and the dump as `dump=`: matched by the dump (the Python app compared the title id with the dump's name and never merged)
    let row = fs::read_to_string(c.fx.p("build_times.tsv")).unwrap();
    let cols: Vec<&str> = row.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(cols[6], "1", "resumed counts the runs: {cols:?}");
    assert!(
        cols[9].parse::<f64>().unwrap() >= 120.0,
        "the earlier run's rewrite seconds are in the row: {cols:?}"
    );
}

/// A Switch 2 reader that counts its calls and always fails: the test proves it was or was not asked.
#[derive(Default)]
struct CountingNup {
    calls: std::sync::atomic::AtomicUsize,
}
impl NupReader for CountingNup {
    fn extract_nup(
        &self,
        _v: &ibuilder_pipeline::ctx::View<'_>,
    ) -> ibuilder_pipeline::error::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ibuilder_pipeline::error::PipelineError::Failed(
            "the reader was asked".into(),
        ))
    }
}

#[test]
fn an_empty_nup_with_its_extraction_on_disk_is_built_without_reading_it() {
    // "Optimize storage" (app patch 0073) leaves an empty file of the game's name once it is extracted; ExeFS is kept, so a rebuild works from the extraction
    let mut c = chain_fixture("");
    let nup = c.fx.p("Ocarina [0400486024DEC000][v131072].nup");
    write(&nup, "");
    c.fx.set_cfg("nup", &nup.to_string_lossy());
    let reader = Arc::new(CountingNup::default());
    c.fx.ctx.adapters.nup = Some(reader.clone());
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_OK, "{:?}", c.fx.log.lines());
    assert_eq!(
        reader.calls.load(Ordering::SeqCst),
        0,
        "the package is not read"
    );
    assert!(
        c.fx.log
            .any(|l| l.contains("is an empty placeholder")
                && l.contains("built from them, not read")),
        "{:?}",
        c.fx.log.lines()
    );
    // without the extraction the reader is asked (and its failure stops the chain, as for any unreadable package)
    let mut c = chain_fixture("");
    fs::remove_file(c.fx.p("extract/RomFS.img")).unwrap();
    write(&nup, "");
    c.fx.set_cfg("nup", &nup.to_string_lossy());
    let reader = Arc::new(CountingNup::default());
    c.fx.ctx.adapters.nup = Some(reader.clone());
    let rep = run_chain(&mut c.fx.ctx);
    assert_ne!(rep.rc, RC_OK);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 1);
    // a package with content is read as before, extraction or not
    let mut c = chain_fixture("");
    write(&nup, "NUP");
    c.fx.set_cfg("nup", &nup.to_string_lossy());
    let reader = Arc::new(CountingNup::default());
    c.fx.ctx.adapters.nup = Some(reader.clone());
    run_chain(&mut c.fx.ctx);
    assert_eq!(reader.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_placeholder_is_an_empty_or_missing_file() {
    let fx = Fx::new();
    assert!(nup_is_placeholder(&fx.p("none.nup")));
    write(fx.p("empty.nup"), "");
    assert!(nup_is_placeholder(&fx.p("empty.nup")));
    write(fx.p("full.nup"), "x");
    assert!(!nup_is_placeholder(&fx.p("full.nup")));
}

// ---- review r4 §4.1: a game with no shader library is not booted, not stamped, and not reported as a success -------------------------------------------

#[test]
fn a_build_with_no_shader_library_is_not_booted_here_nor_handed_to_the_next_launch() {
    // with boot_after_build=1 the chain booted the guest with no library (it draws nothing) while the app read "Ready"
    let mut c = chain_fixture("boot_after_build=1\nboot_seconds=45\n");
    c.fx.ctx.adapters.shaders = Some(Arc::new(MockShaders {
        shaders: 5,
        fail_build: true,
        ..Default::default()
    }));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_NO_SHADER_LIBRARY, "{:?}", c.fx.log.lines());
    assert_eq!(rep.result, "failed (shaders)");
    assert_eq!(
        rep.status["assemble"], "ok",
        "the chain still went on to the assembly"
    );
    assert_eq!(
        c.boot.requests.lock().unwrap().len(),
        0,
        "the guest is not booted"
    );
    assert!(
        !c.fx.p("gameapp.boot-only").exists(),
        "nor handed to the next launch"
    );
    assert!(
        c.fx.log
            .contains("stage 5 NOT STARTED: [RED] the game has no shader library (stage 3: failed (no library: the translation failed: mock translation failure))"),
        "{:?}",
        c.fx.log.lines()
    );
    assert!(
        !c.fx.p("gameapp.running").exists(),
        "the run ended by itself: the interrupted-run marker is cleared"
    );
    assert!(
        !c.fx.p("gameapp.framework").exists(),
        "no stamp: the app reads the game as not built"
    );
    // `none (...)` is not a failure: a game with nothing to build is booted as before
    let mut c = chain_fixture("boot_after_build=1\nboot_seconds=45\nshader_build=0\n");
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_OK, "{:?}", rep.status);
    assert!(
        rep.status["shaders"].starts_with("none ("),
        "{:?}",
        rep.status
    );
    assert_eq!(c.boot.requests.lock().unwrap().len(), 1);
    assert!(c.fx.p("gameapp.framework").exists());
}

#[test]
fn a_boot_only_launch_whose_shader_library_cannot_be_built_refuses_the_boot() {
    // boot.rs called build_shaders_on_device and booted whatever came back
    let mut c = chain_fixture("keep_exefs=1\n");
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.all_ok(), "{:?}", c.fx.log.lines());
    let pack = c.fx.p(&format!("shaders/{TID}"));
    assert!(pack.join("b-cc0f2129e1de.img").is_file());
    // the library goes (removed in Files), and this launch cannot build it (too little free space)
    fs::remove_dir_all(&pack).unwrap();
    let cfg_path = c.fx.p("gameapp.config");
    let cfg = fs::read_to_string(&cfg_path).unwrap();
    write(&cfg_path, format!("{cfg}shader_free_gib=1000000\n"));
    let boot_launch = |c: &Chain| {
        write(
            c.fx.p("gameapp.boot-only"),
            "boot the framework that is already built and signed\n",
        );
        write(c.fx.p("gameapp.running"), "marker\nbootonly=1\n");
        let mut ctx = ibuilder_pipeline::Ctx::builder(c.fx.docs())
            .env(c.fx.env.clone())
            .log(c.fx.log.clone())
            .mem(c.fx.mem.clone())
            .adapters(c.fx.ctx.adapters.clone())
            .boot_enabled(true)
            .build();
        load_config(&mut ctx, Some(&c.fx.p("gameapp.config")));
        run_chain(&mut ctx)
    };
    let rep = boot_launch(&c);
    assert_eq!(rep.rc, RC_NO_SHADER_LIBRARY, "{:?}", c.fx.log.lines());
    assert!(
        rep.status["shaders"].starts_with("failed (no library: ")
            && rep.status["shaders"].contains("free"),
        "{:?}",
        rep.status
    );
    assert_eq!(rep.result, "failed (shaders)");
    assert_eq!(
        c.boot.requests.lock().unwrap().len(),
        0,
        "the guest is not booted"
    );
    assert!(c.fx.log.contains("stage 5 boot-only REFUSED: [RED] the game has no shader library and this launch could not build it"));
    assert!(
        !c.fx.p("gameapp.boot-only").exists(),
        "consumed: a later launch must not start, by itself, a boot that would be refused the same way"
    );
    assert!(
        !c.fx.p("gameapp.running").exists(),
        "the run ended by itself"
    );
    assert!(
        c.fx.p("gameapp.framework").exists(),
        "the framework is still this build's: a later tap tries the library again"
    );
    // the space is there again: the same request builds the library first, then boots
    write(&cfg_path, &cfg);
    let rep = boot_launch(&c);
    assert_eq!(rep.rc, RC_OK, "{:?}", c.fx.log.lines());
    assert_eq!(
        c.boot.requests.lock().unwrap().len(),
        1,
        "booted, with its library"
    );
    assert!(pack.join("b-cc0f2129e1de.img").is_file());
}

#[test]
fn a_pack_install_cut_short_by_a_kill_gets_its_library_back_at_the_next_start() {
    // a kill between install_pack's two renames left `<id>.old` (the previous library) and no `<id>`; the next start deleted the `.old`, the only copy, and built everything again
    let mut c = chain_fixture("");
    let old = c.fx.p(&format!("shaders/{TID}.old"));
    let part = c.fx.p(&format!("shaders/{TID}.part"));
    write(old.join("b-cc0f2129e1de.img"), "OLD-LIBRARY");
    write(
        old.join("shader_map.tsv"),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\npkg0\t0\tvs\t0123456789abcdef\tx\n",
    );
    write(part.join("b-cc0f2129e1de.img"), "HALF-MADE");
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.all_ok(), "{:?}", c.fx.log.lines());
    let pack = c.fx.p(&format!("shaders/{TID}"));
    assert_eq!(
        fs::read_to_string(pack.join("b-cc0f2129e1de.img")).unwrap(),
        "OLD-LIBRARY",
        "put back, not built again"
    );
    assert_eq!(
        c.shaders.splits.load(Ordering::SeqCst),
        0,
        "the game's shaders were not read again"
    );
    assert!(
        !old.exists() && !part.exists(),
        "nothing left beside the pack"
    );
    assert!(
        c.fx.log.contains(".old put back as "),
        "{:?}",
        c.fx.log.lines()
    );
    // an `.old` beside a pack in place is the end of an install that finished: it goes, the pack stays
    let mut c = chain_fixture("");
    write(old_of(&c).join("b-cc0f2129e1de.img"), "OLDER");
    let pack = c.fx.p(&format!("shaders/{TID}"));
    write(pack.join("b-cc0f2129e1de.img"), "NEW");
    write(
        pack.join("shader_map.tsv"),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\npkg0\t0\tvs\t0123456789abcdef\tx\n",
    );
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.all_ok());
    assert_eq!(
        fs::read_to_string(pack.join("b-cc0f2129e1de.img")).unwrap(),
        "NEW"
    );
    assert!(!old_of(&c).exists());
}

fn old_of(c: &Chain) -> std::path::PathBuf {
    c.fx.p(&format!("shaders/{TID}.old"))
}

#[test]
fn the_timings_row_and_the_game_icon_do_not_use_a_fixed_temporary_name() {
    // review r4 §4.1: `build_times.tsv.tmp` and `icon.jpg.tmp` were fixed names shared by every writer; a folder in their place (what no write can go into) now changes nothing
    let mut c = chain_fixture("");
    fs::create_dir_all(c.fx.p("build_times.tsv.tmp")).unwrap();
    fs::create_dir_all(c.fx.p(&format!("games/{TID}/icon.jpg.tmp"))).unwrap();
    let rep = run_chain(&mut c.fx.ctx);
    assert!(rep.all_ok(), "{:?}", c.fx.log.lines());
    assert!(
        !c.fx.log.contains("build times: not recorded"),
        "{:?}",
        c.fx.log.lines()
    );
    assert_eq!(
        fs::read_to_string(c.fx.p("build_times.tsv"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(c.fx.p(&format!("games/{TID}/icon.jpg"))).unwrap(),
        "JPEG"
    );
    let tmp_left = fs::read_dir(c.fx.docs())
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().ends_with(".tmp")
                && e.file_name() != "build_times.tsv.tmp"
        })
        .count();
    assert_eq!(tmp_left, 0, "no temporary file left behind");
}

#[test]
fn a_build_that_fails_at_stage_4_and_at_stage_3_returns_no_framework() {
    // review r4 follow-up: 5 wins over 4 (the game has neither a framework nor a library); the status names both
    let mut c = chain_fixture("");
    c.fx.ctx.adapters.signer = Arc::new(MockSigner { rc: 12 });
    c.fx.ctx.adapters.shaders = Some(Arc::new(MockShaders {
        shaders: 5,
        fail_build: true,
        ..Default::default()
    }));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_NO_FRAMEWORK, "{:?}", rep.status);
    assert_eq!(rep.result, "failed (assemble, shaders)");
    assert!(rep.status["shaders"].starts_with("failed (no library: "));
    assert!(!rep.all_ok());
    assert!(!c.fx.p("gameapp.framework").exists());
    // the codes are distinct from every other the chain returns
    let codes = [
        RC_OK,
        RC_FAILED,
        RC_NO_EXTRACT_OUT,
        RC_NOTHING_TO_BOOT,
        RC_NO_SHADER_LIBRARY,
        RC_NO_FRAMEWORK,
        RESTART_RC,
        RC_CANCELLED,
        RC_PAUSED,
    ];
    let mut sorted = codes.to_vec();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), codes.len(), "{codes:?}");
}

// ---- export without a .p12: stage 4b signs ad hoc when this install has no identity ------------------------------------------------------------------

#[test]
fn the_ad_hoc_fallback_is_for_no_identity_only() {
    use ibuilder_pipeline::assemble::adhoc_fallback;
    for rc in [10, 12, 13] {
        assert!(adhoc_fallback(rc), "rc {rc}: no identity on this install");
    }
    for rc in [0, 11, 14, 20, 1, -1] {
        assert!(!adhoc_fallback(rc), "rc {rc}: not a missing identity");
    }
}

#[test]
fn no_identity_signs_ad_hoc_stamps_export_only_and_boots_nothing() {
    for rc in [10, 12, 13] {
        let mut c = chain_fixture("boot_after_build=1\nboot_seconds=45\n");
        let signer = Arc::new(MockAdhocSigner {
            rc,
            ..Default::default()
        });
        c.fx.ctx.adapters.signer = signer.clone();
        let rep = run_chain(&mut c.fx.ctx);
        assert_eq!(rep.rc, RC_OK, "rc {rc}: {:?}", c.fx.log.lines());
        assert!(rep.all_ok(), "{:?}", rep.status);
        assert_eq!(signer.adhoc_calls.load(Ordering::SeqCst), 1);
        let stamp = fs::read_to_string(c.fx.p("gameapp.framework")).unwrap();
        assert!(stamp.ends_with("\nsignature=adhoc-export\n"), "{stamp}");
        assert!(
            ibuilder_pipeline::state::parse_framework_stamp(Some(&stamp))
                .unwrap()
                .export_only
        );
        let fw = fs::read(c.fx.p("DmGameProbe.framework/DmGameProbe")).unwrap();
        assert!(fw.ends_with(b"MOCK-ADHOC-SIGNATURE\n"));
        assert!(c.fx.log.contains("stage 4b no signing identity here"));
        assert!(c.fx.log.contains("stage 4b sign ad hoc rc=0"));
        assert!(
            c.fx.log
                .contains("stage 5 NOT STARTED: the framework is signed ad hoc for export only"),
            "{:?}",
            c.fx.log.lines()
        );
        assert_eq!(c.boot.requests.lock().unwrap().len(), 0, "never dlopen'd");
        assert!(!c.fx.p("gameapp.running").exists());
    }
}

#[test]
fn a_broken_certificate_still_fails_and_an_identity_signs_as_before() {
    for rc in [11, 14, 20] {
        let mut c = chain_fixture("");
        let signer = Arc::new(MockAdhocSigner {
            rc,
            ..Default::default()
        });
        c.fx.ctx.adapters.signer = signer.clone();
        let rep = run_chain(&mut c.fx.ctx);
        assert_eq!(rep.rc, RC_NO_FRAMEWORK, "rc {rc}");
        assert_eq!(signer.adhoc_calls.load(Ordering::SeqCst), 0);
        assert!(!c.fx.p("gameapp.framework").exists());
    }
    // the ad hoc signature itself refused: the build fails, and says both
    let mut c = chain_fixture("");
    c.fx.ctx.adapters.signer = Arc::new(MockAdhocSigner {
        rc: 12,
        adhoc_rc: 20,
        ..Default::default()
    });
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_NO_FRAMEWORK);
    assert!(
        c.fx.log
            .contains("the ad hoc signature (export only) failed too")
    );
    assert!(!c.fx.p("gameapp.framework").exists());
    // with an identity: the ad hoc mode is never called and the stamp has its four lines, as before
    let mut c = chain_fixture("");
    let signer = Arc::new(MockAdhocSigner::default());
    c.fx.ctx.adapters.signer = signer.clone();
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_OK);
    assert_eq!(signer.adhoc_calls.load(Ordering::SeqCst), 0);
    let stamp = fs::read_to_string(c.fx.p("gameapp.framework")).unwrap();
    assert_eq!(stamp.lines().count(), 4, "{stamp}");
    assert!(!stamp.contains("signature="));
    assert!(!c.fx.log.contains("sign ad hoc"));
    assert_eq!(
        ibuilder_pipeline::markers::framework_stamp_text("B1", "", "G.nsp", "T", "c", false),
        "app_build=B1\ngame=G.nsp\nbuilt=T\ncontent=c\n",
        "the stamp of a signed framework is byte for byte what it was"
    );
    assert_eq!(
        ibuilder_pipeline::markers::framework_stamp_text("B1", "", "G.nsp", "T", "c", true),
        "app_build=B1\ngame=G.nsp\nbuilt=T\ncontent=c\nsignature=adhoc-export\n"
    );
    assert_eq!(
        ibuilder_pipeline::markers::framework_stamp_text("B1", "F1", "G.nsp", "T", "c", true),
        "app_build=B1\nframework_id=F1\ngame=G.nsp\nbuilt=T\ncontent=c\nsignature=adhoc-export\n",
        "the framework id follows the build id"
    );
}

#[test]
fn a_boot_only_launch_refuses_an_export_only_framework() {
    let mut c = chain_fixture("");
    c.fx.ctx.adapters.signer = Arc::new(MockAdhocSigner {
        rc: 12,
        ..Default::default()
    });
    assert_eq!(run_chain(&mut c.fx.ctx).rc, RC_OK);
    write(c.fx.p("gameapp.boot-only"), "boot the framework\n");
    let mut ctx = ibuilder_pipeline::Ctx::builder(c.fx.docs())
        .env(c.fx.env.clone())
        .log(c.fx.log.clone())
        .mem(c.fx.mem.clone())
        .adapters(c.fx.ctx.adapters.clone())
        .boot_enabled(true)
        .build();
    let p = c.fx.p("gameapp.config");
    load_config(&mut ctx, Some(&p));
    let rep = run_chain(&mut ctx);
    assert_eq!(rep.rc, RC_NOTHING_TO_BOOT, "{:?}", c.fx.log.lines());
    assert_eq!(c.boot.requests.lock().unwrap().len(), 0);
    assert!(
        !c.fx.p("gameapp.boot-only").exists(),
        "the request is consumed: it can never be honoured"
    );
    assert!(c.fx.log.contains("signed ad hoc for export only"));
}

// ---- GPUs without BC textures: every boot decodes them on the CPU --------------------------------------------------------------------------------------

#[test]
fn a_gpu_without_bc_boots_with_tex_bc_cpu_and_a_gpu_with_them_is_unchanged() {
    use ibuilder_pipeline::boot::{BC_CPU_ENV, BC_CPU_LINE};
    let mut with = chain_fixture("boot_after_build=1\nboot_seconds=45\n");
    assert!(run_chain(&mut with.fx.ctx).all_ok());
    assert_eq!(with.boot.requests.lock().unwrap().len(), 1);
    assert_eq!(with.fx.env.get(BC_CPU_ENV), None, "a BC GPU: no switch");
    assert!(!with.fx.log.contains(BC_CPU_LINE));
    let mut without = chain_fixture("boot_after_build=1\nboot_seconds=45\n");
    without.fx.ctx.gpu_without_bc = true;
    assert!(run_chain(&mut without.fx.ctx).all_ok());
    assert_eq!(without.boot.requests.lock().unwrap().len(), 1);
    assert_eq!(without.fx.env.get(BC_CPU_ENV).as_deref(), Some("1"));
    assert_eq!(
        without
            .fx
            .log
            .lines()
            .iter()
            .filter(|l| l.as_str() == BC_CPU_LINE)
            .count(),
        1,
        "one line"
    );
    assert_eq!(
        fs::read(with.fx.p("profile.tsv")).unwrap(),
        fs::read(without.fx.p("profile.tsv")).unwrap(),
        "the profile is the same: the switch is the boot's environment only"
    );
    // a 0 from env.txt (or anything else) cannot stop it: the host would stop at the first BC texture
    let mut forced = chain_fixture("boot_after_build=1\nboot_seconds=45\n");
    forced.fx.env.set(BC_CPU_ENV, "0");
    forced.fx.ctx.gpu_without_bc = true;
    assert!(run_chain(&mut forced.fx.ctx).all_ok());
    assert_eq!(forced.fx.env.get(BC_CPU_ENV).as_deref(), Some("1"));
}
