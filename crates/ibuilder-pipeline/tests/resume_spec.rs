//! The semantics `gameapp/test_resume.py` tests (what keeps a build going when the phone interferes, docs/DECISIONS.md traps 74-76), as Rust tests. `// py:` names the Python check.
//! Not carried over: the split's `JobMemo` / stand-in-engine checks (an interrupted split resumed is byte-identical): those test `shader_device.py`, i.e. the `ibuilder-shader` crate's
//! memo, and are listed in docs/chain/PIPELINE-RUST.md as that crate's; and the "markers the build script looks for" source grep.

mod common;

use common::*;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::gate::ShaderMemoryGate;
use ibuilder_pipeline::log::{MemLog, NullLog};
use ibuilder_pipeline::markers::*;
use ibuilder_pipeline::memory::{AddressSpace, FakeMem};
use ibuilder_pipeline::rewrite::{rewrite_cache_dir, rewrite_is_cached, rewrite_key};
use ibuilder_pipeline::shaders::*;
use ibuilder_pipeline::testutil::set_mtime;
use ibuilder_pipeline::thermal::*;
use ibuilder_pipeline::timings::*;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---- the thermal file ---------------------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_thermal_file_reads() {
    // py: "no thermal file reads as "0 0"", ""2 1": serious, Low performance on", "a garbled file reads as "0 0"", "a line with the state only: low performance off", "a state out of range is clamped"
    let d = ibuilder_pipeline::testutil::TempDir::new("thermal");
    let path = d.join("gameapp.thermal");
    assert_eq!(read_thermal(&path), (0, false));
    write(&path, "2 1\n");
    assert_eq!(read_thermal(&path), (2, true));
    write(&path, "garbage");
    assert_eq!(read_thermal(&path), (0, false));
    write(&path, "3\n");
    assert_eq!(read_thermal(&path), (3, false));
    write(&path, "9 1");
    assert_eq!(read_thermal(&path), (3, true));
    write(&path, "-4 0");
    assert_eq!(
        read_thermal(&path),
        (0, false),
        "a negative state is clamped to nominal"
    );
    write(&path, "2 x");
    assert_eq!(
        read_thermal(&path),
        (0, false),
        "a garbled second field makes the whole read '0 0', as int() raising did"
    );
}

type SleepHook = Box<dyn Fn(usize) + Send + Sync>;

/// a clock and a sleep that agree: sleeping moves time, and calls a hook (the test changes the thermal file there)
struct FakeClock {
    t: Mutex<f64>,
    slept: AtomicUsize,
    hook: Mutex<Option<SleepHook>>,
}
impl FakeClock {
    fn new(hook: Option<SleepHook>) -> Arc<FakeClock> {
        Arc::new(FakeClock {
            t: Mutex::new(1000.0),
            slept: AtomicUsize::new(0),
            hook: Mutex::new(hook),
        })
    }
    fn advance(&self, s: f64) {
        *self.t.lock().unwrap() += s;
    }
}
impl Clock for FakeClock {
    fn now(&self) -> f64 {
        *self.t.lock().unwrap()
    }
    fn sleep(&self, s: f64) {
        *self.t.lock().unwrap() += s;
        let n = self.slept.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(h) = self.hook.lock().unwrap().as_ref() {
            h(n);
        }
    }
}

