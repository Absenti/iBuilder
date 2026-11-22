//! `zbench FILE OFFSET LEN [LEVEL]`: compresses LEN bytes of FILE from OFFSET with the C libzstd (one 8 MiB frame at a time, as a NUP packer stores one frame per file) and times
//! the two decoders over the same frames: decode + SHA-256 of the output, one thread, in memory (no disk).
use ibuilder_nup::{NativeZstd, Ruzstd, ZstdBackend};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::fs::FileExt;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, off, len): (&str, u64, usize) =
        (&a[1], a[2].parse().unwrap(), a[3].parse().unwrap());
    let level: i32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
    let f = std::fs::File::open(path).unwrap();
    let mut data = vec![0u8; len];
    f.read_exact_at(&mut data, off).unwrap();
    let frames: Vec<Vec<u8>> = data
        .chunks(8 << 20)
        .map(|c| zstd::bulk::compress(c, level).unwrap())
        .collect();
    let stored: usize = frames.iter().map(|f| f.len()).sum();
    println!(
        "{} MB of {path} at {off}: {} frames, {:.1} MB stored (level {level}, ratio {:.2})",
        len / 1_000_000,
        frames.len(),
        stored as f64 / 1e6,
        len as f64 / stored as f64
    );
    let want = Sha256::digest(&data);
    for (name, b) in [
        (
            "libzstd (C, via the zstd crate)",
            Box::new(NativeZstd) as Box<dyn ZstdBackend>,
        ),
        ("ruzstd (pure Rust)", Box::new(Ruzstd)),
    ] {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let mut h = Sha256::new();
            for fr in &frames {
                let mut src: &[u8] = fr;
                b.decode_frame(&mut src as &mut dyn Read, &mut |c| {
                    h.update(c);
                    Ok(())
                })
                .unwrap();
            }
            assert_eq!(h.finalize(), want);
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!(
            "  {name}: {:.3} s, {:.0} MB/s of output (decode + SHA-256)",
            best,
            len as f64 / best / 1e6
        );
    }
    let t = std::time::Instant::now();
    let _ = Sha256::digest(&data);
    println!(
        "  SHA-256 alone: {:.0} MB/s",
        len as f64 / t.elapsed().as_secs_f64() / 1e6
    );
}
