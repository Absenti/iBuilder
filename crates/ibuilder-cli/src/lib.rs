//! # ibuilder-cli
//!
//! The Mac command line over the chain driver: `ibuilder --exefs <ExeFS dir> --out <dir> ...` runs profile -> rewrite -> assemble -> sign on an already extracted game, through
//! the real stage crates (`adapters`), or on stand-in workers with `--mock`. With `--shaders --shader-chain ID --engine DIR` (a build with the `shader-host-tools` feature) stage 3
//! runs too, between the rewrite and the assembly: the split and the build of `ibuilder-shadersplit` / `ibuilder-shaderbuild` through the pipeline's own stage 3
//! (`ibuilder_pipeline::shaders`: work folder and resume, gates, cache, library pack), the engine's native `dmsh_xlate` and `spirv-cross` as child processes. Per-step wall time and
//! peak memory are measured (`meter`) and printed. The library half is also the adapter set the FFI crate uses for the app (`adapters::real_adapters`).

pub mod adapters;
pub mod meter;

/// The big-block allocator (`ibuilder_alloc`), re-exported so that a host that links this crate (the FFI archive) can install it with one line and no new dependency:
/// `#[global_allocator] static GLOBAL: ibuilder_cli::alloc::BigBlocks = ibuilder_cli::alloc::BigBlocks;` (docs/chain/MEMORY-RUST.md section 6).
pub use ibuilder_alloc as alloc;

use ibuilder_pipeline::adapters::Adapters;
use ibuilder_pipeline::chain::{ChainReport, banner, load_config, run_chain};
use ibuilder_pipeline::config::Config;
use ibuilder_pipeline::ctx::Ctx;
use ibuilder_pipeline::env::MapEnv;
use ibuilder_pipeline::log::{FileLog, Log, NullLog, TeeLog};
use ibuilder_pipeline::memory::MemSource;
use ibuilder_pipeline::sys::mem::SystemMem;
use meter::{Meter, Segment};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What `ibuilder` was asked to do.
#[derive(Clone, Debug, Default)]
pub struct CliArgs {
    /// the game's ExeFS folder (read only): `main`, `sdk`, `subsdk0`, ...
    pub exefs: PathBuf,
    /// the output folder, laid out like the app's Documents (`extract/rewrite`, `extract/flat`, `DmGameProbe.framework/`, `profile.tsv`, the logs)
    pub out: PathBuf,
    /// the engine's folder with `dm/games/<id>/game.tsv` (registered games), and `dm/host` for a provisional profile
    pub bundle: Option<PathBuf>,
    /// the prebuilt host dylib the assembler grafts the game onto
    pub host: Option<PathBuf>,
    /// RomFS.img (default `<exefs>/../RomFS.img`), control.nacp and icon.jpg (default beside it)
    pub romfs: Option<PathBuf>,
    pub control: Option<PathBuf>,
    /// the modules to rewrite (default: the profile's)
    pub modules: Option<String>,
    /// rewrite threads for the big module (default: the pipeline's memory/thermal decision)
    pub threads: Option<usize>,
    pub app_build: String,
    pub identifier: String,
    /// stand-in workers instead of the real crates
    pub mock: bool,
    /// print every pipeline log line to stderr (otherwise only the stage lines and failures)
    pub verbose: bool,
    /// write the step table as TSV here
    pub report: Option<PathBuf>,
    /// run stage 3 (shaders): the split of the game's RomFS and `main`, then the build of the library the host takes (`shader_chain`). Needs `shader_chain` and, outside `--mock`,
    /// `engine` and a build with the `shader-host-tools` feature (the engine's tools run as child processes: never in the app)
    pub shaders: bool,
    /// the host's shader chain (`b-<chain>.img` is the library it takes): the app build's `shader_chain=`
    pub shader_chain: Option<String>,
    /// the engine folder holding `dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate` and `dm/gfx/tools/spirv_cross_bin/spirv-cross` (only run, never written)
    pub engine: Option<PathBuf>,
    /// threads of stage 3 (config `shader_jobs`; default: the pipeline's decision: min(6, cores), one after an interrupted build, fewer when memory or heat ask)
    pub shader_jobs: Option<usize>,
    /// extra config rows `KEY=VALUE` (`--set`), applied last: `shader_cache_gib`, `shader_free_gib`, `shader_pull`, ...
    pub set: Vec<(String, String)>,
    /// a Switch 2 `.nup` container (read only) instead of `exefs`: the chain's stage 1 reads it (`ibuilder-nup`) into `<out>/extract` (ExeFS, RomFS.img, control data), then the rest runs
    pub nup: Option<PathBuf>,
}

