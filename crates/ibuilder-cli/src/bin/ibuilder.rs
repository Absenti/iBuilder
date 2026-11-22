//! `ibuilder`: run the chain (profile -> rewrite -> [shaders] -> assemble -> sign) on an already extracted game.

use ibuilder_cli::{CliArgs, adapters::wired, exit_code, format_report, report_tsv, run};
use std::path::PathBuf;
use std::process::ExitCode;

/// Big blocks (128 KiB and up) are mapped and unmapped, so a stage's buffers leave the footprint when it ends (docs/chain/MEMORY-RUST.md); the rest is the system allocator.
#[cfg(not(any(feature = "memprofile", feature = "system-alloc")))]
#[global_allocator]
static GLOBAL: ibuilder_alloc::BigBlocks = ibuilder_alloc::BigBlocks;

/// `--features memprofile`: the same big-block rule, counted and sampled (`ibuilder_alloc::prof`), and a report at the end
#[cfg(feature = "memprofile")]
#[global_allocator]
static GLOBAL: ibuilder_alloc::prof::Prof = ibuilder_alloc::prof::Prof;

const USAGE: &str = "usage: ibuilder (--exefs <ExeFS dir> | --nup <game.nup>) --out <dir> [options]

  --exefs DIR       the game's ExeFS (main, sdk, subsdk0, ...): read only
  --nup FILE        a Switch 2 game in a .nup container (read only) instead of --exefs: stage 1 reads it into <out>/extract (ExeFS, RomFS.img written straight from the container,
                    control.nacp, icon.jpg; its DLC into <out>/games/<title id>/aoc), every file checked against its SHA-256, then the rest of the chain runs
  --out DIR         output folder, laid out like the app's Documents: extract/rewrite/<module>/, extract/flat/, DmGameProbe.framework/DmGameProbe, profile.tsv, logs
  --bundle DIR      the engine folder holding dm/games/<title id>/game.tsv (registered games) and dm/host (provisional profiles)   [default: --out]
  --host FILE       the prebuilt host dylib the assembler grafts the game onto (host-prebuilt.dylib)
  --romfs FILE      RomFS.img / romfs_manifest.tsv / folder        [default: <exefs>/../RomFS.img]  (--shaders reads the real RomFS.img)
  --control FILE    control.nacp                                    [default: <exefs>/../control.nacp]
  --modules LIST    modules to rewrite, e.g. \"main subsdk0 sdk\"      [default: the profile's]
  --threads N       rewrite threads for the big module               [default: min(4, cores), less when memory or heat ask]
  --app-build ID    the id stamped on the framework                  [default: cli]
  --identifier ID   the CodeDirectory identifier of the signature    [default: dev.ibuilder.app]
  --shaders         also run stage 3 (shaders: the split of the RomFS and main, then the library the host takes), between the rewrite and the assembly.
                    Needs --shader-chain and --engine, and a build with --features shader-host-tools (the engine's tools run as child processes: Mac only)
  --shader-chain ID the host's shader chain: the library is b-<ID>.img (the app build's shader_chain)       [required with --shaders]
  --engine DIR      the engine folder holding dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate and dm/gfx/tools/spirv_cross_bin/spirv-cross (run, never written)   [required with --shaders]
  --shader-jobs N   stage 3 threads                                  [default: min(6, cores); 1 after an interrupted build, fewer when memory or heat ask]
  --set KEY=VALUE   one more row of the chain's config (repeatable), e.g. shader_cache_gib=4, shader_free_gib=4, shader_pull=1, shader_tables=0
  --mock            stand-in stage workers (no game data needed; build with --features mock-stages)
  --report FILE     write the per-step wall/memory table as TSV
  -v                print every pipeline log line to stderr
  --version, -h

exit codes: 0 every stage ok, 1 a stage failed (stage 3 with no library included; the chain still ran to the end), 2 usage, 75 ended to be run again (memory before stage 3),
            130 cancelled, 131 paused";

fn parse() -> Result<CliArgs, String> {
    let mut a = CliArgs::default();
    let mut it = std::env::args().skip(1);
    let need = |name: &str, it: &mut dyn Iterator<Item = String>| {
        it.next().ok_or_else(|| format!("{name} needs a value"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--exefs" => a.exefs = need("--exefs", &mut it)?.into(),
            "--out" => a.out = need("--out", &mut it)?.into(),
            "--nup" => a.nup = Some(PathBuf::from(need("--nup", &mut it)?)),
            "--bundle" => a.bundle = Some(PathBuf::from(need("--bundle", &mut it)?)),
            "--host" => a.host = Some(PathBuf::from(need("--host", &mut it)?)),
            "--romfs" => a.romfs = Some(PathBuf::from(need("--romfs", &mut it)?)),
            "--control" => a.control = Some(PathBuf::from(need("--control", &mut it)?)),
            "--modules" => a.modules = Some(need("--modules", &mut it)?),
            "--threads" => {
                a.threads = Some(
                    need("--threads", &mut it)?
                        .parse()
                        .map_err(|_| "--threads needs a number".to_string())?,
                )
            }
            "--app-build" => a.app_build = need("--app-build", &mut it)?,
            "--identifier" => a.identifier = need("--identifier", &mut it)?,
            "--report" => a.report = Some(PathBuf::from(need("--report", &mut it)?)),
            "--shaders" => a.shaders = true,
            "--shader-chain" => a.shader_chain = Some(need("--shader-chain", &mut it)?),
            "--engine" => a.engine = Some(PathBuf::from(need("--engine", &mut it)?)),
            "--shader-jobs" => {
                a.shader_jobs = Some(
                    need("--shader-jobs", &mut it)?
                        .parse()
                        .map_err(|_| "--shader-jobs needs a number".to_string())?,
                )
            }
            "--set" => {
                let kv = need("--set", &mut it)?;
                let (k, v) = kv
                    .split_once('=')
                    .filter(|(k, _)| !k.is_empty())
                    .ok_or_else(|| format!("--set needs KEY=VALUE, got {kv}"))?;
                a.set.push((k.to_string(), v.to_string()));
            }
            "--mock" => a.mock = true,
            "-v" | "--verbose" => a.verbose = true,
            "--version" => {
                println!(
                    "ibuilder {} (stage workers: {})",
                    env!("CARGO_PKG_VERSION"),
                    wired().join(", ")
                );
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if (a.exefs.as_os_str().is_empty() && a.nup.is_none()) || a.out.as_os_str().is_empty() {
        return Err("--exefs (or --nup) and --out are required".into());
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ibuilder: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    #[cfg(feature = "memprofile")]
    ibuilder_alloc::prof::set_big_threshold(ibuilder_alloc::BIG_BLOCK);
    #[cfg(feature = "memprofile")]
    let prof_run = ibuilder_alloc::prof::Run::start();
    let outcome = run(&args);
    #[cfg(feature = "memprofile")]
    print!("\n{}", prof_run.finish(30, 8, true));
    match outcome {
        Ok(r) => {
            print!("{}", format_report(&r));
            if let Some(p) = &args.report
                && let Err(e) = std::fs::write(p, report_tsv(&r))
            {
                eprintln!("ibuilder: {}: {e}", p.display());
            }
            ExitCode::from(exit_code(&r.chain))
        }
        Err(e) => {
            eprintln!("ibuilder: {e}");
            ExitCode::from(2)
        }
    }
}
