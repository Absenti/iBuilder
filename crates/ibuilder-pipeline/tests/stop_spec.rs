//! Pause and cancel of a running chain (the app's buttons): the request (`ibuilder_pipeline::stop`) is read at the chain's safe points, the run ends with `RC_PAUSED` (131) or
//! `RC_CANCELLED` (130), a pause keeps everything a later run continues from, a cancel overrides a pause, and a request that is not for this run is forgotten when it starts.
//!
//! The request is one byte of the process: every test takes `serial()` (this binary's tests would otherwise stop each other's chains).

mod common;

use common::*;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::chain::*;
use ibuilder_pipeline::ctx::{Ctx, View};
use ibuilder_pipeline::error::PipelineError;
use ibuilder_pipeline::gate::ShaderMemoryGate;
use ibuilder_pipeline::log::{Log, MemLog};
use ibuilder_pipeline::memory::FakeMem;
use ibuilder_pipeline::mock::*;
use ibuilder_pipeline::stop;
use ibuilder_pipeline::testutil::TempDir;
use ibuilder_pipeline::thermal::{Clock, ThermalGate};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

const TID: &str = "0100DFA027504000";

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    stop::clear();
    g
}

/// a rewriter that does the mock's work and then raises a stop after its `after`-th module (what the UI thread does while the chain works)
struct RaisingRewriter {
    inner: MockRewriter,
    kind: u8,
    after: usize,
    calls: AtomicUsize,
}
impl Rewriter for RaisingRewriter {
    fn rewrite(&self, req: &RewriteRequest<'_>, v: &View<'_>) -> ibuilder_pipeline::Result<()> {
        self.inner.rewrite(req, v)?;
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.after {
            stop::request(self.kind);
        }
        Ok(())
    }
}

/// a shader backend whose first build is the one a stop interrupts: it raises the request and ends with the error the real build gives (`"cancelled"`); a later build finishes
struct Interrupted {
    kind: u8,
    splits: AtomicUsize,
    builds: AtomicUsize,
}
impl ShaderBackend for Interrupted {
    fn split_version(&self) -> String {
        "mocksplit001".into()
    }
    fn split(
        &self,
        req: &SplitRequest<'_>,
        log: &dyn Log,
    ) -> ibuilder_pipeline::Result<SplitResult> {
        self.splits.fetch_add(1, Ordering::SeqCst);
        MockShaders {
            shaders: 5,
            ..Default::default()
        }
        .split(req, log)
    }
    fn build(
        &self,
        req: &BuildRequest<'_>,
        progress: &mut dyn FnMut(&str),
    ) -> ibuilder_pipeline::Result<PathBuf> {
        if self.builds.fetch_add(1, Ordering::SeqCst) == 0 && self.kind != stop::NONE {
            progress("compiling: 2/5 shaders");
            fs::create_dir_all(req.cas.join("cas-x.dmsb"))?; // what the finished shaders are: in the cache folder, which a stop leaves alone
            fs::write(
                req.cas.join("cas-x.dmsb").join("entry"),
                b"a finished shader",
            )?;
            fs::create_dir_all(req.out_dir.join("_work"))?;
            stop::request(self.kind);
            return Err(PipelineError::failed("cancelled"));
        }
        MockShaders {
            shaders: 5,
            ..Default::default()
        }
        .build(req, progress)
    }
    fn live_translate(
        &self,
        req: &LiveRequest<'_>,
        log: &dyn Log,
    ) -> ibuilder_pipeline::Result<LiveResult> {
        MockShaders::default().live_translate(req, log)
    }
}

struct Chain {
    fx: Fx,
}

fn adapters(rw: Arc<dyn Rewriter>, shaders: Arc<dyn ShaderBackend>) -> Adapters {
    Adapters {
        nup: None,
        profile: Arc::new(MockProfile::new(TID, "main subsdk0 sdk")),
        rewriter: rw,
        shaders: Some(shaders),
        tools: Some(mock_tools()),
        assembler: Arc::new(MockAssembler),
        signer: Arc::new(MockSigner { rc: 0 }),
        boot: Some(Arc::new(MockBoot::default())),
        launcher: Some(Arc::new(MockLauncher)),
        mods: None,
    }
}

