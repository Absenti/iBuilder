//! The whole chain's stage 3 over the real shader build (`ibuilder_shaderbuild::build`), a synthetic game and the pipeline's stand-in workers for the other stages: what the app's Pause and
//! Cancel do end to end. The chain ends with 131 / 130, the shaders that finished are in `Documents/shadercache` whole, the work folder is kept, and the run that follows restores them from the
//! cache and installs the library an uninterrupted chain installs, byte for byte. (The app's own backend, `ibuilder-cli`'s `RustShaders::build`, runs this same `build`.)
//!
//! The request is one byte of the process: every test takes `serial()`.
mod synth;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::chain::{RC_CANCELLED, RC_OK, RC_PAUSED, load_config, run_chain};
use ibuilder_pipeline::ctx::Ctx;
use ibuilder_pipeline::error::PipelineError;
use ibuilder_pipeline::log::{Log, MemLog};
use ibuilder_pipeline::mock::*;
use ibuilder_pipeline::stop;
use ibuilder_pipeline::testutil::{TempDir, write};
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, build};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};
use synth::*;

const TID: &str = "0100DFA027504000";
const TOTAL: usize = 1200;

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    stop::clear();
    g
}

/// The app's shader backend, with the split replaced by a synthetic one: the build is the real `build`, on the cache folder and the work folder the pipeline hands it, as `ibuilder-cli` does.
struct Backend {
    fin: Arc<Slow>,
    xlate_ms: u64,
    splits: AtomicUsize,
}

impl ShaderBackend for Backend {
    fn split_version(&self) -> String {
        "synth-split-1".into()
    }
    fn split(
        &self,
        req: &SplitRequest<'_>,
        _log: &dyn Log,
    ) -> ibuilder_pipeline::Result<SplitResult> {
        self.splits.fetch_add(1, Ordering::SeqCst);
        write_split(req.pkg_dir, req.tables_dir, TOTAL);
        Ok(SplitResult {
            shaders: TOTAL,
            jobs_reused: 0,
            jobs_ran: 1,
        })
    }
    fn build(
        &self,
        req: &BuildRequest<'_>,
        progress: &mut dyn FnMut(&str),
    ) -> ibuilder_pipeline::Result<PathBuf> {
        let mut o = BuildOptions::new(
            req.pkg_dir,
            req.profile_dir,
            req.game_id,
            req.out_dir,
            req.chain,
            req.cas,
        );
        o.jobs = req.jobs.max(1);
        o.tables = req.tables;
        o.pulling = req.pulling;
        let tools = tools(self.xlate_ms);
        let fin = self.fin.clone();
        // the build logs from its worker threads; `progress` is the caller's (not `Send`): the lines travel to this thread, as in the app's backend
        let (tx, rx) = mpsc::channel::<String>();
        let report = std::thread::scope(|s| {
            let h = s.spawn(move || {
                let log = move |l: &str| {
                    let _ = tx.send(l.to_string());
                };
                build(
                    &o,
                    &BuildEnv {
                        tools: &tools,
                        finisher: &*fin,
                        log: &log,
                        cancel: None,
                    },
                )
            });
            for line in rx {
                progress(&line);
            }
            h.join()
        });
        match report {
            Ok(Ok(r)) => Ok(r.image),
            Ok(Err(e)) => Err(PipelineError::failed(e)),
            Err(_) => Err(PipelineError::failed("the shader build thread panicked")),
        }
    }
    fn live_translate(
        &self,
        req: &LiveRequest<'_>,
        log: &dyn Log,
    ) -> ibuilder_pipeline::Result<LiveResult> {
        MockShaders::default().live_translate(req, log)
    }
}

struct Run {
    docs: TempDir,
}

