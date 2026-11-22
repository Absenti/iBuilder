//! `zbicdiff DIR`: DIR holds `*.zbic` frames and `manifest.tsv` (name, length, SHA-256 of what the `zbic` oracle decompresses it to, or ERR; see
//! tools/gen_zbic_frames.py). Decodes each frame with this crate's decoder and counts: identical, both fail, and the disagreements (listed).
use sha2::{Digest, Sha256};

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let manifest = std::fs::read_to_string(format!("{dir}/manifest.tsv")).unwrap();
    let (mut same, mut both_err, mut rust_ok_py_err, mut rust_err_py_ok, mut differ, mut panics) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut shown = 0;
    let mut why: std::collections::BTreeMap<String, u64> = Default::default();
    let mut bytes = 0u64;
    let t = std::time::Instant::now();
    for line in manifest.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let frame = std::fs::read(format!("{dir}/{}.zbic", f[0])).unwrap();
        let want_len: usize = f[1].parse().unwrap();
        let cap = want_len.max(1 << 20) + 64;
        let r = std::panic::catch_unwind(|| {
            let mut out = vec![0u8; cap];
            ibuilder_nso::zbic::decompress_into(&frame, &mut out).map(|n| {
                out.truncate(n);
                out
            })
        });
        let mut note = String::new();
        match r {
            Err(_) => {
                panics += 1;
                note = "PANIC".into();
            }
            Ok(Ok(out)) => {
                bytes += out.len() as u64;
                let h: String = Sha256::digest(&out)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                if f[2] == "ERR" {
                    rust_ok_py_err += 1;
                    note = format!("rust ok ({} bytes), oracle ERR", out.len());
                } else if h == f[2] && out.len() == want_len {
                    same += 1;
                } else {
                    differ += 1;
                    note = "outputs differ".into();
                }
            }
            Ok(Err(e)) => {
                if f[2] == "ERR" {
                    both_err += 1;
                } else {
                    rust_err_py_ok += 1;
                    *why.entry(e.to_string()).or_default() += 1;
                    note = format!("rust ERR ({e}), oracle ok");
                }
            }
        }
        if !note.is_empty() && shown < 0 {
            println!("{}: {note}", f[0]);
            shown += 1;
        }
    }
    for (k, v) in &why {
        println!("  rust ERR / oracle ok: {v} x {k}");
    }
    println!(
        "{dir}: identical {same}, both fail {both_err}, rust ok / oracle ERR {rust_ok_py_err}, rust ERR / oracle ok {rust_err_py_ok}, outputs differ {differ}, panics {panics}; {:.1} MB decoded in {:.2} s",
        bytes as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
}
