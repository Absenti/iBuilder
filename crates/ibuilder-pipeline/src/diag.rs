//! Hidden diagnostics of the Rust chain: `Documents/.diag/pipeline-<session>.jsonl`, and the host's cheap diagnostic switches turned on for a game.
//!
//! The app's own side (`GameAppDiag.cs`, `app/patches/0050-hidden-diagnostics.patch`) writes `events-<session>.tsv` (lifecycle, memory warnings, thermal, every UI log line with its time),
//! `samples-<session>.tsv` (memory every 5 s) and `previous-run.txt` (how the last launch ended); this module writes what only the chain knows, one JSON object per line:
//!
//! * `chain`        the process, the config (secrets left out), the device (`hw.machine`, `kern.osversion`, memory), the bundle;
//! * `stage_begin` / `stage_end`   every stage of the chain with its seconds, its outcome and the memory when it began and ended;
//! * `stage`        the `stage k/5: text` line the UI shows, when the step changes (and every 30 s of one long step);
//! * `mem`          every `[mem:tag]` line the build prints, as numbers (resident, peak, footprint, available, free disk);
//! * `log`          the log lines that say something went wrong (`[RED]`, `FAILED`, `REFUSED`, `warning`, `error`) with their time;
//! * `env`          the environment the game is started with (the diagnostic switches below, the paths);
//! * `boot_begin` / `boot_end`   the hand-over to the host and what came back.
//!
//! Every line carries `t` (seconds since the chain started) and `ts` (UTC). Nothing here changes what the chain does: a failure to write is swallowed. The session id is the one the app wrote in
//! `.diag/session.live` (so the two sides' files pair up); without the app (the CLI, the tests) the chain makes its own.
//!
//! [`host_env`] sets, for a game about to boot and only where nothing set them already (a harness, `Documents/env.txt`), the host's CHEAP diagnostic switches (`dm/tools/env_switches.py` lists
//! them): the boot's timing breakdown, a per-second memory accounting file, the draws that waited for their pipeline, the dropped frames' timing, a thread dump when no frame is presented
//! for a while, the open file descriptors, the command recording's usage. `diag_host=0` in `gameapp.config` leaves all of them off.

use crate::ctx::Ctx;
use crate::memory::MemInfo;
use crate::util::utc_now_iso;
use serde_json::{Map, Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

/// the newest sessions kept
const KEEP_SESSIONS: usize = 6;
/// the size at which only the lines that matter are still written
const CAP: u64 = 12 << 20;

struct Diag {
    file: File,
    t0: Instant,
    bytes: u64,
    last_stage_key: String,
    last_stage_at: Instant,
}

static DIAG: Mutex<Option<Diag>> = Mutex::new(None);

/// `Documents/.diag`
pub fn dir(docs: &Path) -> PathBuf {
    docs.join(".diag")
}

/// The session id: the app's (first line of `.diag/session.live`) or one made here.
pub fn session_id(docs: &Path) -> String {
    if let Ok(text) = fs::read_to_string(dir(docs).join("session.live")) {
        let id = text.lines().next().unwrap_or("").trim().to_string();
        if !id.is_empty()
            && id.len() < 64
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return id;
        }
    }
    let iso = utc_now_iso();
    let compact: String = iso.chars().filter(|c| c.is_ascii_digit()).collect();
    format!(
        "{}-{}-{}",
        &compact[..8.min(compact.len())],
        compact.get(8..14).unwrap_or("000000"),
        std::process::id()
    )
}

#[cfg(unix)]
fn sysctl_text(name: &str) -> String {
    crate::sys::sysctl_text(name).unwrap_or_else(|| "?".into())
}

#[cfg(not(unix))]
fn sysctl_text(_name: &str) -> String {
    "?".into()
}

/// The config as its log line shows it (`{'k': 'v', ...}`), with the rows whose key could name a secret left out. It goes by row: a value that itself holds `", "` is left out whole (the
/// filter used to split the line on `", "`, and kept the tail of such a value: review r3 §4.10).
pub fn redacted_config(cfg: &crate::config::Config) -> String {
    let mut kept = crate::config::Config::new();
    for (k, v) in cfg.iter() {
        let key = k.to_ascii_lowercase();
        if !["pass", "secret", "token", "p12", "key"]
            .iter()
            .any(|w| key.contains(w))
        {
            kept.set(k, v.clone());
        }
    }
    kept.to_string()
}

