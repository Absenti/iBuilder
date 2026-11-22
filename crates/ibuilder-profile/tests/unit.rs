//! Unit tests on hand-built inputs: NSO loading and its refusals, dynamic symbols, facts and rules on a tiny
//! ExeFS, NACP (plain and compressed title area), RomFS image reading, the provisional profile writer.
use ibuilder_profile::derive::{DeriveConfig, derive};
use ibuilder_profile::facts::facts_of;
use ibuilder_profile::guess::guess;
use ibuilder_profile::nso;
use ibuilder_profile::syms::{HookSet, dynsyms};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("profile-unit")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

struct NsoSpec<'a> {
    syms: &'a [(&'a str, bool, u8)],
    extra_ro: &'a [u8],
    bss: u32,
    compress: bool,
    check_hash: bool,
    bid: [u8; 0x20],
}

fn nso(spec: &NsoSpec) -> Vec<u8> {
    let mut names = vec![0u8];
    let mut offs = Vec::new();
    for (n, _, _) in spec.syms {
        offs.push(names.len() as u32);
        names.extend_from_slice(n.as_bytes());
        names.push(0);
    }
    let nsym = spec.syms.len() + 1;
    let ro_base = 0x1000usize;
    let dyn_off = ro_base + 0x20;
    let hash_off = dyn_off + 4 * 16;
    let symtab_off = (hash_off + 16 + 7) & !7;
    let strtab_off = symtab_off + nsym * 24;
    let extra_off = strtab_off + names.len();
    let mut ro = vec![0u8; extra_off + spec.extra_ro.len() - ro_base];
    ro[0..4].copy_from_slice(b"MOD0");
    ro[4..8].copy_from_slice(&((dyn_off - ro_base) as i32).to_le_bytes());
    for (i, (t, v)) in [(4i64, hash_off), (5, strtab_off), (6, symtab_off), (0, 0)]
        .iter()
        .enumerate()
    {
        let o = dyn_off - ro_base + i * 16;
        ro[o..o + 8].copy_from_slice(&t.to_le_bytes());
        ro[o + 8..o + 16].copy_from_slice(&(*v as u64).to_le_bytes());
    }
    let o = hash_off - ro_base;
    ro[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
    ro[o + 4..o + 8].copy_from_slice(&(nsym as u32).to_le_bytes());
    for (i, (_, defined, bind)) in spec.syms.iter().enumerate() {
        let o = symtab_off - ro_base + (i + 1) * 24;
        ro[o..o + 4].copy_from_slice(&offs[i].to_le_bytes());
        ro[o + 4] = (bind << 4) | 2;
        ro[o + 6..o + 8].copy_from_slice(&(*defined as u16).to_le_bytes());
    }
    ro[strtab_off - ro_base..strtab_off - ro_base + names.len()].copy_from_slice(&names);
    ro[extra_off - ro_base..].copy_from_slice(spec.extra_ro);
    ro.resize(ro.len().div_ceil(0x1000) * 0x1000, 0);
    let mut text = vec![0u8; 0x1000];
    text[4..8].copy_from_slice(&(ro_base as u32).to_le_bytes());
    let data_base = ro_base + ro.len();
    let segs = [text, ro, vec![0u8; 0x1000]];
    let mem = [0usize, ro_base, data_base];
    let mut hdr = vec![0u8; 0x100];
    hdr[0..4].copy_from_slice(b"NSO0");
    let mut flags = 0u32;
    let comp: Vec<Vec<u8>> = segs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if spec.compress {
                flags |= 1 << i;
            }
            if spec.check_hash {
                flags |= 1 << (3 + i);
            }
            if spec.compress {
                lz4_flex::block::compress(s)
            } else {
                s.clone()
            }
        })
        .collect();
    hdr[0xc..0x10].copy_from_slice(&flags.to_le_bytes());
    let mut off = 0x100u32;
    for (i, s) in segs.iter().enumerate() {
        hdr[0x10 + i * 0x10..0x14 + i * 0x10].copy_from_slice(&off.to_le_bytes());
        hdr[0x14 + i * 0x10..0x18 + i * 0x10].copy_from_slice(&(mem[i] as u32).to_le_bytes());
        hdr[0x18 + i * 0x10..0x1c + i * 0x10].copy_from_slice(&(s.len() as u32).to_le_bytes());
        hdr[0x60 + i * 4..0x64 + i * 4].copy_from_slice(&(comp[i].len() as u32).to_le_bytes());
        hdr[0xa0 + 0x20 * i..0xc0 + 0x20 * i].copy_from_slice(&Sha256::digest(s));
        off += comp[i].len() as u32;
    }
    hdr[0x3c..0x40].copy_from_slice(&spec.bss.to_le_bytes());
    hdr[0x40..0x60].copy_from_slice(&spec.bid);
    let mut out = hdr;
    for c in comp {
        out.extend(c);
    }
    out
}

