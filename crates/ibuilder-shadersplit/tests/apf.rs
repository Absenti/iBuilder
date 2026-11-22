//! Vector Unit APF packages (`FPUV`, version 7: Beach Buggy Racing 2), end to end on a synthetic package: entries stored, zlib and LZ4, the directory and header sums, the refusals.
//! No game data: the package and the GLSLC outputs in it are generated here.

use ibuilder_shadersplit::apf::fnv1a32;
use ibuilder_shadersplit::ctl::*;
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GAME: &str = "0100TESTTEST0000";

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-apf-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lcg(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

/// a control section + code area of stage `st` (as tests/synthetic.rs makes them)
fn shader(seed: u64, st: St) -> (Vec<u8>, Vec<u8>) {
    let (n, ncode) = (0x850usize, 0x100usize);
    let mut code = lcg(seed, ncode);
    let mut ctl = lcg(seed + 1, n);
    ctl[..4].copy_from_slice(&CTL_MAGIC);
    ctl[0x20..0x24].copy_from_slice(&(n as u32).to_le_bytes());
    ctl[0x704..0x708].copy_from_slice(&(ncode as u32).to_le_bytes());
    for buf in [&mut ctl[4..], &mut code[..]] {
        let mut i = 0;
        while let Some(p) = buf[i..].windows(4).position(|w| w == CTL_MAGIC) {
            buf[i + p] ^= 0x55;
            i += p + 1;
        }
    }
    code[..4].copy_from_slice(&GFX_MAGIC);
    let w: u32 = if st == St::Vs { 0x461 } else { 1 | (5 << 10) };
    code[0x30..0x34].copy_from_slice(&w.to_le_bytes());
    (ctl, code)
}

/// a GLSLC output of one stage (tests/synthetic.rs)
fn glslc(seed: u64, st: St) -> Vec<u8> {
    let (ctl, code) = shader(seed, st);
    let hdr = 0x90 + 0x90;
    let total = hdr + ctl.len() + code.len();
    let mut d = vec![0u8; total];
    d[..4].copy_from_slice(&GLSLC_MAGIC);
    d[0x44..0x48].copy_from_slice(&(total as u32).to_le_bytes());
    d[0x4c..0x50].copy_from_slice(&1u32.to_le_bytes());
    d[0x90..0x94].copy_from_slice(&((ctl.len() + code.len()) as u32).to_le_bytes());
    d[0x94..0x98].copy_from_slice(&(hdr as u32).to_le_bytes());
    d[0x90 + 0x34..0x90 + 0x38].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
    d[0x90 + 0x38..0x90 + 0x3c].copy_from_slice(&(code.len() as u32).to_le_bytes());
    d[0x90 + 0x3c..0x90 + 0x40].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
    d[hdr..hdr + ctl.len()].copy_from_slice(&ctl);
    d[hdr + ctl.len()..].copy_from_slice(&code);
    d
}

/// a VuShaderAsset's data as the game has it: a 0x18-byte header (count, lengths, a sum), then the GLSLC outputs
fn shader_asset(outputs: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = outputs.concat();
    let mut d = Vec::new();
    for w in [
        outputs.len() as u32,
        body.len() as u32,
        0x7403_b5d1,
        body.len() as u32,
        0xbfb5_a3fa,
        body.len() as u32 - 0x10,
    ] {
        d.extend_from_slice(&w.to_le_bytes());
    }
    d.extend_from_slice(&body);
    d
}

fn zlib(d: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(d).unwrap();
    z.finish().unwrap()
}

/// One entry: name, decoded data, compression (0 stored, 1 zlib, 5 LZ4), and the stored bytes when the test gives them itself
struct E<'a>(&'a str, Vec<u8>, u16, Option<Vec<u8>>);

