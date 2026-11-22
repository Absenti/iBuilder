//! `ibuilder-fix-tlv <natural.dylib> <segaddr.dylib>`: port of `tools/fix_tlv.py` (patches the second file in place).

use ibuilder_macho::tlv::{TlvOutcome, fix_tlv};
use std::fs::{File, OpenOptions};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: fix_tlv.py <natural.dylib> <segaddr.dylib>");
        return ExitCode::from(1);
    }
    let run = || -> ibuilder_macho::Result<TlvOutcome> {
        let nat =
            File::open(&args[0]).map_err(|e| ibuilder_macho::Error::io(args[0].clone(), e))?;
        let mut seg = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&args[1])
            .map_err(|e| ibuilder_macho::Error::io(args[1].clone(), e))?;
        fix_tlv(&nat, &mut seg)
    };
    match run() {
        Ok(TlvOutcome::AlreadyCorrect) => {
            println!("fix_tlv: already correct, nothing to do");
            ExitCode::SUCCESS
        }
        Ok(TlvOutcome::Patched { count, delta }) => {
            println!(
                "fix_tlv: patched {count} TLV descriptors, removed the constant {} that -segaddr added to their offset",
                ibuilder_macho::embed::hex_py(delta)
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}