fn rotate(dir: &Path, keep_new: &str) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut mine: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.starts_with("pipeline-") && n.ends_with(".jsonl") && !n.contains(keep_new))
                .then(|| (n, e.path()))
        })
        .collect();
    mine.sort();
    let extra = mine.len().saturating_sub(KEEP_SESSIONS - 1);
    for (_, p) in mine.into_iter().take(extra) {
        let _ = fs::remove_file(p);
    }
}

/// Open this process's pipeline file and say what the chain starts with. Called once, first thing in the chain; a second call (a later chain in the same process) appends to the same file.
pub fn start(ctx: &Ctx) {
    let mut g = DIAG.lock().unwrap();
    if g.is_some() {
        return;
    }
    let d = dir(ctx.docs.root());
    if fs::create_dir_all(&d).is_err() {
        return;
    }
    let session = session_id(ctx.docs.root());
    rotate(&d, &session);
    let Ok(file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(d.join(format!("pipeline-{session}.jsonl")))
    else {
        return;
    };
    let bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    *g = Some(Diag {
        file,
        t0: Instant::now(),
        bytes,
        last_stage_key: String::new(),
        last_stage_at: Instant::now(),
    });
    drop(g);
    let m = ctx.mem.mem();
    event(
        "chain",
        json!({
            "session": session,
            "pid": std::process::id(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "machine": sysctl_text("hw.machine"),
            "kernel": sysctl_text("kern.osversion"),
            "crate": env!("CARGO_PKG_VERSION"),
            "bundle": ctx.bundle.display().to_string(),
            "config": redacted_config(&ctx.cfg),
            "mem": mem_json(&m),
        }),
    );
}

/// The memory numbers as a JSON object (what the events carry under `mem`).
pub fn mem_value(m: &MemInfo) -> Value {
    mem_json(m)
}

fn mem_json(m: &MemInfo) -> Value {
    let mib = |n: u64| (n as f64 / 1_048_576.0 * 10.0).round() / 10.0;
    json!({"resident_mib": mib(m.resident), "peak_mib": mib(m.peak), "footprint_mib": mib(m.footprint), "available_mib": mib(m.available), "free_gib": ((m.free as f64 / 1_073_741_824.0) * 100.0).round() / 100.0})
}

fn write_line(g: &mut Diag, kind: &str, fields: Value) {
    let mut o = Map::new();
    o.insert(
        "t".into(),
        json!((g.t0.elapsed().as_secs_f64() * 1000.0).round() / 1000.0),
    );
    o.insert("ts".into(), json!(utc_now_iso()));
    o.insert("k".into(), json!(kind));
    match fields {
        Value::Object(f) => o.extend(f),
        Value::Null => {}
        other => {
            o.insert("d".into(), other);
        }
    }
    let mut line = Value::Object(o).to_string();
    line.push('\n');
    if g.file.write_all(line.as_bytes()).is_ok() {
        g.bytes += line.len() as u64;
    }
}

/// One event (a JSON object's fields, or any value under `d`). Past the size cap only the kinds that matter are still written.
pub fn event(kind: &str, fields: Value) {
    let mut g = DIAG.lock().unwrap();
    let Some(d) = g.as_mut() else { return };
    if d.bytes > CAP
        && !matches!(
            kind,
            "chain" | "stage_begin" | "stage_end" | "boot_begin" | "boot_end" | "fail"
        )
    {
        return;
    }
    write_line(d, kind, fields);
}

/// A `[mem:tag]` line as numbers.
pub fn mem_event(tag: &str, m: &MemInfo) {
    let mut v = mem_json(m);
    if let Value::Object(o) = &mut v {
        o.insert("tag".into(), json!(tag));
    }
    event("mem", v);
}

/// A log line that says something went wrong, with its time (the UI log has the line; this has when, and where it falls among the stages and the memory). Every line of the chain comes
/// here (`Ctx::log`): the words are looked for first, and only a line that has one takes the process-wide lock (`event`, which writes nothing when there is no file): it took the lock for
/// every line, to see whether there was a file (review r4 §6.3).
pub fn log_line(line: &str) {
    if [
        "[RED]",
        "FAILED",
        "REFUSED",
        "warning",
        "Warning",
        "error",
        "Error",
        "cannot",
        "could not",
    ]
    .iter()
    .any(|w| line.contains(w))
    {
        let text = if line.len() > 1500 {
            &line[..line
                .char_indices()
                .take_while(|(i, _)| *i < 1500)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0)]
        } else {
            line
        };
        event("log", json!({ "line": text.trim() }));
    }
}

/// The `stage k/n: text` line: written when the step changes (the text before its `(`), and every 30 s of one long step.
pub fn stage(k: u32, n: u32, text: &str) {
    let mut g = DIAG.lock().unwrap();
    let Some(d) = g.as_mut() else { return };
    if d.bytes > CAP {
        return; // past the cap only the kinds `event` keeps are written, and `stage` is not one of them (review r3 §5.9)
    }
    let key = format!("{k}/{n} {}", text.split('(').next().unwrap_or("").trim());
    if key == d.last_stage_key && d.last_stage_at.elapsed().as_secs() < 30 {
        return;
    }
    d.last_stage_key = key;
    d.last_stage_at = Instant::now();
    write_line(
        d,
        "stage",
        json!({"k_of_n": format!("{k}/{n}"), "text": text}),
    );
}

/// The host's cheap diagnostic switches for this game, where nothing set them yet; the paths are in `Documents/.diag/`.
pub fn host_env(ctx: &Ctx) {
    if ctx.cfg.get_or("diag_host", "1") == "0" {
        event("env", json!({"diag_host": "off (diag_host=0)"}));
        return;
    }
    let d = dir(ctx.docs.root());
    if fs::create_dir_all(&d).is_err() {
        return;
    }
    let session = session_id(ctx.docs.root());
    let memstat = d.join(format!("memstat-{session}.tsv"));
    let wanted: [(&str, String); 7] = [
        ("DM_BOOTPROF", "1".into()), // the boot's timing: mapping, relocation, Metal library loading, pipeline creation, uploads, the first present
        ("DM_MEMSTAT", memstat.to_string_lossy().into_owned()), // one line a second: footprint, Metal allocation, resident and charged memory by region class
        ("DM_PSO_WAITLOG", "1".into()), // every draw that waited more than 20 ms for its pipeline, with its shader pair
        ("DM_SLOWFRAME", "1".into()),   // the timing breakdown of the frames that dropped
        ("DM_HITCH_DUMP", "2000:20".into()), // no frame for 2 s (from the 20th second): every guest thread once per stall
        ("DM_FD_LOG", "120".into()),         // the open file descriptors, every 2 minutes
        ("DM_NVN_CMDSTAT", "1".into()),      // the command recording's usage, every 300 frames
    ];
    let mut set = Map::new();
    let mut kept = Map::new();
    for (name, value) in wanted {
        match ctx.env.get(name).filter(|v| !v.is_empty()) {
            Some(have) => {
                kept.insert(name.into(), json!(have));
            }
            None => {
                ctx.env.set(name, &value);
                set.insert(name.into(), json!(value));
            }
        }
    }
    ctx.log(&format!(
        "game env: diagnostics on ({} switches, files in Documents/.diag)",
        set.len()
    ));
    event(
        "env",
        json!({"diag_set": Value::Object(set), "already_set": Value::Object(kept)}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    // the writer is one per process and other unit tests of the crate may write stage lines while this runs: every assertion is on lines carrying this test's own text
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn lines(docs: &Path) -> Vec<Value> {
        let mut out = vec![];
        for e in fs::read_dir(dir(docs)).unwrap().flatten() {
            if e.file_name().to_string_lossy().starts_with("pipeline-") {
                for l in fs::read_to_string(e.path()).unwrap().lines() {
                    out.push(serde_json::from_str(l).unwrap());
                }
            }
        }
        out
    }

    #[test]
    fn secrets_are_left_out_of_the_config_line() {
        let r = redacted_config(&crate::config::Config::parse(
            "nup=/x/a.nup\np12_passphrase=hunter2\ngame=g\napi_key=k\n",
        ));
        assert!(r.contains("nup") && r.contains("game"), "{r}");
        assert!(!r.contains("hunter2") && !r.contains("api_key"), "{r}");
    }

    /// Review r3 §4.10: a secret value that itself holds `", "` is left out whole (the filter goes by row, not by pieces of the line).
    #[test]
    fn a_secret_value_holding_a_comma_is_left_out_whole() {
        let mut c = crate::config::Config::new();
        c.set("game", "g.nsp");
        c.set("p12_passphrase", "hunter2, 'tail': 'still-secret");
        c.set("zz_after", "kept");
        let r = redacted_config(&c);
        assert!(
            r.contains("'game': 'g.nsp'") && r.contains("'zz_after': 'kept'"),
            "{r}"
        );
        assert!(
            !r.contains("hunter2") && !r.contains("still-secret") && !r.contains("p12"),
            "{r}"
        );
    }

    /// Review r4 §6.3: a line with none of the words goes by without the process-wide lock (here held by the test: the old code waited for it on every line).
    #[test]
    fn an_ordinary_log_line_does_not_take_the_lock() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let held = DIAG.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            log_line("stage 3 shaders: 1200/21990 shaders");
            let _ = tx.send(());
        });
        let passed = rx.recv_timeout(std::time::Duration::from_secs(5)).is_ok();
        drop(held);
        assert!(passed, "an ordinary line waited for the diagnostics lock");
    }

    /// Review r3 §5.9: past the size cap the `stage` lines stop like every other line that is not one of the few that matter.
    #[test]
    fn stage_lines_stop_at_the_size_cap() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let d = TempDir::new("diag-cap");
        let path = d.join("pipeline-cap.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        *DIAG.lock().unwrap() = Some(Diag {
            file,
            t0: Instant::now(),
            bytes: CAP + 1,
            last_stage_key: String::new(),
            last_stage_at: Instant::now(),
        });
        stage(3, 5, "diag-cap-step (1/10 shaders)");
        event("log", json!({"line": "diag-cap-log"}));
        event("stage_end", json!({"stage": "diag-cap-end"}));
        *DIAG.lock().unwrap() = None;
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("diag-cap-step"), "{text}");
        assert!(!text.contains("diag-cap-log"), "{text}");
        assert!(
            text.contains("diag-cap-end"),
            "the lines that matter still go: {text}"
        );
    }

    #[test]
    fn the_session_id_is_the_apps_when_it_wrote_one() {
        let d = TempDir::new("diag-session");
        fs::create_dir_all(dir(d.path())).unwrap();
        fs::write(dir(d.path()).join("session.live"), "20261119-123456-42\n").unwrap();
        assert_eq!(session_id(d.path()), "20261119-123456-42");
        fs::write(dir(d.path()).join("session.live"), "../../etc/passwd\n").unwrap();
        assert!(
            !session_id(d.path()).contains('/'),
            "an id that is not plain is not used as a file name"
        );
    }

    #[test]
    fn a_chain_leaves_stage_mem_log_and_env_lines_and_existing_switches_win() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let d = TempDir::new("diag-chain");
        let ctx = Ctx::builder(d.path()).build();
        ctx.env.set("DM_SLOWFRAME", "0"); // a harness / env.txt choice
        *DIAG.lock().unwrap() = None;
        start(&ctx);
        stage(3, 5, "diag-test-step (1/10 shaders)");
        stage(3, 5, "diag-test-step (2/10 shaders)"); // the same step: not written again
        mem_event("diag-test-tag", &ctx.mem.mem());
        log_line("stage 3 FAILED: diag-test-boom");
        log_line("diag-test: everything is fine");
        host_env(&ctx);
        *DIAG.lock().unwrap() = None;
        let all = lines(ctx.docs.root());
        let mine = |k: &str, field: &str, needle: &str| {
            all.iter()
                .filter(|v| {
                    v["k"] == k
                        && v[field]
                            .as_str()
                            .map(|t| t.contains(needle))
                            .unwrap_or(false)
                })
                .count()
        };
        assert_eq!(mine("stage", "text", "diag-test-step"), 1, "{all:?}");
        assert_eq!(mine("mem", "tag", "diag-test-tag"), 1);
        assert_eq!(mine("log", "line", "diag-test-boom"), 1);
        assert_eq!(
            mine("log", "line", "everything is fine"),
            0,
            "only the line that says something went wrong"
        );
        assert!(
            all.iter()
                .any(|v| v["k"] == "chain" && v["machine"].is_string())
        );
        assert!(all.iter().any(|v| v["k"] == "env"));
        assert_eq!(ctx.env.get("DM_BOOTPROF").as_deref(), Some("1"));
        assert_eq!(
            ctx.env.get("DM_SLOWFRAME").as_deref(),
            Some("0"),
            "a switch that was already set is left as it was"
        );
        assert!(
            ctx.env
                .get("DM_MEMSTAT")
                .unwrap()
                .contains(".diag/memstat-")
        );
    }

    #[test]
    fn diag_host_off_sets_nothing() {
        let d = TempDir::new("diag-off");
        let mut ctx = Ctx::builder(d.path()).build();
        ctx.cfg.set("diag_host", "0");
        host_env(&ctx);
        assert!(ctx.env.get("DM_BOOTPROF").is_none());
    }
}
