//! `gameapp/test_mods.py`, check by check (the `// py:` comments carry the Python's check names). Synthetic data only: NSOs built here, IPS / IPS32 /
//! pchtxt text and cheat files written here. The checks that went through `gameapp_pipeline.py` (`rewrite_key`, `do_rewrite`, `rewrite_is_cached`,
//! `apply_mods`) are in `crates/ibuilder-pipeline/tests/mods_pipeline_spec.rs` and `crates/ibuilder-cli/tests/mods_chain.rs`.

use ibuilder_mods::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "mods-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bid() -> Vec<u8> {
    (0x10u8..0x30).collect()
}
fn hexu(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}
fn text() -> Vec<u8> {
    (0..16).flat_map(|_| 0u8..=255).collect() // 0x1000
}
fn ro() -> Vec<u8> {
    b"ro-data-".repeat(512)
}
fn data() -> Vec<u8> {
    vec![b'D'; 0x1000]
}

fn lz4_literals(raw: &[u8]) -> Vec<u8> {
    let n = raw.len();
    let mut out = vec![(n.min(15) << 4) as u8];
    if n >= 15 {
        let mut rest = n - 15;
        while rest >= 255 {
            out.push(255);
            rest -= 255;
        }
        out.push(rest as u8);
    }
    out.extend_from_slice(raw);
    out
}

fn make_nso(bid: &[u8], compressed: bool) -> Vec<u8> {
    let segs = [text(), ro(), data()];
    let mem = [0u32, 0x1000, 0x2000];
    let flags: u32 = (if compressed { 0b111 } else { 0 }) | 0b111000;
    let stored: Vec<Vec<u8>> = segs
        .iter()
        .map(|s| {
            if compressed {
                lz4_literals(s)
            } else {
                s.clone()
            }
        })
        .collect();
    let mut h = vec![0u8; 0x100];
    h[0..4].copy_from_slice(b"NSO0");
    h[0xC..0x10].copy_from_slice(&flags.to_le_bytes());
    let mut at = 0x100u32;
    for i in 0..3 {
        h[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&at.to_le_bytes());
        h[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&mem[i].to_le_bytes());
        h[0x18 + 0x10 * i..0x1C + 0x10 * i].copy_from_slice(&(segs[i].len() as u32).to_le_bytes());
        h[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(stored[i].len() as u32).to_le_bytes());
        h[0xA0 + 0x20 * i..0xC0 + 0x20 * i].copy_from_slice(&Sha256::digest(&segs[i]));
        at += stored[i].len() as u32;
    }
    h[0x3C..0x40].copy_from_slice(&0x4000u32.to_le_bytes());
    h[0x40..0x60].copy_from_slice(bid);
    for s in &stored {
        h.extend_from_slice(s);
    }
    h
}

