//! Timings per build (DECISIONS trap 76): one row of `Documents/build_times.tsv` per build (the last 50, under a header) and one summary line at
//! the end of the log. A build the process left on purpose (the restart before stage 3, trap 75) or lost (a kill, trap 74) saves what it measured in
//! `Documents/gameapp.timing.partial` after every stage; the build that continues it adds to it (`resumed` counts the runs), so a row is a whole build.

use crate::config::Config;
use crate::env::Env;
use crate::log::Log;
use crate::memory::MemSource;
use crate::util::{MIB, read_text, utc_now_iso, write_atomic};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const BUILD_TIMES_KEEP: usize = 50;
pub const BUILD_TIMES_COLUMNS: [&str; 21] = [
    "date",
    "game",
    "app_build",
    "device",
    "ram_unlocked",
    "eva",
    "resumed",
    "nup_s",
    "profile_s",
    "rewrite_s",
    "shaders_s",
    "split_s",
    "translate_s",
    "shaders",
    "read_jobs",
    "jobs",
    "peak_mib",
    "cooldown_s",
    "assemble_s",
    "total_s",
    "result",
];

/// A stage 3 number: ints print as ints, floats with one decimal (`take_shader_stats`).
#[derive(Clone, Debug, PartialEq)]
pub enum Stat {
    Int(i64),
    Float(f64),
    Text(String),
}

impl Stat {
    fn text(&self) -> String {
        match self {
            Stat::Int(i) => i.to_string(),
            Stat::Float(f) => format!("{f:.1}"),
            Stat::Text(s) => s.clone(),
        }
    }
}

/// `BUILD_STATS`: the last stage 3's numbers (the timings row reads them): resumed, read_jobs, jobs, split_s, translate_s, shaders, peak_mib, cooldown_s, ...
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildStats(BTreeMap<String, Stat>);

