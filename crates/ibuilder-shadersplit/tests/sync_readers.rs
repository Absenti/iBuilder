//! The readers ported from nxnative after a3663659 (NXSD, Falcom .pkg / .pka, Media.Vision MVGL and NXVS / NXFS / NXCS), on damaged input: every container of the synthetic games in
//! tests/data/games/sync-* (made by tools/gen_sync_readers.py, no game data) cut at many lengths and with bytes flipped where the readers look, read by each reader and split end to end.
//! A damaged container is an `Err` (a `[RED]` line) or fewer shaders, never a panic, and a length it claims is checked before anything that size is allocated.

use ibuilder_shadersplit::falcom::{falcom_pka_index, falcom_pkg};
use ibuilder_shadersplit::mvgl::{
    DobozError, archive, doboz, lz4_block, nxsh_embedded, nxsh_shader,
};
use ibuilder_shadersplit::nxsd::nxsd_shaders;
use ibuilder_shadersplit::source::MemSource;
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GAME: &str = "0100TESTTEST0000";

fn data(game: &str, rel: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/games")
            .join(game)
            .join("RomFS")
            .join(rel),
    )
    .unwrap()
}

/// the container cut at lengths all along it, and with one byte flipped at each of the offsets given (and every 0x40th byte)
fn mutations(d: &[u8], hot: &[usize]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let step = (d.len() / 97).max(1);
    for n in (0..d.len())
        .step_by(step)
        .chain([d.len().saturating_sub(1)])
    {
        out.push(d[..n].to_vec());
    }
    let mut at: Vec<usize> = hot.iter().copied().filter(|&o| o < d.len()).collect();
    at.extend((0..d.len()).step_by(0x40));
    for o in at {
        for x in [0xff, 0x80, 0x01] {
            let mut m = d.to_vec();
            m[o] ^= x;
            out.push(m);
        }
    }
    out
}

#[test]
fn damaged_nxsd_containers_are_red_never_a_panic() {
    let good = data("sync-nxsd", "Data/shaders.binary");
    assert_eq!(nxsd_shaders(&good, "/x").unwrap().len(), 3);
    let hot: Vec<usize> = (0..0x40).chain(0x50..0x50 + 3 * 24).collect();
    let (mut ok, mut red) = (0, 0);
    for m in mutations(&good, &hot) {
        match nxsd_shaders(&m, "/x") {
            Ok(v) => {
                assert!(v.len() <= 3);
                ok += 1
            }
            Err(e) => {
                assert!(e.to_string().starts_with("/x NXSD"), "{e}");
                red += 1
            }
        }
    }
    assert!(red > 100 && ok > 0, "{ok} read, {red} refused");
}

