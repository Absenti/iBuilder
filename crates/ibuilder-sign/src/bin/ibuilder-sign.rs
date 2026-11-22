//! `ibuilder-sign`: ad-hoc sign / verify a Mach-O dylib in place (port of `pipeline/adhoc_sign.py`'s `__main__`).
//!
//!   ibuilder-sign <dylib> [identifier]          sign, then verify (what `python adhoc_sign.py` does)
//!   ibuilder-sign sign <dylib> [identifier]     sign only
//!   ibuilder-sign verify <dylib>                verify only (re-hash every page)
//!
//! Option `--shrink-linkedit` (anywhere): also shrink `__LINKEDIT` when a larger old signature is dropped (the
//! Python leaves it reaching past the end of the file, which `codesign` rejects); default: byte-identical to Python.
//!
//! The two result lines use the Python's `%r` dict spelling so the output can be diffed against the Python's.
use ibuilder_sign::{SignInfo, SignOptions, VerifyInfo, sign_file_with, verify_file};
use std::path::Path;
use std::process::ExitCode;

/// Python's `repr()` of a str: single quotes unless the text holds a single and no double quote.
fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut o = String::new();
    o.push(q);
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if c == q => {
                o.push('\\');
                o.push(c)
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                o.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push(q);
    o
}

fn fmt_sign(i: &SignInfo) -> String {
    format!(
        "{{'identifier': {}, 'code_limit': {}, 'n_code': {}, 'datasize': {}, 'filelen': {}}}",
        py_repr(&i.identifier),
        i.code_limit,
        i.n_code,
        i.datasize,
        i.filelen
    )
}

fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

fn fmt_verify(v: &VerifyInfo) -> String {
    format!(
        "{{'flag_adhoc': {}, 'n_code': {}, 'pages_ok': {}, 'code_limit': {}, 'dataoff': {}, 'match': {}}}",
        py_bool(v.flag_adhoc),
        v.n_code,
        v.pages_ok,
        v.code_limit,
        v.dataoff,
        py_bool(v.matches)
    )
}

fn usage() -> ExitCode {
    eprintln!("usage: ibuilder-sign [sign|verify] <dylib> [identifier]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut opts = SignOptions::default();
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| {
            if a == "--shrink-linkedit" {
                opts.shrink_linkedit = true;
                false
            } else {
                true
            }
        })
        .collect();
    let (cmd, rest): (&str, &[String]) = match args.first().map(String::as_str) {
        Some("sign") => ("sign", &args[1..]),
        Some("verify") => ("verify", &args[1..]),
        Some(_) => ("both", &args[..]),
        None => return usage(),
    };
    let Some(path) = rest.first() else {
        return usage();
    };
    let path = Path::new(path);
    let ident = rest.get(1).map(String::as_str);
    let run = || -> Result<(), ibuilder_sign::Error> {
        if cmd != "verify" {
            let info = sign_file_with(path, ident, &opts)?;
            println!("signed {}: {}", path.display(), fmt_sign(&info));
        }
        if cmd != "sign" {
            let v = verify_file(path)?;
            println!("verify: {}", fmt_verify(&v));
        }
        Ok(())
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ibuilder-sign: {e}");
            ExitCode::from(1)
        }
    }
}
