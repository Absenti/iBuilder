//! `ibuilder-assemble`: CLI of the Rust port of `tools/assemble.py`.
//!
//! ibuilder-assemble --host <prebuilt.dylib> --game <dir with <name>.patched.bin/.fixups.tsv> --out <new.dylib>
//!                   [--modules main subsdk0 sdk] [--tag <t>] [--synthetic]
//!
//! stdout is the same `k=v` + `segment ...` listing as the Python tool.

use ibuilder_macho::assemble::{Game, assemble_files};
use ibuilder_macho::cli::{Spec, parse};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "--out OUT [--host HOST] [--game GAME] [--modules MODULES [MODULES ...]] [--synthetic] [--tag TAG]";
    let specs = [
        Spec {
            name: "host",
            arity: 1,
        },
        Spec {
            name: "game",
            arity: 1,
        },
        Spec {
            name: "out",
            arity: 1,
        },
        Spec {
            name: "modules",
            arity: 2,
        },
        Spec {
            name: "synthetic",
            arity: 0,
        },
        Spec {
            name: "tag",
            arity: 1,
        },
    ];
    let p = match parse("ibuilder-assemble", usage, &specs, &args) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("{m}");
            return ExitCode::from(2);
        }
    };
    if !p.positionals.is_empty() {
        eprintln!(
            "usage: ibuilder-assemble {usage}\nibuilder-assemble: error: unrecognized arguments: {}",
            p.positionals.join(" ")
        );
        return ExitCode::from(2);
    }
    let Some(out) = p.get("out") else {
        eprintln!(
            "usage: ibuilder-assemble {usage}\nibuilder-assemble: error: the following arguments are required: --out"
        );
        return ExitCode::from(2);
    };
    let modules: Vec<String> = p.all("modules").map(<[String]>::to_vec).unwrap_or_else(|| {
        ["main", "subsdk0", "sdk"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    });
    let tag = p.get("tag").unwrap_or("");
    let synthetic = p.flag("synthetic");
    let (host, game) = if synthetic {
        let host = match p.get("host") {
            Some(h) => PathBuf::from(h),
            None => ibuilder_macho::embed::abspath(Path::new(out))
                .parent()
                .unwrap_or(Path::new("/"))
                .join("host-gap.dylib"),
        };
        (host, Game::Synthetic { modules })
    } else {
        let (Some(h), Some(g)) = (p.get("host"), p.get("game")) else {
            eprintln!("--host and --game are required (or --synthetic)");
            return ExitCode::from(1);
        };
        (
            PathBuf::from(h),
            Game::Dir {
                dir: PathBuf::from(g),
                modules,
            },
        )
    };
    match assemble_files(&host, Path::new(out), &game, tag) {
        Ok(r) => {
            let i = &r.info;
            println!("path={out}");
            println!("size={}", i.size);
            println!("host_size={}", i.host_size);
            println!("text_end_old={}", i.text_end_old);
            println!("text_size_new={}", i.text_size_new);
            println!("game_bytes={}", i.game_bytes);
            println!("w_bytes={}", i.w_bytes);
            println!("n_load_commands={}", i.n_load_commands);
            println!("header_slack_used={}", i.header_slack_used);
            println!("header_slack_total={}", i.header_slack_total);
            for s in &r.segments {
                println!(
                    "segment {:<14} vm {:#x} size {:#x} file {:#x} nsects {}",
                    s.name,
                    s.vmaddr,
                    s.vmsize,
                    s.fileoff,
                    s.secs.len()
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ibuilder-assemble: error: {e}");
            ExitCode::from(1)
        }
    }
}