fn config(docs: &std::path::Path, extra: &str) -> String {
    format!(
        "extract_out={0}/extract\nexe_dir={0}/extract/ExeFS\nmodules=main subsdk0 sdk\npatched_dir={0}/extract/rewrite\nhost_dylib={0}/host-prebuilt.dylib\nframework_out={0}/DmGameProbe.framework/DmGameProbe\nprofile={0}/profile.tsv\nromfs={0}/extract/RomFS.img\nboot_seconds=0\napp_build=B1\nshader_chain=cc0f2129e1de\ngame=Grizzy.nsp\nshader_free_gib=0\n{1}\n",
        docs.display(),
        extra
    )
}

fn chain(extra_cfg: &str, rw: Arc<dyn Rewriter>, shaders: Arc<dyn ShaderBackend>) -> Chain {
    let mut fx = Fx::new();
    let docs = fx.docs().to_path_buf();
    for m in ["main", "subsdk0", "sdk"] {
        write(docs.join("extract/ExeFS").join(m), format!("NSO:{m}"));
    }
    write(docs.join("extract/RomFS.img"), "rom");
    write(docs.join("extract/control.nacp"), "nacp");
    write(docs.join("extract/icon.jpg"), "JPEG");
    write(docs.join("host-prebuilt.dylib"), "HOST");
    write(docs.join("gameapp.config"), config(&docs, extra_cfg));
    fx.ctx.adapters = adapters(rw, shaders);
    fx.ctx.boot_enabled = false;
    let p = fx.p("gameapp.config");
    ibuilder_pipeline::chain::load_config(&mut fx.ctx, Some(&p));
    write(fx.p("gameapp.running"), "marker");
    Chain { fx }
}

/// the same Documents, a new run (the app starts the chain again)
fn rerun(c: &Chain, rw: Arc<dyn Rewriter>, shaders: Arc<dyn ShaderBackend>) -> (Ctx, Arc<MemLog>) {
    let log = MemLog::new();
    let mut ctx = Ctx::builder(c.fx.docs())
        .env(c.fx.env.clone())
        .log(log.clone())
        .mem(c.fx.mem.clone())
        .adapters(adapters(rw, shaders))
        .boot_enabled(false)
        .build();
    let p = c.fx.p("gameapp.config");
    ibuilder_pipeline::chain::load_config(&mut ctx, Some(&p));
    (ctx, log)
}

fn plain_rw() -> Arc<MockRewriter> {
    Arc::new(MockRewriter::default())
}
fn plain_shaders() -> Arc<MockShaders> {
    Arc::new(MockShaders {
        shaders: 5,
        ..Default::default()
    })
}

fn framework_bytes(docs: &std::path::Path) -> Vec<u8> {
    fs::read(docs.join("DmGameProbe.framework/DmGameProbe")).unwrap()
}

fn stray(docs: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![docs.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(e.path());
            } else if n.ends_with(".part") || n.contains(".tmp") {
                out.push(e.path().display().to_string());
            }
        }
    }
    out
}

// ---- the request --------------------------------------------------------------------------------------------------------------------------------------

/// A cancel overrides a pause; a pause never overrides a cancel; any other value withdraws whatever was asked.
#[test]
fn a_cancel_overrides_a_pause_and_a_pause_never_overrides_a_cancel() {
    let _g = serial();
    assert_eq!(stop::requested(), stop::NONE);
    assert_eq!(stop::request(stop::PAUSE), stop::PAUSE);
    assert_eq!(
        stop::request(stop::CANCEL),
        stop::CANCEL,
        "a cancel replaces a pause"
    );
    assert_eq!(
        stop::request(stop::PAUSE),
        stop::CANCEL,
        "a pause does not replace a cancel"
    );
    assert_eq!(stop::requested(), stop::CANCEL);
    assert_eq!(stop::request(0), stop::NONE, "0 withdraws");
    assert_eq!(stop::request(stop::PAUSE), stop::PAUSE);
    assert_eq!(
        stop::request(7),
        stop::NONE,
        "any other value withdraws too"
    );
    assert_eq!(stop::requested(), stop::NONE);
    assert_eq!(
        (
            stop::name(stop::PAUSE),
            stop::name(stop::CANCEL),
            stop::name(stop::NONE)
        ),
        ("pause", "cancel", "none")
    );
}

