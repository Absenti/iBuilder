//! `jobs::plan` indexes a LINKDATA archive through `first(i)` = the u64 at `raw[32 * i..]`, where `raw` is what `Source::read` returned. `read` clamps to the file on purpose, so for a member
//! whose declared length is longer than what the file holds (a truncated RomFS image, a file cut short after it was listed) it returns fewer bytes than `n * 32`, and `first(i)` panicked on the
//! first entry past them (review finding 4.1). The Python's `struct.unpack_from` raises there: here it is an error that names the index.

use ibuilder_shadersplit::image::Loc;
use ibuilder_shadersplit::jobs::plan;
use ibuilder_shadersplit::scan::FileRef;
use std::path::PathBuf;

fn dir(tag: &str) -> PathBuf {
    let base = std::env::var("IBUILDER_TEST_TMP")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let d = base.join(format!(
        "ibuilder-shadersplit-short-index-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// DATA0.bin (the index) of `index_bytes` real bytes but a declared length of `declared`, and DATA1.bin (the archive) of `archive` bytes
fn files(d: &std::path::Path, index_bytes: usize, declared: u64, archive: u64) -> Vec<FileRef> {
    let idx = d.join("DATA0.bin");
    let bin = d.join("DATA1.bin");
    let mut entries = Vec::new();
    for i in 0..index_bytes / 32 {
        entries.extend_from_slice(&(i as u64 * 100).to_le_bytes()); // offset
        entries.extend_from_slice(&[0u8; 24]);
    }
    std::fs::write(&idx, &entries).unwrap();
    std::fs::write(&bin, vec![0u8; archive as usize]).unwrap();
    vec![
        FileRef {
            root: 0,
            rel: "DATA0.bin".into(),
            loc: Loc::Path(idx, declared),
        },
        FileRef {
            root: 0,
            rel: "DATA1.bin".into(),
            loc: Loc::Path(bin, archive),
        },
    ]
}

#[test]
fn an_index_shorter_than_its_declared_length_is_an_error_not_a_panic() {
    let d = dir("short");
    // 2 real entries (64 bytes), declared as 200
    let f = files(&d, 64, 200 * 32, 1000);
    let e = plan(&f, &|_| {})
        .err()
        .expect("a truncated index must be refused");
    assert!(e.to_string().contains("DATA0.bin"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_honest_index_plans() {
    let d = dir("honest");
    let f = files(&d, 64 * 32, 64 * 32, 64_000);
    let p = plan(&f, &|_| {}).unwrap_or_else(|e| panic!("{e}"));
    assert!(!p.jobs.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}