fn docs() -> Run {
    let d = TempDir::new("stop-chain");
    let p = d.path().to_path_buf();
    for m in ["main", "subsdk0", "sdk"] {
        write(p.join("extract/ExeFS").join(m), format!("NSO:{m}"));
    }
    write(p.join("extract/RomFS.img"), "rom");
    write(p.join("extract/control.nacp"), "nacp");
    write(p.join("extract/icon.jpg"), "JPEG");
    write(p.join("host-prebuilt.dylib"), "HOST");
    write(
        p.join("gameapp.config"),
        format!(
            "extract_out={0}/extract\nexe_dir={0}/extract/ExeFS\nmodules=main subsdk0 sdk\npatched_dir={0}/extract/rewrite\nhost_dylib={0}/host-prebuilt.dylib\nframework_out={0}/DmGameProbe.framework/DmGameProbe\nprofile={0}/profile.tsv\nromfs={0}/extract/RomFS.img\nboot_seconds=0\napp_build=B1\nshader_chain={1}\ngame=Grizzy.nsp\nshader_free_gib=0\nshader_jobs=6\nshader_tables=0\n",
            p.display(),
            CHAIN
        ),
    );
    Run { docs: d }
}

fn ctx_for(r: &Run, backend: Arc<Backend>) -> (Ctx, Arc<MemLog>) {
    let log = MemLog::new();
    let mut ctx = Ctx::builder(r.docs.path()).log(log.clone()).build();
    ctx.adapters = Adapters {
        nup: None,
        profile: Arc::new(MockProfile::new(TID, "main subsdk0 sdk")),
        rewriter: Arc::new(MockRewriter::default()),
        shaders: Some(backend),
        tools: Some(mock_tools()),
        assembler: Arc::new(MockAssembler),
        signer: Arc::new(MockSigner { rc: 0 }),
        boot: None,
        launcher: Some(Arc::new(MockLauncher)),
        mods: None,
    };
    load_config(&mut ctx, Some(&r.docs.path().join("gameapp.config")));
    (ctx, log)
}

fn backend(fin: Slow, xlate_ms: u64) -> (Arc<Backend>, Arc<Slow>) {
    let fin = Arc::new(fin);
    (
        Arc::new(Backend {
            fin: fin.clone(),
            xlate_ms,
            splits: AtomicUsize::new(0),
        }),
        fin,
    )
}

fn library(r: &Run) -> Vec<u8> {
    fs::read(
        r.docs
            .path()
            .join("shaders")
            .join(TID)
            .join(format!("b-{CHAIN}.img")),
    )
    .unwrap()
}

/// The chain that is stopped by `kind` when the `at`-th shader begins: ends with `rc`, whole entries in the cache for every shader that was started, the work folder kept and marked, no pack,
/// nothing left under a temporary name anywhere in Documents. Returns the finished count.
fn stopped_chain(kind: u8, at: usize, rc: i32) -> (Run, usize) {
    let r = docs();
    let (b, fin) = backend(
        Slow::raising_at(2, at, move || {
            stop::request(kind);
        }),
        0,
    );
    let (mut ctx, log) = ctx_for(&r, b.clone());
    let rep = run_chain(&mut ctx);
    assert_eq!(rep.rc, rc, "{:?}", log.lines());
    let (started, finished) = (fin.started(), fin.finished());
    assert!(
        started >= at && started <= at + 6,
        "{started} shaders were started after the stop was raised by shader {at} (6 workers)"
    );
    assert_eq!(started, finished);
    let cas = r.docs.path().join("shadercache");
    assert_eq!(
        assert_entries_intact(&cas),
        finished,
        "every shader that finished is in the shader cache, whole"
    );
    let work = r.docs.path().join("shaders").join(format!("{TID}.work"));
    assert!(
        work.join("split.ok").exists() && work.join("stopped.by.user").exists(),
        "the work folder is kept and marked"
    );
    assert!(
        !r.docs.path().join("shaders").join(TID).exists(),
        "no pack was installed"
    );
    assert!(
        !r.docs
            .path()
            .join("DmGameProbe.framework/DmGameProbe")
            .exists(),
        "nothing was assembled"
    );
    assert!(
        stray_files(r.docs.path()).is_empty(),
        "left behind: {:?}",
        stray_files(r.docs.path())
    );
    assert!(
        log.contains("[stop] shader build stopped"),
        "{:?}",
        log.lines()
            .iter()
            .filter(|l| l.contains("[stop]"))
            .collect::<Vec<_>>()
    );
    (r, finished)
}