#[test]
fn the_gate_reads_cheaply_caps_low_performance_and_pauses_once_for_serious_heat() {
    // py: "50 asks within the poll interval read the file once", "and again once the interval has passed", "Low performance mode: at most 2 jobs", "off again: no cap"
    let d = ibuilder_pipeline::testutil::TempDir::new("gate");
    let path = d.join("gameapp.thermal");
    let lines = MemLog::new();
    let clock = FakeClock::new(None);
    let gate = ThermalGate::new(&path, lines.clone(), clock.clone());
    write(&path, "0 0\n");
    for _ in 0..50 {
        gate.wait(2, "stage 3");
        gate.parallel_cap();
    }
    assert_eq!(gate.file_reads(), 1);
    clock.advance(3.5);
    gate.parallel_cap();
    assert_eq!(gate.file_reads(), 2);
    write(&path, "1 1\n");
    clock.advance(3.5);
    assert_eq!(gate.parallel_cap(), Some(2));
    write(&path, "1 0\n");
    clock.advance(3.5);
    assert_eq!(gate.parallel_cap(), None);

    // py: "state serious: the job waits until the state is fair or nominal", "one "cooling down" line for the pause (not one per poll)", "and one line saying how long it lasted",
    //     "the seconds are totalled for the timings row", "state fair: nothing waits"
    let states = ["2 0\n", "3 0\n", "2 0\n", "1 0\n"];
    write(&path, states[0]);
    let p2 = path.clone();
    let clock2 = FakeClock::new(Some(Box::new(move |n| {
        write(&p2, states[n.min(states.len() - 1)])
    })));
    let lines = MemLog::new();
    let gate2 = ThermalGate::new(&path, lines.clone(), clock2.clone());
    let waited = gate2.wait(2, "stage 3");
    assert!(
        waited >= 3.0 * ThermalGate::POLL && gate2.read().0 == 1,
        "{waited} {:?}",
        gate2.read()
    );
    let cooling = lines
        .lines()
        .into_iter()
        .filter(|l| l.contains("cooling down"))
        .count();
    assert!(
        cooling == 1 && lines.lines().len() == 2,
        "{:?}",
        lines.lines()
    );
    assert!(lines.last().unwrap().contains("cooled down after"));
    assert!(gate2.paused_s() >= 3.0 * ThermalGate::POLL);
    lines.clear();
    assert!(gate2.wait(2, "stage 3") == 0.0 && lines.lines().is_empty());
}

