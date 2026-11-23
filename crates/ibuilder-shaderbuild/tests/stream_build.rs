//! The build's per-shader stream (speed-up B4): a paired vertex shader is finished once its own pixel-shader partners are, never before them and without waiting for every shader
//! without pairs; how many jobs a translator call holds (`BuildOptions::xlate_group`) and how many workers there are change nothing in the library; a partner that fails does not hold
//! its vertex shaders forever; a pause in the middle of the pairs resumes to the library an uninterrupted build writes. A synthetic game, no engine.
//!
//! The process-wide stop request is one byte of the process: every test takes `serial()`.
mod synth;
use ibuilder_pipeline::stop;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use ibuilder_shaderbuild::finish::{Finished, Finisher};
use ibuilder_shaderbuild::tools::FnTools;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, BuildReport, build};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use synth::*;

/// shaders 0..PLAIN have no pairs; PLAIN..TOTAL are vertex shaders paired with two of them each
const PLAIN: usize = 160;
const TOTAL: usize = 200;

fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let g = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    stop::clear();
    g
}

/// vertex shader -> its two pixel shaders (all among the plain ones, never shader `SLOW`)
fn pairs() -> HashMap<String, Vec<String>> {
    (PLAIN..TOTAL)
        .map(|v| {
            (
                key_of(v),
                vec![key_of(1 + (v * 7) % 100), key_of(1 + (v * 13 + 5) % 100)],
            )
        })
        .collect()
}

/// a plain shader that is nobody's partner
const SLOW: usize = PLAIN - 1;

fn write_paired_game(root: &Path) {
    write_game(root, "G", TOTAL);
    let mut t = String::new();
    let mut rows: Vec<(String, String)> = pairs()
        .into_iter()
        .flat_map(|(v, ps)| ps.into_iter().map(move |p| (v.clone(), p)))
        .collect();
    rows.sort();
    for (v, p) in rows {
        t.push_str(&format!("{v} {p}\n"));
    }
    fs::write(root.join("profile").join("G").join("shader_pairs.tsv"), t).unwrap();
}

fn options(root: &Path, out: &str, cas: &Path, jobs: usize, group: usize) -> BuildOptions {
    let mut o = BuildOptions::new(
        &root.join("pkg"),
        &root.join("profile"),
        "G",
        &root.join(out),
        CHAIN,
        cas,
    );
    o.jobs = jobs;
    o.xlate_group = group;
    o.tables = false;
    o.pulling = false;
    o
}

/// the translator's stand-in: a stub `.spv` / `.res.json` for each job, the `[RED]` line for the keys in `refuse`
fn translator(refuse: BTreeSet<String>) -> FnTools {
    let xlate = move |argv: &[String]| -> ToolRun {
        let jobs: Value = serde_json::from_str(&fs::read_to_string(&argv[0]).unwrap()).unwrap();
        let mut err = String::new();
        for j in jobs.as_array().unwrap() {
            let k = j["key"].as_str().unwrap();
            if refuse.contains(k) {
                err.push_str(&format!("[RED] {k}: refused\n"));
            } else {
                fs::write(Path::new(&argv[1]).join(format!("{k}.spv")), b"spv").unwrap();
                fs::write(Path::new(&argv[1]).join(format!("{k}.res.json")), b"{}").unwrap();
            }
        }
        ToolRun {
            rc: i32::from(!err.is_empty()),
            stdout: String::new(),
            stderr: err,
        }
    };
    FnTools::new(
        Arc::new(xlate),
        Arc::new(|a: &[String]| {
            if a == ["--version"] {
                ToolRun {
                    rc: 0,
                    stdout: "stand-in 1\n".into(),
                    stderr: String::new(),
                }
            } else {
                ToolRun::ok()
            }
        }),
    )
}

/// the `user(locnN)` numbers a plain shader's "MSL" declares: a function of its key
fn locs_of(key: &str) -> BTreeSet<u32> {
    let n = u64::from_str_radix(key, 16).unwrap();
    [(n % 31) as u32, (n % 17) as u32 + 40]
        .into_iter()
        .collect()
}