fn npdm(sysres: u32, program: u64) -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    d[0..4].copy_from_slice(b"META");
    d[0xc] = 1;
    d[0x14..0x18].copy_from_slice(&sysres.to_le_bytes());
    d[0x80..0x84].copy_from_slice(b"ACI0");
    d[0x90..0x98].copy_from_slice(&program.to_le_bytes());
    d
}

fn spec<'a>(syms: &'a [(&'a str, bool, u8)], extra: &'a [u8]) -> NsoSpec<'a> {
    NsoSpec {
        syms,
        extra_ro: extra,
        bss: 0,
        compress: true,
        check_hash: true,
        bid: [7; 0x20],
    }
}

#[test]
fn nso_loads_compressed_and_plain_and_checks_hashes() {
    for compress in [true, false] {
        let mut s = spec(&[("nnMain", true, 1), ("memcpy", false, 1)], b"hello extra");
        s.compress = compress;
        let m = nso::load_bytes(&nso(&s), "t").unwrap();
        assert_eq!(m.segments[0], (0, 0x1000));
        assert_eq!(m.segments[1].0, 0x1000);
        assert_eq!(m.bid_hex(), "07".repeat(16));
        let syms = dynsyms(&m.image).unwrap();
        assert_eq!(syms.len(), 2);
        assert!(syms[0].defined && syms[0].name == "nnMain" && syms[0].bind == 1);
        assert!(!syms[1].defined && syms[1].name == "memcpy");
        assert!(m.image.windows(11).any(|w| w == b"hello extra"));
    }
}

#[test]
fn nso_refusals() {
    let good = nso(&spec(&[("a", true, 1)], b""));
    let err = |b: &[u8]| nso::load_bytes(b, "t").unwrap_err().to_string();
    assert!(
        err(&good[..0x80]).contains("not an NSO or NRO")
            || err(&good[..0x80]).contains("cut short")
    );
    assert!(err(&good[..0x108]).contains("past the end of the file"));
    let mut bad_hash = good.clone();
    let last = bad_hash.len() - 1;
    bad_hash[last] ^= 0xff; // damages the compressed data segment's stream or bytes
    let e = err(&bad_hash);
    assert!(
        e.contains("does not decompress") || e.contains("SHA-256") || e.contains("decompresses to"),
        "{e}"
    );
    let mut overlap = good.clone();
    overlap[0x24..0x28].copy_from_slice(&0x800u32.to_le_bytes()); // ro at 0x800 (not page aligned)
    assert!(err(&overlap).contains("not on a page"));
    let mut zbic = good.clone();
    zbic[0xc] |= 0x80;
    assert!(err(&zbic).contains("ZBIC")); // LZ4 data is no ZBIC frame: the error says so, it is not a silent wrong image
    let mut text_off = good.clone();
    text_off[0x14..0x18].copy_from_slice(&0x1000u32.to_le_bytes());
    assert!(err(&text_off).contains("text segment is at"));
    assert!(err(b"\0\0\0\0").contains("not an NSO"));
}

fn exefs(dir: &Path, files: &[(&str, Vec<u8>)]) {
    fs::create_dir_all(dir).unwrap();
    for (n, b) in files {
        fs::write(dir.join(n), b).unwrap();
    }
}

fn manifest(dir: &Path, lines: &[&str]) -> PathBuf {
    let p = dir.join("romfs_manifest.tsv");
    fs::write(&p, format!("# path\tbytes\thead\n{}\n", lines.join("\n"))).unwrap();
    p
}

