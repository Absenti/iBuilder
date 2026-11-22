//! A synthetic game for the stop tests: N shaders in one split package, a translator that writes a stub `.spv` for each job, and a finisher whose work is a short sleep and a few
//! bytes that depend on the key only (so two builds of the same game make the same library, whoever stopped in between). Nothing here needs the game data or the engine.
#![allow(dead_code)]
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use ibuilder_shaderbuild::finish::{Finished, Finisher};
use ibuilder_shaderbuild::tools::FnTools;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub const CHAIN: &str = "c0ffee123456";

pub fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ibsb-stop-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// the 16 hex digits of shader `i`
pub fn key_of(i: usize) -> String {
    format!("{:016x}", 0x1000_0000_0000_0000u64 + i as u64 * 7919)
}

/// a split record that `job_for` makes a pixel-shader job of: the marker, the key, a code area of printable bytes, and a control section that is not NVN2 (the magic, zeros to +0x800)
fn record(key: &str) -> Vec<u8> {
    let mut r = b"BTCDBTCD".to_vec();
    r.extend((0..8).map(|i| u8::from_str_radix(&key[2 * i..2 * i + 2], 16).unwrap()));
    r.extend([0u8; 0x28]);
    r.extend(format!("code of {key} ").repeat(6).bytes());
    let mut ctl = vec![0u8; 0x800];
    ctl[..4].copy_from_slice(&[0x34, 0x12, 0x76, 0x98]);
    r.extend(ctl);
    r
}

/// the shader map of `n` shaders, all in `ps_main.bin`
pub fn map_text(n: usize) -> String {
    let mut map = "package\tindex\tstage\tdm_ctl_sha16\tsource\n".to_string();
    for i in 0..n {
        map.push_str(&format!("ps_main.bin\t{i}\tps\t{}\tsynthetic\n", key_of(i)));
    }
    map
}

/// what the split writes for `n` shaders: `<pkg>/ps_main.bin` and the four tables in `<tables>`
pub fn write_split(pkg: &Path, tables: &Path, n: usize) {
    fs::create_dir_all(pkg).unwrap();
    fs::create_dir_all(tables).unwrap();
    let mut bin = Vec::new();
    for i in 0..n {
        bin.extend(record(&key_of(i)));
    }
    fs::write(pkg.join("ps_main.bin"), bin).unwrap();
    fs::write(tables.join("shader_map.tsv"), map_text(n)).unwrap();
    for t in ["shader_facts.tsv", "shader_pairs.tsv", "pso_pairs.tsv"] {
        fs::write(tables.join(t), "").unwrap();
    }
}

/// `<root>/profile/<game>/{game.tsv, shader_map.tsv, shader_facts.tsv, shader_pairs.tsv}` and `<root>/pkg/ps_main.bin` for `n` shaders
pub fn write_game(root: &Path, game: &str, n: usize) {
    let g = root.join("profile").join(game);
    fs::create_dir_all(&g).unwrap();
    fs::write(g.join("game.tsv"), format!("title_id\t{game}\n")).unwrap();
    write_split(&root.join("pkg"), &g, n);
}

/// the translator's stand-in: a stub `.spv` and `.res.json` for each job of the file it is given, after `xlate_ms`
pub fn tools(xlate_ms: u64) -> FnTools {
    let xlate = move |argv: &[String]| -> ToolRun {
        if xlate_ms > 0 {
            std::thread::sleep(Duration::from_millis(xlate_ms));
        }
        let jobs: Value = serde_json::from_str(&fs::read_to_string(&argv[0]).unwrap()).unwrap();
        for j in jobs.as_array().unwrap() {
            let k = j["key"].as_str().unwrap();
            fs::write(Path::new(&argv[1]).join(format!("{k}.spv")), b"spv").unwrap();
            fs::write(Path::new(&argv[1]).join(format!("{k}.res.json")), b"{}").unwrap();
        }
        ToolRun::ok()
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

/// what a finished shader's library file holds: a function of the key
pub fn body(key: &str) -> Vec<u8> {
    format!("// shader {key}\nkernel void k_{key}() {{}}\n").into_bytes()
}

/// a finisher that takes `item_ms` a shader, counts its calls, and calls `raise` when its `raise_at`-th call begins
pub struct Slow {
    pub started: AtomicUsize,
    pub finished: AtomicUsize,
    pub item_ms: u64,
    pub raise_at: Option<usize>,
    pub raise: Box<dyn Fn() + Send + Sync>,
}

impl Slow {
    pub fn new(item_ms: u64) -> Slow {
        Slow {
            started: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            item_ms,
            raise_at: None,
            raise: Box::new(|| {}),
        }
    }
    pub fn raising_at(item_ms: u64, at: usize, raise: impl Fn() + Send + Sync + 'static) -> Slow {
        Slow {
            raise_at: Some(at),
            raise: Box::new(raise),
            ..Slow::new(item_ms)
        }
    }
    pub fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }
    pub fn finished(&self) -> usize {
        self.finished.load(Ordering::SeqCst)
    }
}

impl Finisher for Slow {
    fn fingerprint(&self) -> String {
        "stop-test-finisher-1".into()
    }
    fn finish(
        &self,
        key: &str,
        _work: &Path,
        _job: &mut Value,
        _c1: &[u8],
        _tools: &dyn ShaderTools,
        _paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        if Some(n) == self.raise_at {
            (self.raise)();
        }
        if self.item_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.item_ms));
        }
        self.finished.fetch_add(1, Ordering::SeqCst);
        Finished {
            status: "ok".into(),
            detail: String::new(),
            refl: None,
            outputs: vec![
                (format!("{key}_b.metal"), body(key)),
                (format!("{key}_b.json"), b"{}\n".to_vec()),
            ],
            ps_locs: BTreeSet::new(),
            log: Vec::new(),
        }
    }
}

/// every entry of the translation cache `<cas>/cas-<chain>.dmsb/<hh>/<digest>` (a file whose name has no temporary suffix)
pub fn cache_entries(cas: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(cas.join(format!("cas-{CHAIN}.dmsb"))) else {
        return out;
    };
    for d in rd.flatten() {
        if let Ok(files) = fs::read_dir(d.path()) {
            out.extend(
                files
                    .flatten()
                    .filter(|f| !f.file_name().to_string_lossy().contains(".tmp"))
                    .map(|f| f.path()),
            );
        }
    }
    out.sort();
    out
}

/// the files under `dir` (any depth) whose names say they are not whole: `.part`, `.tmp...`
pub fn stray_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(p);
            } else if e.file_name().to_string_lossy().ends_with(".part")
                || e.file_name().to_string_lossy().contains(".tmp")
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// every cache entry reads back whole and holds the files `Slow` makes for its key
pub fn assert_entries_intact(cas: &Path) -> usize {
    let entries = cache_entries(cas);
    for p in &entries {
        let st = ibuilder_shaderbuild::cache::read_header(p)
            .unwrap_or_else(|| panic!("{} does not read back as a cache entry", p.display()));
        let names: Vec<&str> = st.files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        let key = names[0]
            .strip_suffix("_b.json")
            .or_else(|| names[0].strip_suffix("_b.metal"))
            .unwrap();
        assert_eq!(
            st.read(&format!("{key}_b.metal")).unwrap().unwrap(),
            body(key),
            "{}: the shader's library file is not what its finisher made",
            p.display()
        );
    }
    entries.len()
}