/// The two native tools inside an engine folder: `<engine>/dm/gfx/tools/{dmsh_xlate_bin/dmsh_xlate, spirv_cross_bin/spirv-cross}`.
pub fn engine_tool_paths(engine: &Path) -> (PathBuf, PathBuf) {
    let t = engine.join("dm").join("gfx").join("tools");
    (
        t.join("dmsh_xlate_bin").join("dmsh_xlate"),
        t.join("spirv_cross_bin").join("spirv-cross"),
    )
}

/// What `--shaders` needs, checked before anything is written: the chain, and (outside `--mock`) the engine's tools and a build that can run them.
pub fn check_shader_args(a: &CliArgs) -> Result<(), String> {
    let loose = |what: &str| format!("{what} needs --shaders");
    if !a.shaders {
        for (given, name) in [
            (a.shader_chain.is_some(), "--shader-chain"),
            (a.engine.is_some(), "--engine"),
            (a.shader_jobs.is_some(), "--shader-jobs"),
        ] {
            if given {
                return Err(loose(name));
            }
        }
        return Ok(());
    }
    let chain = a.shader_chain.as_deref().unwrap_or("");
    if chain.is_empty() {
        return Err("--shaders needs --shader-chain ID (the shader chain of the host: its library is b-<chain>.img; 6c53446391cc for the reference host)".into());
    }
    if !chain.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "--shader-chain {chain}: a chain id is letters and digits only (it names files)"
        ));
    }
    if a.shader_jobs == Some(0) {
        return Err("--shader-jobs needs a positive number".into());
    }
    if a.mock {
        return Ok(()); // the stand-in backend carries its own tools
    }
    if !cfg!(feature = "shader-host-tools") {
        return Err("--shaders runs the engine's dmsh_xlate and spirv-cross as child processes: build the CLI with --features shader-host-tools (the app never has them: it passes callbacks)".into());
    }
    if !cfg!(feature = "shader-finish") || !cfg!(feature = "shadersplit") {
        return Err("--shaders needs the per-shader finish and the split in this build (cargo features shader-finish and shadersplit, on by default)".into());
    }
    let Some(engine) = &a.engine else {
        return Err("--shaders needs --engine DIR (the engine folder holding dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate and dm/gfx/tools/spirv_cross_bin/spirv-cross)".into());
    };
    for tool in [engine_tool_paths(engine).0, engine_tool_paths(engine).1] {
        if !tool.is_file() {
            return Err(format!(
                "--engine {}: {} is not there",
                engine.display(),
                tool.display()
            ));
        }
    }
    Ok(())
}

pub struct CliReport {
    pub chain: ChainReport,
    pub segments: Vec<Segment>,
    pub wall_s: f64,
    /// the process high-water mark (ru_maxrss), bytes
    pub peak_rss: u64,
}

/// The process exit code of a run that reached the chain: 0 every stage ok (stage 3 `none (...)` included: nothing to build), 1 a stage failed (one that stops the chain, or one that does not:
/// stage 3 with no library, the chain's 4; the assembly or the signature, the chain's 5), 75 the chain ended on purpose to be run again (memory before stage 3), 130 cancelled, 131 paused. The
/// chain's 2, 3, 4 and 5 are 1 here: 2 is the CLI's usage error (review r3 §3.1, §5.3, r4 §4.1).
pub fn exit_code(chain: &ChainReport) -> u8 {
    use ibuilder_pipeline::chain::{RC_CANCELLED, RC_OK, RC_PAUSED, RC_RESTART};
    match chain.rc {
        RC_OK if chain.all_ok() => 0,
        RC_RESTART => RC_RESTART as u8,
        RC_CANCELLED => RC_CANCELLED as u8,
        RC_PAUSED => RC_PAUSED as u8,
        _ => 1,
    }
}

