//! The second cache level (`toolmemo`): the tools' answers are reused only for the same tool and the same input bytes, a finishing change re-finishes every shader (from the memo, no
//! translation), a translator or SPIRV-Cross change translates again, and failures are never kept. Stand-in tools whose answers carry their version, a synthetic game, no engine.
mod synth;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use ibuilder_shaderbuild::finish::{Finished, Finisher};
use ibuilder_shaderbuild::toolmemo::{MemoTools, cross_identity, xlate_identity};
use ibuilder_shaderbuild::tools::FnTools;
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, build};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use synth::*;

/// stand-ins whose answers carry their version: the translator writes `<key>.spv` / `<key>.res.json` (and `<key>.reader.spv` for a geometry job), `[RED]` for a code that starts with
/// `BAD`, exit 1 then; SPIRV-Cross writes `msl v<n>:` + its input (to `--output`, or stdout)
#[derive(Clone)]
struct Stand {
    xv: Arc<AtomicU64>,
    cv: Arc<AtomicU64>,
    /// the job keys of every translator call (identity calls not counted)
    calls: Arc<Mutex<Vec<Vec<String>>>>,
    /// SPIRV-Cross calls other than `--version`
    cross: Arc<AtomicU64>,
    /// a file the translator writes besides the jobs' (an output the memo does not know)
    junk: Arc<AtomicU64>,
}

impl Stand {
    fn new() -> Stand {
        Stand {
            xv: Arc::new(AtomicU64::new(1)),
            cv: Arc::new(AtomicU64::new(1)),
            calls: Arc::new(Mutex::new(Vec::new())),
            cross: Arc::new(AtomicU64::new(0)),
            junk: Arc::new(AtomicU64::new(0)),
        }
    }
    fn tools(&self) -> FnTools {
        let s = self.clone();
        let xlate = move |argv: &[String]| -> ToolRun {
            let v = s.xv.load(Ordering::SeqCst);
            if argv == ["--source-sha"] {
                return ToolRun {
                    rc: 0,
                    stdout: format!("{:016x}\n", 0xabc0 + v),
                    stderr: String::new(),
                };
            }
            let jobs: Value = serde_json::from_str(&fs::read_to_string(&argv[0]).unwrap()).unwrap();
            let out = Path::new(&argv[1]);
            fs::create_dir_all(out).unwrap();
            let (mut keys, mut err) = (Vec::new(), String::new());
            for j in jobs.as_array().unwrap() {
                let k = j["key"].as_str().unwrap().to_string();
                let code = fs::read(j["code"].as_str().unwrap()).unwrap();
                if code.starts_with(b"BAD") {
                    err.push_str(&format!("[RED] {k}: refused\n"));
                } else {
                    if code.starts_with(b"ODD") {
                        // a `[RED]` line for a job that has files, in a call that exits 0 (not what dmsh_xlate does: the memo must not keep it either)
                        err.push_str(&format!("[RED] {k}: written, but refused\n"));
                    }
                    let mut spv = format!("spv v{v} stage {}: ", j["stage"]).into_bytes();
                    spv.extend_from_slice(&code);
                    fs::write(out.join(format!("{k}.spv")), spv).unwrap();
                    fs::write(out.join(format!("{k}.res.json")), format!("{{\"v\": {v}}}"))
                        .unwrap();
                    if j.get("geometry_topology").is_some_and(|g| !g.is_null()) {
                        fs::write(out.join(format!("{k}.reader.spv")), format!("reader v{v}"))
                            .unwrap();
                    }
                }
                keys.push(k);
            }
            if s.junk.load(Ordering::SeqCst) > 0 {
                fs::write(out.join("junk.txt"), b"?").unwrap();
            }
            s.calls.lock().unwrap().push(keys);
            ToolRun {
                rc: if err.contains("refused\n") && !err.contains("but refused") {
                    1
                } else {
                    0
                },
                stdout: String::new(),
                stderr: err,
            }
        };
        let s2 = self.clone();
        let cross = move |argv: &[String]| -> ToolRun {
            let v = s2.cv.load(Ordering::SeqCst);
            if argv == ["--version"] {
                return ToolRun::fail(
                    1,
                    format!(
                        "Git commit: stand-in-{v} Timestamp: 2026-11-21\nUsage: spirv-cross [SPIR-V file] [options]\n"
                    ),
                );
            }
            s2.cross.fetch_add(1, Ordering::SeqCst);
            let input = argv
                .iter()
                .find(|a| a.contains('/') && Path::new(a).is_file())
                .unwrap();
            let mut msl = format!(
                "msl v{v} {}: ",
                argv.iter()
                    .filter(|a| !a.contains('/'))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .into_bytes();
            msl.extend_from_slice(&fs::read(input).unwrap());
            if msl.windows(3).any(|w| w == b"BAD") {
                return ToolRun::fail(2, "cross refused");
            }
            match argv.iter().position(|a| a == "--output") {
                Some(i) => {
                    fs::write(&argv[i + 1], msl).unwrap();
                    ToolRun::ok()
                }
                None => ToolRun {
                    rc: 0,
                    stdout: String::from_utf8(msl).unwrap(),
                    stderr: "a warning\n".into(),
                },
            }
        };
        FnTools::new(Arc::new(xlate), Arc::new(cross))
    }
    fn job_calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

/// the memo over the stand-ins, with the identities a build takes from them
fn memo<'a>(t: &'a FnTools, cache: &Path, chain: &str) -> MemoTools<'a> {
    MemoTools::new(
        t,
        cache,
        chain,
        xlate_identity(&t.xlate(&["--source-sha".into()])),
        cross_identity(&t.spirv_cross(&["--version".into()])),
    )
}