/// The context and the views the adapters get see the request, and its kind (the run's own token, `ibuilder_cancel`, is a cancel).
#[test]
fn the_context_and_its_views_see_the_request() {
    let _g = serial();
    let d = TempDir::new("stop-ctx");
    let ctx = Ctx::builder(d.path()).build();
    assert!(!ctx.cancelled() && !ctx.view().cancelled());
    stop::request(stop::PAUSE);
    assert!(ctx.cancelled() && ctx.view().cancelled());
    assert_eq!(ctx.stop_kind(), stop::PAUSE);
    stop::clear();
    assert!(!ctx.cancelled());
    ctx.cancel.store(true, Ordering::SeqCst);
    assert!(ctx.cancelled() && ctx.view().cancelled());
    assert_eq!(
        ctx.stop_kind(),
        stop::CANCEL,
        "the run's own token is a cancel"
    );
}

// ---- the chain ------------------------------------------------------------------------------------------------------------------------------------------

/// A pause raised while module 1 of 3 is rewritten: the module in hand finishes, the next is not started, the run ends with 131 and the result says paused. The timing partial is kept (the
/// build continues: no row is written for it) and nothing is left half written.
#[test]
fn a_pause_during_the_rewrite_ends_the_run_at_the_next_module_with_the_paused_result() {
    let _g = serial();
    let rw = Arc::new(RaisingRewriter {
        inner: MockRewriter::default(),
        kind: stop::PAUSE,
        after: 1,
        calls: AtomicUsize::new(0),
    });
    let mut c = chain("", rw.clone(), plain_shaders());
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_PAUSED, "{:?}", c.fx.log.lines());
    assert!(rep.result.starts_with("paused"), "{}", rep.result);
    assert_eq!(
        rw.inner.calls.lock().unwrap().len(),
        1,
        "main was rewritten, subsdk0 was not started"
    );
    assert!(
        c.fx.log.contains("[stop] the chain was paused"),
        "{:?}",
        c.fx.log.lines()
    );
    assert!(
        c.fx.p("gameapp.timing.partial").exists(),
        "a pause keeps what the build measured"
    );
    assert!(
        !c.fx.p("build_times.tsv").exists(),
        "a paused build is not a finished one: no row"
    );
    assert!(
        !c.fx.p("DmGameProbe.framework/DmGameProbe").exists(),
        "nothing was assembled"
    );
    assert!(stray(c.fx.docs()).is_empty(), "{:?}", stray(c.fx.docs()));
}

/// A cancel at the same point ends with 130, `cancelled` (as it always did), and says so in the log.
#[test]
fn a_cancel_during_the_rewrite_ends_with_the_cancelled_result() {
    let _g = serial();
    let rw = Arc::new(RaisingRewriter {
        inner: MockRewriter::default(),
        kind: stop::CANCEL,
        after: 1,
        calls: AtomicUsize::new(0),
    });
    let mut c = chain("", rw.clone(), plain_shaders());
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_CANCELLED);
    assert_eq!(rep.result, "cancelled");
    assert_eq!(rw.inner.calls.lock().unwrap().len(), 1);
    assert!(
        c.fx.log.contains("the chain was cancelled by the caller")
            && c.fx.log.contains("[stop] the chain was cancelled"),
        "{:?}",
        c.fx.log.lines()
    );
}

