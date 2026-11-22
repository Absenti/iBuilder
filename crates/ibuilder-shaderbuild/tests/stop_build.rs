//! A stop asked of a running build (the app's Pause and Cancel): the workers finish the shader in hand and start no other, what finished is in the shader cache whole, nothing is left
//! beside it under a temporary name, and a build that follows restores the finished shaders from the cache and makes the library an uninterrupted build makes. A synthetic game, no engine.
//!
//! The process-wide request (`ibuilder_pipeline::stop`) is one byte of the process, so every test here takes `serial()` and starts with it cleared.
mod synth;
use ibuilder_pipeline::stop;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, BuildReport, build};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};
use synth::*;

const TOTAL: usize = 1200;
const WORKERS: usize = 6;

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    stop::clear();
    g
}

fn options(root: &std::path::Path, out: &str, cas: &std::path::Path) -> BuildOptions {
    let mut o = BuildOptions::new(
        &root.join("pkg"),
        &root.join("profile"),
        "G",
        &root.join(out),
        CHAIN,
        cas,
    );
    o.jobs = WORKERS;
    o.tables = false;
    o.pulling = false;
    o
}

/// the build on a thread with a hard time limit: a stop that does not take hold fails the test instead of hanging it
fn run_limited(
    o: BuildOptions,
    fin: &Slow,
    xlate_ms: u64,
    cancel: Option<&AtomicBool>,
) -> Result<BuildReport, String> {
    std::thread::scope(|s| {
        let (tx, rx) = mpsc::channel();
        s.spawn(move || {
            let t = tools(xlate_ms);
            let log = |_: &str| {};
            let r = build(
                &o,
                &BuildEnv {
                    tools: &t,
                    finisher: fin,
                    log: &log,
                    cancel,
                },
            );
            let _ = tx.send(r);
        });
        rx.recv_timeout(Duration::from_secs(120))
            .expect("the build did not end within 120 s")
    })
}

/// the state a stopped build must leave: every shader it started is finished and in the cache whole, no temporary file anywhere
fn assert_clean_stop(
    root: &std::path::Path,
    cas: &std::path::Path,
    fin: &Slow,
    raised_at: usize,
) -> usize {
    let (started, finished) = (fin.started(), fin.finished());
    assert!(
        started >= raised_at,
        "the stop was raised by shader {raised_at}: {started}"
    );
    assert!(
        started <= raised_at + WORKERS,
        "{started} shaders were started: the workers went on after the stop (a batch is {})",
        4 * WORKERS
    );
    assert_eq!(
        started, finished,
        "a shader that was started is finished, never abandoned"
    );
    assert_eq!(
        assert_entries_intact(cas),
        finished,
        "every shader that finished is in the cache, whole"
    );
    assert!(
        stray_files(root).is_empty(),
        "left behind: {:?}",
        stray_files(root)
    );
    finished
}

/// The stop token the build already had (`BuildEnv::cancel`), raised by the finisher of the 100th shader. A worker that sees it finishes the shader in hand and starts no other: at most one
/// shader per worker after the raise, never the rest of the batch of 24 (nor the next one, which the translator thread had already begun).
#[test]
fn a_stop_token_ends_the_build_after_at_most_one_more_shader_per_worker() {
    let _g = serial();
    let root = scratch("token");
    write_game(&root, "G", TOTAL);
    let cas = root.join("cas");
    let flag: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false))); // the finisher's closure outlives any borrow of this test
    let fin = Slow::raising_at(2, 100, move || flag.store(true, Ordering::SeqCst));
    let r = run_limited(options(&root, "out", &cas), &fin, 0, Some(flag));
    assert_eq!(r.err().as_deref(), Some("cancelled"));
    assert_clean_stop(&root, &cas, &fin, 100);
    let _ = std::fs::remove_dir_all(&root);
}

/// The process-wide request, as the app raises it (a PAUSE from another thread's point of view: the finisher stands for it). The build ends with the error `"cancelled"` (the pipeline reads
/// the request itself to tell a pause from a cancel), and the request is left as it was: the build does not clear it.
#[test]
fn the_process_wide_pause_ends_the_build_the_same_way_and_is_left_raised() {
    let _g = serial();
    let root = scratch("pause");
    write_game(&root, "G", TOTAL);
    let cas = root.join("cas");
    let fin = Slow::raising_at(2, 250, || {
        stop::request(stop::PAUSE);
    });
    let r = run_limited(options(&root, "out", &cas), &fin, 0, None);
    assert_eq!(r.err().as_deref(), Some("cancelled"));
    assert_clean_stop(&root, &cas, &fin, 250);
    assert_eq!(
        stop::requested(),
        stop::PAUSE,
        "the build does not clear a request: the chain does, when its next run starts"
    );
    stop::clear();
    let _ = std::fs::remove_dir_all(&root);
}

