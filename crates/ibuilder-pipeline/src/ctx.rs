//! The run's context: everything the Python chain kept in module globals (`DOCS`, `BUNDLE`, `SHADER_PACKS`, `THERMAL`, `BUILD_STATS`,
//! `REWRITE_RAN`, `os.environ`, `_dmgameapp`) is a field here, so two chains can run in one process (tests) and nothing is global.

use crate::adapters::Adapters;
use crate::config::Config;
use crate::env::{Env, MapEnv};
use crate::layout::Docs;
use crate::live::LiveQueue;
use crate::log::{Log, MemLog};
use crate::memory::{FakeMem, MemSource};
use crate::thermal::{Clock, RealClock, ThermalGate};
use crate::timings::{BuildStats, Timings, device_model};
use crate::tsv::profile_title_id;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Ctx {
    /// `Documents`
    pub docs: Docs,
    /// the app bundle: `dm/games/<id>/game.tsv` of the games the engine knows, the host dylib, `dm/games/settings.tsv`
    pub bundle: PathBuf,
    pub cfg: Config,
    pub env: Arc<dyn Env>,
    /// `pipeline.stages.log`
    pub log: Arc<dyn Log>,
    /// `gameapp.chain.log` (the shim's `[gameapp] ...` lines: the boot, the time limit, signals)
    pub chain_log: Arc<dyn Log>,
    pub mem: Arc<dyn MemSource>,
    pub clock: Arc<dyn Clock>,
    pub thermal: Arc<ThermalGate>,
    pub timing: Timings,
    pub stats: BuildStats,
    /// the modules this process rewrote (not taken from the cache)
    pub rewrite_ran: Vec<String>,
    /// `SHADER_PACKS`: `Documents/shaders`, or the bundle's own folder for an exported game
    pub shader_packs: PathBuf,
    /// the run's own cancel token (`ibuilder_cancel(handle)`); the process-wide request (`stop`) is read beside it
    pub cancel: Arc<AtomicBool>,
    pub adapters: Adapters,
    pub live: Arc<LiveQueue>,
    /// `PIPELINE_BOOT=1`: stage 5 runs in this process (the app); a CLI that only builds leaves it off
    pub boot_enabled: bool,
    /// the stage status map (`status` in `_chain`)
    pub status: BTreeMap<String, String>,
}

/// What an adapter sees of the context (a shared view, so the context stays free to be borrowed mutably around the call).
pub struct View<'a> {
    pub docs: &'a Docs,
    pub bundle: &'a Path,
    pub cfg: &'a Config,
    pub env: &'a dyn Env,
    pub log: &'a dyn Log,
    pub chain_log: &'a Arc<dyn Log>,
    pub mem: &'a dyn MemSource,
    pub cancel: &'a AtomicBool,
    pub shader_packs: &'a Path,
}

impl View<'_> {
    /// The run was cancelled, or a pause or cancel was requested (`stop`): the adapters that poll this (the Switch 2 package reader) end the work in hand.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || crate::stop::requested() != crate::stop::NONE
    }
}

impl Ctx {
    pub fn view(&self) -> View<'_> {
        View {
            docs: &self.docs,
            bundle: &self.bundle,
            cfg: &self.cfg,
            env: &*self.env,
            log: &*self.log,
            chain_log: &self.chain_log,
            mem: &*self.mem,
            cancel: &self.cancel,
            shader_packs: &self.shader_packs,
        }
    }

    /// `_cfg_title_id`: the title id of the profile named in the config; `None` when it names none.
    pub fn title_id(&self) -> Option<String> {
        let p = self.cfg.get_nonempty("profile")?;
        profile_title_id(Path::new(p))
    }

    /// A stop is asked of this run: its own token (`ibuilder_cancel`) or the process-wide pause / cancel request (`stop`). `stop_kind` says which.
    pub fn cancelled(&self) -> bool {
        self.stop_kind() != crate::stop::NONE
    }

    /// The stop asked of this run: `stop::CANCEL` when the run's own token is set (`ibuilder_cancel`), else the process-wide request (`stop::NONE`, `PAUSE` or `CANCEL`).
    pub fn stop_kind(&self) -> u8 {
        if self.cancel.load(Ordering::Relaxed) {
            crate::stop::CANCEL
        } else {
            crate::stop::requested()
        }
    }

    pub fn log(&self, s: &str) {
        self.log.line(s);
        crate::diag::log_line(s);
    }

    pub fn builder(docs: impl Into<PathBuf>) -> CtxBuilder {
        CtxBuilder::new(docs)
    }
}