/// A cancel that comes after a pause (the player pauses, then gives up before the run has ended) wins: the run ends cancelled.
#[test]
fn a_cancel_after_a_pause_ends_the_run_cancelled() {
    let _g = serial();
    let rw = Arc::new(RaisingRewriter {
        inner: MockRewriter::default(),
        kind: stop::PAUSE,
        after: 1,
        calls: AtomicUsize::new(0),
    });
    let mut c = chain("", rw, plain_shaders());
    // the second request lands while the chain winds down from the first one: the rewriter raises the pause when module 1 is done, and the line the chain says right after it raises the cancel,
    // before the chain has looked at the request again
    struct CancelAfterPause(Arc<MemLog>);
    impl Log for CancelAfterPause {
        fn line(&self, s: &str) {
            self.0.line(s);
            if s.contains("[mem:after-rewrite-main] resident") {
                stop::request(stop::CANCEL);
            }
        }
    }
    let log = c.fx.log.clone();
    c.fx.ctx.log = Arc::new(CancelAfterPause(log));
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_CANCELLED, "{:?}", c.fx.log.lines());
    assert_eq!(rep.result, "cancelled");
}

/// A request that is there before a run starts, or that a finished run left behind, is not for the next run: the chain clears it when it starts.
#[test]
fn a_stale_request_does_not_stop_the_next_run() {
    let _g = serial();
    for kind in [stop::PAUSE, stop::CANCEL] {
        let mut c = chain("", plain_rw(), plain_shaders());
        stop::request(kind);
        let rep = run_chain(&mut c.fx.ctx);
        assert_eq!(rep.rc, RC_OK, "kind {kind}: {:?}", c.fx.log.lines());
        assert!(rep.all_ok());
        assert_eq!(stop::requested(), stop::NONE);
        assert!(c.fx.p("DmGameProbe.framework/DmGameProbe").exists());
    }
}

/// A pause between two stages: raised while stage 2 ends (its last module), the run stops before stage 3 starts: no shader work begins.
#[test]
fn a_pause_that_comes_with_the_last_module_stops_before_the_shaders() {
    let _g = serial();
    let shaders = Arc::new(Interrupted {
        kind: stop::NONE,
        splits: AtomicUsize::new(0),
        builds: AtomicUsize::new(0),
    });
    let rw = Arc::new(RaisingRewriter {
        inner: MockRewriter::default(),
        kind: stop::PAUSE,
        after: 3,
        calls: AtomicUsize::new(0),
    });
    let mut c = chain("", rw, shaders.clone());
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_PAUSED, "{:?}", c.fx.log.lines());
    assert_eq!(
        shaders.splits.load(Ordering::SeqCst),
        0,
        "stage 3 did not start"
    );
    assert!(!c.fx.p("shaders").join(TID).exists());
}

/// A pause that ends stage 3 (the build answers a stop with an error, as the real one does): the run ends paused, not failed; the work folder (the split, the cache) is kept and marked as
/// stopped by the user; nothing of the pack is installed.
#[test]
fn a_pause_in_the_shader_build_keeps_the_work_and_the_cache_and_ends_paused() {
    let _g = serial();
    let shaders = Arc::new(Interrupted {
        kind: stop::PAUSE,
        splits: AtomicUsize::new(0),
        builds: AtomicUsize::new(0),
    });
    let mut c = chain("", plain_rw(), shaders.clone());
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_PAUSED, "{:?}", c.fx.log.lines());
    assert!(!rep.status.contains_key("assemble"), "{:?}", rep.status);
    assert!(
        rep.status
            .get("shaders")
            .map(|s| s.starts_with("none"))
            .unwrap_or(true),
        "a stop is not 'no library': {:?}",
        rep.status
    );
    let work = c.fx.p(&format!("shaders/{TID}.work"));
    assert!(
        work.join("resume.json").exists() && work.join("split.ok").exists(),
        "the work folder is kept: the split is not lost"
    );
    assert!(
        work.join("stopped.by.user").exists(),
        "and says a person stopped it"
    );
    assert!(
        !work.join("out").exists(),
        "the build's own folder (its image, its stage folder) is not kept"
    );
    assert!(
        !c.fx.p(&format!("shaders/{TID}")).exists(),
        "no pack was installed"
    );
    assert!(
        c.fx.p("shadercache/cas-x.dmsb/entry").exists(),
        "the cache is not touched"
    );
    assert!(
        !c.fx.log.contains("the translation failed"),
        "{:?}",
        c.fx.log.lines()
    );
    assert!(
        c.fx.log.contains("[stop] stage 3 shaders: stopped"),
        "{:?}",
        c.fx.log.lines()
    );
    assert!(stray(c.fx.docs()).is_empty(), "{:?}", stray(c.fx.docs()));
}