/// A finisher that writes, into a paired vertex shader's file, the locations its partners' finished MSL declared (what `vs_pad` reads): a library built with a partner not yet finished
/// differs. It also notes such a shader (`early`), when each shader ended, and it can be slow for one key and raise a stop at its n-th call.
struct PairFin {
    pairs: HashMap<String, Vec<String>>,
    /// partners that fail on purpose: their vertex shaders see no locations, rightly
    failing: BTreeSet<String>,
    slow: Option<(String, Duration)>,
    raise_at: Option<usize>,
    started: AtomicUsize,
    early: Mutex<Vec<String>>,
    ended: Mutex<HashMap<String, Instant>>,
}

impl PairFin {
    fn new() -> PairFin {
        PairFin {
            pairs: pairs(),
            failing: BTreeSet::new(),
            slow: None,
            raise_at: None,
            started: AtomicUsize::new(0),
            early: Mutex::new(Vec::new()),
            ended: Mutex::new(HashMap::new()),
        }
    }
}

impl Finisher for PairFin {
    fn fingerprint(&self) -> String {
        "stream-test-finisher-1".into()
    }
    fn finish(
        &self,
        key: &str,
        _work: &Path,
        _job: &mut Value,
        _c1: &[u8],
        _tools: &dyn ShaderTools,
        paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        if Some(n) == self.raise_at {
            stop::request(stop::PAUSE);
        }
        if let Some((k, d)) = &self.slow
            && k == key
        {
            std::thread::sleep(*d);
        }
        let mut text = body(key);
        let mut ps_locs = BTreeSet::new();
        match self.pairs.get(key) {
            Some(ps) => {
                for p in ps {
                    match paired(p) {
                        Some(l) => text.extend(format!("// {p}: {l:?}\n").bytes()),
                        None if self.failing.contains(p) => {
                            text.extend(format!("// {p}: none\n").bytes())
                        }
                        None => self.early.lock().unwrap().push(format!("{key} before {p}")),
                    }
                }
            }
            None => ps_locs = locs_of(key),
        }
        self.ended
            .lock()
            .unwrap()
            .insert(key.to_string(), Instant::now());
        Finished {
            status: "ok".into(),
            detail: String::new(),
            refl: None,
            outputs: vec![
                (format!("{key}_b.metal"), text),
                (format!("{key}_b.json"), b"{}\n".to_vec()),
            ],
            ps_locs,
            log: Vec::new(),
        }
    }
}

fn run(o: &BuildOptions, fin: &PairFin, tools: &FnTools) -> Result<BuildReport, String> {
    let log = |_: &str| {};
    build(
        o,
        &BuildEnv {
            tools,
            finisher: fin,
            log: &log,
            cancel: None,
        },
    )
}

/// Workers and jobs per translator call change nothing in the library; every paired vertex shader saw its partners finished; one worker with groups of `g` makes `ceil(n / g)` calls.
#[test]
fn workers_and_group_size_change_nothing_and_pairs_see_their_partners() {
    let _g = serial();
    let root = scratch("stream-same");
    write_paired_game(&root);
    let tools = translator(BTreeSet::new());
    let mut images = Vec::new();
    for (i, (jobs, group)) in [(1, 1), (1, 7), (3, 64), (6, 4)].into_iter().enumerate() {
        let fin = PairFin::new();
        let cas = root.join(format!("cas{i}"));
        let r = run(
            &options(&root, &format!("out{i}"), &cas, jobs, group),
            &fin,
            &tools,
        )
        .unwrap();
        assert_eq!((r.shaders, r.ok, r.cache_miss), (TOTAL, TOTAL, TOTAL));
        assert!(
            fin.early.lock().unwrap().is_empty(),
            "jobs {jobs}, group {group}: finished before a partner: {:?}",
            fin.early.lock().unwrap()
        );
        if jobs == 1 {
            assert_eq!(
                r.translate_calls as usize,
                TOTAL.div_ceil(group),
                "jobs {jobs}, group {group}"
            );
        }
        eprintln!(
            "jobs {jobs}, group {group}: wall {:.2} s, {} translator calls",
            r.wall_s, r.translate_calls
        );
        images.push(fs::read(&r.image).unwrap());
    }
    for (i, img) in images.iter().enumerate().skip(1) {
        assert!(img == &images[0], "library {i} differs from library 0");
    }
    let _ = fs::remove_dir_all(&root);
}

