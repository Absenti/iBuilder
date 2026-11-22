//! `ibuilder-gen-embed [--tag T] [--jit IMG] <build dir> <out.S> <module...>`: port of
//! `engine/dm/ios/tools/gen_embed.py`. Writes the assembly, prints the link flags.

use ibuilder_macho::embed::{GenEmbedOptions, abspath, gen_embed};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = GenEmbedOptions::default();
    while a.len() >= 2 && (a[0] == "--tag" || a[0] == "--jit") {
        if a[0] == "--tag" {
            opts.tag = a[1].clone();
        } else {
            opts.jit = Some(abspath(Path::new(&a[1])));
        }
        a.drain(..2);
    }
    if a.len() < 2 {
        eprintln!(
            "usage: ibuilder-gen-embed [--tag <tag>] [--jit <jit.img>] <build output dir> <output .S> <module name...>"
        );
        return ExitCode::from(1);
    }
    let (dir, dst, names) = (&a[0], &a[1], &a[2..]);
    match gen_embed(Path::new(dir), names, &opts) {
        Ok((asm, flags)) => {
            if let Err(e) = std::fs::write(dst, asm) {
                eprintln!("ibuilder-gen-embed: error: {dst}: {e}");
                return ExitCode::from(1);
            }
            println!("{flags}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ibuilder-gen-embed: error: {e}");
            ExitCode::from(1)
        }
    }
}