/// A cancel in the shader build ends 130 and keeps the cache and the work folder as well (the app removes what it wants removed; the chain only guarantees the stop).
#[test]
fn a_cancel_in_the_shader_build_ends_cancelled_and_leaves_the_same_state() {
    let _g = serial();
    let shaders = Arc::new(Interrupted {
        kind: stop::CANCEL,
        splits: AtomicUsize::new(0),
        builds: AtomicUsize::new(0),
    });
    let mut c = chain("", plain_rw(), shaders);
    let rep = run_chain(&mut c.fx.ctx);
    assert_eq!(rep.rc, RC_CANCELLED, "{:?}", c.fx.log.lines());
    assert!(
        c.fx.p(&format!("shaders/{TID}.work/stopped.by.user"))
            .exists()
    );
    assert!(c.fx.p("shadercache/cas-x.dmsb/entry").exists());
    assert!(!c.fx.p(&format!("shaders/{TID}")).exists());
}

/// The run after a pause continues from what the pause kept, and finishes with the framework an uninterrupted run makes. The work folder of a run a person stopped is not a work folder an
/// earlier run died under: the resumed run is not slowed to one thread (that is for a kill by the OS), and it does not read the game's shaders again.
#[test]
fn the_run_after_a_pause_continues_and_finishes_with_the_uninterrupted_result() {
    let _g = serial();
    let shaders = Arc::new(Interrupted {
        kind: stop::PAUSE,
        splits: AtomicUsize::new(0),
        builds: AtomicUsize::new(0),
    });
    let c = chain("", plain_rw(), shaders.clone());
    let mut first = Ctx::builder(c.fx.docs())
        .env(c.fx.env.clone())
        .log(c.fx.log.clone())
        .mem(c.fx.mem.clone())
        .adapters(adapters(plain_rw(), shaders.clone()))
        .boot_enabled(false)
        .build();
    ibuilder_pipeline::chain::load_config(&mut first, Some(&c.fx.p("gameapp.config")));
    assert_eq!(run_chain(&mut first).rc, RC_PAUSED);
    assert!(c.fx.p("gameapp.timing.partial").exists());

    let rw2 = plain_rw();
    let (mut ctx2, log2) = rerun(&c, rw2.clone(), shaders.clone());
    let rep = run_chain(&mut ctx2);
    assert_eq!(rep.rc, RC_OK, "{:?}", log2.lines());
    assert!(rep.all_ok(), "{:?}", rep.status);
    assert_eq!(
        shaders.splits.load(Ordering::SeqCst),
        1,
        "the split of the first run is complete in the work folder: not read again"
    );
    assert!(
        log2.contains("the split of the earlier run is complete"),
        "{:?}",
        log2.lines()
    );
    assert!(log2.contains("stopped by the user"), "{:?}", log2.lines());
    assert!(
        !log2.contains("never finished"),
        "a pause is not a kill by the OS: {:?}",
        log2.lines()
    );
    assert!(
        !c.fx.p(&format!("shaders/{TID}.work")).exists(),
        "the finished build removes its work folder"
    );
    assert!(c.fx.p(&format!("shaders/{TID}")).exists());
    // the oracle: the same game, built straight through
    let oracle = chain("", plain_rw(), plain_shaders());
    let mut octx = Ctx::builder(oracle.fx.docs())
        .env(oracle.fx.env.clone())
        .log(oracle.fx.log.clone())
        .mem(oracle.fx.mem.clone())
        .adapters(adapters(plain_rw(), plain_shaders()))
        .boot_enabled(false)
        .build();
    ibuilder_pipeline::chain::load_config(&mut octx, Some(&oracle.fx.p("gameapp.config")));
    assert_eq!(run_chain(&mut octx).rc, RC_OK);
    let norm = |b: Vec<u8>, docs: &std::path::Path| {
        String::from_utf8_lossy(&b)
            .replace(&docs.display().to_string(), "<docs>")
            .into_bytes()
    };
    assert_eq!(
        norm(framework_bytes(c.fx.docs()), c.fx.docs()),
        norm(framework_bytes(oracle.fx.docs()), oracle.fx.docs()),
        "the framework after a pause is the uninterrupted one"
    );
    assert!(stray(c.fx.docs()).is_empty());
}