#[test]
fn a_pause_in_stage_3_keeps_the_finished_shaders_and_the_run_after_it_installs_the_uninterrupted_library()
 {
    let _g = serial();
    let (r, done) = stopped_chain(stop::PAUSE, 300, RC_PAUSED);
    assert!(done < TOTAL);
    let (b2, fin2) = backend(Slow::new(0), 0);
    let (mut ctx2, log2) = ctx_for(&r, b2.clone());
    let rep = run_chain(&mut ctx2);
    assert_eq!(rep.rc, RC_OK, "{:?}", log2.lines());
    assert!(rep.all_ok(), "{:?}", rep.status);
    assert_eq!(
        b2.splits.load(Ordering::SeqCst),
        0,
        "the split of the first run was kept: not read again"
    );
    assert_eq!(
        fin2.started(),
        TOTAL - done,
        "only the shaders that were not finished are finished now; the others are restored from the cache"
    );
    assert!(
        log2.contains("shader cache: hit")
            && log2
                .lines()
                .iter()
                .any(|l| l.contains(&format!("hit {done}, rebuilt {}", TOTAL - done))),
        "{:?}",
        log2.lines()
            .iter()
            .filter(|l| l.contains("cache"))
            .collect::<Vec<_>>()
    );
    assert!(
        !log2.contains("never finished"),
        "a pause is not a kill by the OS: it is not read on one thread"
    );
    assert!(
        stray_files(r.docs.path()).is_empty(),
        "{:?}",
        stray_files(r.docs.path())
    );
    // the oracle: the same game, one chain run, nobody stopped it
    let o = docs();
    let (bo, _) = backend(Slow::new(0), 0);
    let (mut octx, olog) = ctx_for(&o, bo);
    assert_eq!(run_chain(&mut octx).rc, RC_OK, "{:?}", olog.lines());
    assert!(
        library(&r) == library(&o),
        "the library after a pause is not the one an uninterrupted chain installs"
    );
}

#[test]
fn a_cancel_in_stage_3_ends_130_in_the_same_state() {
    let _g = serial();
    let (_r, done) = stopped_chain(stop::CANCEL, 500, RC_CANCELLED);
    assert!(done < TOTAL);
}

/// The request raised by another thread (the UI's) while the chain runs on this one: how long the chain takes to end. The translator calls here take 100 ms (about four shaders' worth for the app's
/// translator); measured, printed, held to the target of under 2 s.
#[test]
fn the_chains_stop_latency_from_another_thread_is_well_under_two_seconds() {
    let _g = serial();
    let r = docs();
    let (b, fin) = backend(Slow::new(5), 100);
    let (mut ctx, log) = ctx_for(&r, b);
    let raised: Mutex<Option<Instant>> = Mutex::new(None);
    let rep = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(1200)); // well after the chain started (it clears the request when it starts) and inside the translation
            *raised.lock().unwrap() = Some(Instant::now());
            stop::request(stop::PAUSE);
        });
        run_chain(&mut ctx)
    });
    let ended = Instant::now();
    assert_eq!(rep.rc, RC_PAUSED, "{:?}", log.lines());
    let latency = ended.duration_since(
        raised
            .lock()
            .unwrap()
            .expect("the chain ended before the request was raised"),
    );
    eprintln!(
        "chain stop latency: {latency:?} ({} of {TOTAL} shaders finished, 6 workers, 100 ms translator calls)",
        fin.finished()
    );
    assert!(latency < Duration::from_secs(2), "{latency:?}");
    assert_eq!(fin.started(), fin.finished());
}
