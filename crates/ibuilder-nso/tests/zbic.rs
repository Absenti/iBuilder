//! ZBIC (NSO flag bit 7): the decoder against the `zbic` package's own compressor (frames made by `tools/gen_zbic_frames.py` from synthetic data and kept
//! under `tests/fixtures/zbic`, with the SHA-256 of what each decodes to), an NSO whose three segments are ZBIC frames, damaged frames (an error or some
//! output, never a panic), and the real Ocarina of Time `main` (skipped when the extracted game is not there).

use ibuilder_nso::zbic::decompress_into;
use ibuilder_nso::{Module, load};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zbic")
}

fn manifest() -> Vec<(String, usize, String)> {
    let text = std::fs::read_to_string(fixtures().join("manifest.tsv")).expect("manifest.tsv");
    text.lines()
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f[0].to_string(), f[1].parse().unwrap(), f[2].to_string())
        })
        .collect()
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode(frame: &[u8], len: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; len];
    decompress_into(frame, &mut out)
        .map(|n| {
            out.truncate(n);
            out
        })
        .map_err(|e| e.0)
}

#[test]
fn the_compressors_frames_decode_to_their_hashes() {
    let m = manifest();
    assert!(m.len() >= 100, "{} fixtures", m.len());
    for (name, len, sha) in &m {
        let frame = std::fs::read(fixtures().join(format!("{name}.zbic"))).unwrap();
        let out = decode(&frame, *len).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(out.len(), *len, "{name}");
        assert_eq!(&hex(&Sha256::digest(&out)), sha, "{name}");
    }
}

#[test]
fn a_frame_is_refused_when_the_output_is_too_small_or_the_magic_is_stock_zstd() {
    let (name, len, _) = manifest().into_iter().find(|(_, l, _)| *l > 1000).unwrap();
    let frame = std::fs::read(fixtures().join(format!("{name}.zbic"))).unwrap();
    assert!(decode(&frame, len - 1).is_err(), "one byte short");
    let mut zstd = frame.clone();
    zstd[..4].copy_from_slice(&[0x28, 0xb5, 0x2f, 0xfd]);
    assert!(decode(&zstd, len).unwrap_err().contains("not a ZBIC frame"));
    assert!(decode(&[], 4).is_err());
    assert!(decode(b"ZBI", 4).is_err());
}

#[test]
fn damaged_frames_are_errors_or_output_never_a_panic() {
    // every fixture with a byte changed at a spread of places, two values each: whatever the decoder says, it says without panicking
    // (the test binary is built with the workspace's panic = abort in release; a panic would end it)
    let mut tried = 0u64;
    for (name, len, _) in manifest().into_iter().filter(|(_, l, _)| *l >= 40) {
        let frame = std::fs::read(fixtures().join(format!("{name}.zbic"))).unwrap();
        let step = (frame.len() / 24).max(1);
        for at in (4..frame.len()).step_by(step) {
            for x in [0x01u8, 0xff] {
                let mut f = frame.clone();
                f[at] ^= x;
                let _ = decode(&f, len);
                tried += 1;
            }
        }
        let _ = decode(&frame[..frame.len() / 2], len);
        let _ = decode(&frame[..frame.len() - 1], len);
    }
    assert!(tried > 3000, "{tried}");
}

/// an NSO whose segments are ZBIC frames of three fixtures (compressed + hash flags set), as a system 22.0.0+ module is
fn zbic_nso(parts: [&(String, usize, String); 3]) -> (Vec<u8>, [Vec<u8>; 3]) {
    let frames: Vec<Vec<u8>> = parts
        .iter()
        .map(|(n, _, _)| std::fs::read(fixtures().join(format!("{n}.zbic"))).unwrap())
        .collect();
    let plain: Vec<Vec<u8>> = parts
        .iter()
        .zip(&frames)
        .map(|((_, l, _), f)| decode(f, *l).unwrap())
        .collect();
    let mut h = vec![0u8; 0x100];
    h[..4].copy_from_slice(b"NSO0");
    let flags: u32 = 0x07 | 0x38 | 0x80;
    h[0xc..0x10].copy_from_slice(&flags.to_le_bytes());
    let mut file_off = 0x100u32;
    let mut mem_off = 0u32;
    for i in 0..3 {
        let sz = plain[i].len() as u32;
        h[0x10 + i * 0x10..0x14 + i * 0x10].copy_from_slice(&file_off.to_le_bytes());
        h[0x14 + i * 0x10..0x18 + i * 0x10].copy_from_slice(&mem_off.to_le_bytes());
        h[0x18 + i * 0x10..0x1c + i * 0x10].copy_from_slice(&sz.to_le_bytes());
        h[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(frames[i].len() as u32).to_le_bytes());
        h[0xa0 + 0x20 * i..0xc0 + 0x20 * i].copy_from_slice(Sha256::digest(&plain[i]).as_slice());
        file_off += frames[i].len() as u32;
        mem_off = (mem_off + sz + 0xfff) & !0xfff;
    }
    for f in &frames {
        h.extend_from_slice(f);
    }
    (h, [plain[0].clone(), plain[1].clone(), plain[2].clone()])
}

