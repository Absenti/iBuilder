//! `read_index` reads the index of a `DMROMFS1` image whose every number comes from the file (review finding 4.1): a header that claims an index of 2^63 bytes, a count the index cannot hold, a
//! directory that lists itself or a child outside the table, a name outside the pool. Each is an error; none may panic, allocate what the header claims, or loop. The image is made by hand.

use ibuilder_nup::romfs::read_index;
use std::path::PathBuf;

fn le(v: u64) -> [u8; 8] {
    v.to_le_bytes()
}

/// header (32) + one file's 8 bytes + index [root dir (children 1..2), file "f" (data 32, size 8)] + pool "f"
fn honest() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"DMROMFS1");
    b.extend_from_slice(&le(40)); // index offset
    b.extend_from_slice(&le(24 * 2 + 1)); // index size
    b.extend_from_slice(&le(2)); // entry count
    b.extend_from_slice(b"abcdefgh");
    // root: a = 1, b = 1, name offset 0, name length 0, type 0 (dir), flags 0
    b.extend_from_slice(&le(1));
    b.extend_from_slice(&le(1));
    b.extend_from_slice(&[0u8; 8]);
    // file: a = 32 (data offset), b = 8, name offset 0, length 1, type 1
    b.extend_from_slice(&le(32));
    b.extend_from_slice(&le(8));
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&[1, 0]);
    b.push(b'f');
    b
}

fn dir_of(tag: &str) -> PathBuf {
    let base = std::env::var("IBUILDER_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!(
        "ibuilder-nup-romfs-damaged-{}-{tag}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn index_of(bytes: &[u8], tag: &str) -> Result<Vec<(String, bool, u64, u64)>, String> {
    let d = dir_of(tag);
    let img = d.join("RomFS.img");
    std::fs::write(&img, bytes).unwrap();
    let r = read_index(&img).map_err(|e| e.to_string());
    let _ = std::fs::remove_dir_all(&d);
    r
}

fn put(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&le(v));
}

#[test]
fn the_honest_image_reads() {
    let r = index_of(&honest(), "honest").unwrap();
    assert_eq!(r, vec![("f".to_string(), false, 32, 8)]);
}

/// `vec![0u8; isz]` of a header's `isz` aborted the process for a size no machine has
#[test]
fn an_index_size_the_file_cannot_hold_is_an_error_and_allocates_nothing() {
    for (tag, isz) in [
        ("h63", 1u64 << 63),
        ("max", u64::MAX),
        ("tb", 1 << 40),
        ("past", 24 * 2 + 1 + 1),
    ] {
        let mut b = honest();
        put(&mut b, 16, isz);
        let e = index_of(&b, tag)
            .expect_err("a header claiming a bigger index than the file is an error");
        assert!(!e.is_empty());
    }
    // an index offset past the end, and one whose sum wraps
    for (tag, ioff) in [
        ("off-past", 1u64 << 40),
        ("off-max", u64::MAX - 4),
        ("off-low", 8),
    ] {
        let mut b = honest();
        put(&mut b, 8, ioff);
        assert!(index_of(&b, tag).is_err(), "index offset {ioff:#x}");
    }
}

/// `&raw[cnt * ENT_SIZE..]` panicked for a count whose entries do not fit in the index
#[test]
fn a_count_the_index_cannot_hold_is_an_error() {
    for (tag, cnt) in [
        ("c3", 3u64),
        ("c63", 1u64 << 63),
        ("cmax", u64::MAX),
        ("c0", 0),
    ] {
        let mut b = honest();
        put(&mut b, 24, cnt);
        assert!(index_of(&b, tag).is_err(), "count {cnt:#x}");
    }
}

/// a directory that lists itself made the walk push the same entry for ever
#[test]
fn a_directory_that_lists_itself_is_an_error_not_a_loop() {
    let mut b = honest();
    put(&mut b, 40, 0); // root: first child = entry 0 (itself)
    put(&mut b, 48, 1);
    assert!(index_of(&b, "loop").is_err());
    // two directories that list each other
    let mut b = honest();
    b[40 + 24 + 20 + 2] = 0; // the "file" entry becomes a directory (type 0)...
    put(&mut b, 40 + 24, 0); // ...whose child is the root
    put(&mut b, 40 + 24 + 8, 1);
    assert!(index_of(&b, "cycle").is_err());
}

#[test]
fn a_child_outside_the_table_or_a_name_outside_the_pool_is_an_error() {
    let mut b = honest();
    put(&mut b, 48, 5); // root claims 5 children
    assert!(index_of(&b, "kids").is_err());
    let mut b = honest();
    put(&mut b, 40, u64::MAX - 1); // first child far away
    assert!(index_of(&b, "kid-far").is_err());
    let mut b = honest();
    b[40 + 24 + 16..40 + 24 + 20].copy_from_slice(&u32::MAX.to_le_bytes()); // name offset
    assert!(index_of(&b, "name").is_err());
    let mut b = honest();
    b[40 + 24 + 20..40 + 24 + 22].copy_from_slice(&u16::MAX.to_le_bytes()); // name length
    assert!(index_of(&b, "namelen").is_err());
}

#[test]
fn no_single_byte_of_the_image_can_make_the_reader_panic() {
    let good = honest();
    for at in 0..good.len() {
        for v in [0x00u8, 0x7F, 0x80, 0xFF] {
            let mut b = good.clone();
            b[at] = v;
            let _ = index_of(&b, "byte");
        }
    }
    for len in 0..=good.len() {
        let _ = index_of(&good[..len], "cut");
    }
}