/// an APF package, version 7: header, the entries' data, the directory
fn apf(entries: &[E<'_>]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut dir = Vec::new();
    for E(name, raw, comp, stored) in entries {
        let packed = match (stored, comp) {
            (Some(s), _) => s.clone(),
            (None, 1) => zlib(raw),
            (None, 5) => lz4_flex::block::compress(raw),
            (None, _) => raw.clone(),
        };
        dir.extend_from_slice(name.as_bytes());
        dir.push(0);
        for w in [
            0x3c + data.len() as u32,
            raw.len() as u32,
            packed.len() as u32,
            fnv1a32(raw),
        ] {
            dir.extend_from_slice(&w.to_le_bytes());
        }
        dir.extend_from_slice(&32u16.to_le_bytes());
        dir.extend_from_slice(&comp.to_le_bytes());
        data.extend_from_slice(&packed);
    }
    let mut h = Vec::new();
    h.extend_from_slice(b"FPUV");
    for w in [
        7u32,
        0x3c + data.len() as u32,
        entries.len() as u32,
        dir.len() as u32,
        fnv1a32(&dir),
    ] {
        h.extend_from_slice(&w.to_le_bytes());
    }
    let mut plat = b"Switch".to_vec();
    plat.resize(32, 0);
    h.extend_from_slice(&plat);
    let s = fnv1a32(&h);
    h.extend_from_slice(&s.to_le_bytes());
    assert_eq!(h.len(), 0x3c);
    [h, data, dir].concat()
}

/// a package with every compression and a texture of another compression; the GLSLC outputs' keys, in the order of the entries
fn package() -> (Vec<u8>, Vec<(St, String)>) {
    let rules = KeyRules::default();
    let outs = [
        (900u64, St::Vs),
        (910, St::Ps),
        (920, St::Vs),
        (930, St::Ps),
    ];
    let keys = outs
        .iter()
        .map(|&(s, st)| {
            let (ctl, code) = shader(s, st);
            (st, skey(&rules, &ctl, &code).unwrap())
        })
        .collect();
    let g: Vec<Vec<u8>> = outs.iter().map(|&(s, st)| glslc(s, st)).collect();
    let catalogue = b"\x01\0\0\0en\0\x01\0\0\0VuShaderAsset\0\x01\0\0\0Fx/A\0".to_vec();
    let d = apf(&[
        E("Assets/AssetData", catalogue, 1, None),
        E(
            "VuShaderAsset/Fx/A",
            shader_asset(&[g[0].clone(), g[1].clone()]),
            5,
            None,
        ),
        E(
            "VuShaderAsset/Fx/B",
            shader_asset(std::slice::from_ref(&g[2])),
            1,
            None,
        ),
        E("VuShaderAsset/Fx/Empty", Vec::new(), 5, Some(Vec::new())),
        E("VuTextureAsset/T", lcg(3, 0x400), 9, None),
        E(
            "VuShaderAsset/Fx/C",
            shader_asset(std::slice::from_ref(&g[3])),
            0,
            None,
        ),
    ]);
    (d, keys)
}

fn run_split(
    name: &str,
    apf_bytes: &[u8],
    jobs: usize,
) -> (ibuilder_shadersplit::SplitStats, Vec<String>, PathBuf) {
    let dir = temp(name);
    let romfs = dir.join("RomFS");
    std::fs::create_dir_all(&romfs).unwrap();
    std::fs::write(romfs.join("Assets.apf"), apf_bytes).unwrap();
    // one shader outside the package, so that a split whose package is left out still has one
    std::fs::write(romfs.join("loose.bin"), glslc(950, St::Vs)).unwrap();
    let main = dir.join("main");
    let mut nso = vec![0u8; 0x100];
    nso[..4].copy_from_slice(b"NSO0");
    std::fs::write(&main, nso).unwrap();
    let prof = dir.join("profile");
    std::fs::create_dir_all(prof.join(GAME)).unwrap();
    std::fs::write(
        prof.join(GAME).join("game.tsv"),
        format!("title_id\t{GAME}\nshader_groups\t/=romfs\nshader_note\tsynthetic APF package\n"),
    )
    .unwrap();
    let o = dir.join("out");
    let (pkg, tables) = (o.join("pkg"), o.join("tables"));
    let mut opts = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, GAME);
    opts.jobs = jobs;
    let log = Mutex::new(Vec::new());
    let r = split(&opts, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let log = log.into_inner().unwrap();
    (r.unwrap_or_else(|e| panic!("{e}: {log:?}")), log, dir)
}

fn map_rows(dir: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(dir.join("out/tables/shader_map.tsv"))
        .unwrap()
        .lines()
        .skip(3)
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

#[test]
fn an_apf_package_gives_the_glslc_outputs_of_its_entries() {
    let (d, keys) = package();
    let (stats, log, dir) = run_split("ok", &d, 3);
    assert_eq!((stats.shaders, stats.unreadable), (4 + 1, 0), "{log:?}");
    let rows: Vec<Vec<String>> = map_rows(&dir)
        .into_iter()
        .filter(|r| r[4].starts_with("/Assets.apf/"))
        .collect();
    let mut got: Vec<(String, String)> =
        rows.iter().map(|r| (r[3].clone(), r[4].clone())).collect();
    got.sort();
    let mut want: Vec<(String, String)> = [
        "/Assets.apf/VuShaderAsset/Fx/A#GLSLC@0x18",
        &format!(
            "/Assets.apf/VuShaderAsset/Fx/A#GLSLC@{:#x}",
            0x18 + glslc(900, St::Vs).len()
        ),
        "/Assets.apf/VuShaderAsset/Fx/B#GLSLC@0x18",
        "/Assets.apf/VuShaderAsset/Fx/C#GLSLC@0x18",
    ]
    .iter()
    .zip(&keys)
    .map(|(src, (_, k))| (k.clone(), src.to_string()))
    .collect();
    want.sort();
    assert_eq!(got, want, "{log:?}");
    for (r, st) in rows.iter().map(|r| (r, r[2].as_str())) {
        let k = keys.iter().find(|(_, k)| *k == r[3]).unwrap();
        assert_eq!(st, if k.0 == St::Vs { "vertex" } else { "pixel" });
    }
    // the texture's compression 9 is counted, once for the package
    let warns: Vec<&String> = log
        .iter()
        .filter(|l| l.starts_with("[WARN] /Assets.apf:"))
        .collect();
    assert_eq!(warns.len(), 1, "{log:?}");
    assert!(
        warns[0].contains("1 entries not read") && warns[0].contains("9 × 1"),
        "{}",
        warns[0]
    );
    assert!(!log.iter().any(|l| l.starts_with("[RED]")), "{log:?}");
    // the same bytes for one thread
    let (_, _, dir1) = run_split("ok1", &d, 1);
    for f in [
        "pkg/vs_romfs.bin",
        "pkg/ps_romfs.bin",
        "tables/shader_map.tsv",
        "tables/shader_pairs.tsv",
    ] {
        assert_eq!(
            std::fs::read(dir.join("out").join(f)).unwrap(),
            std::fs::read(dir1.join("out").join(f)).unwrap(),
            "{f}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir1);
}

#[test]
fn a_damaged_apf_package_is_left_out_with_its_reason() {
    let (good, _) = package();
    let dir_at = u32::from_le_bytes(good[8..12].try_into().unwrap()) as usize;
    let rehash = |mut d: Vec<u8>| {
        let s = fnv1a32(&d[..0x38]);
        d[0x38..0x3c].copy_from_slice(&s.to_le_bytes());
        d
    };
    let mut version = good.clone();
    version[4] = 8;
    let mut header_sum = good.clone();
    header_sum[0x20] ^= 1;
    let mut dir_sum = good.clone();
    dir_sum[dir_at + 3] ^= 1;
    let mut outside = good.clone();
    outside[0x10..0x14].copy_from_slice(&(good.len() as u32).to_le_bytes());
    let mut count = good.clone();
    count[0xc..0x10].copy_from_slice(&u32::MAX.to_le_bytes());
    let one = |comp: u16, raw: Vec<u8>, stored: Vec<u8>| {
        apf(&[E("VuShaderAsset/X", raw, comp, Some(stored))])
    };
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "version",
            rehash(version),
            "Vector Unit APF version 8 (only version 7 is read)",
        ),
        (
            "header",
            header_sum,
            "the APF header does not match its FNV-1a sum",
        ),
        (
            "dir",
            dir_sum,
            "the APF directory does not match its FNV-1a sum",
        ),
        ("outside", rehash(outside), "lies outside the file"),
        ("count", rehash(count), "more than fit in its"),
        (
            "bomb",
            one(5, vec![0; 0x10000], vec![0; 4]),
            "more than an LZ4 block can hold (255 to 1)",
        ),
        (
            "lz4",
            one(5, vec![0; 0x100], vec![0xff; 0x40]),
            "stored bytes are not an LZ4 block",
        ),
        (
            "zlib",
            one(1, vec![0; 0x100], b"not zlib".to_vec()),
            "invalid zlib stream",
        ),
        (
            "stored",
            one(0, vec![0; 0x10], b"1234".to_vec()),
            "stored bytes, the entry says 0x10",
        ),
        (
            "short",
            one(1, vec![0; 0x100], zlib(&[0; 0x80])),
            "0x80 bytes decoded, the entry says 0x100",
        ),
    ];
    for (name, d, why) in cases {
        let (stats, log, dir) = run_split(&format!("bad-{name}"), &d, 2);
        assert_eq!((stats.shaders, stats.unreadable), (1, 1), "{name}: {log:?}");
        assert!(
            log.iter().any(|l| l.starts_with("[RED] /Assets.apf")
                && l.contains(why)
                && l.ends_with("(its shaders are left out)")),
            "{name}: {log:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
