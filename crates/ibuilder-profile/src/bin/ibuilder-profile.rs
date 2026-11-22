//! `ibuilder-profile`: derive the per-game runtime profile from the dump, compare profiles, show the facts.
//!
//!   ibuilder-profile derive <extract dir> [out profile.tsv] [profile_out dir]  [--bundle B] [--romfs F] [--hooks-file F]
//!       == `GAMEAPP_BUNDLE=B python3 gameapp/profile_derive.py <extract> <out.tsv> [<profile_out>]`
//!   ibuilder-profile compare <app game.tsv> <orchestrator game.tsv> [<app profile.tsv> <harness profile.tsv>]
//!       == `python3 gameapp/compare_profile.py ...`
//!   ibuilder-profile facts <ExeFS dir> <romfs_manifest.tsv | RomFS.img | RomFS dir> [--bundle B] [--hooks-file F]
//!       == `python3 dm/games/profile_guess.py <ExeFS> <romfs>`
//!   ibuilder-profile render <game dir | title ID> <out profile.tsv> [--bundle B]
//!       == `python3 dm/games/game.py profile <id> <out>`
//!   ibuilder-profile hooks [--bundle B]       the host hook names, one per line (for --hooks-file)
//!
//! `--bundle` (or `GAMEAPP_BUNDLE`) is the engine layout: `<B>/dm/games`, `<B>/dm/host`, `<B>/dm/gfx`.
use ibuilder_profile::HookSet;
use ibuilder_profile::compare::main_text;
use ibuilder_profile::derive::{DeriveConfig, derive};
use ibuilder_profile::facts::facts_of;
use ibuilder_profile::game::{Game, find_dir, write_profile};
use ibuilder_profile::guess::{guess, report};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: ibuilder-profile derive <extract> [out.tsv] [profile_out] [--bundle B] [--romfs F] [--hooks-file F]\n       ibuilder-profile compare <app game.tsv> <orchestrator game.tsv> [<app profile.tsv> <harness profile.tsv>]\n       ibuilder-profile facts <ExeFS> <romfs> [--bundle B] [--hooks-file F]\n       ibuilder-profile render <game dir|id> <out.tsv> [--bundle B]\n       ibuilder-profile hooks [--bundle B]"
    );
    ExitCode::from(2)
}

struct Opts {
    pos: Vec<String>,
    bundle: Option<PathBuf>,
    romfs: Option<PathBuf>,
    hooks_file: Option<PathBuf>,
}

fn parse(args: Vec<String>) -> Option<Opts> {
    let mut o = Opts {
        pos: vec![],
        bundle: std::env::var_os("GAMEAPP_BUNDLE").map(PathBuf::from),
        romfs: None,
        hooks_file: None,
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--bundle" => o.bundle = Some(it.next()?.into()),
            "--romfs" => o.romfs = Some(it.next()?.into()),
            "--hooks-file" => o.hooks_file = Some(it.next()?.into()),
            _ => o.pos.push(a),
        }
    }
    Some(o)
}

fn hooks(o: &Opts) -> Result<HookSet, ibuilder_profile::Error> {
    if let Some(f) = &o.hooks_file {
        return Ok(HookSet::parse_list(&std::fs::read_to_string(f)?));
    }
    let b = o.bundle.clone().ok_or_else(|| {
        ibuilder_profile::Error::Format(
            "--bundle (or GAMEAPP_BUNDLE) is required to read the host hooks".into(),
        )
    })?;
    HookSet::from_host_dir(&b.join("dm").join("host"), false)
}

fn run(sub: &str, args: Vec<String>) -> Result<ExitCode, ibuilder_profile::Error> {
    let Some(o) = parse(args) else {
        return Ok(usage());
    };
    match sub {
        "derive" => {
            let Some(extract) = o.pos.first() else {
                return Ok(usage());
            };
            let bundle = o.bundle.clone().ok_or_else(|| {
                ibuilder_profile::Error::Format("--bundle (or GAMEAPP_BUNDLE) is required".into())
            })?;
            let mut cfg = DeriveConfig::new(extract, bundle);
            cfg.profile = o.pos.get(1).map(PathBuf::from);
            cfg.profile_out = o.pos.get(2).map(PathBuf::from);
            cfg.romfs = o.romfs.clone();
            if o.hooks_file.is_some() {
                cfg.hooks = Some(hooks(&o)?);
            }
            let out = derive(&cfg, &mut |m| println!("{m}"))?;
            println!("{}", out.display());
            Ok(ExitCode::SUCCESS)
        }
        "compare" => {
            if o.pos.len() < 2 {
                return Ok(usage());
            }
            print!("{}", main_text(&o.pos)?);
            Ok(ExitCode::SUCCESS)
        }
        "facts" => {
            if o.pos.len() != 2 {
                return Ok(usage());
            }
            let f = facts_of(o.pos[0].as_ref(), o.pos[1].as_ref())?;
            let g = guess(&f, &hooks(&o)?)?;
            print!("{}", report(&f, &g));
            Ok(ExitCode::SUCCESS)
        }
        "render" => {
            if o.pos.len() != 2 {
                return Ok(usage());
            }
            let games = o
                .bundle
                .clone()
                .map(|b| b.join("dm").join("games"))
                .unwrap_or_default();
            let g = Game::load_dir(&find_dir(&o.pos[0], &games, &[], false))?;
            let (rows, warnings) = write_profile(&g, o.pos[1].as_ref())?;
            for w in warnings {
                eprintln!("{w}");
            }
            println!("{}", o.pos[1]);
            eprintln!("{rows} rows");
            Ok(ExitCode::SUCCESS)
        }
        "hooks" => {
            print!("{}", hooks(&o)?.to_list());
            Ok(ExitCode::SUCCESS)
        }
        _ => Ok(usage()),
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return usage();
    }
    let sub = args.remove(0);
    match run(&sub, args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
