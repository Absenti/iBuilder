//! The ZBIC decoder's shared machinery (literals, Huffman, FSE tables, sequences, repeat offsets) against libzstd: stock frames made by the C library
//! at several levels from several kinds of data, decoded by the decoder in its stock-zstd mode, must come back byte for byte.

use ibuilder_nso::zbic::zstd_decompress_into;

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *seed >> 33
}

fn sample(kind: usize, n: usize, seed: u64) -> Vec<u8> {
    let mut s = seed;
    match kind {
        0 => (0..n).map(|_| lcg(&mut s) as u8).collect(), // random: raw blocks
        1 => (0..n)
            .map(|_| b"abcd"[(lcg(&mut s) % 4) as usize])
            .collect(), // 4 symbols
        2 => {
            // words from a small vocabulary
            let words: Vec<Vec<u8>> = (0..40)
                .map(|i| (0..(3 + i % 9)).map(|_| lcg(&mut s) as u8).collect())
                .collect();
            let mut v = Vec::with_capacity(n);
            while v.len() < n {
                v.extend_from_slice(&words[(lcg(&mut s) % 40) as usize]);
            }
            v.truncate(n);
            v
        }
        3 => {
            // 8-byte records with counters and zeros: long matches, repeat offsets
            let mut v = Vec::with_capacity(n);
            let mut i = 0u32;
            while v.len() < n {
                v.extend_from_slice(&i.to_le_bytes());
                v.extend_from_slice(&[0, 0, (lcg(&mut s) % 3) as u8, 0]);
                i += 1;
            }
            v.truncate(n);
            v
        }
        _ => {
            // text-like: skewed byte distribution with copies of earlier stretches
            let mut v: Vec<u8> = Vec::with_capacity(n);
            while v.len() < n {
                if v.len() > 100 && lcg(&mut s).is_multiple_of(3) {
                    let len = 3 + (lcg(&mut s) % 40) as usize;
                    let from = (lcg(&mut s) as usize) % (v.len() - len.min(v.len() - 1));
                    for k in 0..len {
                        let b = v[from + k % (v.len() - from)];
                        v.push(b);
                    }
                } else {
                    let r = lcg(&mut s) % 100;
                    v.push(if r < 60 {
                        b"etaoin shrdlu"[(r % 13) as usize]
                    } else {
                        (lcg(&mut s) % 128) as u8
                    });
                }
            }
            v.truncate(n);
            v
        }
    }
}

#[test]
fn stock_frames_decode_like_libzstd() {
    let mut count = 0;
    let mut failures: Vec<String> = vec![];
    for kind in 0..5 {
        for &n in &[1usize, 40, 300, 5_000, 70_000, 300_000, 1_000_000] {
            for &level in &[1, 3, 9, 19] {
                let data = sample(kind, n, 17 + n as u64 + kind as u64);
                let frame = zstd::bulk::compress(&data, level).unwrap();
                let mut out = vec![0u8; data.len()];
                match zstd_decompress_into(&frame, &mut out) {
                    Ok(got) if got == data.len() && out == data => count += 1,
                    Ok(_) => {
                        failures.push(format!("kind {kind} n {n} level {level}: bytes differ"))
                    }
                    Err(e) => failures.push(format!("kind {kind} n {n} level {level}: {e}")),
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} frames failed:\n{}",
        failures.len(),
        count + failures.len(),
        failures.join("\n")
    );
    assert!(count >= 140);
}
