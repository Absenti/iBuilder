//! `shaderbuild`: the stage 3 build driver on the Mac (the engine's native tools as child processes).
//!
//! ```text
//! shaderbuild build --pkg <split pkg dir> --profile <profile dir (<id>/game.tsv inside)> --game <title id> --out <dir> --chain <chain id>
//!                   [--keys <file of 16-hex keys>] [--jobs N] [--tables 0|1] [--pulling 0|1] [--cas <dir>]
//!                   [--engine <engine dir> | --xlate <bin> --spirv-cross <bin>] [--finisher mock|replay:<library image>|rust]
//!                   [--tool-memo 0|1] [--finisher-salt <text>]
//! ```
//! `--tool-memo 1`: the second cache level (`toolmemo`, the tools' answers); `--finisher-salt`: a text added to the finisher's fingerprint, which is what a change of the finishing code does to
//! every shader's digest (measuring only: a build after a "finishing change" without changing the code).
//! ```text
//! shaderbuild live  --entries <dir of <key>.ctl/<key>.code> --pack <pack dir> --derived <profile table dir> --game <id> --work <dir> --chain <chain> [--cas <dir>] [--engine ..] [--finisher ..]
//! ```
//! Prints the engine's progress lines to stderr, and at the end the report (wall time, shader counts, cache hits, tool time, peak RSS).
use ibuilder_shaderbuild::build::{BuildEnv, BuildOptions, build};
use ibuilder_shaderbuild::finish::{Finished, Finisher, MockFinisher, Partners, ReplayFinisher};
use ibuilder_shaderbuild::live::{LiveEntry, live_translate};
use ibuilder_shaderbuild::tools::ProcessTools;
use ibuilder_shaderbuild::tools::ShaderTools;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn die(msg: &str) -> ! {
    eprintln!("shaderbuild: {msg}");
    std::process::exit(2);
}

fn peak_rss_bytes() -> u64 {
    // ru_maxrss is bytes on macOS
    let mut ru = std::mem::MaybeUninit::<libc_rusage>::zeroed();
    // SAFETY: getrusage fills a plain C struct of integers; RUSAGE_SELF = 0
    unsafe {
        getrusage(0, ru.as_mut_ptr());
        ru.assume_init().ru_maxrss as u64
    }
}

#[repr(C)]
struct Timeval {
    s: i64,
    us: i32,
}
#[repr(C)]
struct libc_rusage {
    utime: Timeval,
    stime: Timeval,
    ru_maxrss: i64,
    rest: [i64; 13],
}
unsafe extern "C" {
    fn getrusage(who: i32, r: *mut libc_rusage) -> i32;
}

fn tools(args: &[String]) -> ProcessTools {
    if let Some(e) = arg(args, "--engine") {
        return ProcessTools::in_engine(&PathBuf::from(e));
    }
    match (arg(args, "--xlate"), arg(args, "--spirv-cross")) {
        (Some(x), Some(s)) => ProcessTools::new(x, s),
        _ => die("give --engine <dir> or both --xlate and --spirv-cross"),
    }
}

/// the finisher named by `--finisher`; `holder` keeps a replayed image's map alive
fn finisher<'a>(
    args: &[String],
    holder: &'a mut Option<memmap2::Mmap>,
    profile: &std::path::Path,
    game: &str,
    chain: &str,
    pulling: bool,
) -> Box<dyn Finisher + 'a> {
    let which = arg(args, "--finisher").unwrap_or_else(|| "mock".into());
    let _ = (profile, game, chain, pulling);
    if which == "mock" {
        return Box::new(MockFinisher);
    }
    if let Some(img) = which.strip_prefix("replay:") {
        let f = std::fs::File::open(img).unwrap_or_else(|e| die(&format!("{img}: {e}")));
        // SAFETY: a read-only map of a finished library image that nothing writes while this tool runs
        let m = unsafe { memmap2::Mmap::map(&f) }.unwrap_or_else(|e| die(&format!("{img}: {e}")));
        *holder = Some(m);
        return Box::new(
            ReplayFinisher::new(&holder.as_ref().unwrap()[..])
                .unwrap_or_else(|e| die(&format!("{img}: {e}"))),
        );
    }
    #[cfg(feature = "f2-finish")]
    if which == "rust" {
        return Box::new(
            ibuilder_shaderbuild::finish_adapter::RustFinisher::load(profile, game, chain, pulling)
                .unwrap_or_else(|e| die(&e)),
        );
    }
    die(&format!(
        "unknown --finisher {which} (mock, replay:<image>{})",
        if cfg!(feature = "f2-finish") {
            ", rust"
        } else {
            ""
        }
    ))
}

/// a finisher whose fingerprint has a text added (`--finisher-salt`): every shader's digest changes as after a change of the finishing code, its outputs do not
struct Salted<'a> {
    inner: Box<dyn Finisher + 'a>,
    salt: String,
}

