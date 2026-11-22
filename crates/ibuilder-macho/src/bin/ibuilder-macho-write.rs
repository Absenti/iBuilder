//! `ibuilder-macho-write`: CLI of the Rust port of `tools/macho_write.py`.
//!
//! ibuilder-macho-write --out <file.dylib> [--platform sim|ios] [--name DmProbe] [--magic 0x5A17BEEF]
//!                      [--minos V] [--sdk V] [--uuid <32 hex digits>]
//!
//! `--uuid` is an addition: pin LC_UUID (Python draws a random uuid4) to get reproducible bytes.

use ibuilder_macho::cli::{Spec, parse, parse_int_auto};
use ibuilder_macho::writer::{ProbeParams, build_probe};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "--out OUT [--platform {sim,ios}] [--name NAME] [--magic MAGIC] [--minos MINOS] [--sdk SDK] [--uuid HEX32]";
    let specs = [
        Spec {
            name: "out",
            arity: 1,
        },
        Spec {
            name: "platform",
            arity: 1,
        },
        Spec {
            name: "name",
            arity: 1,
        },
        Spec {
            name: "magic",
            arity: 1,
        },
        Spec {
            name: "minos",
            arity: 1,
        },
        Spec {
            name: "sdk",
            arity: 1,
        },
        Spec {
            name: "uuid",
            arity: 1,
        },
    ];
    let p = match parse("ibuilder-macho-write", usage, &specs, &args) {
        Ok(p) => p,
        Err(m) => {
            eprintln!("{m}");
            return ExitCode::from(2);
        }
    };
    let usage_err = |m: &str| {
        eprintln!("usage: ibuilder-macho-write {usage}\nibuilder-macho-write: error: {m}");
        ExitCode::from(2)
    };
    let Some(out) = p.get("out") else {
        return usage_err("the following arguments are required: --out");
    };
    let sim = match p.get("platform").unwrap_or("sim") {
        "sim" => true,
        "ios" => false,
        o => {
            return usage_err(&format!(
                "argument --platform: invalid choice: '{o}' (choose from 'sim', 'ios')"
            ));
        }
    };
    let mut params = ProbeParams::for_platform(sim);
    if let Some(n) = p.get("name") {
        params.name = n.to_string();
    }
    if let Some(m) = p.get("magic") {
        match parse_int_auto(m).ok().and_then(|v| u32::try_from(v).ok()) {
            Some(v) => params.magic = v,
            None => {
                eprintln!("ibuilder-macho-write: error: bad --magic {m}");
                return ExitCode::from(1);
            }
        }
    }
    if let Some(v) = p.get("minos") {
        params.minos = v.to_string();
    }
    if let Some(v) = p.get("sdk") {
        params.sdk = v.to_string();
    }
    if let Some(u) = p.get("uuid") {
        let h: String = u.chars().filter(|c| *c != '-').collect();
        if h.len() != 32 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return usage_err("--uuid must be 32 hex digits");
        }
        let mut b = [0u8; 16];
        for (i, byte) in b.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap();
        }
        params.uuid = Some(b);
    }
    match build_probe(&params) {
        Ok((bytes, i)) => {
            if let Err(e) = std::fs::write(out, &bytes) {
                eprintln!("ibuilder-macho-write: error: {out}: {e}");
                return ExitCode::from(1);
            }
            println!("path={out}");
            println!("size={}", i.size);
            println!("platform={}", i.platform);
            println!("text_off={}", i.text_off);
            println!("x0_off={}", i.x0_off);
            println!("x1_off={}", i.x1_off);
            println!("w_fileoff={}", i.w_fileoff);
            println!("le_fileoff={}", i.le_fileoff);
            println!("magic={}", i.magic);
            println!("entry_sym={}", i.entry_sym);
            println!("marker_sym={}", i.marker_sym);
            println!("id_dylib={}", i.id_dylib);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ibuilder-macho-write: error: {e}");
            ExitCode::from(1)
        }
    }
}
