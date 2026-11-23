//! `shader_finish --cap <capture dir> --expect <library image> --profile <profile dir> --xlate <dmsh_xlate> --spirv-cross <spirv-cross> [--keys FILE] [--threads N] [--pulling]
//!                [--partners expected|rust] [--dump DIR] [--only ps|vs|cs|gs] [--show N] [--limit N]`
//!
//! The verification harness of `ibuilder_shader::finish` (Mac only: the two native tools are driven as child processes by the test-only `ProcessTools`, which is why this lives
//! in `src/bin` and not in the library). For every captured shader (`<key>/{<key>.spv, <key>.res.json, <key>.bin, job.json, c1.hex, result.json}`, written by
//! `tools/finish/make_cap_engine.py`'s instrumented `dmsh_build.py`) it runs the Rust `finish` and compares every file it returns with the file of the same name in the library image
//! the Python build wrote, and the Python's own status with ours. The reflection is compared as the `<key>_b.json` file the image holds (byte for byte, with its first
//! differing json path when it differs): the capture's in-memory `result.json` `refl` is not compared, it is dumped with `default=repr` and is not like for like.
//! Reports identical / different / unported / failed per stage and per file kind.
use ibuilder_shader::finish::{Ctx, Finished, Options, finish_with_locs};
use ibuilder_shader::romfs::Image;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use rayon::prelude::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

struct ProcessTools {
    xlate: PathBuf,
    spirv_cross: PathBuf,
    /** `--mutate discard`: vertex shader keys whose MSL gets the marker comment appended */
    discard_keys: std::collections::HashSet<String>,
}
const MUT_DISCARD: (&str, &str) = (
    "main0_out out = {};",
    "main0_out out = {};\n    // access::write",
);
/// nanoseconds spent inside the child processes (all threads), and the number of calls: subtracted from the time of `finish` to get the Rust part
static TOOL_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TOOL_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn run(bin: &Path, argv: &[String]) -> ToolRun {
    let t0 = Instant::now();
    let r = run_inner(bin, argv);
    TOOL_NS.fetch_add(
        t0.elapsed().as_nanos() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
    TOOL_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    r
}
fn run_inner(bin: &Path, argv: &[String]) -> ToolRun {
    match Command::new(bin).args(argv).output() {
        Ok(o) => ToolRun {
            rc: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => ToolRun::fail(127, format!("{}: {e}", bin.display())),
    }
}
impl ShaderTools for ProcessTools {
    fn xlate(&self, argv: &[String]) -> ToolRun {
        run(&self.xlate, argv)
    }
    fn spirv_cross(&self, argv: &[String]) -> ToolRun {
        let r = run(&self.spirv_cross, argv);
        if r.rc == 0
            && let Some(i) = argv.iter().position(|a| a == "--output")
        {
            let path = Path::new(&argv[i + 1]);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if self.discard_keys.contains(stem)
                && let Ok(text) = std::fs::read_to_string(path)
            {
                let _ = std::fs::write(path, text.replacen(MUT_DISCARD.0, MUT_DISCARD.1, 1));
            }
        }
        r
    }
}

static FINISH_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

/// the first path where two json values differ
fn first_diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    None => return Some(format!("{path}/{k}: only in ours")),
                    Some(w) => {
                        if let Some(d) = first_diff(v, w, &format!("{path}/{k}")) {
                            return Some(d);
                        }
                    }
                }
            }
            for k in y.keys() {
                if !x.contains_key(k) {
                    return Some(format!("{path}/{k}: only in the Python's"));
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (v, w)) in x.iter().zip(y).enumerate() {
                if let Some(d) = first_diff(v, w, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            if x.len() != y.len() {
                return Some(format!("{path}: length {} vs {}", x.len(), y.len()));
            }
            None
        }
        _ => {
            if a == b {
                None
            } else {
                Some(format!(
                    "{path}: ours {} / python {}",
                    a.to_string().chars().take(100).collect::<String>(),
                    b.to_string().chars().take(100).collect::<String>()
                ))
            }
        }
    }
}

