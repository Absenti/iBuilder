//! The build driver's own behaviour (everything but the finish port): over a Grizzy sample (`keys-s300`: 291 keys + their paired pixel shaders = 401 shaders) with a stand-in translator
//! and a finisher that replays the Python's library files, so what is checked is the driver: digests, the cache, phases, pairs, failures, partial builds, cancel, the image layout.
//! (The library image must equal the Python build's byte for byte: `s300-j1`.)
mod common;
use common::*;
use ibuilder_shader::romfs::Image;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use ibuilder_shaderbuild::finish::{Finished, Finisher, ReplayFinisher};
use ibuilder_shaderbuild::game::{Game, load_pairs};
use ibuilder_shaderbuild::tools::FnTools;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, BuildReport, build};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Fix {
    image_bytes: Vec<u8>,
    keys: Vec<String>,
    work: PathBuf,
}

fn fixture(name: &str) -> Option<Fix> {
    let img = old("shader/runs/s300-j1/b-b24b25240cad.img");
    if !have(&img) || !have(&pkg()) || !have(&profile()) {
        return None;
    }
    let keys = keys_file("keys-s300.txt")?;
    let work = std::env::temp_dir().join(format!("ibsb-driver-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    Some(Fix {
        image_bytes: std::fs::read(&img).unwrap(),
        keys,
        work,
    })
}

