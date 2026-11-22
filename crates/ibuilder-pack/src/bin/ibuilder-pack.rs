//! `ibuilder-pack`: `pack_nxgame.py` and `compare_nxgame.py` in one binary.
//!
//!   ibuilder-pack pack --out <dir> [--profile <profile.tsv>] [--framework <dylib|dir>] --package-version v0 ...
//!   ibuilder-pack compare <reference> <candidate> [--reason KEY=TEXT]... [--json-out FILE] [--label TEXT]
//!
//! Options are the Python's (long names, `--opt value` or `--opt=value`; argparse prefix abbreviations are not
//! accepted). Exit status 1 on a refusal (`[RED] ...`) or an unexplained manifest difference, 2 on usage errors.
use ibuilder_pack::{PackArgs, compare_files_cli, pack};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage(msg: &str) -> ExitCode {
    if !msg.is_empty() {
        eprintln!("ibuilder-pack: error: {msg}");
    }
    eprintln!(
        "usage: ibuilder-pack pack --out <dir> [--profile F] [--framework F|D] [--framework-name N] [--dm-dir D] [--shader-map F]\n\
         \x20                       [--launcher F] [--msl F] [--icon F] [--extra SRC=DEST]... [--romfs-img F] [--aoc D] [--nro-table F]\n\
         \x20                       [--game ID] [--name N] [--display-version V] [--package-version V] [--modules \"a b\"] [--romfs-marker P]\n\
         \x20                       [--romfs-identity S] [--chain b-x] [--min-os 26.0] [--loader FR|FH] [--created-by T] [--no-console]\n\
         \x20                       [--manifest-only] [--force] [--clone] [--json]\n\
         \x20      ibuilder-pack compare <reference> <candidate> [--reason KEY=TEXT]... [--json-out FILE] [--label TEXT]"
    );
    ExitCode::from(2)
}

/// `--opt value` / `--opt=value` splitter over the args after the subcommand.
struct Args {
    items: Vec<String>,
    i: usize,
}

impl Args {
    fn next(&mut self) -> Option<(String, Option<String>)> {
        let a = self.items.get(self.i)?.clone();
        self.i += 1;
        if let Some(r) = a.strip_prefix("--")
            && let Some((k, v)) = r.split_once('=')
        {
            return Some((format!("--{k}"), Some(v.to_string())));
        }
        Some((a, None))
    }
    fn value(&mut self, inline: Option<String>, name: &str) -> Result<String, String> {
        if let Some(v) = inline {
            return Ok(v);
        }
        let v = self
            .items
            .get(self.i)
            .cloned()
            .ok_or_else(|| format!("argument {name}: expected one argument"))?;
        self.i += 1;
        Ok(v)
    }
}

fn run_pack(items: Vec<String>) -> ExitCode {
    let mut a = Args { items, i: 0 };
    let mut out: Option<PathBuf> = None;
    let mut p = PackArgs::new("");
    let mut json = false;
    while let Some((k, inl)) = a.next() {
        macro_rules! val {
            () => {
                match a.value(inl, &k) {
                    Ok(v) => v,
                    Err(e) => return usage(&e),
                }
            };
        }
        match k.as_str() {
            "--out" => out = Some(val!().into()),
            "--profile" => p.profile = Some(val!().into()),
            "--framework" => p.framework = Some(val!().into()),
            "--framework-name" => p.framework_name = Some(val!()),
            "--dm-dir" => p.dm_dir = Some(val!().into()),
            "--shader-map" => p.shader_map = Some(val!().into()),
            "--launcher" => p.launcher = Some(val!().into()),
            "--msl" => p.msl = Some(val!().into()),
            "--icon" => p.icon = Some(val!().into()),
            "--extra" => p.extra.push(val!()),
            "--romfs-img" => p.romfs_img = Some(val!().into()),
            "--aoc" => p.aoc = Some(val!().into()),
            "--nro-table" => p.nro_table = Some(val!().into()),
            "--game" => p.game = Some(val!()),
            "--name" => p.name = Some(val!()),
            "--display-version" => p.display_version = Some(val!()),
            "--package-version" => p.package_version = Some(val!()),
            "--modules" => p.modules = Some(val!()),
            "--romfs-marker" => p.romfs_marker = Some(val!()),
            "--romfs-identity" => p.romfs_identity = Some(val!()),
            "--chain" => p.chain = Some(val!()),
            "--min-os" => p.min_os = val!(),
            "--loader" => {
                let v = val!();
                if v != "FR" && v != "FH" {
                    return usage(&format!(
                        "argument --loader: invalid choice: '{v}' (choose from 'FR', 'FH')"
                    ));
                }
                p.loader = v;
            }
            "--created-by" => p.created_by = val!(),
            "--no-console" => p.no_console = true,
            "--manifest-only" => p.manifest_only = true,
            "--force" => p.force = true,
            "--clone" => p.clone = true,
            "--json" => json = true,
            other => return usage(&format!("unrecognized arguments: {other}")),
        }
    }
    let Some(out) = out else {
        return usage("the following arguments are required: --out");
    };
    p.out = out;
    match pack(&p) {
        Ok(o) => {
            for w in o.warnings() {
                eprintln!("{w}");
            }
            for l in o.stdout_lines() {
                println!("{l}");
            }
            if json {
                println!("{}", o.json_summary());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn run_compare(items: Vec<String>) -> ExitCode {
    let mut a = Args { items, i: 0 };
    let mut pos: Vec<String> = Vec::new();
    let mut reasons: Vec<(String, String)> = Vec::new();
    let (mut json_out, mut label): (Option<PathBuf>, String) = (None, String::new());
    while let Some((k, inl)) = a.next() {
        match k.as_str() {
            "--reason" => {
                let v = match a.value(inl, &k) {
                    Ok(v) => v,
                    Err(e) => return usage(&e),
                };
                let Some((key, text)) = v.split_once('=') else {
                    eprintln!("[RED] --reason '{v}' is not KEY=TEXT");
                    return ExitCode::from(1);
                };
                if let Some(slot) = reasons.iter_mut().find(|(r, _)| r == key) {
                    slot.1 = text.to_string();
                } else {
                    reasons.push((key.to_string(), text.to_string()));
                }
            }
            "--json-out" => match a.value(inl, &k) {
                Ok(v) => json_out = Some(v.into()),
                Err(e) => return usage(&e),
            },
            "--label" => match a.value(inl, &k) {
                Ok(v) => label = v,
                Err(e) => return usage(&e),
            },
            s if s.starts_with("--") => return usage(&format!("unrecognized arguments: {s}")),
            _ => pos.push(k),
        }
    }
    if pos.len() != 2 {
        return usage("compare takes <reference> <candidate>");
    }
    let mut out = String::new();
    let r = compare_files_cli(
        pos[0].as_ref(),
        pos[1].as_ref(),
        &reasons,
        &label,
        json_out.as_deref(),
        &mut out,
    );
    print!("{out}");
    match r {
        Ok(c) => ExitCode::from(c as u8),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return usage("");
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "pack" => run_pack(args),
        "compare" => run_compare(args),
        _ => usage("expected a subcommand: pack | compare"),
    }
}