pub struct CtxBuilder {
    docs: PathBuf,
    bundle: Option<PathBuf>,
    cfg: Config,
    env: Option<Arc<dyn Env>>,
    log: Option<Arc<dyn Log>>,
    chain_log: Option<Arc<dyn Log>>,
    mem: Option<Arc<dyn MemSource>>,
    clock: Option<Arc<dyn Clock>>,
    adapters: Option<Adapters>,
    boot_enabled: bool,
}

impl CtxBuilder {
    pub fn new(docs: impl Into<PathBuf>) -> CtxBuilder {
        CtxBuilder {
            docs: docs.into(),
            bundle: None,
            cfg: Config::new(),
            env: None,
            log: None,
            chain_log: None,
            mem: None,
            clock: None,
            adapters: None,
            boot_enabled: false,
        }
    }
    pub fn bundle(mut self, b: impl Into<PathBuf>) -> Self {
        self.bundle = Some(b.into());
        self
    }
    pub fn config(mut self, c: Config) -> Self {
        self.cfg = c;
        self
    }
    pub fn env(mut self, e: Arc<dyn Env>) -> Self {
        self.env = Some(e);
        self
    }
    pub fn log(mut self, l: Arc<dyn Log>) -> Self {
        self.log = Some(l);
        self
    }
    pub fn chain_log(mut self, l: Arc<dyn Log>) -> Self {
        self.chain_log = Some(l);
        self
    }
    pub fn mem(mut self, m: Arc<dyn MemSource>) -> Self {
        self.mem = Some(m);
        self
    }
    pub fn clock(mut self, c: Arc<dyn Clock>) -> Self {
        self.clock = Some(c);
        self
    }
    pub fn adapters(mut self, a: Adapters) -> Self {
        self.adapters = Some(a);
        self
    }
    pub fn boot_enabled(mut self, on: bool) -> Self {
        self.boot_enabled = on;
        self
    }

    /// Defaults suitable for tests and tools: a `MapEnv`, a `MemLog`, a `FakeMem` with no ceiling, the real clock, unwired adapters.
    pub fn build(self) -> Ctx {
        let docs = Docs::new(self.docs);
        let env: Arc<dyn Env> = self.env.unwrap_or_else(|| Arc::new(MapEnv::new()));
        let log: Arc<dyn Log> = self.log.unwrap_or_else(|| MemLog::new());
        let chain_log: Arc<dyn Log> = self.chain_log.unwrap_or_else(|| MemLog::new());
        let mem: Arc<dyn MemSource> = self.mem.unwrap_or_else(|| Arc::new(FakeMem::new(0)));
        let clock: Arc<dyn Clock> = self.clock.unwrap_or_else(|| RealClock::new());
        let thermal = Arc::new(ThermalGate::new(
            &docs.thermal(),
            log.clone(),
            clock.clone(),
        ));
        let timing = Timings::new(
            &docs.build_times(),
            &docs.timing_partial(),
            &device_model(&*env),
        );
        let shader_packs = docs.shaders();
        let live = LiveQueue::new(env.clone());
        Ctx {
            bundle: self.bundle.unwrap_or_else(|| docs.root().to_path_buf()),
            docs,
            cfg: self.cfg,
            env,
            log,
            chain_log,
            mem,
            clock,
            thermal,
            timing,
            stats: BuildStats::new(),
            rewrite_ran: Vec::new(),
            shader_packs,
            cancel: Arc::new(AtomicBool::new(false)),
            adapters: self.adapters.unwrap_or_else(Adapters::unwired),
            live,
            boot_enabled: self.boot_enabled,
            status: BTreeMap::new(),
        }
    }
}