/// a translator that writes a stub `.spv` / `.res.json` for each job, and the `[RED]` line for the keys in `refuse`
fn fake_tools(refuse: Vec<String>, calls: Arc<AtomicUsize>) -> FnTools {
    let xlate = move |argv: &[String]| -> ToolRun {
        calls.fetch_add(1, Ordering::SeqCst);
        let jobs: Value =
            serde_json::from_str(&std::fs::read_to_string(&argv[0]).unwrap()).unwrap();
        let mut err = String::new();
        for j in jobs.as_array().unwrap() {
            let k = j["key"].as_str().unwrap();
            if refuse.iter().any(|r| r == k) {
                err.push_str(&format!("[RED] {k}: the translator refused this one\n"));
            } else {
                std::fs::write(Path::new(&argv[1]).join(format!("{k}.spv")), b"spv").unwrap();
                std::fs::write(Path::new(&argv[1]).join(format!("{k}.res.json")), b"{}").unwrap();
            }
        }
        ToolRun {
            rc: 0,
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

fn options(f: &Fix, out: &str, cas: &Path, keys: Option<Vec<String>>) -> BuildOptions {
    let mut o = BuildOptions::new(&pkg(), &profile(), GAME, &f.work.join(out), CHAIN, cas);
    o.keys = keys;
    o.jobs = 3;
    o.tables = false;
    o.pulling = false;
    o
}

fn run(
    o: &BuildOptions,
    tools: &dyn ShaderTools,
    fin: &dyn Finisher,
) -> Result<BuildReport, String> {
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

fn files_of(p: &Path) -> HashMap<String, Vec<u8>> {
    let b = std::fs::read(p).unwrap();
    let img = Image::open(&b).unwrap();
    img.files
        .iter()
        .map(|f| (f.name.clone(), img.bytes(f).to_vec()))
        .collect()
}

#[test]
fn a_partial_build_is_the_pythons_image_and_the_second_run_is_all_hits() {
    let Some(f) = fixture("hits") else { return };
    let cas = f.work.join("cas");
    let calls = Arc::new(AtomicUsize::new(0));
    let tools = fake_tools(vec![], calls.clone());
    let fin = ReplayFinisher::new(&f.image_bytes).unwrap();
    let r1 = run(
        &options(&f, "one", &cas, Some(f.keys.clone())),
        &tools,
        &fin,
    )
    .unwrap();
    assert_eq!(
        (r1.shaders, r1.ok, r1.cache_hit, r1.cache_miss),
        (401, 401, 0, 401)
    );
    assert_eq!(
        std::fs::read(&r1.image).unwrap(),
        f.image_bytes,
        "the library image of the build is not the Python's"
    );
    let first_calls = calls.load(Ordering::SeqCst);
    assert!(first_calls > 0);
    // the same build again: every shader comes from the cache, no translator call, the same image
    let r2 = run(
        &options(&f, "two", &cas, Some(f.keys.clone())),
        &tools,
        &fin,
    )
    .unwrap();
    assert_eq!((r2.ok, r2.cache_hit, r2.cache_miss), (401, 401, 0));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        first_calls,
        "a build of cached shaders called the translator"
    );
    assert_eq!(std::fs::read(&r2.image).unwrap(), f.image_bytes);
    // another chain never hits (and removes this one's entries: they can never match)
    let mut o3 = options(&f, "three", &cas, Some(f.keys.clone()));
    o3.chain = "ffffffffffff".into();
    let r3 = run(&o3, &tools, &fin).unwrap();
    assert_eq!((r3.cache_hit, r3.cache_miss), (0, 401));
    assert!(!cas.join(format!("cas-{CHAIN}.dmsb")).exists());
    // a changed profile row (a pulling switch changes outputs, so it is in every digest)
    let mut o4 = options(&f, "four", &cas, Some(f.keys.clone()));
    o4.chain = "ffffffffffff".into();
    o4.pulling = true;
    assert_eq!(run(&o4, &tools, &fin).unwrap().cache_hit, 0);
    let _ = std::fs::remove_dir_all(&f.work);
}

/// wraps the replay: records the phase order and the partner information a paired vertex shader is given
struct Phases<'a> {
    inner: ReplayFinisher<'a>,
    pairs: HashMap<String, BTreeSet<String>>,
    unpaired_total: usize,
    unpaired_done: AtomicUsize,
    paired_started_early: AtomicBool,
    missing_partner: AtomicUsize,
    partners_seen: AtomicUsize,
    order: Mutex<Vec<String>>,
}

impl Finisher for Phases<'_> {
    fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
    fn finish(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        self.order.lock().unwrap().push(key.to_string());
        if let Some(ps) = self.pairs.get(key) {
            if self.unpaired_done.load(Ordering::SeqCst) < self.unpaired_total {
                self.paired_started_early.store(true, Ordering::SeqCst);
            }
            for p in ps {
                match paired(p) {
                    Some(_) => self.partners_seen.fetch_add(1, Ordering::SeqCst),
                    None => self.missing_partner.fetch_add(1, Ordering::SeqCst),
                };
            }
        }
        let f = self.inner.finish(key, work, job, c1, tools, paired);
        if !self.pairs.contains_key(key) {
            self.unpaired_done.fetch_add(1, Ordering::SeqCst);
        }
        f
    }
}

#[test]
fn paired_vertex_shaders_go_last_and_see_their_pixel_shaders() {
    let Some(f) = fixture("phases") else { return };
    let game = Game::load(&profile().join(GAME)).unwrap();
    let pairs = load_pairs(&game, &pkg()).unwrap();
    let keys: Vec<String> = f.keys.clone();
    // the build's own key set: the keys and the pixel shaders paired with them
    let mut all: BTreeSet<String> = keys.iter().cloned().collect();
    for k in &keys {
        for p in pairs.get(k).into_iter().flatten() {
            all.insert(p.clone());
        }
    }
    let unpaired_total = all.iter().filter(|k| !pairs.contains_key(*k)).count();
    let paired_total = all.iter().filter(|k| pairs.contains_key(*k)).count();
    assert!(
        paired_total > 50 && unpaired_total > 50,
        "the sample must have both phases"
    );
    let ph = Phases {
        inner: ReplayFinisher::new(&f.image_bytes).unwrap(),
        pairs: pairs.clone().into_iter().collect(),
        unpaired_total,
        unpaired_done: AtomicUsize::new(0),
        paired_started_early: AtomicBool::new(false),
        missing_partner: AtomicUsize::new(0),
        partners_seen: AtomicUsize::new(0),
        order: Mutex::new(vec![]),
    };
    let tools = fake_tools(vec![], Arc::new(AtomicUsize::new(0)));
    let r = run(
        &options(&f, "phases", &f.work.join("cas"), Some(keys)),
        &tools,
        &ph,
    )
    .unwrap();
    assert_eq!(r.ok, 401);
    assert!(
        !ph.paired_started_early.load(Ordering::SeqCst),
        "a paired vertex shader started before every unpaired shader was finished"
    );
    assert_eq!(
        ph.missing_partner.load(Ordering::SeqCst),
        0,
        "a paired vertex shader was not given a built partner's locations"
    );
    assert!(ph.partners_seen.load(Ordering::SeqCst) > 0);
    eprintln!(
        "{unpaired_total} unpaired shaders first, then {paired_total} paired vertex shaders; {} partner lookups answered",
        ph.partners_seen.load(Ordering::SeqCst)
    );
    let _ = std::fs::remove_dir_all(&f.work);
}

/// fails the keys it is given
struct Failing<'a> {
    inner: ReplayFinisher<'a>,
    fail: Vec<String>,
    panic_on: Option<String>,
}
impl Finisher for Failing<'_> {
    fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
    fn finish(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        if self.panic_on.as_deref() == Some(key) {
            panic!("boom {key}");
        }
        if self.fail.iter().any(|k| k == key) {
            return Finished::failed("MSL failed", "spirv-cross said no");
        }
        self.inner.finish(key, work, job, c1, tools, paired)
    }
}

