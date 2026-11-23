//! IoStore containers and encrypted paks where the phone and upstream part: upstream decodes Oodle with the game's own decoder on the Mac, the phone has none, so an IoStore shader
//! library that needs Oodle, and a pak whose AES key main does not hold, end the split as an unsupported container (with the reason). The games themselves are compared with the
//! Python in tests/games.rs (ue-io-lib, ue-io-bundles, ue-aes-window, ue-aes-code: tools/gen_uepak_io.py).

use ibuilder_shadersplit::{NoGate, Ruzstd, SplitError, SplitOptions, split};
use std::path::{Path, PathBuf};

fn game(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/games")
        .join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// a copy of a game, changed by `edit`, split
fn split_copy(name: &str, tag: &str, edit: impl FnOnce(&Path)) -> Result<(), SplitError> {
    let w = std::env::temp_dir().join(format!("ibuilder-uepak-io-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&w);
    copy_dir(&game(name), &w);
    edit(&w);
    let (romfs, main, prof) = (w.join("RomFS"), w.join("main"), w.join("profile"));
    let (pkg, tables) = (w.join("out/pkg"), w.join("out/tables"));
    let o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, "0100TESTTEST0000");
    let r = split(&o, &Ruzstd, &NoGate, &|_| {}).map(|_| ());
    let _ = std::fs::remove_dir_all(&w);
    r
}

#[test]
fn a_shader_library_in_oodle_blocks_is_an_unsupported_container() {
    // the patch container's library chunk (its first block) marked Oodle: header 144 bytes, 2 chunk IDs (12 each), 2 ranges (10 each), then the block entries (method at +11)
    let e = split_copy("ue-io-lib", "oodle", |w| {
        let p = w.join("RomFS/Game/Content/Paks/Game-Switch_0_P.utoc");
        let mut b = std::fs::read(&p).unwrap();
        b[144 + 24 + 20 + 11] = 3;
        std::fs::write(&p, &b).unwrap();
    })
    .expect_err("refused");
    let m = e.to_string();
    assert!(
        m.starts_with("unsupported container: Unreal Engine IoStore shader library with Oodle-compressed data (")
            && m.contains("Game-Switch_0_P.utoc: shader library chunk 0)")
            && m.contains("FINAL FANTASY RESONANCE"),
        "{m}"
    );
}

#[test]
fn a_shader_group_compressed_as_a_whole_is_an_unsupported_container() {
    // the patch container's library (chunk 0, stored, the first bytes of its .ucas) says its second group's raw size is one byte more than it stores: the group is Oodle as a whole.
    // The library ends with the groups (16 bytes each) and the shader indices (a count and 4 of them: 20 bytes)
    let e = split_copy("ue-io-lib", "group", |w| {
        let toc = std::fs::read(w.join("RomFS/Game/Content/Paks/Game-Switch_0_P.utoc")).unwrap();
        let len = toc[144 + 24 + 5..144 + 24 + 10]
            .iter()
            .fold(0usize, |v, &c| (v << 8) | c as usize);
        let p = w.join("RomFS/Game/Content/Paks/Game-Switch_0_P.ucas");
        let mut b = std::fs::read(&p).unwrap();
        let raw = len - 20 - 16 + 8;
        let v = u32::from_le_bytes(b[raw..raw + 4].try_into().unwrap()) + 1;
        b[raw..raw + 4].copy_from_slice(&v.to_le_bytes());
        std::fs::write(&p, &b).unwrap();
    })
    .expect_err("refused");
    let m = e.to_string();
    assert!(
        m.contains("Unreal Engine IoStore shader library with Oodle-compressed data (")
            && m.contains("shader group 1 ("),
        "{m}"
    );
}

#[test]
fn an_encrypted_pak_whose_key_main_lacks_is_an_unsupported_container() {
    let e = split_copy("ue-aes-window", "nokey", |w| {
        std::fs::copy(game("ue-io-lib").join("main"), w.join("main")).unwrap();
    })
    .expect_err("refused");
    let m = e.to_string();
    assert!(
        m.starts_with("unsupported container: Unreal Engine pak with an encrypted index whose AES key is not in main (")
            && m.contains("Shaders-Switch.pak: the index is encrypted and no key in main"),
        "{m}"
    );
}

#[test]
fn aes_256_ecb_and_the_test_key() {
    // the key search's own check: a block the test key encrypts decrypts back (FIPS-197 is in the unit tests)
    let key: [u8; 32] = std::array::from_fn(|i| 7 + i as u8);
    let d = ibuilder_shadersplit::aes::decrypt(&key, &[0u8; 32]).unwrap();
    assert_eq!(d.len(), 32);
    assert_eq!(d[..16], d[16..]);
}