#[test]
fn six_jobs_held_back_at_once_make_one_pause_and_one_line() {
    // py: "six jobs held back at once while it is hot", "all go on when it cools, with one "cooling down" line"
    let d = ibuilder_pipeline::testutil::TempDir::new("gate6");
    let path = d.join("gameapp.thermal");
    write(&path, "2 0\n");
    let lines = MemLog::new();
    let gate = Arc::new(ThermalGate::new(&path, lines.clone(), RealClock::new()));
    gate.set_poll(0.05);
    let done = Arc::new(AtomicUsize::new(0));
    let hs: Vec<_> = (0..6)
        .map(|_| {
            let (g, dn) = (gate.clone(), done.clone());
            std::thread::spawn(move || {
                g.wait(2, "stage 3");
                dn.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        done.load(Ordering::SeqCst),
        0,
        "six jobs held back at once while it is hot"
    );
    write(&path, "1 0\n");
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(done.load(Ordering::SeqCst), 6);
    assert_eq!(
        lines
            .lines()
            .into_iter()
            .filter(|l| l.contains("cooling down"))
            .count(),
        1,
        "{:?}",
        lines.lines()
    );
}

#[test]
fn the_rewrite_waits_between_modules_only_at_critical() {
    // py: "the rewrite does not pause at serious (level 3 only)", "at critical it waits until the state is below 3"
    let d = ibuilder_pipeline::testutil::TempDir::new("gate-rw");
    let path = d.join("gameapp.thermal");
    write(&path, "2 0\n");
    let gate4 = ThermalGate::new(&path, Arc::new(NullLog), FakeClock::new(None));
    assert_eq!(gate4.wait(3, "stage 2 rewrite"), 0.0);
    let states4 = ["3 0\n", "2 0\n"];
    write(&path, states4[0]);
    let p2 = path.clone();
    let c4 = FakeClock::new(Some(Box::new(move |n| write(&p2, states4[n.min(1)]))));
    let gate5 = ThermalGate::new(&path, Arc::new(NullLog), c4);
    assert!(gate5.wait(3, "stage 2 rewrite") > 0.0 && gate5.read().0 == 2);
}

// ---- the memory gate asks the thermal gate ----------------------------------------------------------------------------------------------------------------------------

fn run_jobs(thermal: Arc<ThermalGate>, n: usize, hold: Duration) -> usize {
    let gate = Arc::new(ShaderMemoryGate::new(
        Arc::new(FakeMem::new(0)),
        Arc::new(NullLog),
        Some(thermal),
        RealClock::new(),
    ));
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let hs: Vec<_> = (0..n)
        .map(|_| {
            let (g, l, p) = (gate.clone(), live.clone(), peak.clone());
            std::thread::spawn(move || {
                g.run(|| {
                    let now = l.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(hold);
                    l.fetch_sub(1, Ordering::SeqCst);
                })
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    peak.load(Ordering::SeqCst)
}

#[test]
fn the_memory_gate_asks_the_thermal_gate() {
    // py: "nothing hot: all six run together", "Low performance mode: no more than 2 at once, all still run", "serious heat: no new job starts", "it starts once the phone is back to fair"
    let d = ibuilder_pipeline::testutil::TempDir::new("memgate");
    let path = d.join("gameapp.thermal");
    let mk = |poll: f64| {
        let t = Arc::new(ThermalGate::new(&path, Arc::new(NullLog), RealClock::new()));
        t.set_poll(poll);
        t
    };
    write(&path, "0 0\n");
    assert_eq!(run_jobs(mk(3.0), 6, Duration::from_millis(150)), 6);
    write(&path, "0 1\n");
    assert_eq!(run_jobs(mk(3.0), 6, Duration::from_millis(150)), 2);
    write(&path, "2 0\n");
    let th = mk(0.05);
    let gate = Arc::new(ShaderMemoryGate::new(
        Arc::new(FakeMem::new(0)),
        Arc::new(NullLog),
        Some(th),
        RealClock::new(),
    ));
    let started = Arc::new(AtomicUsize::new(0));
    let (g, s) = (gate.clone(), started.clone());
    let t = std::thread::spawn(move || g.run(|| s.fetch_add(1, Ordering::SeqCst)));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        started.load(Ordering::SeqCst),
        0,
        "serious heat: no new job starts"
    );
    write(&path, "1 0\n");
    t.join().unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 1);
}

// ---- the work folder: same inputs resume, others start again ---------------------------------------------------------------------------------------------------------

#[test]
fn the_work_folder_same_inputs_resume_others_start_again() {
    // py: "no folder: a fresh start", "a split that was not finished has no result", "the same inputs: the folder is kept", "a half-written job file is gone", "other inputs: the folder is emptied",
    //     "a folder from before resume existed is emptied"
    let fx = Fx::new();
    let work = fx.p("shaders/T.work");
    let mut r = ShaderResume::new(&work, "sig-a");
    assert!(!r.open() && r.jobs_dir().is_dir());
    write(work.join("pkg/ps_a.bin"), "x");
    for t in ["shader_map.tsv", "shader_facts.tsv"] {
        write(work.join("tables").join(t), "x");
    }
    assert_eq!(
        r.split_done(),
        None,
        "a split that was not finished has no result"
    );
    r.mark_split(120);
    write(work.join("splitjobs/half.pkl.tmp-1-1"), "x");
    let mut r2 = ShaderResume::new(&work, "sig-a");
    assert!(r2.open() && r2.split_done() == Some(120));
    assert!(!work.join("splitjobs/half.pkl.tmp-1-1").exists());
    let mut r3 = ShaderResume::new(&work, "sig-b");
    assert!(!r3.open() && !work.join("pkg").exists() && r3.split_done().is_none());
    fs::remove_file(work.join("resume.json")).unwrap();
    write(work.join("pkg/leftover"), "x");
    assert!(!ShaderResume::new(&work, "sig-a").open() && !work.join("pkg").exists());
}

#[test]
fn the_signature_follows_the_inputs_not_their_times() {
    // py: "the signature ignores a file's time", "and changes with the DLC", "with the profile table", "with the translation chain", "with the game's files"
    let fx = Fx::new();
    let (a, b, d, m) = (
        fx.p("RomFS.img"),
        fx.p("main"),
        fx.p("derived"),
        fx.p("aoc.img"),
    );
    write(&a, "rom".repeat(100000));
    write(&b, "main");
    write(d.join("game.tsv"), "title_id\tT\n");
    write(&m, "dlc1");
    let sig = |chain: &str| {
        shader_work_signature(
            "split1",
            "T",
            chain,
            std::slice::from_ref(&m),
            &a,
            &b,
            Some(&d),
        )
    };
    let s1 = sig("chain");
    set_mtime(&a, 1);
    assert_eq!(
        sig("chain"),
        s1,
        "a new extraction of the same bytes still resumes"
    );
    write(&m, "dlc2");
    assert_ne!(sig("chain"), s1, "and changes with the DLC");
    write(&m, "dlc1");
    write(d.join("game.tsv"), "title_id\tT\nx\t1\n");
    assert_ne!(sig("chain"), s1, "with the profile table");
    write(d.join("game.tsv"), "title_id\tT\n");
    assert_ne!(sig("other"), s1, "with the translation chain");
    write(&a, format!("{}more", "rom".repeat(100000)));
    assert_ne!(sig("chain"), s1, "with the game's files");
    assert_eq!(s1.len(), 24);
}

#[test]
fn the_fingerprint_covers_the_first_and_last_four_mib_of_a_big_file() {
    let fx = Fx::new();
    let p = fx.p("big.img");
    let mut data = vec![7u8; 20 << 20];
    write(&p, &data);
    let f1 = file_fingerprint(&p);
    data[10 << 20] = 9; // in the middle: not read, so the same fingerprint (a RomFS.img is many GB)
    write(&p, &data);
    assert_eq!(file_fingerprint(&p), f1);
    data[(20 << 20) - 1] = 1; // in the last 4 MiB
    write(&p, &data);
    assert_ne!(file_fingerprint(&p), f1);
    assert_eq!(file_fingerprint(&fx.p("missing")), "-");
}

// ---- build_shaders_on_device resumes ------------------------------------------------------------------------------------------------------------------------------------------

/// The Python test's STUB `shader_device`: counts calls; `kill` makes the build die (the process is gone).
#[derive(Default)]
struct Stub {
    splits: AtomicUsize,
    builds: AtomicUsize,
    cas: Mutex<Vec<PathBuf>>,
    jobs: Mutex<Vec<usize>>,
    kill: bool,
}
impl ShaderBackend for Stub {
    fn split_version(&self) -> String {
        "stubsplit".into()
    }
    fn split(
        &self,
        req: &SplitRequest<'_>,
        _log: &dyn ibuilder_pipeline::log::Log,
    ) -> ibuilder_pipeline::Result<SplitResult> {
        self.splits.fetch_add(1, Ordering::SeqCst);
        self.jobs.lock().unwrap().push(req.jobs);
        fs::create_dir_all(req.pkg_dir)?;
        fs::create_dir_all(req.tables_dir)?;
        fs::write(req.pkg_dir.join("ps_a.bin"), b"x")?;
        for t in [
            "shader_map.tsv",
            "shader_facts.tsv",
            "shader_pairs.tsv",
            "pso_pairs.tsv",
        ] {
            fs::write(req.tables_dir.join(t), "#\n")?;
        }
        Ok(SplitResult {
            shaders: 42,
            jobs_reused: 0,
            jobs_ran: 1,
        })
    }
    fn build(
        &self,
        req: &BuildRequest<'_>,
        _p: &mut dyn FnMut(&str),
    ) -> ibuilder_pipeline::Result<PathBuf> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        self.cas.lock().unwrap().push(req.cas.to_path_buf());
        if self.kill {
            panic!("the process was killed");
        }
        fs::create_dir_all(req.out_dir.join("_work/profile").join(req.game_id))?;
        let img = req.out_dir.join(format!("b-{}.img", req.chain));
        fs::write(&img, b"image")?;
        Ok(img)
    }
    fn live_translate(
        &self,
        _r: &LiveRequest<'_>,
        _l: &dyn ibuilder_pipeline::log::Log,
    ) -> ibuilder_pipeline::Result<LiveResult> {
        Ok(LiveResult::default())
    }
}

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

fn with_stub(fx: &mut Fx, kill: bool) -> Arc<Stub> {
    let s = Arc::new(Stub {
        kill,
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(s.clone());
    s
}

#[test]
fn stage_3_ended_by_a_kill_resumes_without_reading_the_shaders_again() {
    // py: "stage 3 ended by a kill leaves its work folder, with the finished split", "and no library", "the next build does not read the game's shaders again", "it translates on 1 thread",
    //     "and builds the library", "the finished work folder is removed", "the translation cache is kept for a resume", "its numbers are recorded for the timings"
    let mut fx = stage3_fixture();
    let killed = with_stub(&mut fx, true);
    let died = catch_unwind(AssertUnwindSafe(|| build_shaders_on_device(&mut fx.ctx))).is_err();
    let work = fx.p(&format!("shaders/{STID}.work"));
    assert!(
        died && work.is_dir() && work.join("split.ok").is_file(),
        "died={died}"
    );
    assert!(!fx.p(&format!("shaders/{STID}")).is_dir(), "no library");
    assert_eq!(killed.splits.load(Ordering::SeqCst), 1);
    // the next run (a new process: the stub is new, its counters start at zero)
    fx.log.clear();
    let stub = with_stub(&mut fx, false);
    let pack = build_shaders_on_device(&mut fx.ctx).expect("a library");
    assert_eq!(
        stub.splits.load(Ordering::SeqCst),
        0,
        "{:?}",
        fx.log.lines()
    );
    assert!(fx.log.contains("not read again"));
    assert!(
        fx.log.contains("on 1 thread"),
        "the unfinished build's safety"
    );
    assert!(
        pack.join(format!("b-{CHAIN}.img")).is_file() && stub.builds.load(Ordering::SeqCst) == 1
    );
    assert!(!work.exists(), "the finished work folder is removed");
    assert_eq!(
        stub.cas.lock().unwrap()[0],
        fx.p("shadercache"),
        "the shared cache is the resume's cache"
    );
    assert!(
        fx.ctx.stats.get_int("shaders") == Some(42)
            && fx.ctx.stats.contains("split_s")
            && fx.ctx.stats.contains("translate_s")
            && fx.ctx.stats.get_int("split_reused") == Some(1),
        "{:?}",
        fx.ctx.stats
    );
    let pt = fs::read_to_string(pack.join("pack.txt")).unwrap();
    assert!(
        pt.contains(&format!(
            "game\t{STID}\nchain\t{CHAIN}\nmade\ton this device, "
        )) && pt.contains("shaders\t42\nsplit\tstubsplit\ndlc\t-\n"),
        "{pt}"
    );

    // py: "a changed game is not resumed: its shaders are read again"
    fs::remove_dir_all(&pack).unwrap();
    with_stub(&mut fx, true);
    let _ = catch_unwind(AssertUnwindSafe(|| build_shaders_on_device(&mut fx.ctx)));
    write(fx.p("extract/RomFS.img"), "a different game dump");
    fx.log.clear();
    let stub = with_stub(&mut fx, false);
    build_shaders_on_device(&mut fx.ctx);
    assert_eq!(
        stub.splits.load(Ordering::SeqCst),
        1,
        "{:?}",
        fx.log.lines()
    );
    assert!(fx.log.contains("is dropped"));

    // py: "shader_cache=0: the cache still lives in the work folder (so a kill is resumable)"
    let _ = fs::remove_dir_all(fx.p(&format!("shaders/{STID}")));
    fx.set_cfg("shader_cache", "0");
    let stub = with_stub(&mut fx, false);
    build_shaders_on_device(&mut fx.ctx);
    assert!(
        stub.cas
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .ends_with(format!("{STID}.work/cas")),
        "{:?}",
        stub.cas.lock().unwrap()
    );
    fx.ctx.cfg.remove("shader_cache");

    // py: "Low performance mode: the split reads with 2 jobs at most"
    let _ = fs::remove_dir_all(fx.p(&format!("shaders/{STID}")));
    write(fx.p("gameapp.thermal"), "0 1\n");
    fx.ctx.thermal.invalidate();
    let stub = with_stub(&mut fx, false);
    build_shaders_on_device(&mut fx.ctx);
    assert!(
        stub.jobs.lock().unwrap().iter().all(|j| *j <= 2),
        "{:?}",
        stub.jobs.lock().unwrap()
    );
}

#[test]
fn a_failing_translation_removes_its_work_folder() {
    // trap 74: "A split or translation that FAILS (an exception) still removes its folder, as before: only an end without a trace is resumed"
    let mut fx = stage3_fixture();
    let s = Arc::new(ibuilder_pipeline::mock::MockShaders {
        shaders: 3,
        fail_build: true,
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(s);
    assert!(build_shaders_on_device(&mut fx.ctx).is_none());
    assert!(fx.log.contains("[RED] the translation failed"));
    assert!(!fx.p(&format!("shaders/{STID}.work")).exists());
}

#[test]
fn a_game_with_no_shader_the_split_reads_is_marked_and_not_tried_again() {
    let mut fx = stage3_fixture();
    let s = Arc::new(ibuilder_pipeline::mock::MockShaders {
        shaders: 0,
        ..Default::default()
    });
    fx.ctx.adapters.shaders = Some(s.clone());
    assert!(build_shaders_on_device(&mut fx.ctx).is_none());
    assert!(fx.p(&format!("shaders/{STID}.none")).is_file());
    assert_eq!(s.splits.load(Ordering::SeqCst), 1);
    fx.log.clear();
    assert!(build_shaders_on_device(&mut fx.ctx).is_none());
    assert_eq!(s.splits.load(Ordering::SeqCst), 1, "not tried again");
    assert!(fx.log.contains("not tried again"));
    // a DLC installed later makes it worth a new try
    write(fx.p(&format!("games/{STID}/aoc/1.img")), "dlc");
    fx.log.clear();
    build_shaders_on_device(&mut fx.ctx);
    assert!(
        fx.log.contains("held no shader without its DLC") && s.splits.load(Ordering::SeqCst) == 2
    );
}

// ---- the kill, said; a build that continues another --------------------------------------------------------------------------------------------------------------------

#[test]
fn the_kill_is_said_and_a_build_can_continue_another() {
    // py: "no note: nothing said", "the killed build's last stage and last memory line go into the new log", "and the note is taken away", "a build the app continues says what it continues",
    //     "and the request is used once"
    let fx = Fx::new();
    assert!(note_killed_run(&fx.ctx).is_empty() && fx.log.lines().is_empty());
    write(
        fx.p("gameapp.killed"),
        "game=Grizzy.nsp\nstage=stage 3/5: shaders: translating for this iPhone (938/21990 shaders)\nmem=  [mem:shaders 938/21990] resident 2926 MiB\n",
    );
    let note = note_killed_run(&fx.ctx);
    assert_eq!(note.get("game").map(String::as_str), Some("Grizzy.nsp"));
    assert!(
        fx.log.contains("938/21990")
            && fx.log.contains("[mem:shaders 938/21990]")
            && fx.log.contains("ended by iOS"),
        "{:?}",
        fx.log.lines()
    );
    assert!(!fx.p("gameapp.killed").exists(), "it is said once");
    write(
        fx.p("gameapp.continue.run"),
        "game=Grizzy.nsp\nstage=3\nwhy=killed\n",
    );
    fx.log.clear();
    let cont = take_continue(&fx.ctx);
    assert_eq!(cont.get("stage").map(String::as_str), Some("3"));
    assert!(fx.log.contains("continues one that stopped at stage 3"));
    assert!(!fx.p("gameapp.continue.run").exists() && take_continue(&fx.ctx).is_empty());
}

// ---- the restart between the rewrite and the shaders --------------------------------------------------------------------------------------------------------------------

#[test]
fn the_chain_restarts_before_stage_3_only_when_stage_3_would_not_fit() {
    // py: the restart block
    let mut fx = stage3_fixture();
    fx.set_cfg("game", "Grizzy.nsp");
    fx.set_cfg("app_build", "b1");
    fx.set_cfg("modules", "main");
    let cache_rewrite = |fx: &Fx| {
        let key = rewrite_key(&fx.ctx, &fx.p("extract/ExeFS/main"), "")
            .unwrap()
            .unwrap();
        let c = rewrite_cache_dir(&fx.ctx, "main").unwrap();
        write(c.join("key"), format!("{key}\n"));
        write(c.join("files/main.patched.bin"), "x");
    };
    assert!(
        !rewrite_is_cached(&fx.ctx),
        "no rewrite cache: not 'cached'"
    );
    cache_rewrite(&fx);
    assert!(
        rewrite_is_cached(&fx.ctx),
        "the rewrite cache for this build and these inputs counts"
    );
    fx.set_cfg("app_build", "b2");
    assert!(!rewrite_is_cached(&fx.ctx), "another app build does not");
    fx.set_cfg("app_build", "b1");
    assert!(
        shaders_will_build(&fx.ctx),
        "the game still needs its shaders"
    );

    fx.mem.set_available(600 * MIB);
    assert!(
        restart_for_shaders(&fx.ctx, true, 3200 * MIB),
        "{:?}",
        fx.log.lines()
    );
    assert!(
        fx.p("gameapp.continue").is_file(),
        "600 MiB left after the rewrite, a new launch has 3200: the chain asks for a restart"
    );
    let req = ibuilder_pipeline::timings::read_kv(&fx.p("gameapp.continue"));
    assert!(
        req.get("game").map(String::as_str) == Some("Grizzy.nsp")
            && req.get("stage").map(String::as_str) == Some("3"),
        "{req:?}"
    );
    fs::remove_file(fx.p("gameapp.continue")).unwrap();
    assert!(
        !restart_for_shaders(&fx.ctx, false, 3200 * MIB),
        "only when the rewrite ran in this process (a continued run took it from the cache)"
    );
    fx.mem.set_available(3000 * MIB);
    fx.log.clear();
    assert!(
        !restart_for_shaders(&fx.ctx, true, 3200 * MIB)
            && !fx.p("gameapp.continue").exists()
            && fx.log.contains("no restart"),
        "3000 MiB left: stage 3 fits"
    );
    fx.mem.set_available(0);
    assert!(
        !restart_for_shaders(&fx.ctx, true, 0),
        "no ceiling (a simulator): no restart"
    );
    fx.mem.set_available(600 * MIB);
    fx.log.clear();
    assert!(
        !restart_for_shaders(&fx.ctx, true, 1000 * MIB) && fx.log.contains("would not have more"),
        "a new launch would not have more room either"
    );
    let mut c = fx.ctx.cfg.clone();
    c.set("rewrite_cache", "0");
    let saved = std::mem::replace(&mut fx.ctx.cfg, c);
    assert!(
        !restart_for_shaders(&fx.ctx, true, 3200 * MIB),
        "the rewrite not in the cache (a loop otherwise): no restart"
    );
    fx.ctx.cfg = saved;
    fx.set_cfg("shader_restart", "0");
    assert!(
        !restart_for_shaders(&fx.ctx, true, 3200 * MIB),
        "shader_restart=0 turns it off"
    );
    fx.ctx.cfg.remove("shader_restart");
    fx.set_cfg("game", "");
    assert!(
        !restart_for_shaders(&fx.ctx, true, 3200 * MIB),
        "no game name (a chain-only run): no restart, the app could not continue it"
    );
    fx.set_cfg("game", "Grizzy.nsp");
    let pack = fx.p(&format!("shaders/{STID}"));
    write(pack.join(format!("b-{CHAIN}.img")), "img");
    write(
        pack.join("shader_map.tsv"),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\np\t0\tv\tabc\ts\n",
    );
    assert!(
        !restart_for_shaders(&fx.ctx, true, 3200 * MIB) && !shaders_will_build(&fx.ctx),
        "the game already has its library: stage 3 builds nothing, so no restart"
    );
}

// ---- the timings ----------------------------------------------------------------------------------------------------------------------------------------------------------------------

#[test]
fn a_build_is_one_row_the_last_50_are_kept_and_the_summary_line_reads() {
    // py: "a build is one row under a header", "the game, the app build, RAM and EVA flags, and stage 3's split and translation", "one summary line at the end of the log", "the file keeps the last 50 builds"
    let fx = Fx::new();
    fx.mem.set_address_space(Some(AddressSpace {
        free: 1 << 40,
        largest: 1 << 40,
        extended: true,
        increased_memory: 1,
    }));
    let cfg = ibuilder_pipeline::config::Config::parse("app_build=build-77");
    let lines = MemLog::new();
    let (path, partial) = (fx.p("build_times.tsv"), fx.p("p.partial"));
    let mut t = Timings::new(&path, &partial, "iPhone16,1");
    t.game = STID.into();
    t.launch_available = 6000 * MIB;
    t.timed("profile", || {});
    t.secs.insert("rewrite".into(), 125.0);
    t.secs.insert("shaders".into(), 840.0);
    t.secs.insert("assemble".into(), 1.5);
    let mut stats = BuildStats::new();
    stats.set_float("split_s", 90.0);
    stats.set_float("translate_s", 750.0);
    stats.set_int("shaders", 21990);
    stats.set_int("read_jobs", 4);
    stats.set_int("jobs", 4);
    stats.set_int("peak_mib", 3400);
    stats.set_float("cooldown_s", 12.0);
    t.finish(&cfg, "ok", &stats, &*fx.mem, &*lines);
    let text: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert!(
        text.len() == 2
            && text[0].split('\t').take(3).collect::<Vec<_>>() == ["date", "game", "app_build"]
            && text[1].split('\t').count() == BUILD_TIMES_COLUMNS.len(),
        "{text:?}"
    );
    let d: std::collections::BTreeMap<&str, &str> = BUILD_TIMES_COLUMNS
        .iter()
        .copied()
        .zip(text[1].split('\t'))
        .collect();
    assert!(
        d["game"] == STID
            && d["app_build"] == "build-77"
            && d["ram_unlocked"] == "1"
            && d["eva"] == "1"
            && d["split_s"] == "90.0"
            && d["translate_s"] == "750.0"
            && d["shaders"] == "21990"
            && d["jobs"] == "4"
            && d["peak_mib"] == "3400"
            && d["rewrite_s"] == "125.0"
            && d["device"] == "iPhone16,1",
        "{d:?}"
    );
    let l = lines.lines();
    assert!(
        l.len() == 1
            && l[0].starts_with("build times:")
            && l[0].contains("reading 90.0s, translation 750.0s")
            && l[0].contains("21990 shaders")
            && l[0].contains("RAM unlocked"),
        "{l:?}"
    );
    for i in 0..60 {
        let mut t2 = Timings::new(&path, &partial, "iPhone16,1");
        t2.game = format!("G{i:02}");
        t2.finish(&cfg, "ok", &BuildStats::new(), &*fx.mem, &NullLog);
    }
    let rows: Vec<String> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert!(
        rows.len() == 51
            && rows.last().unwrap().split('\t').nth(1) == Some("G59")
            && rows[1].split('\t').nth(1) == Some("G10"),
        "{} rows",
        rows.len()
    );
}

#[test]
fn a_paused_build_writes_no_row_and_the_next_one_adds_to_it() {
    // py: "a paused build writes no row, only its partial", "the continuing build adds to it: one row for the whole build", "a partial of another game is dropped"
    let fx = Fx::new();
    let cfg = ibuilder_pipeline::config::Config::parse("app_build=build-77");
    let (b2, p2) = (fx.p("b2.tsv"), fx.p("p2.partial"));
    let mut p1 = Timings::new(&b2, &p2, "iPhone16,1");
    p1.game = "GAME".into();
    p1.secs.insert("profile".into(), 0.1);
    p1.secs.insert("rewrite".into(), 120.0);
    p1.finish(
        &cfg,
        "paused: restart",
        &BuildStats::new(),
        &*fx.mem,
        &NullLog,
    );
    assert!(!b2.exists() && p2.is_file());
    let mut t2 = Timings::new(&b2, &p2, "iPhone16,1");
    t2.game = "GAME".into();
    t2.merge_partial("GAME", true);
    t2.secs.insert("shaders".into(), 300.0);
    t2.finish(&cfg, "ok", &BuildStats::new(), &*fx.mem, &NullLog);
    let line = fs::read_to_string(&b2)
        .unwrap()
        .lines()
        .nth(1)
        .unwrap()
        .to_string();
    let d: std::collections::BTreeMap<&str, &str> = BUILD_TIMES_COLUMNS
        .iter()
        .copied()
        .zip(line.split('\t'))
        .collect();
    assert!(
        d["rewrite_s"] == "120.0"
            && d["shaders_s"] == "300.0"
            && d["resumed"] == "1"
            && d["total_s"] == "420.1",
        "{d:?}"
    );
    let (b3, p3) = (fx.p("b3.tsv"), fx.p("p3.partial"));
    let mut t3 = Timings::new(&b3, &p3, "x");
    write(&p3, "game=OTHER\nrewrite_s=99.0\n");
    t3.merge_partial("GAME", true);
    assert!(!t3.secs.contains_key("rewrite") && !p3.exists());
}

#[test]
fn the_device_model_of_a_row() {
    // py: "the simulator names the model it imitates", "a device model is never empty"
    let env = ibuilder_pipeline::env::MapEnv::with(&[("SIMULATOR_MODEL_IDENTIFIER", "iPhone18,1")]);
    assert_eq!(device_model(&env), "iPhone18,1 (simulator)");
    assert!(!device_model(&ibuilder_pipeline::env::MapEnv::new()).is_empty());
}