enum Rec {
    Data(&'static [u8]),
    Rle(u16, u8),
}
fn ips(records: &[(u32, Rec)], wide: bool) -> Vec<u8> {
    let mut out = if wide {
        b"IPS32".to_vec()
    } else {
        b"PATCH".to_vec()
    };
    for (off, d) in records {
        let ob = off.to_be_bytes();
        out.extend_from_slice(if wide { &ob[..] } else { &ob[1..] });
        match d {
            Rec::Rle(count, value) => {
                out.extend_from_slice(&0u16.to_be_bytes());
                out.extend_from_slice(&count.to_be_bytes());
                out.push(*value);
            }
            Rec::Data(b) => {
                out.extend_from_slice(&(b.len() as u16).to_be_bytes());
                out.extend_from_slice(b);
            }
        }
    }
    out.extend_from_slice(if wide { b"EEOF" } else { b"EOF" });
    out
}

fn put(base: &Path, rel: &str, data: impl AsRef<[u8]>) {
    let p = base.join("mods").join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

fn w(off: i128, b: &[u8]) -> (i128, Vec<u8>) {
    (off, b.to_vec())
}

fn is_mod_err<T>(r: Result<T>) -> bool {
    matches!(r, Err(ModsError::Mod(_)))
}

fn pch() -> String {
    format!(
        "// 60 FPS mod for Test Game 1.0.1\n@nsobid-{}\n@flag offset_shift 0x100\n// Unlock 60 fps\n@enabled\n00001000 1F2003D5\n00001004 20008052\n// Alternative: 30 fps\n@disabled\n00001000 00000000\n@stop\nCredits: someone (prose after @stop is ignored)\n@nsobid-{}\n@enabled\n00000500 AA\n@stop\n",
        hexu(&bid()),
        "FF".repeat(20)
    )
}

#[test]
fn ips_and_ips32() {
    // py: "IPS: a record"
    assert_eq!(
        parse_ips(&ips(
            &[(0x104, Rec::Data(&[0x1f, 0x20, 0x03, 0xd5]))],
            false
        ))
        .unwrap(),
        vec![w(0x104, &[0x1f, 0x20, 0x03, 0xd5])]
    );
    // py: "IPS: an RLE record is expanded"
    assert_eq!(
        parse_ips(&ips(&[(0x200, Rec::Rle(5, 0xAA))], false)).unwrap(),
        vec![w(0x200, &[0xaa; 5])]
    );
    // py: "IPS32: a 4-byte offset and the EEOF end"
    assert_eq!(
        parse_ips(&ips(&[(0x01000104, Rec::Data(&[1, 2]))], true)).unwrap(),
        vec![w(0x01000104, &[1, 2])]
    );
    // py: "IPS: a record whose offset spells EOF is a record, not the end (only a header-less EOF ends the file)"
    let mut spells = b"PATCH".to_vec();
    spells.extend_from_slice(&[0x45, 0x4f, 0x47, 0, 1, 7]);
    spells.extend_from_slice(b"EOF");
    assert_eq!(parse_ips(&spells).unwrap(), vec![w(0x454F47, &[7])]);
    // py: "IPS: not an IPS file is refused"
    assert!(is_mod_err(parse_ips(b"hello world")));
    // py: "IPS: a cut record is refused, never half-applied"
    assert!(is_mod_err(parse_ips(b"PATCH\x00\x01\x04\x00\x08\x01\x02")));
    // py: "IPS: no end marker is refused"
    assert!(is_mod_err(parse_ips(b"PATCH\x00\x01\x04\x00\x01\xaa")));
}

#[test]
fn pchtxt() {
    let ps = parse_pchtxt(&pch(), "m", "f.pchtxt").unwrap();
    // py: "pchtxt: three groups, two builds"
    assert!(ps.len() == 3 && ps[0].bid == hexu(&bid()) && ps[2].bid == "FF".repeat(20));
    // py: "pchtxt: the comment above the block is its name"
    assert!(ps[0].name == "Unlock 60 fps" && ps[1].name == "Alternative: 30 fps");
    // py: "pchtxt: @enabled / @disabled are the default state"
    assert_eq!(
        ps.iter().map(|p| p.on).collect::<Vec<_>>(),
        vec![true, false, true]
    );
    // py: "pchtxt: address + offset_shift - 0x100 is the module offset"
    assert_eq!(
        ps[0].writes,
        vec![
            w(0x1000, &[0x1F, 0x20, 0x03, 0xD5]),
            w(0x1004, &[0x20, 0x00, 0x80, 0x52])
        ]
    );
    // py: "pchtxt: a flag stays for the rest of the file (second build: 0x500 + 0x100 - 0x100)"
    assert_eq!(ps[2].writes, vec![w(0x500, &[0xaa])]);
    // py: "pchtxt: without a flag the address is an IPS offset (0x500 -> module 0x400)"
    assert_eq!(
        parse_pchtxt(
            &format!("@nsobid-{}\n@enabled\n00000500 AA\n", "11".repeat(16)),
            "",
            ""
        )
        .unwrap()[0]
            .writes,
        vec![w(0x400, &[0xaa])]
    );
    // py: "pchtxt: a quoted string value is refused"
    assert!(is_mod_err(parse_pchtxt(
        &format!(
            "@nsobid-{}\n@enabled\n00001000 \"hello\"\n",
            "11".repeat(16)
        ),
        "",
        ""
    )));
    // py: "pchtxt: /* */ comments and a missing @enabled (one implicit enabled block)"
    let p = parse_pchtxt(
        &format!("/* x */\n@nsobid-{}\n00000100 01\n", "11".repeat(16)),
        "",
        "",
    )
    .unwrap();
    assert_eq!(
        p.iter()
            .map(|p| (p.on, p.writes.clone()))
            .collect::<Vec<_>>(),
        vec![(true, vec![w(0, &[1])])]
    );
    // py: "pchtxt: lines before any @nsobid belong to no build and are dropped"
    assert!(
        parse_pchtxt(
            &format!("00000100 01\n@nsobid-{}\n", "11".repeat(16)),
            "",
            ""
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn build_id_matching() {
    let b = bid();
    let hexl = |x: &[u8]| x.iter().map(|v| format!("{v:02x}")).collect::<String>();
    // py: "bid: the first 16 bytes (32 hex) name the NSO"
    assert!(bid_matches(&hexl(&b[..16]), &b));
    // py: "bid: the first 8 bytes (16 hex), as cheat sites name theirs"
    assert!(bid_matches(&hexu(&b[..8]), &b));
    // py: "bid: the whole id, with its zero tail trimmed or not"
    assert!(
        bid_matches(&hexl(&b), &b)
            && bid_matches(&format!("{}{}", hexl(&b[..16]), "00".repeat(16)), &b)
    );
    // py: "bid: another build does not match"
    assert!(!bid_matches(&"AA".repeat(16), &b));
    // py: "bid: fewer than 16 hex digits is not a build id"
    assert!(!bid_matches(&hexl(&b[..7]), &b));
    // py: "bid: an all-zero name matches nothing"
    assert!(!bid_matches(&"00".repeat(16), &b));
    // py: "bid: a file name that is not hex is not a build id"
    assert!(
        stem_bid("main.ips").is_none()
            && stem_bid(&format!("{}.ips", hexu(&b[..8]).to_lowercase())) == Some(hexu(&b[..8]))
    );
}

#[test]
fn nso_patching() {
    let b = bid();
    let patch = Patch::new(
        "m",
        "x.ips",
        "ips",
        &hexu(&b),
        true,
        vec![
            w(0x10, &[0xde, 0xad, 0xbe, 0xef]),
            w(0x1010, b"RO"),
            w(0x2000, &[0x99]),
        ],
    );
    for comp in [false, true] {
        let tag = if comp { "compressed" } else { "plain" };
        let nso = make_nso(&b, comp);
        let patches = [patch.clone()];
        let p = patch_nso(&nso, &patches).unwrap().expect("a match");
        let new = &p.data;
        let (_h, segs) = nso_image(new).unwrap();
        // py: "NSO (%s): the three writes land in text, ro and data at their memory offsets"
        assert!(
            segs[0].1[0x10..0x14] == [0xde, 0xad, 0xbe, 0xef]
                && &segs[1].1[0x10..0x12] == b"RO"
                && segs[2].1[0] == 0x99
                && p.applied.len() == 1
                && p.notes.is_empty(),
            "{tag}"
        );
        // py: "NSO (%s): nothing else changes"
        assert!(
            segs[0].1[..0x10] == text()[..0x10]
                && segs[0].1[0x14..] == text()[0x14..]
                && segs[1].1[..0x10] == ro()[..0x10]
                && segs[2].1[1..] == data()[1..],
            "{tag}"
        );
        // py: "NSO (%s): stored plain, compression and hash flags cleared, the build id kept"
        let flags = u32::from_le_bytes(new[0xC..0x10].try_into().unwrap());
        assert!(flags & 0xBF == 0 && new[0x40..0x60] == b[..], "{tag}");
        // py: "NSO (%s): the header hashes follow the new bytes"
        assert_eq!(new[0xA0..0xC0], Sha256::digest(&segs[0].1)[..], "{tag}");
        // the Rust port of the engine's nso.py reads what this builds (py: "the engine's nso.load reads the patched NSO: segments, bss, build id, the patched bytes")
        let m = ibuilder_nso::load(new, "patched").unwrap();
        assert!(
            m.segments == [(0, 0x1000), (0x1000, 0x1000), (0x2000, 0x1000)]
                && m.bss == 0x4000
                && m.build_id[..] == b[..]
                && m.image[0x10..0x14] == [0xde, 0xad, 0xbe, 0xef],
            "{tag}"
        );
    }
    // py: "NSO: a patch for another build id changes nothing (None)"
    let mut other_bid = vec![0u8; 31];
    other_bid.push(1);
    assert!(
        patch_nso(&make_nso(&other_bid, false), std::slice::from_ref(&patch))
            .unwrap()
            .is_none()
    );
    // py: "NSO: writes in the header, in bss or in a gap are counted, not made (a write across two segments is whole)"
    let off = [Patch::new(
        "m",
        "y.ips",
        "ips",
        &hexu(&b),
        true,
        vec![w(-4, b"abcd"), w(0x3000, b"zz"), w(0xFFE, b"1234")],
    )];
    let r = patch_nso(&make_nso(&b, false), &off).unwrap().unwrap();
    assert!(!r.notes.is_empty() && r.notes[0].1 == 2);
    // py: "NSO: a disabled block is not applied"
    assert!(
        patch_nso(
            &make_nso(&b, false),
            &[Patch::new(
                "m",
                "z",
                "n",
                &hexu(&b),
                false,
                vec![w(0, b"a")]
            )]
        )
        .unwrap()
        .is_none()
    );
    // py: "NSO: a file that is not an NSO is refused"
    assert!(is_mod_err(nso_image(
        &[b"NRO0".to_vec(), vec![0; 0x200]].concat()
    )));
    // py: "NSO: a truncated one is refused"
    assert!(is_mod_err(nso_image(&make_nso(&b, false)[..0x180])));
}

/// The store, the plan and what the rewrite cache key is made of (`rewrite_key` itself is in the pipeline crate).
#[test]
fn store_plan_apply_signature_cheats_boot() {
    let b = bid();
    let b16 = hexu(&b[..8]);
    let t = Tmp::new();
    let docs = t.0.join("Documents");
    let tid = "0100AAAA00000000";
    let gdir = docs.join("games").join(tid);
    let exe = t.0.join("ExeFS");
    fs::create_dir_all(&exe).unwrap();
    fs::write(exe.join("main"), make_nso(&b, false)).unwrap();
    fs::write(exe.join("subsdk0"), make_nso(&[0x77; 32], false)).unwrap();
    fs::write(
        exe.join("main.npdm"),
        [b"META".to_vec(), vec![0; 100]].concat(),
    )
    .unwrap();
    put(&gdir, "60fps/exefs/test.pchtxt", pch());
    put(
        &gdir,
        &format!("Cheater/exefs/{}.ips", hexu(&b)),
        ips(&[(0x110, Rec::Data(&[1, 2, 3, 4]))], false),
    );
    put(
        &gdir,
        "Cheater/exefs/notabuildid.ips",
        ips(&[(0x110, Rec::Data(&[1]))], false),
    );
    put(
        &gdir,
        &format!("Cheater/cheats/{}.txt", b16),
        "{Master}\n04000000 00123456 00000001\n[Infinite HP]\n04000000 00200000 000003E7\n[Shared]\n04000000 00300000 00000001\n",
    );
    put(
        &gdir,
        &format!("Other/cheats/{}.txt", b16.to_lowercase()),
        "[Shared]\n04000000 00400000 00000002\n[Own]\n04000000 00500000 00000003\n",
    );
    put(&gdir, "Other/cheats/readme.txt", "not a cheat file");
    put(&gdir, "Skins/romfs/ui/icon.bin", "icon");
    put(&gdir, "Skins/romfs/new.txt", "new");
    put(&gdir, "Zed/romfs/ui/icon.bin", "other icon");
    let state_path = gdir.join("mods.tsv");
    let set_state = |rows: &[&[&str]]| {
        fs::write(
            &state_path,
            rows.iter().map(|r| r.join("\t") + "\n").collect::<String>(),
        )
        .unwrap()
    };

    // py: "state: no file is no mod on"
    assert!(enabled_mods(&docs, tid).unwrap().is_empty());
    // py: "mods: the store lists them sorted"
    assert_eq!(
        list_mods(&docs, tid),
        vec!["60fps", "Cheater", "Other", "Skins", "Zed"]
    );
    set_state(&[
        &["mod", "60fps", "1"],
        &["mod", "Cheater", "1"],
        &["mod", "Other", "1"],
        &["mod", "Skins", "1"],
        &["mod", "Zed", "0"],
        &["patch", "60fps", "Alternative: 30 fps", "0"],
        &["cheat", "Cheater", "Infinite HP", "1"],
        &["cheat", "Other", "Own", "1"],
    ]);
    // py: "state: rows are read"
    let st = read_state(&state_path).unwrap();
    assert_eq!(
        st.cheat,
        BTreeMap::from([
            (("Cheater".into(), "Infinite HP".into()), true),
            (("Other".into(), "Own".into()), true)
        ])
    );
    assert!(!st.mods["Zed"]);

    let mut logs: Vec<String> = Vec::new();
    let modules: Vec<String> = ["main", "subsdk0", "sdk"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let pl = plan(
        &docs,
        tid,
        &exe,
        &modules,
        &mut |l| logs.push(l.into()),
        false,
    )
    .unwrap();
    // py: "plan: only main has patches that name its build id (60fps block 1 and 3 do not: the second build id is another game)"
    assert_eq!(
        pl.modules
            .iter()
            .map(|(m, _)| m.as_str())
            .collect::<Vec<_>>(),
        vec!["main"]
    );
    let mut names: Vec<_> = pl
        .get("main")
        .unwrap()
        .patches
        .iter()
        .map(|p| p.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Unlock 60 fps", "ips"]);
    // py: "plan: a block of another build id and an IPS not named by a build id are said, not hidden"
    assert!(
        logs.iter()
            .any(|l| l.contains("notabuildid.ips") && l.contains("not used")),
        "{logs:#?}"
    );
    assert!(
        logs.iter()
            .any(|l| l.contains("[patch 3]") && l.contains("matches none")),
        "{logs:#?}"
    );
    // py: "plan: the build ids are recorded for the importer"
    assert_eq!(
        fs::read_to_string(gdir.join("build_ids.txt"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        vec![
            format!("main\t{}", hexu(&b)),
            format!("subsdk0\t{}", "77".repeat(32))
        ]
    );

    let d1 = pl.get("main").unwrap().digest.clone();
    let path = apply_module(
        &docs,
        tid,
        &exe,
        "main",
        &pl.get("main").unwrap().patches,
        &mut |l| logs.push(l.into()),
    )
    .unwrap()
    .unwrap();
    let (_hdr, segs) = nso_image(&fs::read(&path).unwrap()).unwrap();
    // py: "apply_module: the 60 fps words are at module offset 0x1000 and the IPS at 0x10 (ips 0x110)"
    assert!(segs[1].1[0..4] == [0x1F, 0x20, 0x03, 0xD5] && segs[0].1[0x10..0x14] == [1, 2, 3, 4]);
    // py: "apply_module: the dumped module is untouched"
    assert_eq!(fs::read(exe.join("main")).unwrap(), make_nso(&b, false));
    remove_patched(&docs, tid);
    // py: "remove_patched: the copies go"
    assert!(!gdir.join("mods-exefs").exists());

    // the digest follows what is applied: a block switched on, a mod switched off, a patch's bytes
    let quiet_digest = |rows: &[&[&str]]| {
        set_state(rows);
        plan(&docs, tid, &exe, &["main".to_string()], &mut |_| {}, true)
            .unwrap()
            .get("main")
            .unwrap()
            .digest
            .clone()
    };
    let d2 = quiet_digest(&[
        &["mod", "60fps", "1"],
        &["mod", "Cheater", "1"],
        &["patch", "60fps", "Alternative: 30 fps", "1"],
    ]);
    let d3 = quiet_digest(&[&["mod", "60fps", "1"]]);
    let d4 = quiet_digest(&[&["mod", "Cheater", "1"]]);
    put(
        &gdir,
        &format!("Cheater/exefs/{}.ips", hexu(&b)),
        ips(&[(0x110, Rec::Data(&[1, 2, 3, 5]))], false),
    );
    let d5 = plan(&docs, tid, &exe, &["main".to_string()], &mut |_| {}, true)
        .unwrap()
        .get("main")
        .unwrap()
        .digest
        .clone();
    // py: "digest: a block switched on, a mod switched off, a patch edited: each changes it"
    assert_eq!(
        [&d1, &d2, &d3, &d4, &d5]
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        5
    );
    // py: "digest: the same set twice is the same digest"
    assert_eq!(
        plan(&docs, tid, &exe, &["main".to_string()], &mut |_| {}, true)
            .unwrap()
            .get("main")
            .unwrap()
            .digest,
        d5
    );
    // py: "plan: mods with no exefs part leave the plan empty (the cache key is the old one)"
    set_state(&[&["mod", "Skins", "1"]]);
    assert!(
        plan(&docs, tid, &exe, &["main".to_string()], &mut |_| {}, true)
            .unwrap()
            .is_empty()
    );

    // ---- cheats ----
    set_state(&[
        &["mod", "Cheater", "1"],
        &["mod", "Other", "1"],
        &["cheat", "Cheater", "Infinite HP", "1"],
        &["cheat", "Other", "Own", "1"],
        &["cheat", "Other", "Shared", "1"],
    ]);
    let folder = write_cheats_folder(&docs, tid, &mut |l| logs.push(l.into()))
        .unwrap()
        .unwrap();
    let txt = fs::read_to_string(folder.join(format!("{b16}.txt"))).unwrap();
    // py: "cheats: two mods' files for one build id are one file named by the first 16 hex digits, upper case"
    let mut listing: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    listing.sort();
    assert_eq!(
        listing,
        vec![
            format!("{b16}.enabled"),
            format!("{b16}.origin.tsv"),
            format!("{b16}.txt")
        ]
    );
    // py: "cheats: a section both mods have gets the mod's name, the others keep theirs"
    for needle in [
        "[Shared (Cheater)]",
        "[Shared (Other)]",
        "[Infinite HP]",
        "[Own]",
        "{Master}",
    ] {
        assert!(txt.contains(needle), "{needle} in {txt}");
    }
    // py: "cheats: .enabled lists what the player turned on (by the joined name), the master is not listed"
    let mut on: Vec<String> = fs::read_to_string(folder.join(format!("{b16}.enabled")))
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    on.sort();
    assert_eq!(on, vec!["Infinite HP", "Own", "Shared (Other)"]);
    // py: "cheats: .origin.tsv maps the joined name back to (mod, name) for the in-game menu"
    assert!(
        fs::read_to_string(folder.join(format!("{b16}.origin.tsv")))
            .unwrap()
            .contains("Shared (Other)\tOther\tShared")
    );
    // py: "cheats: a file not named by a build id is said and left out"
    assert!(
        logs.iter()
            .any(|l| l.contains("readme.txt") && l.contains("not named by a build id"))
    );
    // py: "cheats: the section names the sheet lists"
    assert_eq!(
        cheat_sections(
            &fs::read_to_string(gdir.join("mods/Cheater/cheats").join(format!("{b16}.txt")))
                .unwrap()
        ),
        vec!["Infinite HP", "Shared"]
    );
    set_state(&[&["mod", "Skins", "1"]]);
    // py: "cheats: no enabled mod with cheats, no folder (the engine's cheats stay off)"
    assert!(
        write_cheats_folder(&docs, tid, &mut |_| {})
            .unwrap()
            .is_none()
            && !folder.exists()
    );

    // ---- the boot's switches ----
    set_state(&[
        &["mod", "Skins", "1"],
        &["mod", "Zed", "1"],
        &["mod", "Cheater", "1"],
    ]);
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let got = apply_boot(&docs, tid, &mut |l| logs.push(l.into()), &mut env).unwrap();
    // py: "boot: DM_ROMFS_OVERLAY lists the enabled mods' romfs folders in order (the first wins a shared file), DM_CHEATS the folder"
    let want_overlay = ["Skins", "Zed"]
        .iter()
        .map(|m| {
            gdir.join("mods")
                .join(m)
                .join("romfs")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>()
        .join(":");
    assert_eq!(env.get("DM_ROMFS_OVERLAY"), Some(&want_overlay));
    assert_eq!(
        env.get("DM_CHEATS"),
        Some(&gdir.join("cheats-run").to_string_lossy().into_owned())
    );
    assert_eq!(
        got.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        vec!["DM_ROMFS_OVERLAY", "DM_CHEATS"]
    );
    // py: "boot: a switch the environment already names wins"
    let mut env = BTreeMap::from([
        ("DM_ROMFS_OVERLAY".to_string(), "/x".to_string()),
        ("DM_CHEATS".to_string(), "/y".to_string()),
    ]);
    assert!(
        apply_boot(&docs, tid, &mut |l| logs.push(l.into()), &mut env)
            .unwrap()
            .is_empty()
    );
    assert_eq!(env.len(), 2);
    assert_eq!(env["DM_ROMFS_OVERLAY"], "/x");
    set_state(&[&["mod", "Skins", "0"]]);
    let mut env = BTreeMap::new();
    // py: "boot: nothing enabled sets nothing"
    assert!(
        apply_boot(&docs, tid, &mut |l| logs.push(l.into()), &mut env)
            .unwrap()
            .is_empty()
            && env.is_empty()
    );

    // ---- the signature: the same text the app computes (GameMods.ExefsSignature) ----
    fs::remove_dir_all(gdir.join("mods")).unwrap();
    put(&gdir, "A/exefs/aa.pchtxt", "x");
    put(&gdir, "B/exefs/bb.ips", "PATCH");
    put(&gdir, "B/romfs/f", "not part of the signature");
    set_state(&[
        &["mod", "A", "1"],
        &["mod", "B", "1"],
        &["mod", "C", "1"],
        &["patch", "A", "Fast", "1"],
        &["patch", "B", "Slow", "0"],
        &["patch", "X", "Not enabled", "1"],
        &["cheat", "A", "Cheat", "1"],
    ]);
    let sig = exefs_signature(&docs, tid).unwrap();
    let hx = |d: &[u8]| {
        Sha256::digest(d)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>()
    };
    // py: "signature: files of the enabled mods with digests, then their patch choices; romfs, cheats and a disabled mod are not in it"
    assert_eq!(
        sig,
        format!(
            "file\tA\taa.pchtxt\t{}\npatch\tA\tFast\t1\nfile\tB\tbb.ips\t{}\npatch\tB\tSlow\t0",
            hx(b"x"),
            hx(b"PATCH")
        )
    );
    // the C# test holds the same value (the digest of the signature of the Python fixture)
    assert_eq!(
        hx(sig.as_bytes()),
        hx(format!(
            "file\tA\taa.pchtxt\t{}\npatch\tA\tFast\t1\nfile\tB\tbb.ips\t{}\npatch\tB\tSlow\t0",
            hx(b"x"),
            hx(b"PATCH")
        )
        .as_bytes())
    );
}

#[test]
fn what_the_python_lets_through_stays_an_error() {
    let b = bid();
    let t = Tmp::new();
    let docs = t.0.join("Documents");
    let tid = "0100AAAA00000000";
    let gdir = docs.join("games").join(tid);
    let exe = t.0.join("ExeFS");
    fs::create_dir_all(&exe).unwrap();
    fs::write(exe.join("main"), make_nso(&b, false)).unwrap();
    // an IPS that writes into the NSO header (offset 0x10 < 0x100): the module offset is negative, Patch.digest() is a struct.error that plan() does not catch
    put(
        &gdir,
        &format!("M/exefs/{}.ips", hexu(&b)),
        ips(&[(0x10, Rec::Data(&[1]))], false),
    );
    fs::write(gdir.join("mods.tsv"), "mod\tM\t1\n").unwrap();
    let err = plan(&docs, tid, &exe, &["main".to_string()], &mut |_| {}, true).unwrap_err();
    assert_eq!(
        (err.exc_name(), err.message()),
        ("error", "argument out of range")
    );
    // a mods.tsv that is not UTF-8 is a UnicodeDecodeError, not "no mod is on"
    fs::write(gdir.join("mods.tsv"), b"mod\tM\t1\n\xff").unwrap();
    let err = read_state(&gdir.join("mods.tsv")).unwrap_err();
    assert_eq!(err.exc_name(), "UnicodeDecodeError");
    assert!(
        err.to_string()
            .contains("can't decode byte 0xff in position 8: invalid start byte"),
        "{err}"
    );
    // a pchtxt token HEX accepts after removing every 0x, that int() does not
    let e = parse_pchtxt(
        &format!("@nsobid-{}\n@enabled\n0x0x12 01\n", "11".repeat(16)),
        "m",
        "f",
    )
    .unwrap_err();
    assert_eq!(e.exc_name(), "ValueError");
    // a bad offset_shift is a ModError with the repr of the token
    let e = parse_pchtxt("@nsobid-AA\n@flag offset_shift it's\n", "m", "f").unwrap_err();
    assert_eq!(e.message(), "bad offset_shift \"it's\"");
    // a non-NSO with an enabled patch: .hex() of None
    let p = Patch::new("m", "x", "n", &hexu(&b), true, vec![]);
    assert_eq!(
        patch_nso(b"not an nso", &[p])
            .err()
            .map(|e| e.exc_name().to_string()),
        Some("AttributeError".into())
    );
    // a ZBIC module is decoded (ibuilder_nso::zbic); a segment that is not a ZBIC frame is `zbic.error` ("ZBIC data is invalid"), which mods.py does not catch
    let mut z = make_nso(&b, true);
    z[0xC] |= 0x80;
    let e = nso_image(&z).expect_err("LZ4 data is no ZBIC frame");
    assert_eq!(
        (e.exc_name(), e.message()),
        ("error", "ZBIC data is invalid")
    );
    let (header, segs) = nso_image(&zbic_nso_bytes()).unwrap();
    assert_eq!(header.len(), 0x100);
    for (i, (mo, bytes)) in segs.iter().enumerate() {
        assert_eq!((*mo as usize, bytes), (0x2000 * i, &zbic_plain()));
    }
    // and a patch lands in it as in any module: the NSO built from the patched segments is stored plain
    let built = build_nso(&header, &segs).unwrap();
    assert_eq!(built.len(), 0x100 + 3 * zbic_plain().len());
}

#[test]
fn lz4_follows_the_reference_decoder() {
    // literals only; a match with overlap (offset 1); the output buffer overrun and a short output
    let lit = lz4_literals(b"hello world, this is a test");
    assert_eq!(
        ibuilder_mods::lz4::decompress_safe(&lit, 27).unwrap(),
        b"hello world, this is a test"
    );
    assert!(ibuilder_mods::lz4::decompress_safe(&lit, 26).is_err());
    assert_eq!(
        ibuilder_mods::lz4::decompress_safe(&lit, 40).unwrap().len(),
        27,
        "a short output is returned short, the caller checks the size"
    );
    assert!(ibuilder_mods::lz4::decompress_safe(&[], 4).is_err());
    assert_eq!(
        ibuilder_mods::lz4::decompress_safe(&[0], 0).unwrap(),
        Vec::<u8>::new()
    );
}

// ---- ZBIC: an NSO whose three segments are the same ZBIC frame (the `zbic` package's compressor, level 3, of `zbic_plain()`) ----
const ZBIC_FRAME_HEX: &str = "5a424943601011dd0900d41254686520717569636b2062726f776e20666f78206a756d7073206f76657220746865206c617a7920646f672e20000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff020000fd0656350b5ee8ab32";
fn zbic_plain() -> Vec<u8> {
    let mut v = b"The quick brown fox jumps over the lazy dog. ".repeat(80);
    for _ in 0..4 {
        v.extend(0u8..=255);
    }
    v
}
fn zbic_frame() -> Vec<u8> {
    (0..ZBIC_FRAME_HEX.len() / 2)
        .map(|i| u8::from_str_radix(&ZBIC_FRAME_HEX[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}
fn zbic_nso_bytes() -> Vec<u8> {
    let (plain, frame) = (zbic_plain(), zbic_frame());
    let mut h = vec![0u8; 0x100];
    h[0..4].copy_from_slice(b"NSO0");
    h[0xC..0x10].copy_from_slice(&(0x07u32 | 0x38 | 0x80).to_le_bytes());
    let mut at = 0x100u32;
    for i in 0..3usize {
        h[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&at.to_le_bytes());
        h[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&(0x2000 * i as u32).to_le_bytes());
        h[0x18 + 0x10 * i..0x1C + 0x10 * i].copy_from_slice(&(plain.len() as u32).to_le_bytes());
        h[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(frame.len() as u32).to_le_bytes());
        h[0xA0 + 0x20 * i..0xC0 + 0x20 * i].copy_from_slice(&Sha256::digest(&plain));
        at += frame.len() as u32;
    }
    for _ in 0..3 {
        h.extend_from_slice(&frame);
    }
    h
}

/// Review 2026-11-20 §4.9: the decompressed size of a segment is the NSO header's word (up to 0xFFFFFFFF) and was the output buffer's
/// reservation. A compressed segment that claims more than Horizon loads (2 GiB, ibuilder-profile's bound) is a ModError before anything is
/// reserved, and the LZ4 decoder reserves only what its input can produce, whatever capacity it is given.
#[test]
fn a_header_that_claims_a_huge_segment_is_refused_before_it_is_reserved() {
    let b = bid();
    for zbic in [false, true] {
        let mut n = make_nso(&b, true);
        if zbic {
            n[0xC] |= 0x80;
        }
        n[0x18..0x1C].copy_from_slice(&u32::MAX.to_le_bytes()); // text: 0xFFFFFFFF bytes
        let e = nso_image(&n).expect_err("a 4 GiB segment was accepted");
        assert!(matches!(e, ModsError::Mod(_)), "zbic {zbic}: {e:?}");
        assert_eq!(
            e.message(),
            "a segment claims 0xffffffff bytes, more than Horizon loads",
            "zbic {zbic}"
        );
    }
    // the decoder itself: a 6-byte literal run with a 4 GiB capacity decodes, into a buffer of its own size
    let out = ibuilder_mods::lz4::decompress_safe(b"\x50hello", u32::MAX as usize).unwrap();
    assert_eq!(out, b"hello");
    assert!(
        out.capacity() <= 6 * 256,
        "{} bytes reserved",
        out.capacity()
    );
}

/// Review 2026-11-20 §5.15 and §5.16: the patched copy is written beside `mods-exefs/<m>` and renamed over it (a copy that is a hard link, as a
/// cached tree's files are, is replaced, not written through, and no partial file is left), and a module name that is not a plain file name
/// (`../x`, an absolute path) is neither read nor written: the module is left as dumped.
#[test]
fn the_patched_copy_replaces_the_old_one_and_a_module_name_cannot_leave_its_folders() {
    let b = bid();
    let t = Tmp::new();
    let docs = t.0.join("Documents");
    let tid = "0100AAAA00000000";
    let gdir = docs.join("games").join(tid);
    let exe = t.0.join("ExeFS");
    fs::create_dir_all(&exe).unwrap();
    fs::write(exe.join("main"), make_nso(&b, false)).unwrap();
    let patches = vec![Patch::new(
        "m",
        "y.ips",
        "ips",
        &hexu(&b),
        true,
        vec![w(0x10, b"abcd")],
    )];
    // an enabled mod whose IPS names main's build id: what `plan` matches
    put(
        &gdir,
        &format!("X/exefs/{}.ips", hexu(&b)),
        ips(&[(0x110, Rec::Data(&[1, 2, 3, 4]))], false),
    );
    fs::write(gdir.join("mods.tsv"), "mod\tX\t1\n").unwrap();
    let planned = |m: &str, logs: &mut Vec<String>| {
        !plan(
            &docs,
            tid,
            &exe,
            &[m.to_string()],
            &mut |l| logs.push(l.into()),
            false,
        )
        .unwrap()
        .is_empty()
    };
    // an older copy that is a hard link to another file
    let out = gdir.join("mods-exefs");
    fs::create_dir_all(&out).unwrap();
    let witness = t.0.join("witness");
    fs::write(&witness, b"WITNESS").unwrap();
    fs::hard_link(&witness, out.join("main")).unwrap();
    let mut logs: Vec<String> = Vec::new();
    let p = apply_module(&docs, tid, &exe, "main", &patches, &mut |l| {
        logs.push(l.into())
    })
    .unwrap()
    .expect("main is patched");
    assert_eq!(p, out.join("main"));
    let (_h, segs) = nso_image(&fs::read(&p).unwrap()).unwrap();
    assert_eq!(&segs[0].1[0x10..0x14], b"abcd");
    assert_eq!(
        fs::read(&witness).unwrap(),
        b"WITNESS",
        "the copy was written through a hard link"
    );
    let mut left: Vec<String> = fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, vec!["main"], "a temporary file was left");

    assert!(planned("main", &mut logs));
    // a module named outside ExeFS: a valid NSO there, which the old code read and patched into gdir/main
    fs::write(t.0.join("outside"), make_nso(&b, false)).unwrap();
    let abs = t.0.join("outside").to_str().unwrap().to_string();
    for m in ["../outside", abs.as_str(), "..", ".", ""] {
        let r = apply_module(&docs, tid, &exe, m, &patches, &mut |l| logs.push(l.into())).unwrap();
        assert!(r.is_none(), "{m:?} gave {r:?}");
        assert!(!planned(m, &mut logs), "{m:?} is planned");
    }
    assert!(!gdir.join("outside").exists());
    assert!(
        logs.iter()
            .any(|l| l.contains("\"../outside\" is not a file name")),
        "{logs:#?}"
    );
    assert!(is_module_name("main") && is_module_name("subsdk0") && !is_module_name("a/b"));
}