#[test]
fn failed_shaders_have_no_files_and_the_build_goes_on() {
    let Some(f) = fixture("fail") else { return };
    let img = Image::open(&f.image_bytes).unwrap();
    // one pixel shader that a paired vertex shader reads, one other shader, one the translator refuses
    let game = Game::load(&profile().join(GAME)).unwrap();
    let pairs = load_pairs(&game, &pkg()).unwrap();
    let in_sample: BTreeSet<String> = img
        .files
        .iter()
        .filter_map(|x| x.name.strip_suffix("_b.json").map(str::to_string))
        .collect();
    let (vs, ps) = pairs
        .iter()
        .find(|(v, p)| in_sample.contains(*v) && p.iter().any(|x| in_sample.contains(x)))
        .map(|(v, p)| {
            (
                v.clone(),
                p.iter().find(|x| in_sample.contains(*x)).unwrap().clone(),
            )
        })
        .unwrap();
    let other = in_sample
        .iter()
        .find(|k| **k != vs && **k != ps && !pairs.contains_key(*k))
        .unwrap()
        .clone();
    let refused = in_sample
        .iter()
        .find(|k| **k != vs && **k != ps && **k != other && !pairs.contains_key(*k))
        .unwrap()
        .clone();
    let tools = fake_tools(vec![refused.clone()], Arc::new(AtomicUsize::new(0)));
    let fin = Failing {
        inner: ReplayFinisher::new(&f.image_bytes).unwrap(),
        fail: vec![ps.clone(), other.clone()],
        panic_on: None,
    };
    let cas = f.work.join("cas");
    let r = run(
        &options(&f, "fail", &cas, Some(f.keys.clone())),
        &tools,
        &fin,
    )
    .unwrap();
    assert_eq!(r.shaders, 401);
    assert_eq!(r.ok, 398);
    assert_eq!(r.failed.get("MSL failed"), Some(&2));
    assert_eq!(r.failed.get("translation failed"), Some(&1));
    let t = r.failures.iter().find(|x| x.key == refused).unwrap();
    assert_eq!(t.detail, "the translator refused this one");
    let got = files_of(&r.image);
    for k in [&ps, &other, &refused] {
        assert!(
            !got.keys().any(|n| n.starts_with(k.as_str())),
            "a failed shader {k} left files in the library"
        );
    }
    assert!(got.contains_key(&format!("{vs}_b.json")));
    // the vertex shader was built without its failed pixel shader's locations: it is NOT cached (its padding is incomplete); the others are
    let tools2 = fake_tools(vec![], Arc::new(AtomicUsize::new(0)));
    let ok_fin = ReplayFinisher::new(&f.image_bytes).unwrap();
    let r2 = run(
        &options(&f, "again", &cas, Some(f.keys.clone())),
        &tools2,
        &ok_fin,
    )
    .unwrap();
    assert_eq!(r2.ok, 401);
    assert!(
        r2.cache_miss >= 4 && r2.cache_hit >= 390,
        "hit {} miss {}",
        r2.cache_hit,
        r2.cache_miss
    ); // ps, other, refused, vs (and any other vertex shader that paired with ps)
    assert!(r2.cache_miss < 20);
    assert_eq!(
        std::fs::read(&r2.image).unwrap(),
        f.image_bytes,
        "the repaired build is not the Python's image"
    );
    // a finisher that panics is a failed shader, not a crash of the build (in an unwinding build; the release profile aborts, which F2's code must never need)
    #[cfg(not(panic = "abort"))]
    {
        let p = Failing {
            inner: ReplayFinisher::new(&f.image_bytes).unwrap(),
            fail: vec![],
            panic_on: Some(other.clone()),
        };
        let r3 = run(
            &options(&f, "panic", &f.work.join("cas3"), Some(f.keys.clone())),
            &tools2,
            &p,
        )
        .unwrap();
        assert_eq!(r3.failed.get("finish panicked"), Some(&1));
    }
    let _ = std::fs::remove_dir_all(&f.work);
}