#[test]
fn facts_and_rules_on_a_tiny_game() {
    let d = tmp("tiny");
    let ex = d.join("ExeFS");
    let extra = b"NintendoSDK_nnSdk-16_2_0\0NintendoSDK_movie-3_1\0spec: Ounce\0";
    exefs(
        &ex,
        &[
            (
                "main",
                nso(&spec(
                    &[
                        ("nnMain", true, 1),
                        ("_ZN2nn2os17SetMemoryHeapSizeEm", false, 1),
                        ("movie_open", false, 1),
                        ("hooked_fn", false, 1),
                    ],
                    extra,
                )),
            ),
            ("rtld", nso(&spec(&[("x", false, 1)], b""))),
            ("sdk", nso(&spec(&[("sdk_fn", true, 1)], b"spec: Ounce"))),
            (
                "subsdk0",
                nso(&spec(&[("movie_open", true, 1)], b"NintendoSDK_movie-3_1")),
            ),
            (
                "subsdk1",
                nso(&spec(&[("hooked_fn", true, 1)], b"NintendoSDK_Vulkan-1_0")),
            ),
            ("subsdk2", nso(&spec(&[("unused", true, 1)], b""))),
            ("main.npdm", npdm(0, 0x0100AAAA00000000)),
            ("notes.txt", b"not a module".to_vec()),
        ],
    );
    let rom = manifest(
        &d,
        &[
            "/Data/a.assets\t5242880\t5a5a",
            "/Data/b.assets\t10\t5a5a",
            "/Other/x.mp4\t100\t6d70",
            "/hagi.config\t1\t00",
        ],
    );
    let f = facts_of(&ex, &rom).unwrap();
    assert_eq!(
        f.mods.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec!["main", "rtld", "sdk", "subsdk0", "subsdk1", "subsdk2"]
    );
    assert_eq!(f.sdk, 16);
    assert!(f.ounce);
    assert_eq!(f.program, Some(0x0100AAAA00000000));
    assert_eq!(f.romfs.biggest, (5242880, "Data/a.assets".to_string()));
    assert!(f.romfs.root.contains("hagi.config"));
    assert_eq!(
        f.romfs.dirs.most_common(5),
        vec![("Data".to_string(), 2), ("Other".to_string(), 1)]
    );
    assert_eq!(f.module("subsdk0").unwrap().kind, "NintendoSDK_movie");
    let hooks = HookSet::from_names(["hooked_fn".to_string()]);
    let g = guess(&f, &hooks).unwrap();
    // subsdk0 supplies movie_open (kept); subsdk1's only export is a host hook and it is a GPU driver; subsdk2 unused
    let (mods, why) = g.get("modules").unwrap();
    assert_eq!(mods, "main subsdk0 sdk");
    assert!(why.starts_with("not loaded, main takes nothing from them the host does not supply: subsdk1 (NintendoSDK_Vulkan), subsdk2 ("), "{why}");
    // no system resource + a heap: pool - 64 MB, Switch 2 pool (Ounce)
    assert_eq!(g.get("app_pool").unwrap().0, "0x240000000");
    assert_eq!(g.get("sys_resource").unwrap().0, "0x0");
    assert!(
        g.get("sys_resource")
            .unwrap()
            .1
            .ends_with("sets a heap only then)")
    );
    assert!(
        g.get("mem_reserve")
            .unwrap()
            .1
            .starts_with("main sets a heap of at most the pool \u{2212} 64 MB: a ")
    );
    assert_eq!(g.get("op_mode").unwrap().0, "handheld"); // Hagi preset
    assert_eq!(g.get("pad_layout").unwrap().0, "full");
    // a drop of the hook table: subsdk1 is then needed, but still a GPU driver, so still not loaded
    let g2 = guess(&f, &HookSet::default()).unwrap();
    assert_eq!(g2.get("modules").unwrap().0, "main subsdk0 sdk");
}