#[test]
fn an_nso_with_zbic_segments_loads_and_is_checked() {
    let m = manifest();
    // frames that are smaller than what they decode to (the loader refuses a compressed segment larger than its memory size, as Horizon does)
    let big: Vec<&(String, usize, String)> = m
        .iter()
        .filter(|(n, l, _)| {
            *l > 500
                && std::fs::metadata(fixtures().join(format!("{n}.zbic")))
                    .unwrap()
                    .len()
                    < (*l as u64)
        })
        .collect();
    assert!(big.len() >= 3, "{} compressible fixtures", big.len());
    let (file, plain) = zbic_nso([big[0], big[1], big[2]]);
    let module: Module = load(&file, "t").unwrap();
    for i in 0..3 {
        let (mo, sz) = module.segments[i];
        assert_eq!(
            &module.image[mo as usize..(mo + sz) as usize],
            &plain[i][..],
            "segment {i}"
        );
    }
    // a damaged segment is an error that says ZBIC (its SHA-256 or its decode)
    let mut bad = file.clone();
    let n = bad.len();
    bad[n - 3] ^= 0x40;
    let e = load(&bad, "t").unwrap_err().0;
    assert!(e.contains("ZBIC") || e.contains("SHA-256"), "{e}");
    // the size in the header is the decoder's budget: a segment that decodes to more is refused
    let mut small = file.clone();
    let sz0 = u32::from_le_bytes(small[0x18..0x1c].try_into().unwrap()) - 1;
    small[0x18..0x1c].copy_from_slice(&sz0.to_le_bytes());
    assert!(load(&small, "t").is_err());
}

/// the extracted Ocarina of Time (the NUP read by `ibuilder-nup`): ExeFS/main is ZBIC (flags 0xff); every segment passes the SHA-256 of its header
fn ocarina_exefs() -> Option<PathBuf> {
    let candidates = [
        std::env::var("IBUILDER_OCARINA_EXEFS").ok(),
        Some(
            ibuilder_testkit::scratch()
                .join("ocarina/extract/ExeFS")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.join("main").is_file())
}

#[test]
fn the_real_ocarina_main_loads() {
    let Some(dir) = ocarina_exefs() else {
        ibuilder_testkit::skip("no extracted Ocarina of Time ExeFS (set IBUILDER_OCARINA_EXEFS)");
        return;
    };
    let t = std::time::Instant::now();
    let m = ibuilder_nso::read(&dir.join("main")).expect("main");
    eprintln!(
        "main: {:#x} bytes of image in {:.3} s",
        m.image.len(),
        t.elapsed().as_secs_f64()
    );
    assert_eq!(
        m.segments,
        [(0, 0x3c461d0), (0x3c47000, 0xc05080), (0x484d000, 0x4f9b80)]
    );
    assert_eq!(m.bss, 0x159480);
    assert_eq!(hex(&m.build_id[..16]), "e0f3ccfaf50065ae4ea01eb5aeb13b1a");
    // the Python oracle's decoded segments, when somebody kept them: byte for byte
    if let Ok(refdir) = std::env::var("IBUILDER_ZBIC_REF") {
        for (i, name) in ["text", "ro", "data"].iter().enumerate() {
            let want = std::fs::read(Path::new(&refdir).join(format!("main.{name}.bin"))).unwrap();
            let (mo, sz) = m.segments[i];
            assert!(
                m.image[mo as usize..(mo + sz) as usize] == want[..],
                "{name} differs from the Python decode"
            );
        }
    }
    for other in ["subsdk0", "sdk"] {
        ibuilder_nso::read(&dir.join(other)).unwrap_or_else(|e| panic!("{other}: {e}"));
    }
}