impl Finisher for Salted<'_> {
    fn fingerprint(&self) -> String {
        format!("{}\0salt {}", self.inner.fingerprint(), self.salt)
    }
    fn finish(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        paired_locs: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        self.inner.finish(key, work, job, c1, tools, paired_locs)
    }
    fn finish_with(
        &self,
        key: &str,
        work: &Path,
        job: &mut Value,
        c1: &[u8],
        tools: &dyn ShaderTools,
        partners: &dyn Partners,
    ) -> Finished {
        self.inner.finish_with(key, work, job, c1, tools, partners)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let log = |l: &str| eprintln!("{l}");
    match args.first().map(|s| s.as_str()) {
        Some("build") => {
            let need = |n: &str| arg(&args, n).unwrap_or_else(|| die(&format!("{n} is required")));
            let (pkg, profile, game, out, chain) = (
                need("--pkg"),
                need("--profile"),
                need("--game"),
                need("--out"),
                need("--chain"),
            );
            let cas = arg(&args, "--cas")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&out).join("cas"));
            std::fs::create_dir_all(&cas)
                .unwrap_or_else(|e| die(&format!("{}: {e}", cas.display())));
            let mut o = BuildOptions::new(
                &PathBuf::from(&pkg),
                &PathBuf::from(&profile),
                &game,
                &PathBuf::from(&out),
                &chain,
                &cas,
            );
            o.jobs = arg(&args, "--jobs")
                .and_then(|s| s.parse().ok())
                .unwrap_or(6);
            o.tables = arg(&args, "--tables").map(|s| s != "0").unwrap_or(true);
            o.pulling = arg(&args, "--pulling").map(|s| s != "0").unwrap_or(false);
            o.keep_work = args.iter().any(|a| a == "--keep-work");
            o.tool_memo = arg(&args, "--tool-memo").is_some_and(|s| s != "0");
            if let Some(k) = arg(&args, "--keys") {
                let t = std::fs::read_to_string(&k).unwrap_or_else(|e| die(&format!("{k}: {e}")));
                o.keys = Some(
                    t.lines()
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect(),
                );
            }
            let pt = tools(&args);
            let mut holder = None;
            let gdir = ibuilder_shaderbuild::game::game_dir(&PathBuf::from(&profile), &game);
            let mut fin = finisher(&args, &mut holder, &gdir, &game, &chain, o.pulling);
            if let Some(salt) = arg(&args, "--finisher-salt") {
                fin = Box::new(Salted { inner: fin, salt });
            }
            let env = BuildEnv {
                tools: &pt,
                finisher: &*fin,
                log: &log,
                cancel: None,
            };
            let t = std::time::Instant::now();
            match build(&o, &env) {
                Ok(r) => {
                    eprintln!(
                        "REPORT wall {:.2} s (digests {:.2} s, library {:.2} s); shaders {} ok {} failed {:?}; cache hit {} miss {}; files {} bytes {}; c1 unproven {} cb shaders {}; xlate {} calls {:.1} s, spirv-cross {} calls {:.1} s (summed over threads); tool memo: translator jobs hit {} run {}, spirv-cross calls hit {} run {}; mesh libraries built {}, missing {}, not composed {}; peak RSS {:.0} MiB; image {}",
                        t.elapsed().as_secs_f64(),
                        r.digest_s,
                        r.library_s,
                        r.shaders,
                        r.ok,
                        r.failed,
                        r.cache_hit,
                        r.cache_miss,
                        r.files,
                        r.bytes,
                        r.c1_unproven,
                        r.cb_shaders,
                        r.translate_calls,
                        r.translate_s,
                        r.spirv_cross_calls,
                        r.spirv_cross_s,
                        r.memo_xlate_hits,
                        r.memo_xlate_misses,
                        r.memo_cross_hits,
                        r.memo_cross_misses,
                        r.mesh_built,
                        r.mesh_missing,
                        r.mesh_not_composed,
                        peak_rss_bytes() as f64 / 1048576.0,
                        r.image.display()
                    );
                }
                Err(e) => {
                    eprintln!("shaderbuild: build failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("live") => {
            let need = |n: &str| arg(&args, n).unwrap_or_else(|| die(&format!("{n} is required")));
            let (dir, pack, derived, game, work, chain) = (
                need("--entries"),
                need("--pack"),
                need("--derived"),
                need("--game"),
                need("--work"),
                need("--chain"),
            );
            let mut entries = Vec::new();
            let mut names: Vec<String> = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| die(&format!("{dir}: {e}")))
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".ctl"))
                .collect();
            names.sort();
            for n in names {
                let key = n[..n.len() - 4].to_string();
                let ctl = std::fs::read(PathBuf::from(&dir).join(&n)).unwrap();
                let code = std::fs::read(PathBuf::from(&dir).join(format!("{key}.code")))
                    .unwrap_or_else(|e| die(&format!("{key}.code: {e}")));
                entries.push(LiveEntry { key, ctl, code });
            }
            let pt = tools(&args);
            let cas = arg(&args, "--cas").map(PathBuf::from);
            let which = arg(&args, "--finisher").unwrap_or_else(|| "mock".into());
            let make = |prof: &std::path::Path, gid: &str| -> Result<Box<dyn Finisher>, String> {
                let _ = (prof, gid);
                match which.as_str() {
                    "mock" => Ok(Box::new(MockFinisher)),
                    #[cfg(feature = "f2-finish")]
                    "rust" => Ok(Box::new(
                        ibuilder_shaderbuild::finish_adapter::RustFinisher::load(
                            &ibuilder_shaderbuild::game::game_dir(prof, gid),
                            gid,
                            &chain,
                            false,
                        )?,
                    )),
                    other => Err(format!(
                        "live: unknown --finisher {other} (mock{})",
                        if cfg!(feature = "f2-finish") {
                            ", rust"
                        } else {
                            ""
                        }
                    )),
                }
            };
            let t = std::time::Instant::now();
            match live_translate(
                &entries,
                &PathBuf::from(&pack),
                &PathBuf::from(&derived),
                &game,
                &PathBuf::from(&work),
                &chain,
                &pt,
                &make,
                &log,
                cas.as_deref(),
            ) {
                Ok(r) => eprintln!(
                    "LIVE {:.2} s: ok {:?} bad {:?} files {}",
                    t.elapsed().as_secs_f64(),
                    r.ok,
                    r.bad,
                    r.files.len()
                ),
                Err(e) => {
                    eprintln!("shaderbuild: live translation failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            die("usage: shaderbuild build|live ... (see the file header of src/bin/shaderbuild.rs)")
        }
    }
}