#[test]
fn a_cancelled_build_keeps_what_it_finished() {
    let Some(f) = fixture("cancel") else { return };
    let cas = f.work.join("cas");
    let cancel = AtomicBool::new(false);
    let done = AtomicUsize::new(0);
    struct Stop<'a> {
        inner: ReplayFinisher<'a>,
        cancel: &'a AtomicBool,
        done: &'a AtomicUsize,
    }
    impl Finisher for Stop<'_> {
        fn fingerprint(&self) -> String {
            self.inner.fingerprint()
        }
        fn finish(
            &self,
            key: &str,
            work: &Path,
            job: &mut Value,
            c1: &[u8],
            tools: &dyn ShaderTools,
            paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
        ) -> Finished {
            if self.done.fetch_add(1, Ordering::SeqCst) == 30 {
                self.cancel.store(true, Ordering::SeqCst);
            }
            self.inner.finish(key, work, job, c1, tools, paired)
        }
    }
    let tools = fake_tools(vec![], Arc::new(AtomicUsize::new(0)));
    let fin = Stop {
        inner: ReplayFinisher::new(&f.image_bytes).unwrap(),
        cancel: &cancel,
        done: &done,
    };
    let log = |_: &str| {};
    let o = options(&f, "c1", &cas, Some(f.keys.clone()));
    let r = build(
        &o,
        &BuildEnv {
            tools: &tools,
            finisher: &fin,
            log: &log,
            cancel: Some(&cancel),
        },
    );
    assert_eq!(r.err().as_deref(), Some("cancelled"));
    // what the stopped build finished is in the cache: the next build resumes (a kill is the same)
    let ok_fin = ReplayFinisher::new(&f.image_bytes).unwrap();
    let r2 = run(
        &options(&f, "c2", &cas, Some(f.keys.clone())),
        &tools,
        &ok_fin,
    )
    .unwrap();
    assert_eq!(r2.ok, 401);
    assert!(
        r2.cache_hit >= 30 && r2.cache_hit < 401,
        "hits {}",
        r2.cache_hit
    );
    assert_eq!(std::fs::read(&r2.image).unwrap(), f.image_bytes);
    eprintln!(
        "resumed: {} of 401 shaders came from the stopped build's cache entries",
        r2.cache_hit
    );
    let _ = std::fs::remove_dir_all(&f.work);
}

#[test]
fn unknown_keys_and_a_missing_profile_are_reported() {
    let Some(f) = fixture("errors") else { return };
    let tools = fake_tools(vec![], Arc::new(AtomicUsize::new(0)));
    let fin = ReplayFinisher::new(&f.image_bytes).unwrap();
    let e = run(
        &options(
            &f,
            "e1",
            &f.work.join("cas"),
            Some(vec!["0123456789abcdef".into()]),
        ),
        &tools,
        &fin,
    )
    .err()
    .unwrap();
    assert!(e.contains("not in the shader map"), "{e}");
    let mut o = options(&f, "e2", &f.work.join("cas"), None);
    o.profile_dir = f.work.join("nowhere");
    let e = run(&o, &tools, &fin).err().unwrap();
    assert!(e.contains("holds no profile of game"), "{e}");
    let _ = std::fs::remove_dir_all(&f.work);
}

#[test]
fn two_builds_at_once_may_share_one_cache_folder() {
    // a live translation during play and the stage 3 build use the same shared folder: nothing in an entry is updated in place, a reader sees whole entries or none
    let Some(f) = fixture("shared") else { return };
    let cas = f.work.join("cas");
    let fin_a = ReplayFinisher::new(&f.image_bytes).unwrap();
    let fin_b = ReplayFinisher::new(&f.image_bytes).unwrap();
    let (ta, tb) = (
        fake_tools(vec![], Arc::new(AtomicUsize::new(0))),
        fake_tools(vec![], Arc::new(AtomicUsize::new(0))),
    );
    let (oa, ob) = (
        options(&f, "a", &cas, Some(f.keys.clone())),
        options(&f, "b", &cas, Some(f.keys.clone())),
    );
    let (ra, rb) = std::thread::scope(|s| {
        let a = s.spawn(|| run(&oa, &ta, &fin_a));
        let b = s.spawn(|| run(&ob, &tb, &fin_b));
        (a.join().unwrap().unwrap(), b.join().unwrap().unwrap())
    });
    assert_eq!((ra.ok, rb.ok), (401, 401));
    assert_eq!(std::fs::read(&ra.image).unwrap(), f.image_bytes);
    assert_eq!(std::fs::read(&rb.image).unwrap(), f.image_bytes);
    // afterwards the folder holds one whole entry per shader and no stray temporary file
    let entries: Vec<_> = walk(&cas.join(format!("cas-{CHAIN}.dmsb")));
    assert_eq!(entries.iter().filter(|n| n.contains(".tmp-")).count(), 0);
    assert_eq!(entries.len(), 401);
    eprintln!(
        "two simultaneous builds over one cache: both identical to the Python's image; {} entries, 0 stray temporary files",
        entries.len()
    );
    let _ = std::fs::remove_dir_all(&f.work);
}

fn walk(d: &Path) -> Vec<String> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out
}