/// No barrier: while one plain shader that nobody pairs with takes long, every paired vertex shader is finished (the old build waited for all of phase 1 first).
#[test]
fn paired_vertex_shaders_do_not_wait_for_unrelated_shaders() {
    let _g = serial();
    let root = scratch("stream-nobarrier");
    write_paired_game(&root);
    let slow_key = key_of(SLOW);
    assert!(pairs().values().flatten().all(|p| *p != slow_key));
    let mut fin = PairFin::new();
    fin.slow = Some((slow_key.clone(), Duration::from_millis(1500)));
    let tools = translator(BTreeSet::new());
    let r = run(
        &options(&root, "out", &root.join("cas"), 4, 4),
        &fin,
        &tools,
    )
    .unwrap();
    assert_eq!((r.shaders, r.ok), (TOTAL, TOTAL));
    assert!(fin.early.lock().unwrap().is_empty());
    let ended = fin.ended.lock().unwrap();
    let slow_end = ended[&slow_key];
    let late = (PLAIN..TOTAL)
        .filter(|&v| ended[&key_of(v)] > slow_end)
        .count();
    assert_eq!(
        late, 0,
        "{late} paired vertex shaders ended after the slow plain shader"
    );
    let _ = fs::remove_dir_all(&root);
}

/// A partner the translator refuses is done all the same (failed): its vertex shaders are built without its locations and the build ends.
#[test]
fn a_failed_partner_releases_its_vertex_shaders() {
    let _g = serial();
    let root = scratch("stream-failed");
    write_paired_game(&root);
    let refused: BTreeSet<String> = [key_of(1 + (PLAIN * 7) % 100)].into_iter().collect();
    let mut fin = PairFin::new();
    fin.failing = refused.clone();
    let tools = translator(refused);
    let r = std::thread::scope(|s| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (fin, tools, root) = (&fin, &tools, &root);
        s.spawn(move || {
            let _ = tx.send(run(
                &options(root, "out", &root.join("cas"), 6, 4),
                fin,
                tools,
            ));
        });
        rx.recv_timeout(Duration::from_secs(60))
            .expect("the build did not end: a vertex shader waits for a failed partner")
    })
    .unwrap();
    assert_eq!((r.shaders, r.ok), (TOTAL, TOTAL - 1));
    assert_eq!(r.failed.get("translation failed"), Some(&1));
    assert!(fin.early.lock().unwrap().is_empty());
    let _ = fs::remove_dir_all(&root);
}

/// A pause while the pairs are being built: the build after it restores the finished shaders (a hit pixel shader gives its vertex shaders its locations from the cache) and writes the
/// library an uninterrupted build writes, byte for byte.
#[test]
fn a_pause_in_the_middle_of_the_pairs_resumes_to_the_same_library() {
    let _g = serial();
    let root = scratch("stream-pause");
    write_paired_game(&root);
    let tools = translator(BTreeSet::new());
    let cas = root.join("cas");
    let mut fin1 = PairFin::new();
    fin1.raise_at = Some(100);
    assert_eq!(
        run(&options(&root, "out1", &cas, 6, 4), &fin1, &tools)
            .err()
            .as_deref(),
        Some("cancelled")
    );
    let done = fin1.started.load(Ordering::SeqCst);
    assert!((100..=106).contains(&done), "{done} shaders were started");
    assert!(stray_files(&root).is_empty(), "{:?}", stray_files(&root));
    stop::clear();
    let fin2 = PairFin::new();
    let r2 = run(&options(&root, "out2", &cas, 6, 4), &fin2, &tools).unwrap();
    assert_eq!((r2.shaders, r2.ok, r2.cache_hit), (TOTAL, TOTAL, done));
    assert!(fin2.early.lock().unwrap().is_empty());
    let fin3 = PairFin::new();
    let r3 = run(
        &options(&root, "out3", &root.join("cas-clean"), 1, 1),
        &fin3,
        &tools,
    )
    .unwrap();
    assert!(
        fs::read(&r2.image).unwrap() == fs::read(&r3.image).unwrap(),
        "the library after a pause is not the one an uninterrupted build writes"
    );
    let _ = fs::remove_dir_all(&root);
}