impl BuildStats {
    pub fn new() -> BuildStats {
        BuildStats::default()
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn set_int(&mut self, k: &str, v: i64) {
        self.0.insert(k.to_string(), Stat::Int(v));
    }
    pub fn set_float(&mut self, k: &str, v: f64) {
        self.0.insert(k.to_string(), Stat::Float(v));
    }
    pub fn get(&self, k: &str) -> Option<&Stat> {
        self.0.get(k)
    }
    pub fn get_int(&self, k: &str) -> Option<i64> {
        match self.0.get(k) {
            Some(Stat::Int(i)) => Some(*i),
            _ => None,
        }
    }
    pub fn contains(&self, k: &str) -> bool {
        self.0.contains_key(k)
    }
}

/// `device_model()`: the device's model identifier ("iPhone16,1"), else the machine's. The simulator names the model it imitates.
pub fn device_model(env: &dyn Env) -> String {
    if let Some(m) = env
        .get("SIMULATOR_MODEL_IDENTIFIER")
        .filter(|m| !m.is_empty())
    {
        return format!("{m} (simulator)");
    }
    crate::sys::machine_model().unwrap_or_else(|| "-".to_string())
}

pub struct Timings {
    path: PathBuf,
    partial: PathBuf,
    pub secs: BTreeMap<String, f64>,
    pub resumed: u32,
    pub game: String,
    /// the dump the build is for (cfg `game`, a file name): the partial of a killed or paused build is matched by it. The Python app matched the partial's `game=` (the TITLE ID, set after the
    /// profile stage) against cfg `game` (the dump's NAME), which never agree, so a continued build never merged: latent bug, fixed here by also writing and comparing `dump=`.
    pub dump: String,
    pub extra: BTreeMap<String, String>,
    /// the config the row is made for (set by the chain)
    pub cfg: Option<Config>,
    /// false for a boot-only launch (no stage of the chain ran: no row)
    pub record: bool,
    pub result: String,
    /// `os_proc_available_memory` at launch, bytes
    pub launch_available: u64,
    pub device: String,
    t0: Instant,
}

impl Timings {
    pub const STAGES: [&'static str; 5] = ["nup", "profile", "rewrite", "shaders", "assemble"];

    pub fn new(path: &Path, partial: &Path, device: &str) -> Timings {
        Timings {
            path: path.to_path_buf(),
            partial: partial.to_path_buf(),
            secs: BTreeMap::new(),
            resumed: 0,
            game: "-".into(),
            dump: String::new(),
            extra: BTreeMap::new(),
            cfg: None,
            record: true,
            result: "stopped".into(),
            launch_available: 0,
            device: device.to_string(),
            t0: Instant::now(),
        }
    }

    pub fn elapsed(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// `merge_partial`: a build that continues another of the same game adds the stages the other had finished. Any other leftover is dropped.
    pub fn merge_partial(&mut self, game: &str, continues: bool) {
        let got = read_kv(&self.partial);
        let _ = fs::remove_file(&self.partial);
        let same = got.get("game").map(|s| s.as_str()) == Some(game)
            || (!game.is_empty() && got.get("dump").map(|s| s.as_str()) == Some(game));
        if !continues || got.is_empty() || !same {
            return;
        }
        self.resumed = got
            .get("resumed")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        for st in Self::STAGES {
            if let Some(v) = got.get(&format!("{st}_s")) {
                let v = if v.is_empty() { "0" } else { v };
                if let Ok(f) = v.parse::<f64>() {
                    self.secs.insert(st.to_string(), f);
                }
            }
        }
        for k in [
            "split_s",
            "translate_s",
            "shaders",
            "read_jobs",
            "jobs",
            "peak_mib",
            "cooldown_s",
        ] {
            if let Some(v) = got.get(k).filter(|v| !v.is_empty()) {
                self.extra.insert(k.to_string(), v.clone());
            }
        }
    }

    /// `with timing.stage(name): ...`: runs `f`, adds its duration under `name` and saves the partial (also when `f` fails).
    pub fn timed<R>(&mut self, name: &str, f: impl FnOnce() -> R) -> R {
        let t = Instant::now();
        let r = f();
        *self.secs.entry(name.to_string()).or_insert(0.0) += t.elapsed().as_secs_f64();
        self.save_partial();
        r
    }

    /// Add `secs` to a stage and save the partial (for callers that cannot hand a closure over).
    pub fn add(&mut self, name: &str, secs: f64) {
        *self.secs.entry(name.to_string()).or_insert(0.0) += secs;
        self.save_partial();
    }

    pub fn save_partial(&self) {
        let mut out = format!("game={}\nresumed={}\n", self.game, self.resumed);
        if !self.dump.is_empty() {
            out.push_str(&format!("dump={}\n", self.dump));
        }
        for st in Self::STAGES {
            if let Some(v) = self.secs.get(st) {
                out.push_str(&format!("{st}_s={v:.1}\n"));
            }
        }
        for (k, v) in &self.extra {
            out.push_str(&format!("{k}={v}\n"));
        }
        let _ = crate::util::write_atomic(&self.partial, out.as_bytes()); // tmp + rename: a kill mid-write must not leave a half partial for the build that continues this one
    }

    pub fn take_shader_stats(&mut self, stats: &BuildStats) {
        for (k, v) in &stats.0 {
            self.extra.insert(k.clone(), v.text());
        }
    }

    /// One row of `build_times.tsv` (21 columns, no tabs or newlines inside a value).
    pub fn row(
        &mut self,
        cfg: &Config,
        result: &str,
        stats: &BuildStats,
        mem: &dyn MemSource,
    ) -> Vec<String> {
        self.take_shader_stats(stats);
        let ceiling = self.launch_available / MIB;
        let eva = match mem.address_space() {
            Some(a) => if a.extended { "1" } else { "0" }.to_string(),
            None => "-".to_string(),
        };
        let total: f64 = self.secs.values().sum();
        let g = |k: &str, d: &str| self.extra.get(k).cloned().unwrap_or_else(|| d.to_string());
        let secs = |k: &str| {
            self.secs
                .get(k)
                .map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "-".into())
        };
        let mut values = vec![
            utc_now_iso(),
            self.game.clone(),
            cfg.get_nonempty("app_build").unwrap_or("-").to_string(),
            self.device.clone(),
            (if ceiling > 4500 {
                "1"
            } else if ceiling > 0 {
                "0"
            } else {
                "-"
            })
            .to_string(),
            eva,
            self.resumed.to_string(),
        ];
        for st in ["nup", "profile", "rewrite", "shaders"] {
            values.push(secs(st));
        }
        values.extend([
            g("split_s", "-"),
            g("translate_s", "-"),
            g("shaders", "-"),
            g("read_jobs", "-"),
            g("jobs", "-"),
            g("peak_mib", "-"),
            g("cooldown_s", "0"),
        ]);
        values.extend([secs("assemble"), format!("{total:.1}"), result.to_string()]);
        values
            .into_iter()
            .map(|v| v.replace(['\t', '\n'], " "))
            .collect()
    }

    /// The summary line of the log.
    pub fn summary(&self, row: &[String]) -> String {
        let d: BTreeMap<&str, &str> = BUILD_TIMES_COLUMNS
            .iter()
            .copied()
            .zip(row.iter().map(|s| s.as_str()))
            .collect();
        let part = |k: &str, label: &str| -> Option<String> {
            if d[k] != "-" {
                Some(format!("{label} {}s", d[k]))
            } else {
                None
            }
        };
        let mut shaders = part("shaders_s", "shaders");
        if let Some(s) = shaders.as_mut()
            && d["split_s"] != "-"
        {
            s.push_str(&format!(
                    " (reading {}s, translation {}s; {} shaders; {} reading / {} translating threads; peak {} MiB)",
                    d["split_s"], d["translate_s"], d["shaders"], d["read_jobs"], d["jobs"], d["peak_mib"]
                ));
        }
        let parts: Vec<String> = [
            part("nup_s", "nup"),
            part("profile_s", "profile"),
            part("rewrite_s", "rewrite"),
            shaders,
            part("assemble_s", "assemble"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let flags = [
            format!(
                "RAM {}",
                match d["ram_unlocked"] {
                    "1" => "unlocked",
                    "0" => "locked",
                    _ => "?",
                }
            ),
            format!(
                "EVA {}",
                match d["eva"] {
                    "1" => "on",
                    "0" => "off",
                    _ => "?",
                }
            ),
        ];
        format!(
            "build times: {} -- total {}s, {} on {} ({}){}",
            if parts.is_empty() {
                "no stage ran".to_string()
            } else {
                parts.join(" | ")
            },
            d["total_s"],
            d["result"],
            d["device"],
            flags.join(", "),
            if self.resumed > 0 {
                format!(
                    ", continued {} time{}",
                    self.resumed,
                    if self.resumed == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            }
        )
    }

    /// Writes the row (the last `BUILD_TIMES_KEEP` are kept) and the log's summary line; never fails (says so in the log instead).
    pub fn finish(
        &mut self,
        cfg: &Config,
        result: &str,
        stats: &BuildStats,
        mem: &dyn MemSource,
        log: &dyn Log,
    ) -> Option<Vec<String>> {
        let row = self.row(cfg, result, stats, mem);
        log.line(&self.summary(&row));
        if result.starts_with("paused") {
            self.save_partial();
            return Some(row);
        }
        let write = || -> std::io::Result<()> {
            let header = BUILD_TIMES_COLUMNS.join("\t");
            let lines: Vec<String> = read_text(&self.path)
                .map(|t| t.lines().map(String::from).collect())
                .unwrap_or_default();
            let old: Vec<String> = if lines.first().map(|l| l == &header).unwrap_or(false) {
                lines[1..].to_vec()
            } else {
                Vec::new()
            };
            let mut rows = old;
            rows.push(row.join("\t"));
            let skip = rows.len().saturating_sub(BUILD_TIMES_KEEP);
            let rows = &rows[skip..];
            // this writer's own temporary name (review r4 §4.1: a fixed `<path>.tmp` was shared by every writer)
            write_atomic(
                &self.path,
                format!("{header}\n{}\n", rows.join("\n")).as_bytes(),
            )?;
            let _ = fs::remove_file(&self.partial);
            Ok(())
        };
        match write() {
            Ok(()) => Some(row),
            Err(e) => {
                log.line(&format!("build times: not recorded (io::Error: {e})"));
                None
            }
        }
    }
}

/// `read_kv`: `key=value` lines (no comments) of a small state file; empty when it is missing.
pub fn read_kv(path: &Path) -> BTreeMap<String, String> {
    let mut rows = BTreeMap::new();
    if let Ok(t) = read_text(path) {
        for line in t.lines() {
            if let Some((k, v)) = line.split_once('=') {
                rows.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    rows
}