/// The rewrite after a pause takes the modules that finished from the rewrite cache: main is not rewritten again.
#[test]
fn the_rewrite_after_a_pause_does_not_redo_the_modules_that_finished() {
    let _g = serial();
    let rw = Arc::new(RaisingRewriter {
        inner: MockRewriter::default(),
        kind: stop::PAUSE,
        after: 1,
        calls: AtomicUsize::new(0),
    });
    let c = chain("", rw, plain_shaders());
    let mut first = Ctx::builder(c.fx.docs())
        .env(c.fx.env.clone())
        .log(c.fx.log.clone())
        .mem(c.fx.mem.clone())
        .adapters(adapters(
            Arc::new(RaisingRewriter {
                inner: MockRewriter::default(),
                kind: stop::PAUSE,
                after: 1,
                calls: AtomicUsize::new(0),
            }),
            plain_shaders(),
        ))
        .boot_enabled(false)
        .build();
    ibuilder_pipeline::chain::load_config(&mut first, Some(&c.fx.p("gameapp.config")));
    assert_eq!(run_chain(&mut first).rc, RC_PAUSED);
    let rw2 = plain_rw();
    let (mut ctx2, log2) = rerun(&c, rw2.clone(), plain_shaders());
    assert_eq!(run_chain(&mut ctx2).rc, RC_OK, "{:?}", log2.lines());
    let done: Vec<String> = rw2
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(m, _)| m.clone())
        .collect();
    assert_eq!(done, ["subsdk0", "sdk"], "main came from the cache");
    assert!(
        log2.contains("main: rewrite taken from the cache"),
        "{:?}",
        log2.lines()
    );
}

// ---- the waits -----------------------------------------------------------------------------------------------------------------------------------------

/// time moves only when the gate sleeps; the hook runs at each sleep and a sleep count past `limit` is the test failing instead of hanging
struct StepClock {
    t: Mutex<f64>,
    slept: AtomicUsize,
    hook: Box<dyn Fn(usize) + Send + Sync>,
}
impl Clock for StepClock {
    fn now(&self) -> f64 {
        *self.t.lock().unwrap()
    }
    fn sleep(&self, s: f64) {
        *self.t.lock().unwrap() += s;
        let n = self.slept.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            n < 2000,
            "the wait went on for {n} sleeps after the stop request"
        );
        (self.hook)(n);
    }
}

/// A phone that stays critically hot holds new work back for as long as it stays hot; a stop request ends that wait (the user does not wait for the phone to cool to stop a build).
#[test]
fn a_stop_ends_the_wait_for_a_hot_phone() {
    let _g = serial();
    let d = TempDir::new("stop-thermal");
    let path = d.join("gameapp.thermal");
    write(&path, "3 0\n");
    let clock = Arc::new(StepClock {
        t: Mutex::new(1000.0),
        slept: AtomicUsize::new(0),
        hook: Box::new(|n| {
            if n == 3 {
                stop::request(stop::PAUSE);
            }
        }),
    });
    let log = MemLog::new();
    let gate = ThermalGate::new(&path, log.clone(), clock.clone());
    let waited = gate.wait(2, "stage 3");
    assert!(waited >= 3.0 * ThermalGate::POLL, "{waited}");
    assert_eq!(
        clock.slept.load(Ordering::SeqCst),
        3,
        "the third sleep raised the request, the wait ended with it"
    );
    assert!(
        log.contains("cooling down") && log.lines().iter().any(|l| l.contains("stop request")),
        "{:?}",
        log.lines()
    );
    stop::clear();
}