#[test]
fn derive_writes_the_provisional_profile_and_reuses_it() {
    let d = tmp("derive");
    let ex = d.join("extract");
    exefs(
        &ex.join("ExeFS"),
        &[
            (
                "main",
                nso(&spec(&[("nnMain", true, 1)], b"NintendoSDK_nnSdk-12_0_0")),
            ),
            ("sdk", nso(&spec(&[("f", true, 1)], b""))),
            ("main.npdm", npdm(0x1000000, 0x0100BBBB00000000)),
        ],
    );
    let rom = manifest(
        &ex,
        &[
            "/Data/a.resS\t9000000\t00",
            "/Data/b.dat\t1\t00",
            "/Data/c.dat\t1\t00",
        ],
    );
    let bundle = d.join("bundle");
    fs::create_dir_all(bundle.join("dm/games")).unwrap();
    fs::create_dir_all(bundle.join("dm/host")).unwrap();
    fs::create_dir_all(bundle.join("dm/gfx")).unwrap();
    let mut cfg = DeriveConfig::new(&ex, &bundle);
    cfg.romfs = Some(rom.clone());
    cfg.profile = Some(d.join("out").join("profile.tsv"));
    cfg.profile_out = Some(d.join("derived").join("0100BBBB00000000"));
    let mut log = Vec::new();
    let out = derive(&cfg, &mut |m| log.push(m.to_string())).unwrap();
    assert_eq!(out, d.join("out/profile.tsv"));
    assert!(
        log[0].contains("no engine table for 0100BBBB00000000"),
        "{log:?}"
    );
    let game = fs::read_to_string(d.join("derived/0100BBBB00000000/game.tsv")).unwrap();
    assert!(
        game.contains("\nname\t0100BBBB00000000\n")
            && game.contains("\nipa\t0100bbbb00000000.ipa\n"),
        "{game}"
    ); // no control.nacp: the id stands in (ipa_name capitalizes an all-caps word: 0100bbbb...)
    assert!(game.contains("# 0100BBBB00000000, SDK 12, derived from "));
    assert!(
        game.contains("\nromfs_marker\tData/a.resS\n")
            && game.contains("\nzip_store\t.resS:.metallib\n")
            || game.contains("\nzip_store\t.dat:.metallib\n")
            || game.contains(".resS"),
        "{game}"
    );
    assert!(game.contains("\nshader_groups\t/Data/=data\n"));
    assert!(
        game.contains("\nbuild\tv0\t?\tmain=")
            && game.contains(
                "\n# the NPDM declares 16 MB of system resource\nsys_resource\t0x1000000\n"
            )
    );
    let prof = fs::read_to_string(d.join("out/profile.tsv")).unwrap();
    assert!(
        prof.contains("\nprogram_id\t0x0100BBBB00000000\n")
            && prof.contains("\nmodules\tmain sdk\n")
            && prof.contains("\nmem_reserve\t0x140000000\n"),
        "{prof}"
    );
    assert!(
        fs::read_to_string(d.join("derived/0100BBBB00000000/shader_map.tsv"))
            .unwrap()
            .starts_with("# kind: evidence")
    );
    // a second run reuses the provisional table
    let mut log2 = Vec::new();
    derive(&cfg, &mut |m| log2.push(m.to_string())).unwrap();
    assert!(
        log2[0].starts_with("profile: reusing the provisional table already derived"),
        "{log2:?}"
    );
    // a registered game: the engine's table is rendered as it is, nothing is derived
    let reg = bundle.join("dm/games/0100BBBB00000000");
    fs::create_dir_all(&reg).unwrap();
    fs::write(reg.join("game.tsv"), "title_id\t0100BBBB00000000\nprogram_id\t0100BBBB00000000\nname\tReg\nromfs_marker\tx\nmodules\tmain\nmem_reserve\t0x40000000\nmem_alias\t0x10000000\nmem_stack\t0x10000000\napp_pool\t0xCD500000\nbuild\tv1\t9.9\tmain=beef\n").unwrap();
    let mut log3 = Vec::new();
    derive(&cfg, &mut |m| log3.push(m.to_string())).unwrap();
    assert!(log3[1].contains("(registered, not derived)"), "{log3:?}");
    assert!(
        fs::read_to_string(d.join("out/profile.tsv"))
            .unwrap()
            .contains("\nname\tReg\n")
    );
}

#[test]
fn nacp_name_and_version() {
    use ibuilder_profile::nacp;
    let d = tmp("nacp");
    let mut b = vec![0u8; 0x4000];
    let name = "Grizzy \u{2122}";
    b[0x300..0x300 + name.len()].copy_from_slice(name.as_bytes());
    b[0x3060..0x3060 + 7].copy_from_slice(b"1.2.3.4");
    let p = d.join("control.nacp");
    fs::write(&p, &b).unwrap();
    assert_eq!(nacp::name(&p).unwrap(), "Grizzy \u{2122}");
    assert_eq!(nacp::version(&p).unwrap(), "1.2.3"); // at most three components
    assert_eq!(nacp::name(&d.join("missing")).unwrap(), "");
    // an unprintable first title makes the name empty (the Python: the first non-empty entry is not printable)
    b[0x300] = 0x01;
    fs::write(&p, &b).unwrap();
    assert_eq!(nacp::name(&p).unwrap(), "");
}