/// `dst` (an entry of `<out>/extract`) becomes a link to `src`, or is cleared when this run has no such input (`src` is `None`). A link an earlier run left there is ours: it is replaced, or removed, so
/// the chain never reads an earlier run's input. A REAL file or folder is not ours: an earlier `--nup` run extracted it (or someone put it there), and linking over it, or leaving it and reading it as
/// if it were this run's input, would silently build the wrong game (review finding 4.10). It is refused, with the way out, unless it already IS the requested input.
fn link(src: Option<&Path>, dst: &Path) -> std::io::Result<()> {
    if let Ok(m) = fs::symlink_metadata(dst) {
        if m.file_type().is_symlink() {
            fs::remove_file(dst)?;
        } else if src.is_some_and(|s| fs::canonicalize(dst).is_ok_and(|d| d == s)) {
            return Ok(()); // the input is the folder the chain extracted before (--exefs <out>/extract/ExeFS)
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "{} is a real {} left by an earlier run (an --nup extraction, or another game's files), not a link to this run's input{}: remove it, or use another --out",
                    dst.display(),
                    if m.is_dir() { "folder" } else { "file" },
                    src.map(|s| format!(" {}", s.display())).unwrap_or_default()
                ),
            ));
        }
    }
    match src {
        Some(s) => std::os::unix::fs::symlink(s, dst),
        None => Ok(()),
    }
}

/// the four entries of `<out>/extract` this module lays out
const EXTRACT_ENTRIES: [&str; 4] = ["ExeFS", "RomFS.img", "control.nacp", "icon.jpg"];

/// Lay the inputs out like the app's extraction (`<out>/extract/{ExeFS,RomFS.img,control.nacp,icon.jpg}`) with symlinks: nothing is copied and the inputs stay read only.
pub fn prepare_extract(a: &CliArgs) -> std::io::Result<PathBuf> {
    let extract = a.out.join("extract");
    fs::create_dir_all(&extract)?;
    if a.nup.is_some() {
        // the chain's stage 1 reads the container into it: nothing to link. A link an earlier `--exefs` run left would make the extraction write THROUGH it into that run's input: removed
        // (only links: what is really there is overwritten by the extraction itself)
        for n in EXTRACT_ENTRIES {
            let p = extract.join(n);
            if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_file(&p)?;
            }
        }
        return Ok(extract);
    }
    let abs = |p: &Path| fs::canonicalize(p);
    let beside = a.exefs.parent().map(Path::to_path_buf).unwrap_or_default();
    // an input the user named must be there (review r3 §5.1: it was left out in silence, and the chain ran without it); the default beside the ExeFS may be missing
    let pick =
        |explicit: &Option<PathBuf>, flag: &str, name: &str| -> std::io::Result<Option<PathBuf>> {
            match explicit {
                Some(p) if p.exists() => Ok(Some(p.clone())),
                Some(p) => Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{flag} {}: no such file or folder", p.display()),
                )),
                None => Ok(Some(beside.join(name)).filter(|p| p.exists())),
            }
        };
    let romfs = pick(&a.romfs, "--romfs", "RomFS.img")?
        .map(|r| abs(&r))
        .transpose()?;
    let control = pick(&a.control, "--control", "control.nacp")?
        .map(|c| abs(&c))
        .transpose()?;
    link(Some(&abs(&a.exefs)?), &extract.join("ExeFS"))?;
    link(romfs.as_deref(), &extract.join("RomFS.img"))?;
    link(control.as_deref(), &extract.join("control.nacp"))?;
    let icon = Some(beside.join("icon.jpg"))
        .filter(|p| p.exists())
        .map(|i| abs(&i))
        .transpose()?;
    link(icon.as_deref(), &extract.join("icon.jpg"))?;
    Ok(extract)
}