/// a job file of `jobs` (key, code) in `dir`, the codes written beside it
fn jobs_in(dir: &Path, jobs: &[(&str, &str)], extra: Value) -> String {
    fs::create_dir_all(dir).unwrap();
    let mut list = Vec::new();
    for (k, code) in jobs {
        let cp = dir.join(format!("{k}.bin"));
        fs::write(&cp, code).unwrap();
        let mut j = json!({"key": k, "stage": "ps", "code": cp.to_string_lossy()});
        for (a, b) in extra.as_object().unwrap() {
            j[a] = b.clone();
        }
        list.push(j);
    }
    let p = dir.join("jobs.json");
    fs::write(&p, serde_json::to_string(&list).unwrap()).unwrap();
    p.to_string_lossy().into_owned()
}

fn read(p: PathBuf) -> Vec<u8> {
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn a_repeated_job_is_answered_from_the_memo_with_the_same_bytes_wherever_it_lies() {
    let root = scratch("memo-repeat");
    let st = Stand::new();
    let t = st.tools();
    let m = memo(&t, &root.join("cache"), CHAIN);
    let (a, b) = (root.join("a"), root.join("b"));
    let ja = jobs_in(
        &a,
        &[("k1", "code one"), ("k2", "code two"), ("g", "geo")],
        json!({}),
    );
    let r = m.xlate(&[ja.clone(), a.to_string_lossy().into_owned()]);
    assert_eq!(r.rc, 0);
    assert_eq!(st.job_calls(), 1);
    let first: Vec<Vec<u8>> = ["k1.spv", "k1.res.json", "k2.spv", "k2.res.json"]
        .iter()
        .map(|n| read(a.join(n)))
        .collect();
    // nothing of the memo's own is left in the output folder
    let names: BTreeSet<String> = fs::read_dir(&a)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.starts_with(".memo")), "{names:?}");
    // the same jobs, their codes in another folder (another path in the job): every one is a hit, the translator is not called, the files are the same bytes
    let jb = jobs_in(
        &b,
        &[("k1", "code one"), ("k2", "code two"), ("g", "geo")],
        json!({}),
    );
    let r = m.xlate(&[jb, b.to_string_lossy().into_owned()]);
    assert_eq!((r.rc, st.job_calls()), (0, 1));
    let again: Vec<Vec<u8>> = ["k1.spv", "k1.res.json", "k2.spv", "k2.res.json"]
        .iter()
        .map(|n| read(b.join(n)))
        .collect();
    assert_eq!(again, first);
    assert_eq!(m.stats.xlate_hits.load(Ordering::SeqCst), 3);
    // a geometry job's extra file is one of its outputs, kept and given back with the rest
    let gj = root.join("gj");
    let jg = jobs_in(&gj, &[("q", "geo")], json!({"geometry_topology": 2}));
    m.xlate(&[jg.clone(), gj.to_string_lossy().into_owned()]);
    let gk = root.join("gk");
    let jk = jobs_in(&gk, &[("q", "geo")], json!({"geometry_topology": 2}));
    let calls = st.job_calls();
    m.xlate(&[jk, gk.to_string_lossy().into_owned()]);
    assert_eq!(st.job_calls(), calls, "a hit");
    assert_eq!(read(gk.join("q.reader.spv")), b"reader v1");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn only_the_missed_jobs_go_to_the_translator() {
    let root = scratch("memo-partial");
    let st = Stand::new();
    let t = st.tools();
    let m = memo(&t, &root.join("cache"), CHAIN);
    let a = root.join("a");
    m.xlate(&[
        jobs_in(&a, &[("k2", "two")], json!({})),
        a.to_string_lossy().into_owned(),
    ]);
    let b = root.join("b");
    let r = m.xlate(&[
        jobs_in(
            &b,
            &[("k1", "one"), ("k2", "two"), ("k3", "three")],
            json!({}),
        ),
        b.to_string_lossy().into_owned(),
    ]);
    assert_eq!(r.rc, 0);
    assert_eq!(
        st.calls.lock().unwrap().last().unwrap(),
        &vec!["k1".to_string(), "k3".to_string()]
    );
    for k in ["k1", "k2", "k3"] {
        assert!(
            b.join(format!("{k}.spv")).is_file() && b.join(format!("{k}.res.json")).is_file(),
            "{k}"
        );
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_new_translator_a_new_job_or_new_code_is_translated_again() {
    let root = scratch("memo-change");
    let st = Stand::new();
    let t = st.tools();
    let cache = root.join("cache");
    let run = |m: &MemoTools, dir: &str, jobs: &[(&str, &str)], extra: Value| {
        let d = root.join(dir);
        m.xlate(&[jobs_in(&d, jobs, extra), d.to_string_lossy().into_owned()]);
        read(d.join("k.spv"))
    };
    let m1 = memo(&t, &cache, CHAIN);
    assert_eq!(
        run(&m1, "a", &[("k", "x")], json!({})),
        b"spv v1 stage \"ps\": x"
    );
    assert_eq!(st.job_calls(), 1);
    assert_eq!(
        run(&m1, "b", &[("k", "x")], json!({})),
        b"spv v1 stage \"ps\": x"
    );
    assert_eq!(st.job_calls(), 1, "the same job: a hit");
    // other code bytes, another job field, another stage: each a miss
    assert_eq!(
        run(&m1, "c", &[("k", "y")], json!({})),
        b"spv v1 stage \"ps\": y"
    );
    assert_eq!(
        run(&m1, "d", &[("k", "x")], json!({"attr": {"0": "uint"}})),
        b"spv v1 stage \"ps\": x"
    );
    assert_eq!(
        run(&m1, "e", &[("k", "x")], json!({"stage": "vs"})),
        b"spv v1 stage \"vs\": x"
    );
    assert_eq!(st.job_calls(), 4);
    // a new translator (its --source-sha changes): the memo of the build that sees it keys on the new identity, so the old entries cannot answer
    st.xv.store(2, Ordering::SeqCst);
    let m2 = memo(&t, &cache, CHAIN);
    assert_eq!(
        run(&m2, "f", &[("k", "x")], json!({})),
        b"spv v2 stage \"ps\": x"
    );
    assert_eq!(st.job_calls(), 5);
    // another chain: a miss too (its folder is another one in a real build; here the same folder, the key differs)
    let m3 = memo(&t, &cache, "another0chain");
    run(&m3, "g", &[("k", "x")], json!({}));
    assert_eq!(st.job_calls(), 6);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn failures_are_never_kept_and_a_failed_call_keeps_nothing() {
    let root = scratch("memo-fail");
    let st = Stand::new();
    let t = st.tools();
    let m = memo(&t, &root.join("cache"), CHAIN);
    for (i, d) in ["a", "b"].iter().enumerate() {
        let d = root.join(d);
        let r = m.xlate(&[
            jobs_in(&d, &[("ok1", "fine"), ("bad", "BAD code")], json!({})),
            d.to_string_lossy().into_owned(),
        ]);
        assert_eq!(r.rc, 1);
        assert!(r.stderr.contains("[RED] bad: refused"), "{}", r.stderr);
        assert_eq!(
            st.job_calls(),
            i + 1,
            "the call that failed ran again: the good job of a failed call was not kept either"
        );
        assert!(!d.join("bad.spv").exists());
    }
    // alone, the good job is kept from then on
    let c = root.join("c");
    m.xlate(&[
        jobs_in(&c, &[("ok1", "fine")], json!({})),
        c.to_string_lossy().into_owned(),
    ]);
    let d = root.join("d");
    m.xlate(&[
        jobs_in(&d, &[("ok1", "fine")], json!({})),
        d.to_string_lossy().into_owned(),
    ]);
    assert_eq!(st.job_calls(), 3);
    // a job with a `[RED]` line is not kept even when it has files and the call exits 0
    for d in ["e", "f"] {
        let d = root.join(d);
        let r = m.xlate(&[
            jobs_in(&d, &[("odd", "ODD code")], json!({})),
            d.to_string_lossy().into_owned(),
        ]);
        assert_eq!(r.rc, 0);
        assert!(r.stderr.contains("[RED] odd:"), "{}", r.stderr);
    }
    assert_eq!(st.job_calls(), 5);
    // SPIRV-Cross: a failure runs again, every time
    let spv = root.join("bad.spv");
    fs::write(&spv, b"BAD").unwrap();
    for _ in 0..2 {
        let r = m.spirv_cross(&["--msl".into(), spv.to_string_lossy().into_owned()]);
        assert_eq!(r.rc, 2);
    }
    assert_eq!(st.cross.load(Ordering::SeqCst), 2);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_damaged_entry_is_a_miss_and_is_written_again() {
    let root = scratch("memo-damaged");
    let st = Stand::new();
    let t = st.tools();
    let cache = root.join("cache");
    let m = memo(&t, &cache, CHAIN);
    let a = root.join("a");
    m.xlate(&[
        jobs_in(&a, &[("k", "x")], json!({})),
        a.to_string_lossy().into_owned(),
    ]);
    let entries: Vec<PathBuf> = walk(&cache.join("t"));
    assert_eq!(entries.len(), 1);
    let full = fs::read(&entries[0]).unwrap();
    fs::write(&entries[0], &full[..full.len() - 3]).unwrap();
    let b = root.join("b");
    m.xlate(&[
        jobs_in(&b, &[("k", "x")], json!({})),
        b.to_string_lossy().into_owned(),
    ]);
    assert_eq!(st.job_calls(), 2, "the truncated entry was not served");
    assert_eq!(
        fs::read(&entries[0]).unwrap(),
        full,
        "and it was written whole again"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_tool_without_an_identity_is_not_memoized_and_unknown_outputs_are_not_kept() {
    let root = scratch("memo-noid");
    let st = Stand::new();
    let t = st.tools();
    let m = MemoTools::new(&t, &root.join("cache"), CHAIN, None, None);
    for d in ["a", "b"] {
        let d = root.join(d);
        m.xlate(&[
            jobs_in(&d, &[("k", "x")], json!({})),
            d.to_string_lossy().into_owned(),
        ]);
    }
    assert_eq!(st.job_calls(), 2);
    assert!(!root.join("cache").join("t").exists());
    // a translator that writes a file that is no job's: handed over, nothing recorded
    st.junk.store(1, Ordering::SeqCst);
    let m = memo(&t, &root.join("cache2"), CHAIN);
    for d in ["c", "e"] {
        let d = root.join(d);
        m.xlate(&[
            jobs_in(&d, &[("k", "x")], json!({})),
            d.to_string_lossy().into_owned(),
        ]);
        assert!(d.join("junk.txt").is_file() && d.join("k.spv").is_file());
    }
    assert_eq!(st.job_calls(), 4);
    assert_eq!(m.stats.stored.load(Ordering::SeqCst), 0);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn spirv_cross_answers_are_keyed_by_the_input_bytes_the_arguments_and_the_version() {
    let root = scratch("memo-cross");
    let st = Stand::new();
    let t = st.tools();
    let cache = root.join("cache");
    let m = memo(&t, &cache, CHAIN);
    let spv = |dir: &str, b: &[u8]| {
        let d = root.join(dir);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("k.spv"), b).unwrap();
        d
    };
    let call = |m: &MemoTools, d: &Path, extra: &str| {
        let out = d.join("k.metal");
        let _ = fs::remove_file(&out);
        let r = m.spirv_cross(&[
            "--msl".into(),
            extra.into(),
            d.join("k.spv").to_string_lossy().into_owned(),
            "--output".into(),
            out.to_string_lossy().into_owned(),
        ]);
        assert_eq!(r.rc, 0);
        read(out)
    };
    let a = spv("a", b"words");
    assert_eq!(call(&m, &a, "--x"), b"msl v1 --msl --x --output: words");
    let b = spv("b", b"words");
    assert_eq!(call(&m, &b, "--x"), b"msl v1 --msl --x --output: words");
    assert_eq!(
        st.cross.load(Ordering::SeqCst),
        1,
        "the same bytes elsewhere: a hit, the output file written"
    );
    assert_eq!(call(&m, &b, "--y"), b"msl v1 --msl --y --output: words");
    let c = spv("c", b"other words");
    assert_eq!(
        call(&m, &c, "--x"),
        b"msl v1 --msl --x --output: other words"
    );
    assert_eq!(st.cross.load(Ordering::SeqCst), 3);
    // stdout answers (no --output) come back with their stderr
    let p = a.join("k.spv").to_string_lossy().into_owned();
    let r1 = m.spirv_cross(&[p.clone(), "--version".into(), "450".into()]);
    let r2 = m.spirv_cross(&[p, "--version".into(), "450".into()]);
    assert_eq!(r1, r2);
    assert_eq!(r2.stdout, "msl v1 --version 450: words");
    assert_eq!(st.cross.load(Ordering::SeqCst), 4);
    // `--version` alone is never memoized (it is the identity)
    m.spirv_cross(&["--version".into()]);
    // a new SPIRV-Cross: a miss
    st.cv.store(2, Ordering::SeqCst);
    let m2 = memo(&t, &cache, CHAIN);
    assert_eq!(call(&m2, &a, "--x"), b"msl v2 --msl --x --output: words");
    assert_eq!(st.cross.load(Ordering::SeqCst), 5);
    let _ = fs::remove_dir_all(&root);
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![d.to_path_buf()];
    while let Some(p) = stack.pop() {
        for e in fs::read_dir(&p).into_iter().flatten().flatten() {
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

// ---- whole builds ------------------------------------------------------------------------------------------------------------------------------------------------------------------

/// a finisher that runs SPIRV-Cross on the shader and signs what it writes with its fingerprint (its "code")
struct Fin(String);

impl Finisher for Fin {
    fn fingerprint(&self) -> String {
        self.0.clone()
    }
    fn finish(
        &self,
        key: &str,
        work: &Path,
        _job: &mut Value,
        _c1: &[u8],
        tools: &dyn ShaderTools,
        _paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let out = work.join(format!("{key}.metal"));
        let r = tools.spirv_cross(&[
            "--msl".into(),
            work.join(format!("{key}.spv"))
                .to_string_lossy()
                .into_owned(),
            "--output".into(),
            out.to_string_lossy().into_owned(),
        ]);
        if r.rc != 0 {
            return Finished::failed("MSL failed", r.stderr);
        }
        let mut msl = fs::read(&out).unwrap();
        msl.extend_from_slice(format!("\n// finished by {}\n", self.0).as_bytes());
        Finished {
            status: "ok".into(),
            detail: String::new(),
            refl: None,
            outputs: vec![
                (format!("{key}_b.metal"), msl),
                (format!("{key}_b.json"), b"{}\n".to_vec()),
            ],
            ps_locs: BTreeSet::new(),
            log: Vec::new(),
        }
    }
}

fn whole(
    root: &Path,
    st: &Stand,
    fin: &str,
    memo_on: bool,
) -> (ibuilder_shaderbuild::BuildReport, Vec<u8>) {
    let mut o = BuildOptions::new(
        &root.join("pkg"),
        &root.join("profile"),
        "G",
        &root.join("out"),
        CHAIN,
        &root.join("cas"),
    );
    o.jobs = 3;
    o.tables = false;
    o.pulling = false;
    o.tool_memo = memo_on;
    let t = st.tools();
    let f = Fin(fin.to_string());
    let log = |_: &str| {};
    let r = build(
        &o,
        &BuildEnv {
            tools: &t,
            finisher: &f,
            log: &log,
            cancel: None,
        },
    )
    .unwrap();
    let img = fs::read(&r.image).unwrap();
    (r, img)
}

fn texts(img: &[u8]) -> Vec<(String, Vec<u8>)> {
    let i = ibuilder_shader::romfs::Image::open(img).unwrap();
    i.files
        .iter()
        .map(|f| (f.name.clone(), i.bytes(f).to_vec()))
        .collect()
}

#[test]
fn a_finishing_change_refinishes_every_shader_from_the_memo_and_a_tool_change_translates_again() {
    stop_lock();
    let root = scratch("memo-build");
    write_game(&root, "G", 40);
    let st = Stand::new();
    let cross_runs = || st.cross.load(Ordering::SeqCst);
    // 1. cold, finisher "f1": everything translated and crossed
    let (r1, img1) = whole(&root, &st, "f1", true);
    assert_eq!((r1.shaders, r1.ok, r1.cache_miss), (40, 40, 40));
    let (calls1, cross1) = (st.job_calls(), cross_runs());
    assert!(calls1 > 0 && cross1 == 40, "{calls1} {cross1}");
    // 2. the finishing code changes ("f2"): every shader is a first-level miss and is finished again -- from the memo: no translator call, no SPIRV-Cross run
    let (r2, img2) = whole(&root, &st, "f2", true);
    assert_eq!((r2.ok, r2.cache_hit, r2.cache_miss), (40, 0, 40));
    assert_eq!(
        (st.job_calls(), cross_runs()),
        (calls1, cross1),
        "nothing translated again"
    );
    assert_eq!(
        (
            r2.memo_xlate_hits,
            r2.memo_xlate_misses,
            r2.memo_cross_hits,
            r2.memo_cross_misses
        ),
        (40, 0, 40, 0)
    );
    let (t1, t2) = (texts(&img1), texts(&img2));
    assert_eq!(t1.len(), t2.len());
    let mut refinished = 0;
    for ((n1, b1), (n2, b2)) in t1.iter().zip(&t2) {
        assert_eq!(n1, n2);
        if n1.ends_with("_b.metal") {
            // the new finisher's output, over the same SPIRV-Cross text
            let s2 = String::from_utf8(b2.clone()).unwrap();
            assert!(
                s2.ends_with("// finished by f2\n") && s2.starts_with("msl v1"),
                "{s2}"
            );
            assert_eq!(
                b1.strip_suffix(b"// finished by f1\n"),
                b2.strip_suffix(b"// finished by f2\n")
            );
            refinished += 1;
        }
    }
    assert_eq!(refinished, 40);
    // 3. the same build again: first-level hits, the finisher does not run
    let (r3, img3) = whole(&root, &st, "f2", true);
    assert_eq!((r3.cache_hit, img3 == img2), (40, true));
    // 4. a new translator (same chain id, new --source-sha): a miss at both levels, translated again, the new translator's output
    st.xv.store(2, Ordering::SeqCst);
    let (r4, img4) = whole(&root, &st, "f2", true);
    assert_eq!((r4.ok, r4.cache_miss, r4.memo_xlate_misses), (40, 40, 40));
    assert!(st.job_calls() > calls1);
    for (n, b) in texts(&img4) {
        if n.ends_with("_b.metal") {
            assert!(String::from_utf8(b).unwrap().contains("spv v2"), "{n}");
        }
    }
    // 5. a new SPIRV-Cross: a miss at both levels, crossed again
    st.cv.store(2, Ordering::SeqCst);
    let before = cross_runs();
    let (r5, img5) = whole(&root, &st, "f2", true);
    assert_eq!(
        (r5.cache_miss, r5.memo_cross_misses, cross_runs() - before),
        (40, 40, 40)
    );
    for (n, b) in texts(&img5) {
        if n.ends_with("_b.metal") {
            assert!(String::from_utf8(b).unwrap().starts_with("msl v2"), "{n}");
        }
    }
    // 6. without the memo the same build is the same library (the memo changes no byte), and it translates everything
    let calls = st.job_calls();
    let root2 = scratch("memo-build-off");
    write_game(&root2, "G", 40);
    let (r6, img6) = whole(&root2, &st, "f2", false);
    assert_eq!((r6.cache_miss, r6.memo_xlate_hits), (40, 0));
    assert!(st.job_calls() > calls);
    assert_eq!(texts(&img6), texts(&img5), "memo on or off: the same files");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&root2);
}

/// the stop tests of this crate share the process-wide stop byte; this test must not see one raised
fn stop_lock() {
    ibuilder_pipeline::stop::clear();
}
