#![allow(dead_code)]
//! Shared fixtures for the Python-parity specs (`test_pipeline.py`, `test_resume.py`) and the chain tests.

use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::config::Config;
use ibuilder_pipeline::ctx::{Ctx, View};
use ibuilder_pipeline::env::MapEnv;
use ibuilder_pipeline::log::MemLog;
use ibuilder_pipeline::memory::FakeMem;
use ibuilder_pipeline::testutil::TempDir;
pub use ibuilder_pipeline::testutil::write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const MIB: u64 = 1 << 20;
pub const G: u64 = 1 << 30;

pub struct Fx {
    pub dir: TempDir,
    pub ctx: Ctx,
    pub log: Arc<MemLog>,
    pub env: Arc<MapEnv>,
    pub mem: Arc<FakeMem>,
}

impl Fx {
    pub fn new() -> Fx {
        let dir = TempDir::new("spec");
        let log = MemLog::new();
        let env = Arc::new(MapEnv::new());
        let mem = Arc::new(FakeMem::new(0));
        let ctx = Ctx::builder(dir.path())
            .env(env.clone())
            .log(log.clone())
            .mem(mem.clone())
            .build();
        Fx {
            dir,
            ctx,
            log,
            env,
            mem,
        }
    }
    pub fn docs(&self) -> &Path {
        self.dir.path()
    }
    pub fn p(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }
    pub fn cfg(&mut self, text: &str) -> &mut Fx {
        self.ctx.cfg = Config::parse(text);
        self
    }
    pub fn set_cfg(&mut self, k: &str, v: &str) {
        self.ctx.cfg.set(k, v);
    }
    pub fn profile(&mut self, text: &str) {
        let p = self.p("profile.tsv");
        write(&p, text);
        self.ctx.cfg.set("profile", p.to_string_lossy().to_string());
    }
}

pub fn rows(n: usize) -> String {
    let mut s = "package\tindex\tstage\tdm_ctl_sha16\tsource\n".to_string();
    for i in 0..n {
        s.push_str(&format!("p\t{i}\tv\tabc\ts\n"));
    }
    s
}

/// What `FakeDm` was in test_pipeline.py: records what the host would have seen at the boot, and can die or fail.
#[derive(Default)]
pub struct FakeBoot {
    pub request: PathBuf,
    pub mode: Mutex<String>, // ok | die | stage3
    pub calls: AtomicUsize,
    pub seen: Mutex<Seen>,
}

#[derive(Default, Clone, Debug)]
pub struct Seen {
    pub request: Option<String>,
    pub save: Option<String>,
    pub temp: Option<String>,
    pub msl: Option<String>,
    pub shmap: Option<String>,
}

impl FakeBoot {
    pub fn new(request: PathBuf, mode: &str) -> Arc<FakeBoot> {
        Arc::new(FakeBoot {
            request,
            mode: Mutex::new(mode.to_string()),
            ..Default::default()
        })
    }
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    pub fn seen(&self) -> Seen {
        self.seen.lock().unwrap().clone()
    }
}

impl BootHost for FakeBoot {
    fn boot(&self, _req: &BootRequest<'_>, v: &View<'_>) -> BootResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.seen.lock().unwrap() = Seen {
            request: std::fs::read_to_string(&self.request).ok(),
            save: v.env.get("DM_SAVE"),
            temp: v.env.get("DM_TEMP"),
            msl: v.env.get("DM_MSL_DIR"),
            shmap: v.env.get("DM_NVN_SHMAP"),
        };
        match self.mode.lock().unwrap().as_str() {
            "die" => panic!("the process was killed inside the guest"),
            "stage3" => BootResult {
                stage: 3,
                guest: -1,
            },
            _ => BootResult { stage: 0, guest: 0 },
        }
    }
}

/// A profile deriver that records `render` calls and may fail them.
#[derive(Default)]
pub struct RecordingProfile {
    pub renders: Mutex<Vec<String>>,
    pub fail_render: bool,
}
impl ProfileDeriver for RecordingProfile {
    fn derive(&self, _v: &View<'_>) -> ibuilder_pipeline::Result<PathBuf> {
        Err(ibuilder_pipeline::PipelineError::failed(
            "not used by this spec",
        ))
    }
    fn render(&self, v: &View<'_>, tid: &str, _profile: &Path) -> ibuilder_pipeline::Result<()> {
        self.renders.lock().unwrap().push(format!(
            "{tid} DM_GAMES_DIR={}",
            v.env.get("DM_GAMES_DIR").unwrap_or_default()
        ));
        if self.fail_render {
            Err(ibuilder_pipeline::PipelineError::failed("render refused"))
        } else {
            Ok(())
        }
    }
}

/// Counts calls and writes deterministic outputs (the Python test's `FakeDriver`).
#[derive(Default)]
pub struct CountingRewriter {
    pub runs: Mutex<Vec<(String, usize)>>,
}
impl Rewriter for CountingRewriter {
    fn rewrite(&self, req: &RewriteRequest<'_>, _v: &View<'_>) -> ibuilder_pipeline::Result<()> {
        self.runs
            .lock()
            .unwrap()
            .push((req.module.to_string(), req.jobs));
        let data = std::fs::read(req.src)?;
        for suffix in [".patched.bin", ".fixups.tsv", ".gates.tsv"] {
            let mut d = data.clone();
            d.extend_from_slice(suffix.as_bytes());
            std::fs::write(req.out_dir.join(format!("{}{suffix}", req.module)), d)?;
        }
        Ok(())
    }
}

/// Build a `<id>.shaders.tar` (ustar) like `make_pack` in the Python test.
pub fn make_pack(path: &Path, top: &str, files: &[(&str, &str)]) {
    let f = std::fs::File::create(path).unwrap();
    let mut b = tar::Builder::new(f);
    for (name, text) in files {
        let mut h = tar::Header::new_ustar();
        let full = if top.is_empty() {
            name.to_string()
        } else {
            format!("{top}/{name}")
        };
        // set_path refuses `..`; write the raw name bytes for the evil case
        let bytes = full.as_bytes();
        h.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
        h.set_size(text.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, text.as_bytes()).unwrap();
    }
    b.finish().unwrap();
}

pub fn default_pack_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "shader_map.tsv",
            "package\tindex\tstage\tdm_ctl_sha16\tsource\nmain\t0\tvs\tabcdef0123456789\tx\n",
        ),
        (
            "c1_full.tsv",
            "# chain b-19d183a2b462 inputs 8ea0535d1d1c\n",
        ),
        ("b-19d183a2b462.img", "IMG"),
    ]
}