/// The config the chain reads, as the app's `ConfigText` writes it, for a CLI run.
pub fn make_config(a: &CliArgs) -> Config {
    let out = &a.out;
    let extract = out.join("extract");
    let mut c = Config::new();
    c.set("extract_out", extract.to_string_lossy());
    c.set("exe_dir", extract.join("ExeFS").to_string_lossy());
    c.set(
        "modules",
        a.modules
            .clone()
            .unwrap_or_else(|| "main subsdk0 sdk".into()),
    );
    c.set("patched_dir", extract.join("rewrite").to_string_lossy());
    if let Some(h) = &a.host {
        c.set("host_dylib", h.to_string_lossy());
    }
    c.set(
        "framework_out",
        out.join("DmGameProbe.framework")
            .join("DmGameProbe")
            .to_string_lossy(),
    );
    c.set("profile", out.join("profile.tsv").to_string_lossy());
    c.set("romfs", extract.join("RomFS.img").to_string_lossy());
    c.set("boot_seconds", "0");
    c.set("boot_after_build", "0");
    c.set("keep_exefs", "1"); // the CLI never deletes its inputs
    c.set(
        "app_build",
        if a.app_build.is_empty() {
            "cli".to_string()
        } else {
            a.app_build.clone()
        },
    );
    c.set(
        "game",
        a.exefs
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    if let Some(nup) = &a.nup {
        // stage 1 reads the container (the config key `nup`, as the app's ConfigText writes it for a .nup in Documents)
        c.set(
            "nup",
            fs::canonicalize(nup)
                .unwrap_or_else(|_| nup.clone())
                .to_string_lossy(),
        );
        c.set(
            "game",
            nup.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    if let Some(t) = a.threads {
        c.set("rewrite_jobs", t.to_string());
    }
    if a.shaders {
        // stage 3 reads these: `shaders_will_build` / `build_shaders_on_device` (the chain, the work folder's inputs `romfs` and `exe_dir/main` are set above)
        c.set("shader_chain", a.shader_chain.clone().unwrap_or_default());
        if let Some(j) = a.shader_jobs {
            c.set("shader_jobs", j.to_string());
        }
    }
    for (k, v) in &a.set {
        c.set(k.as_str(), v.clone());
    }
    c
}

/// Run the chain for `a` and measure it.
pub fn run(a: &CliArgs) -> Result<CliReport, String> {
    check_shader_args(a)?;
    fs::create_dir_all(&a.out).map_err(|e| format!("{}: {e}", a.out.display()))?;
    if let Some(nup) = &a.nup {
        if !a.exefs.as_os_str().is_empty() {
            return Err("--nup and --exefs are two ways to give the game: use one".into());
        }
        if !cfg!(feature = "nup") && !a.mock {
            return Err("--nup needs a build with the nup feature (on by default)".into());
        }
        if !nup.is_file() {
            return Err(format!("--nup {} is not a file", nup.display()));
        }
    } else if !a.exefs.is_dir() {
        return Err(format!("--exefs {} is not a folder", a.exefs.display()));
    }
    prepare_extract(a).map_err(|e| format!("laying out {}: {e}", a.out.display()))?;
    let bundle = a.bundle.clone().unwrap_or_else(|| a.out.clone());
    let identifier = if a.identifier.is_empty() {
        adapters::DEFAULT_IDENTIFIER.to_string()
    } else {
        a.identifier.clone()
    };
    #[allow(unused_mut)] // only the shader-host-tools build adds the tools
    let mut adapters: Adapters = if a.mock {
        #[cfg(feature = "mock-stages")]
        {
            ibuilder_pipeline::mock::mock_adapters("0100DFA027504000")
        }
        #[cfg(not(feature = "mock-stages"))]
        {
            return Err("--mock needs a build with the mock-stages feature".into());
        }
    } else {
        adapters::real_adapters(&identifier)
    };
    // stage 3's translator and SPIRV-Cross: the engine's own binaries as child processes (the app passes callbacks instead; this feature is never in the app)
    #[cfg(feature = "shader-host-tools")]
    if a.shaders && !a.mock {
        if let Some(engine) = &a.engine {
            let (xlate, spirv) = engine_tool_paths(engine);
            adapters.tools = Some(adapters::shaders::process_tools(&xlate, &spirv));
        }
    }
    let docs = ibuilder_pipeline::layout::Docs::new(&a.out);
    let file: Option<FileLog> = FileLog::open(&docs.stages_log(), &banner()).ok();
    let mut sinks: Vec<Arc<dyn Log>> = Vec::new();
    if let Some(f) = file {
        sinks.push(Arc::new(f));
    }
    let verbose = a.verbose;
    sinks.push(Arc::new(move |l: &str| {
        if verbose
            || l.starts_with("stage ")
            || l.contains("FAILED")
            || l.contains("[RED]")
            || l.starts_with("the chain stops")
        {
            eprintln!("{l}");
        }
    }));
    let log: Arc<dyn Log> = Arc::new(TeeLog(sinks));
    let mem: Arc<dyn MemSource> = Arc::new(SystemMem::new(&a.out));
    let mut ctx: Ctx = Ctx::builder(&a.out)
        .bundle(&bundle)
        .env(Arc::new(MapEnv::new()))
        .log(log)
        .chain_log(Arc::new(NullLog))
        .mem(mem)
        .adapters(adapters)
        .build();
    ctx.cfg = make_config(a);
    // a stale framework stamp / running marker belongs to the previous run
    let _ = fs::remove_file(docs.running());
    let meter = Meter::start();
    ctx.log(&format!(
        "ibuilder-cli: {} -> {}",
        a.exefs.display(),
        a.out.display()
    ));
    // load_config reads gameapp.config; the CLI builds its config in memory, so write it where the chain's other tools would look for it too
    let cfg_text: String = ctx.cfg.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let cfg_path = docs.config();
    fs::write(&cfg_path, cfg_text).map_err(|e| format!("{}: {e}", cfg_path.display()))?;
    load_config(&mut ctx, Some(&cfg_path));
    let chain = run_chain(&mut ctx);
    let (segments, wall_s, peak_rss) = meter.finish();
    Ok(CliReport {
        chain,
        segments,
        wall_s,
        peak_rss,
    })
}

/// The table the CLI prints: per step the wall time, the largest resident size and the largest `phys_footprint` sampled during it, and what the step left behind (resident / footprint
/// when it ended); the last row carries the high-water marks of the whole run (the resident one is `ru_maxrss`, the kernel's own).
pub fn format_report(r: &CliReport) -> String {
    let mib = |b: u64| format!("{:>8.0}", b as f64 / 1_048_576.0);
    let mut s = String::new();
    s.push_str(&format!(
        "{:<34} {:>9} {:>14} {:>14} {:>14} {:>14}\n",
        "step", "wall (s)", "peak RSS (MiB)", "peak footprint", "left: RSS", "left: footpr."
    ));
    for g in &r.segments {
        s.push_str(&format!(
            "{:<34} {:>9.2} {:>14} {:>14} {:>14} {:>14}\n",
            g.label,
            g.wall_s,
            mib(g.peak_resident),
            mib(g.peak_footprint),
            mib(g.resident_at_end),
            mib(g.footprint_at_end)
        ));
    }
    s.push_str(&format!(
        "{:<34} {:>9.2} {:>14} {:>14} {:>14} {:>14}\n",
        "total (high-water marks)",
        r.wall_s,
        mib(r
            .segments
            .iter()
            .map(|g| g.peak_resident)
            .max()
            .unwrap_or(0)
            .max(r.peak_rss)),
        mib(r
            .segments
            .iter()
            .map(|g| g.peak_footprint)
            .max()
            .unwrap_or(0)),
        "",
        ""
    ));
    s.push_str(&format!(
        "result: {} (rc {}), stages: {}\n",
        r.chain.result,
        r.chain.rc,
        r.chain
            .status
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s
}

pub fn report_tsv(r: &CliReport) -> String {
    let mut s = String::from(
        "step\twall_s\tpeak_resident_bytes\thigh_water_bytes\tpeak_footprint_bytes\tresident_at_end_bytes\tfootprint_at_end_bytes\n",
    );
    for g in &r.segments {
        s.push_str(&format!(
            "{}\t{:.3}\t{}\t{}\t{}\t{}\t{}\n",
            g.label,
            g.wall_s,
            g.peak_resident,
            g.high_water_at_end,
            g.peak_footprint,
            g.resident_at_end,
            g.footprint_at_end
        ));
    }
    s.push_str(&format!(
        "total\t{:.3}\t{}\t{}\t{}\t\t\n",
        r.wall_s,
        r.segments
            .iter()
            .map(|g| g.peak_resident)
            .max()
            .unwrap_or(0),
        r.peak_rss,
        r.segments
            .iter()
            .map(|g| g.peak_footprint)
            .max()
            .unwrap_or(0)
    ));
    s
}