/// After a pause: a build of the same game restores what finished from the cache (no shader of it is finished again), finishes the rest, and the library it writes is the library an
/// uninterrupted build in a cache of its own writes, byte for byte.
#[test]
fn a_build_after_a_pause_restores_the_finished_shaders_and_writes_the_uninterrupted_library() {
    let _g = serial();
    let root = scratch("resume");
    write_game(&root, "G", TOTAL);
    let cas = root.join("cas");
    let fin1 = Slow::raising_at(1, 400, || {
        stop::request(stop::PAUSE);
    });
    assert_eq!(
        run_limited(options(&root, "out1", &cas), &fin1, 0, None)
            .err()
            .as_deref(),
        Some("cancelled")
    );
    let done = assert_clean_stop(&root, &cas, &fin1, 400);
    assert!(done < TOTAL, "the build was stopped before the end");
    // the request is withdrawn (the chain does it when a run starts), the same game again
    stop::clear();
    let fin2 = Slow::new(0);
    let r2 = run_limited(options(&root, "out2", &cas), &fin2, 0, None).unwrap();
    assert_eq!(
        (r2.shaders, r2.ok, r2.cache_hit, r2.cache_miss),
        (TOTAL, TOTAL, done, TOTAL - done),
        "the finished shaders are hits"
    );
    assert_eq!(
        fin2.started(),
        TOTAL - done,
        "only the shaders that were not finished are finished now"
    );
    // the oracle: the same game, built straight through in a cache of its own
    let fin3 = Slow::new(0);
    let r3 = run_limited(
        options(&root, "out3", &root.join("cas-clean")),
        &fin3,
        0,
        None,
    )
    .unwrap();
    assert_eq!((r3.cache_hit, r3.cache_miss), (0, TOTAL));
    assert!(
        std::fs::read(&r2.image).unwrap() == std::fs::read(&r3.image).unwrap(),
        "the library after a pause is not the one an uninterrupted build writes"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A stop that is raised before the build starts its work (a request is read at the first loop of the build, the digests) ends it there, with nothing finished and nothing left over.
#[test]
fn a_request_that_is_already_there_stops_the_build_before_any_shader_is_translated() {
    let _g = serial();
    let root = scratch("early");
    write_game(&root, "G", 300);
    let cas = root.join("cas");
    stop::request(stop::CANCEL);
    let fin = Slow::new(0);
    let r = run_limited(options(&root, "out", &cas), &fin, 0, None);
    assert_eq!(r.err().as_deref(), Some("cancelled"));
    assert_eq!(fin.started(), 0);
    assert_eq!(cache_entries(&cas).len(), 0);
    assert!(stray_files(&root).is_empty(), "{:?}", stray_files(&root));
    stop::clear();
    let _ = std::fs::remove_dir_all(&root);
}

/// The live translation of a running game turns the request off (`follow_stop`): a Pause or Cancel that comes late is not for the game's shaders.
#[test]
fn a_build_that_does_not_follow_the_request_is_not_stopped_by_it() {
    let _g = serial();
    let root = scratch("nofollow");
    write_game(&root, "G", 120);
    stop::request(stop::PAUSE);
    let fin = Slow::new(0);
    let mut o = options(&root, "out", &root.join("cas"));
    o.follow_stop = false;
    let r = run_limited(o, &fin, 0, None).unwrap();
    assert_eq!((r.shaders, r.ok), (120, 120));
    stop::clear();
    let _ = std::fs::remove_dir_all(&root);
}

/// How long a stop takes to take hold, with the request raised from another thread (the UI's) while 6 workers build: the translator call in hand (here 100 ms, about what four shaders cost the
/// app's translator) and the shader in hand (5 ms). Measured, printed, and held to the target of the feature: under 2 s.
#[test]
fn the_stop_latency_from_another_thread_is_well_under_two_seconds() {
    let _g = serial();
    let root = scratch("latency");
    write_game(&root, "G", TOTAL);
    let cas = root.join("cas");
    let fin = Slow::new(5);
    let raised: Mutex<Option<Instant>> = Mutex::new(None);
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(700));
            *raised.lock().unwrap() = Some(Instant::now());
            stop::request(stop::PAUSE);
        });
        run_limited(options(&root, "out", &cas), &fin, 100, None)
    });
    let ended = Instant::now();
    assert_eq!(r.err().as_deref(), Some("cancelled"));
    let latency = ended.duration_since(raised.lock().unwrap().expect(
        "the request was raised before the build ended: the build is too short for this test",
    ));
    eprintln!(
        "stop latency: {:?} ({} of {TOTAL} shaders finished, 6 workers, 100 ms translator calls, 5 ms shaders)",
        latency,
        fin.finished()
    );
    assert!(
        latency < Duration::from_secs(2),
        "the stop took {latency:?}"
    );
    assert_eq!(fin.started(), fin.finished());
    assert_eq!(assert_entries_intact(&cas), fin.finished());
    stop::clear();
    let _ = std::fs::remove_dir_all(&root);
}