#[test]
fn damaged_nxsh_containers_are_red_when_strict_and_skipped_when_embedded() {
    let shad = data("sync-mvgl", "Pack/loose.shad");
    assert!(nxsh_shader(&shad, 0, "/s", true).unwrap().is_some());
    for m in mutations(&shad, &(0..0x40).collect::<Vec<_>>()) {
        if let Err(e) = nxsh_shader(&m, 0, "/s", true) {
            assert!(e.to_string().starts_with("/s "), "{e}");
        }
        // embedded, a damaged container is no container
        let mut host = vec![0x11u8; 64];
        host.extend_from_slice(&m);
        host.extend_from_slice(&[0x22; 64]);
        let _ = nxsh_embedded(&host, "main").unwrap();
    }
    // a size that points past the data, and a container inside a container's span
    let mut big = shad.clone();
    big[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(nxsh_shader(&big, 0, "/s", false).unwrap().is_none());
    assert!(nxsh_shader(&big, 0, "/s", true).is_err());
}

/// the stream of tests/test_split_mvgl.py's doboz_match_stream: 4 literals, 12 matches (offset 1, length 3), the 8-byte literal tail
fn doboz_stream() -> (Vec<u8>, Vec<u8>) {
    let mut raw = b"AAAA".to_vec();
    raw.extend_from_slice(&[b'A'; 36]);
    raw.extend_from_slice(b"12345678");
    let ctl: u32 = (((1 << 12) - 1) << 4) | (1 << 31);
    let mut body = ctl.to_le_bytes().to_vec();
    body.extend_from_slice(b"AAAA");
    body.extend_from_slice(&[0x04; 12]);
    body.extend_from_slice(b"12345678");
    let mut s = vec![3 << 3];
    s.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    s.extend_from_slice(&(9 + body.len() as u32).to_le_bytes());
    s.extend_from_slice(&body);
    (raw, s)
}

#[test]
fn doboz_decodes_matches_and_the_literal_tail_and_refuses_damage() {
    let (raw, s) = doboz_stream();
    assert_eq!(doboz(&s).unwrap(), raw);
    // stored payload, a version, a short header, a match before the start
    let mut stored = vec![0x80 | (3 << 3)];
    stored.extend_from_slice(&3u32.to_le_bytes());
    stored.extend_from_slice(&3u32.to_le_bytes());
    stored.extend_from_slice(b"xyz");
    assert_eq!(doboz(&stored).unwrap(), b"xyz");
    assert!(matches!(doboz(&[1]), Err(DobozError::Value(m)) if m == "doboz version 1"));
    assert!(
        matches!(doboz(&[3 << 3, 1]), Err(DobozError::Value(m)) if m == "truncated doboz header")
    );
    let mut far = s.clone();
    far[9..13].copy_from_slice(&1u32.to_le_bytes()); // one match first: nothing to copy from
    far[13] = 0x04;
    assert!(matches!(doboz(&far), Err(DobozError::Value(m)) if m == "doboz match out of range"));
    // a length of 2^56 bytes from a few: refused when the stream runs out, never allocated
    let mut huge = vec![7 << 3];
    huge.extend_from_slice(&(1u64 << 56).to_le_bytes());
    huge.extend_from_slice(&64u64.to_le_bytes());
    huge.extend_from_slice(&[0u8; 32]);
    assert!(doboz(&huge).is_err());
    for m in mutations(&s, &(0..s.len()).collect::<Vec<_>>()) {
        let _ = doboz(&m);
    }
}

#[test]
fn lz4_blocks_are_checked_before_they_are_allocated() {
    assert_eq!(
        lz4_block(&[0; 4], 0x10000).unwrap_err(),
        "impossible LZ4 length 0x10000 for 0x4 bytes"
    );
    assert_eq!(
        lz4_block(&[], 1).unwrap_err(),
        "impossible LZ4 length 0x1 for 0x0 bytes"
    );
    // a decoder error: the Python's text is python-lz4's ("... Error code: N"), this port's names lz4_flex's reason after the same words
    let e = lz4_block(&[0xff; 4], 8).unwrap_err();
    assert!(
        e.starts_with("invalid LZ4 block: Decompression failed: corrupt input or insufficient space in destination buffer"),
        "{e}"
    );
    let raw = vec![7u8; 100];
    let c = lz4_flex::block::compress(&raw);
    assert_eq!(lz4_block(&c, 100).unwrap(), raw);
    assert_eq!(
        lz4_block(&c, 101).unwrap_err(),
        "LZ4 decoded 0x64 bytes, expected 0x65"
    );
}

#[test]
fn damaged_mvgl_archives_and_falcom_tables_are_refused_never_a_panic() {
    for (rel, wide) in [
        ("Data/app_0.nx64.mvgl", true),
        ("Data/legacy.nx64.mvgl", false),
    ] {
        let good = data("sync-mvgl", rel);
        let (w, files) = archive(&MemSource(&good), "p").unwrap();
        assert_eq!(w, wide);
        assert!(files.keys().any(|k| k.to_lowercase().ends_with(".shad")));
        let hot: Vec<usize> = (0..0x20).collect();
        for m in mutations(&good, &hot) {
            if let Err(e) = archive(&MemSource(&m), "p") {
                assert!(e.starts_with("p: "), "{e}");
            }
        }
    }
    let pkg = data("sync-falcom", "Data/fx.pkg");
    assert_eq!(falcom_pkg(&MemSource(&pkg)).unwrap().unwrap().len(), 8);
    for m in mutations(&pkg, &(0..8 + 0x50 * 8).step_by(4).collect::<Vec<_>>()) {
        let _ = falcom_pkg(&MemSource(&m));
    }
    let pka = data("sync-falcom", "Pack/assets.pka");
    // 9 blobs, 6 shader archives read once each (one named twice), the texture not, the external blob and the GLSLC output
    assert_eq!(falcom_pka_index(&MemSource(&pka), "/a").unwrap().len(), 8);
    for m in mutations(&pka, &(0..0x400).step_by(4).collect::<Vec<_>>()) {
        if let Err(e) = falcom_pka_index(&MemSource(&m), "/a") {
            assert!(e.to_string().starts_with("/a: PKA"), "{e}");
        }
    }
    // counts that do not fit the archive are refused before a table that size is read
    let mut lie = pka.clone();
    lie[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(
        falcom_pka_index(&MemSource(&lie), "/a")
            .unwrap_err()
            .to_string()
            .contains("more than fit in the archive")
    );
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-sync-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("RomFS/Data")).unwrap();
    d
}

/// one split of a RomFS holding `files`; the shader count and the log
fn run(dir: &Path, files: &[(&str, &[u8])]) -> (usize, Vec<String>) {
    let romfs = dir.join("RomFS");
    let _ = std::fs::remove_dir_all(&romfs);
    for (rel, b) in files {
        let p = romfs.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    let main = dir.join("main");
    let mut nso = vec![0u8; 0x100];
    nso[..4].copy_from_slice(b"NSO0");
    std::fs::write(&main, nso).unwrap();
    let prof = dir.join("profile");
    std::fs::create_dir_all(prof.join(GAME)).unwrap();
    std::fs::write(
        prof.join(GAME).join("game.tsv"),
        format!("title_id\t{GAME}\nshader_groups\t/=romfs\nshader_note\tsynthetic\n"),
    )
    .unwrap();
    let o = dir.join("out");
    let (pkg, tables) = (o.join("pkg"), o.join("tables"));
    let mut opts = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, GAME);
    opts.jobs = 2;
    let log = Mutex::new(Vec::new());
    let r = split(&opts, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let log = log.into_inner().unwrap();
    (r.unwrap_or_else(|e| panic!("{e}: {log:?}")).shaders, log)
}

#[test]
fn damaged_archives_split_end_to_end_leave_only_their_own_shaders_out() {
    let dir = temp("e2e");
    let loose = data("sync-falcom", "Pack/loose.fx.phyre");
    let (n0, log) = run(&dir, &[("Data/loose.fx.phyre", &loose)]);
    assert_eq!(n0, 2, "{log:?}");
    for (game, rel, hot) in [
        ("sync-falcom", "Data/fx.pkg", 8 + 0x50 * 8),
        ("sync-falcom", "Pack/assets.pka", 0x300),
        ("sync-mvgl", "Data/app_0.nx64.mvgl", 0x200),
        ("sync-mvgl", "Data/legacy.nx64.mvgl", 0x100),
        ("sync-nxsd", "Data/shaders.binary", 0xc8),
    ] {
        let good = data(game, rel);
        let name = rel.rsplit('/').next().unwrap();
        let all: Vec<Vec<u8>> = mutations(&good, &(0..hot).step_by(8).collect::<Vec<_>>());
        for m in all.iter().step_by(7) {
            let (n, log) = run(
                &dir,
                &[
                    ("Data/loose.fx.phyre", &loose),
                    (&format!("Data/{name}"), m),
                ],
            );
            assert!(n >= n0, "{rel}: {log:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