/// A job held back by the memory gate (less than 900 MiB left while another runs) waits up to 30 s; a stop request lets it through at once, so that the build can wind down.
#[test]
fn a_stop_lets_a_job_through_the_memory_gate_at_once() {
    let _g = serial();
    let mem = Arc::new(FakeMem::new(300 * (1 << 20)));
    let clock = Arc::new(StepClock {
        t: Mutex::new(0.0),
        slept: AtomicUsize::new(0),
        hook: Box::new(|n| {
            if n == 2 {
                stop::request(stop::CANCEL);
            }
        }),
    });
    let gate = Arc::new(ShaderMemoryGate::new(
        mem,
        MemLog::new(),
        None,
        clock.clone(),
    ));
    let started = Arc::new(AtomicUsize::new(0));
    let (g2, s2) = (gate.clone(), started.clone());
    // one job is running; the second has to wait
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (tx2, rx2) = std::sync::mpsc::channel::<()>();
    let runner = std::thread::spawn(move || {
        g2.run(|| {
            s2.fetch_add(1, Ordering::SeqCst);
            tx.send(()).unwrap();
            rx2.recv().unwrap();
        })
    });
    rx.recv().unwrap();
    assert_eq!(gate.active(), 1);
    let ran = gate.run(|| 7);
    assert_eq!(ran, 7);
    assert_eq!(
        clock.slept.load(Ordering::SeqCst),
        2,
        "waited two sleeps, then the request let it through (without one it waits 120)"
    );
    tx2.send(()).unwrap();
    runner.join().unwrap();
    stop::clear();
}

// ---- review r4 follow-up H1: a stop during a boot-only launch's stage 3 is a stop, not a boot with no library -----------------------------------------

#[test]
fn a_stop_during_a_boot_only_launchs_stage_3_boots_nothing_and_ends_paused_or_cancelled() {
    let _one = serial();
    for (kind, rc) in [(stop::PAUSE, RC_PAUSED), (stop::CANCEL, RC_CANCELLED)] {
        stop::clear();
        let c = chain("keep_exefs=1\n", plain_rw(), plain_shaders());
        let mut fx_ctx = rerun(&c, plain_rw(), plain_shaders()).0;
        assert!(
            run_chain(&mut fx_ctx).all_ok(),
            "the build: {:?}",
            c.fx.log.lines()
        );
        let docs = c.fx.docs().to_path_buf();
        fs::remove_dir_all(docs.join(format!("shaders/{TID}"))).unwrap(); // the library is gone: the boot-only launch builds it first
        write(
            docs.join("gameapp.boot-only"),
            "boot the framework that is already built and signed\n",
        );
        write(docs.join("gameapp.running"), "marker\nbootonly=1\n");
        let boot = Arc::new(MockBoot::default());
        let mut a = adapters(
            plain_rw(),
            Arc::new(Interrupted {
                kind,
                splits: AtomicUsize::new(0),
                builds: AtomicUsize::new(0),
            }),
        );
        a.boot = Some(boot.clone());
        let log = MemLog::new();
        let mut ctx = Ctx::builder(&docs)
            .env(c.fx.env.clone())
            .log(log.clone())
            .mem(c.fx.mem.clone())
            .adapters(a)
            .boot_enabled(true)
            .build();
        ibuilder_pipeline::chain::load_config(&mut ctx, Some(&docs.join("gameapp.config")));
        let rep = run_chain(&mut ctx);
        stop::clear();
        assert_eq!(rep.rc, rc, "{}: {:?}", stop::name(kind), log.lines());
        assert_eq!(
            boot.requests.lock().unwrap().len(),
            0,
            "{}: the game was booted with no library",
            stop::name(kind)
        );
        assert!(
            log.contains("[stop] stage 5 boot-only: a ") && log.contains("the game is not booted"),
            "{:?}",
            log.lines()
        );
        assert!(
            !docs.join("gameapp.running").exists(),
            "the run ended by itself"
        );
        assert_eq!(
            docs.join("gameapp.boot-only").exists(),
            kind == stop::PAUSE,
            "a pause keeps the request (a later launch continues), a cancel consumes it"
        );
        assert!(
            !docs
                .join(format!("shaders/{TID}/b-cc0f2129e1de.img"))
                .exists(),
            "no library was made"
        );
    }
}
