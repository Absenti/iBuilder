//! `ibuilder-rewrite <module file> <output dir> <module name>`: what `dm/tools/patch_nso.py` (through
//! `pyrewrite_driver.py`) does. The environment variables of the Python are honoured (`DM_REWRITE_J`, `DM_GAME`,
//! `DM_GAMES_DIR`, `DM_SPIN_LIST`); `--threads N`, `--game ID`, `--games-dir DIR` (repeatable) and `--py-tools DIR`
//! (write `build_src.sha256` from the engine's Python tools folder) override or add to them. The lines the Python
//! prints go to stdout; failures print to stderr and exit with 1. `--list-ported` prints the per-game patches this build can rewrite (see scripts/check-patch-coverage.sh).

use ibuilder_rewrite::{Options, rewrite};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    std::process::exit(match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    });
}

fn run() -> Result<(), String> {
    let mut opts = Options::default();
    if let Ok(j) = std::env::var("DM_REWRITE_J") {
        opts.workers = j
            .parse()
            .ok()
            .filter(|&n: &usize| n >= 1)
            .ok_or("[RED] DM_REWRITE_J must be a positive integer")?;
    }
    opts.game = std::env::var("DM_GAME").ok().filter(|g| !g.is_empty());
    opts.spin_list = std::env::var("DM_SPIN_LIST").is_ok_and(|v| !v.is_empty());
    let mut games_dirs: Vec<PathBuf> = std::env::var("DM_GAMES_DIR")
        .map(|v| {
            v.split(':')
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    let mut pos: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--threads" => {
                opts.workers = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--threads needs a number")?
            }
            "--game" => opts.game = Some(it.next().ok_or("--game needs a value")?),
            "--games-dir" => {
                games_dirs.push(PathBuf::from(it.next().ok_or("--games-dir needs a value")?))
            }
            "--py-tools" => {
                opts.py_tools = Some(PathBuf::from(it.next().ok_or("--py-tools needs a value")?))
            }
            "--spin-list" => opts.spin_list = true,
            "--no-relayout" => opts.no_relayout = true,
            // what this build can rewrite, in the format of `tools/gen_patch_tables.py --coverage` (scripts/check-patch-coverage.sh compares the two)
            "--list-ported" => {
                let mut so = std::io::stdout().lock();
                writeln!(so, "# kind\tprofile or file\tkey\tcode digest\tgame")
                    .map_err(|e| e.to_string())?;
                for r in ibuilder_rewrite::stubs_games::coverage_rows() {
                    writeln!(so, "{}", r.join("\t")).map_err(|e| e.to_string())?;
                }
                return Ok(());
            }
            _ => pos.push(a),
        }
    }
    opts.games_dirs = games_dirs;
    if pos.len() != 3 {
        return Err("usage: ibuilder-rewrite [--threads N] [--game ID] [--games-dir DIR] [--py-tools DIR] [--spin-list] [--no-relayout] <module file> <output dir> <module name> | ibuilder-rewrite --list-ported".into());
    }
    let (src, outdir, name) = (&pos[0], PathBuf::from(&pos[1]), &pos[2]);
    let n = if opts.workers == 0 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    } else {
        opts.workers
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(n)
        .build()
        .map_err(|e| e.to_string())?;
    let file = std::fs::File::open(src).map_err(|e| format!("{src}: {e}"))?;
    // SAFETY: the map is only read while the rewrite runs
    let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("{src}: {e}"))?;
    let result = pool.install(|| rewrite(&map, src, name, &opts));
    let r = match result {
        Ok(r) => r,
        Err(e) => {
            // lines printed before the failure
            return Err(e.msg);
        }
    };
    let stdout = std::io::stdout();
    let mut so = stdout.lock();
    for l in &r.log {
        writeln!(so, "{l}").map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&outdir).map_err(|e| format!("{}: {e}", outdir.display()))?;
    let p = |ext: &str| outdir.join(format!("{name}.{ext}"));
    let w = |path: PathBuf, s: &str| {
        std::fs::write(&path, s).map_err(|e| format!("{}: {e}", path.display()))
    };
    {
        let f = std::fs::File::create(p("patched.bin")).map_err(|e| e.to_string())?;
        let mut bw = std::io::BufWriter::with_capacity(1 << 20, f);
        r.write_patched(&mut bw).map_err(|e| e.to_string())?;
        bw.flush().map_err(|e| e.to_string())?;
    }
    if let Some(b) = &r.build_src {
        w(outdir.join("build_src.sha256"), b)?;
    }
    w(p("fixups.tsv"), &r.fixups)?;
    w(p("gates.tsv"), &r.gates)?;
    w(p("sites.tsv"), &r.sites)?;
    w(p("cuts.tsv"), &r.cuts)?;
    w(p("changed.tsv"), &r.changed)?;
    w(p("obs.tsv"), &r.obs)?;
    if let Some(m) = &r.match_tsv {
        w(p("match.tsv"), m)?;
    }
    Ok(())
}