#[test]
fn romfs_listing_from_a_dmromfs1_image() {
    use ibuilder_profile::romfs::romfs_for_each;
    let d = tmp("romfs");
    // index: root(dir, children 1..4), "Data"(dir children 4..5), "a.bin"(file), "z.txt"(file), "x.dat"(file in Data)
    let data = b"HEADER0123456789012345678901234567890123".to_vec(); // 40 bytes: file a.bin
    let mut ents: Vec<(u64, u64, u32, u16, u8)> = Vec::new();
    let names = ["", "Data", "a.bin", "z.txt", "x.dat"];
    let mut pool = Vec::new();
    let mut offs = Vec::new();
    for n in names {
        offs.push((pool.len() as u32, n.len() as u16));
        pool.extend_from_slice(n.as_bytes());
    }
    let f0 = 32u64; // file data region starts after the header
    ents.push((1, 3, offs[0].0, offs[0].1, 0)); // root: Data, a.bin, z.txt
    ents.push((4, 1, offs[1].0, offs[1].1, 0)); // Data: x.dat
    ents.push((f0, 40, offs[2].0, offs[2].1, 1));
    ents.push((f0 + 40, 4, offs[3].0, offs[3].1, 1));
    ents.push((f0 + 44, 3, offs[4].0, offs[4].1, 1));
    let mut index = Vec::new();
    for (a, b, no, nl, t) in &ents {
        index.extend_from_slice(&a.to_le_bytes());
        index.extend_from_slice(&b.to_le_bytes());
        index.extend_from_slice(&no.to_le_bytes());
        index.extend_from_slice(&nl.to_le_bytes());
        index.push(*t);
        index.push(0);
    }
    index.extend_from_slice(&pool);
    let body: Vec<u8> = [data.clone(), b"ABCD".to_vec(), b"xyz".to_vec()].concat();
    let off = 32 + body.len() as u64;
    let mut img = Vec::new();
    img.extend_from_slice(b"DMROMFS1");
    img.extend_from_slice(&off.to_le_bytes());
    img.extend_from_slice(&(index.len() as u64).to_le_bytes());
    img.extend_from_slice(&(ents.len() as u64).to_le_bytes());
    img.extend(&body);
    img.extend(&index);
    let p = d.join("RomFS.img");
    fs::write(&p, &img).unwrap();
    let mut seen = Vec::new();
    romfs_for_each(&p, &mut |path, size, head| {
        seen.push((path.to_string(), size, head.len()))
    })
    .unwrap();
    // os.walk order: the root's files (a.bin, z.txt) first, then Data's
    assert_eq!(
        seen,
        vec![
            ("/a.bin".to_string(), 40, 32),
            ("/z.txt".to_string(), 4, 4),
            ("/Data/x.dat".to_string(), 3, 3)
        ]
    );
    // damaged: unsorted names
    let mut bad = img.clone();
    let swap = bad.len() - pool.len();
    bad[swap + 4..swap + 9].copy_from_slice(b"~~~~~"); // renames "a.bin" so it sorts after "z.txt"
    fs::write(d.join("bad.img"), &bad).unwrap();
    let e = romfs_for_each(&d.join("bad.img"), &mut |_, _, _| {})
        .unwrap_err()
        .to_string();
    assert!(e.contains("unsorted or duplicate RomFS image names"), "{e}");
    let mut trunc = img.clone();
    trunc.truncate(img.len() - 5);
    fs::write(d.join("trunc.img"), &trunc).unwrap();
    assert!(romfs_for_each(&d.join("trunc.img"), &mut |_, _, _| {}).is_err());
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

#[test]
fn nso_loads_zbic_segments_and_checks_them() {
    let f = zbic_nso_bytes();
    let m = nso::load_bytes(&f, "t").unwrap();
    for i in 0..3 {
        let (mo, sz) = m.segments[i];
        assert_eq!(
            (mo as usize, &m.image[mo as usize..(mo + sz) as usize]),
            (0x2000 * i, &zbic_plain()[..])
        );
    }
    let mut bad = f.clone();
    let n = bad.len();
    bad[n - 5] ^= 0x10;
    let e = nso::load_bytes(&bad, "t").unwrap_err().to_string();
    assert!(e.contains("ZBIC") || e.contains("SHA-256"), "{e}");
}