fn kind_of(key: &str, name: &str) -> String {
    name.strip_prefix(key)
        .map(|s| format!("<key>{s}"))
        .unwrap_or_else(|| name.to_string())
}

#[derive(Default)]
struct Row {
    stage: String,
    status: String,
    files_same: u32,
    same_kinds: Vec<String>,
    files_diff: Vec<String>,
    files_missing: Vec<String>,
    files_extra: Vec<String>,
    py_status: String,
    detail: String,
    refl_diff: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cap = PathBuf::from(arg(&args, "--cap").ok_or("--cap")?);
    let expect = arg(&args, "--expect").ok_or("--expect")?;
    let profile = PathBuf::from(arg(&args, "--profile").ok_or("--profile")?);
    let mutate: BTreeSet<String> = arg(&args, "--mutate")
        .map(|m| m.split(',').map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let pulling = args.iter().any(|a| a == "--pulling");
    let partners = arg(&args, "--partners").unwrap_or_else(|| "expected".into());
    let only = arg(&args, "--only");
    let show: usize = arg(&args, "--show")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let limit: usize = arg(&args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let dump = arg(&args, "--dump");
    if let Some(n) = arg(&args, "--threads").and_then(|s| s.parse::<usize>().ok()) {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()?;
    }
    let scratch = PathBuf::from(arg(&args, "--work").unwrap_or_else(|| {
        std::env::temp_dir()
            .join(format!("shader_finish-{}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }));
    std::fs::create_dir_all(&scratch)?;

    let f = std::fs::File::open(&expect)?;
    // SAFETY: a read-only map of a finished library image; nothing writes to it meanwhile
    let map = unsafe { memmap2::Mmap::map(&f)? };
    let img = Image::open(&map[..])?;
    let chain = Path::new(&expect)
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("b-"))
        .and_then(|n| n.strip_suffix(".img"))
        .unwrap_or("6c53446391cc")
        .to_string();
    let ctx = Ctx::load(
        &profile,
        "0100DFA027504000",
        &chain,
        Options::ios_source_mode(pulling),
    )?;

    let mut keys: Vec<String> = std::fs::read_dir(&cap)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("job.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    keys.sort();
    if let Some(kf) = arg(&args, "--keys") {
        let want: BTreeSet<String> = std::fs::read_to_string(kf)?
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        keys.retain(|k| want.contains(k));
    }
    let jobs: Vec<(String, Value, String)> = keys
        .iter()
        .filter_map(|k| {
            let job: Value =
                serde_json::from_slice(&std::fs::read(cap.join(k).join("job.json")).ok()?).ok()?;
            Some((
                k.clone(),
                job.clone(),
                job["stage"].as_str().unwrap_or("?").to_string(),
            ))
        })
        .filter(|(_, _, st)| only.as_deref().map(|o| o == st).unwrap_or(true))
        .take(limit)
        .collect();
    let tools = ProcessTools {
        xlate: arg(&args, "--xlate").ok_or("--xlate")?.into(),
        spirv_cross: arg(&args, "--spirv-cross").ok_or("--spirv-cross")?.into(),
        discard_keys: if mutate.contains("discard") {
            jobs.iter()
                .filter(|j| j.2 == "vs")
                .map(|j| j.0.clone())
                .collect()
        } else {
            Default::default()
        },
    };
    println!(
        "{} shaders to finish ({} in the capture){}",
        jobs.len(),
        keys.len(),
        if mutate.is_empty() {
            String::new()
        } else {
            format!(", mutations: {mutate:?}")
        }
    );

    let expected = |name: &str| -> Option<&[u8]> { img.find(name).map(|f| img.bytes(f)) };
    let ps_locs_expected = |ps: &str| -> Option<BTreeSet<u32>> {
        expected(&format!("{ps}_b.metal"))
            .map(|b| ibuilder_shader::finish::user_locations(&String::from_utf8_lossy(b)))
    };

    let rust_locs: Mutex<HashMap<String, BTreeSet<u32>>> = Mutex::new(HashMap::new());
    let rows: Mutex<BTreeMap<String, Row>> = Mutex::new(BTreeMap::new());
    let dumped: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());
    let one = |(key, job0, stage): &(String, Value, String)| {
        let work = scratch.join(key);
        let _ = std::fs::create_dir_all(&work);
        for ext in ["spv", "res.json", "bin"] {
            let _ = std::fs::copy(
                cap.join(key).join(format!("{key}.{ext}")),
                work.join(format!("{key}.{ext}")),
            );
        }
        let mut job = job0.clone();
        job["code"] = Value::String(
            work.join(format!("{key}.bin"))
                .to_string_lossy()
                .into_owned(),
        );
        if mutate.contains("tf") && stage == "vs" {
            job["tf"] = serde_json::json!([{"stride": 16, "outputs": [[0, 4]]}, [], [], []]);
        }
        let c1 = unhex(
            std::fs::read_to_string(cap.join(key).join("c1.hex"))
                .unwrap_or_default()
                .trim(),
        );
        let paired = |ps: &str| -> Option<BTreeSet<u32>> {
            if partners == "rust" {
                rust_locs.lock().unwrap().get(ps).cloned()
            } else {
                ps_locs_expected(ps)
            }
        };
        let t0 = Instant::now();
        let r: Finished = finish_with_locs(&ctx, key, &work, &mut job, &c1, &tools, &paired);
        FINISH_NS.fetch_add(
            t0.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        if stage == "ps" {
            rust_locs
                .lock()
                .unwrap()
                .insert(key.clone(), r.ps_locs.clone());
        }
        let py: Value = std::fs::read(cap.join(key).join("result.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null);
        let py_status = py["status"].as_str().unwrap_or("?").to_string();
        let mut row = Row {
            stage: stage.clone(),
            status: r.status.clone(),
            py_status: py_status.clone(),
            detail: r.detail.clone(),
            ..Default::default()
        };
        if r.is_ok() {
            let mine: BTreeMap<&str, &Vec<u8>> =
                r.outputs.iter().map(|(n, b)| (n.as_str(), b)).collect();
            for (n, b) in &mine {
                match expected(n) {
                    Some(e) if e == &b[..] => {
                        row.files_same += 1;
                        row.same_kinds.push(kind_of(key, n));
                    }
                    Some(e) => {
                        row.files_diff.push(n.to_string());
                        if n.ends_with(".json")
                            && let (Ok(a), Ok(b2)) = (
                                serde_json::from_slice::<Value>(b),
                                serde_json::from_slice::<Value>(e),
                            )
                            && row.refl_diff.is_none()
                        {
                            row.refl_diff = first_diff(&a, &b2, "");
                        }
                        if dump.is_some() {
                            dumped.lock().unwrap().push((n.to_string(), b.to_vec()));
                        }
                    }
                    None => row.files_extra.push(n.to_string()),
                }
            }
            let prefix = format!("{key}_b");
            for f in &img.files {
                if f.name.starts_with(&prefix)
                    && !mine.contains_key(f.name.as_str())
                    && !f.name.contains("_m3")
                {
                    row.files_missing.push(f.name.clone());
                }
            }
        }
        let _ = std::fs::remove_dir_all(&work);
        rows.lock().unwrap().insert(key.clone(), row);
    };
    let t0 = Instant::now();
    let (p1, p2): (Vec<_>, Vec<_>) = jobs
        .iter()
        .partition(|(k, _, st)| !(ctx.is_paired(k) && st == "vs"));
    p1.par_iter().for_each(|j| one(j));
    let t1 = t0.elapsed().as_secs_f64();
    p2.par_iter().for_each(|j| one(j));
    println!(
        "finished in {:.1} s (phase 1: {} shaders {:.1} s, phase 2: {} paired vertex shaders {:.1} s)",
        t0.elapsed().as_secs_f64(),
        p1.len(),
        t1,
        p2.len(),
        t0.elapsed().as_secs_f64() - t1
    );

    // ---- report
    {
        let (fin, tool, calls) = (
            FINISH_NS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9,
            TOOL_NS.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e9,
            TOOL_CALLS.load(std::sync::atomic::Ordering::Relaxed),
        );
        let n = jobs.len().max(1) as f64;
        println!(
            "time inside finish (summed over threads, loaded machine): {fin:.1} s = {:.1} ms a shader; of it in the native tools ({calls} child processes): {tool:.1} s; the Rust part: {:.1} s = {:.2} ms a shader",
            fin * 1e3 / n,
            (fin - tool).max(0.0),
            (fin - tool).max(0.0) * 1e3 / n
        );
    }
    let rows = rows.into_inner().unwrap();
    let mut by_stage: BTreeMap<String, [u32; 4]> = BTreeMap::new(); // identical, differ, unported, failed
    let mut kinds: BTreeMap<String, [u32; 3]> = BTreeMap::new(); // identical, differ, missing/extra
    let mut unported: BTreeMap<String, u32> = BTreeMap::new();
    let mut bad: Vec<(&String, &Row)> = Vec::new();
    let mut py_mismatch = 0;
    for (key, r) in &rows {
        let e = by_stage.entry(r.stage.clone()).or_default();
        if r.status == "ok" {
            if r.py_status != "ok" {
                py_mismatch += 1;
            }
            if r.files_diff.is_empty() && r.files_missing.is_empty() && r.files_extra.is_empty() {
                e[0] += 1;
            } else {
                e[1] += 1;
                bad.push((key, r));
            }
            for n in r.files_diff.iter() {
                kinds.entry(kind_of(key, n)).or_default()[1] += 1;
            }
            for n in r.files_missing.iter().chain(r.files_extra.iter()) {
                kinds.entry(kind_of(key, n)).or_default()[2] += 1;
            }
        } else if r.status.starts_with("unported") {
            e[2] += 1;
            *unported.entry(r.detail.clone()).or_default() += 1;
        } else {
            e[3] += 1;
            bad.push((key, r));
        }
    }
    // identical files per kind: counted from the returned files that matched
    for (key, r) in &rows {
        if r.status == "ok" {
            let _ = key;
        }
    }
    let total = |i: usize| by_stage.values().map(|v| v[i]).sum::<u32>();
    println!(
        "shaders: identical {}, differ {}, unported {}, failed/other {} of {}  (Python status not ok while Rust ok: {})",
        total(0),
        total(1),
        total(2),
        total(3),
        rows.len(),
        py_mismatch
    );
    for (st, v) in &by_stage {
        println!(
            "  {st}: identical {}, differ {}, unported {}, failed {}",
            v[0], v[1], v[2], v[3]
        );
    }
    println!("unported reasons: {unported:?}");
    println!("differing / missing / extra files by kind: {kinds:?}");
    let mut same_by_kind: BTreeMap<String, u32> = BTreeMap::new();
    for r in rows.values() {
        for k in &r.same_kinds {
            *same_by_kind.entry(k.clone()).or_default() += 1;
        }
    }
    println!("byte-identical files by kind: {same_by_kind:?}");
    let files_same: u32 = rows.values().map(|r| r.files_same).sum();
    println!("files byte-identical to the library image: {files_same}");
    for (k, r) in bad.iter().take(show) {
        println!(
            "BAD {k} [{}] status={} (python {}) detail={} diff={:?} missing={:?} extra={:?} json: {:?}",
            r.stage,
            r.status,
            r.py_status,
            r.detail.chars().take(120).collect::<String>(),
            r.files_diff,
            r.files_missing,
            r.files_extra,
            r.refl_diff
        );
    }
    if let Some(d) = dump {
        std::fs::create_dir_all(&d)?;
        for (n, b) in dumped.into_inner().unwrap() {
            std::fs::write(format!("{d}/{n}"), b)?;
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}
