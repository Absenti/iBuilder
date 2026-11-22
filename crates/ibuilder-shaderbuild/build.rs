//! Build script: a fingerprint of the Rust sources whose behaviour decides the library's bytes -- this crate's `src/*.rs` and `ibuilder-shader`'s `src/**` (the post-rewrite chain,
//! the finish port, the tables) -- exported as `IBSB_SRC_FP`. It is part of every shader's cache digest (`key::CODE_FP`), so a change to any of them can never be served a stale
//! cache entry: the Python got this from hashing its own source text (`dmsh_build.CODE_FP`), and nobody has to remember to bump a version. When the sources are not beside this crate
//! (a vendored copy), the fingerprint is `none` and the crate version alone stands (documented in `key.rs`).
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn collect(dir: &Path, recurse: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            // src/bin holds command line tools, not what decides an output
            if recurse && p.file_name().map(|n| n != "bin").unwrap_or(true) {
                collect(&p, true, out);
            }
        } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
            out.push(p);
        }
    }
}

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let own = here.join("src");
    let shader = here.join("..").join("ibuilder-shader").join("src");
    let mut files = Vec::new();
    collect(&own, false, &mut files);
    let have_shader = shader.is_dir();
    if have_shader {
        collect(&shader, true, &mut files);
    }
    files.sort();
    let mut h = Sha256::new();
    for f in &files {
        let rel = f.strip_prefix(&here).unwrap_or(f);
        h.update(rel.to_string_lossy().as_bytes());
        h.update([0u8]);
        if let Ok(b) = std::fs::read(f) {
            h.update((b.len() as u64).to_le_bytes());
            h.update(&b);
        }
    }
    let d = h.finalize();
    let hex: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    println!(
        "cargo:rustc-env=IBSB_SRC_FP={}",
        if have_shader { hex } else { "none".to_string() }
    );
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../ibuilder-shader/src");
}
