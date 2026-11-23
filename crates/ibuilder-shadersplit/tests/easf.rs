//! EA's archives (easf.rs): BIG4 → EASF (AES-128-CBC, the key found in main by its ID record) → chunk container → BIG4 of NXSHADER `.fxo` → GLSLC outputs, end to end on synthetic
//! fixtures made here with a test key. No game data, no real key.

use ibuilder_shadersplit::aes::Aes128;
use ibuilder_shadersplit::ctl::*;
use ibuilder_shadersplit::easf;
use ibuilder_shadersplit::record::classify_source;
use ibuilder_shadersplit::source::MemSource;
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GAME: &str = "0100TESTTEST0000";
const KEY: [u8; 16] = *b"0123456789abcdef"; // a test key, nothing else
const KEY_ID: &[u8; 8] = b"testkey ";

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
    if st == St::Cs {
        code[..4].copy_from_slice(&CS_MAGIC);
        ctl[0x748..0x758].copy_from_slice(&[64, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
    } else {
        code[..4].copy_from_slice(&GFX_MAGIC);
        let w: u32 = if st == St::Vs { 0x461 } else { 1 | (5 << 10) };
        code[0x30..0x34].copy_from_slice(&w.to_le_bytes());
    }
    (ctl, code)
}

/// a GLSLC output of the given stages (one program), with its section headers
fn glslc(stages: &[(u64, St)]) -> Vec<u8> {
    let hdr = 0x90 + 0x90 * stages.len();
    let shaders: Vec<_> = stages.iter().map(|&(s, st)| shader(s, st)).collect();
    let total = hdr
        + shaders
            .iter()
            .map(|(c, k)| c.len() + k.len())
            .sum::<usize>();
    let mut d = vec![0u8; hdr];
    d[..4].copy_from_slice(&GLSLC_MAGIC);
    d[0x44..0x48].copy_from_slice(&(total as u32).to_le_bytes());
    d[0x4c..0x50].copy_from_slice(&(stages.len() as u32).to_le_bytes());
    for (i, (ctl, code)) in shaders.iter().enumerate() {
        let s = 0x90 + 0x90 * i;
        let at = d.len();
        d[s..s + 4].copy_from_slice(&((ctl.len() + code.len()) as u32).to_le_bytes());
        d[s + 4..s + 8].copy_from_slice(&(at as u32).to_le_bytes());
        d[s + 0x34..s + 0x38].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
        d[s + 0x38..s + 0x3c].copy_from_slice(&(code.len() as u32).to_le_bytes());
        d[s + 0x3c..s + 0x40].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
        d.extend_from_slice(ctl);
        d.extend_from_slice(code);
    }
    assert_eq!(d.len(), total);
    d
}

/// NXSHADER: magic, version, `GLSL` + the size of the GLSLC output at +0x18, then `REFL`
fn fxo(stages: &[(u64, St)]) -> Vec<u8> {
    let g = glslc(stages);
    let mut d = b"NXSHADER".to_vec();
    d.extend_from_slice(&[1, 8, 0, 0, 0, 0, 0, 0]);
    d.extend_from_slice(b"GLSL");
    d.extend_from_slice(&(g.len() as u32).to_le_bytes());
    d.extend_from_slice(&g);
    d.extend_from_slice(b"REFL");
    d.extend_from_slice(&lcg(7, 0x40));
    d
}

/// BIG4 of `members`, data 16-aligned after the index
fn big4(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let index: usize = 16 + members.iter().map(|(n, _)| 9 + n.len()).sum::<usize>();
    let mut at = index.div_ceil(16) * 16;
    let mut table = Vec::new();
    let mut data = Vec::new();
    for (name, m) in members {
        table.extend_from_slice(&(at as u32).to_be_bytes());
        table.extend_from_slice(&(m.len() as u32).to_be_bytes());
        table.extend_from_slice(name.as_bytes());
        table.push(0);
        data.extend_from_slice(m);
        let pad = (16 - m.len() % 16) % 16;
        data.extend(std::iter::repeat_n(0u8, pad));
        at += m.len() + pad;
    }
    let mut d = b"BIG4".to_vec();
    d.extend_from_slice(&((index.div_ceil(16) * 16 + data.len()) as u32).to_le_bytes());
    d.extend_from_slice(&(members.len() as u32).to_be_bytes());
    d.extend_from_slice(&(index as u32).to_be_bytes());
    d.extend_from_slice(&table);
    d.resize(index.div_ceil(16) * 16, 0);
    d.extend_from_slice(&data);
    d
}

/// a chunk container of `d` in blocks of `chunk` bytes, each of type `typ` (0: stored as is)
fn chunked(magic: &[u8; 8], d: &[u8], chunk: usize, typ: u32) -> Vec<u8> {
    let count = d.len().div_ceil(chunk);
    let mut out = magic.to_vec();
    for v in [2u32, d.len() as u32, chunk as u32, count as u32, 16] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&[0u8; 12]);
    for c in d.chunks(chunk) {
        let packed = match typ {
            0 => c.to_vec(),
            1 => {
                let mut e =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(c).unwrap();
                e.finish().unwrap()
            }
            6 => ruzstd::encoding::compress_to_vec(c, ruzstd::encoding::CompressionLevel::Fastest),
            _ => lcg(c.len() as u64, 40), // an LZMA / RefPack block (never decoded)
        };
        while !(out.len() + 8).is_multiple_of(16) {
            out.push(0);
        }
        out.extend_from_slice(&(packed.len() as u32).to_be_bytes());
        out.extend_from_slice(&typ.max(1).to_be_bytes());
        out.extend_from_slice(&packed);
    }
    out
}

fn easf_of(plain: &[u8], key: &[u8; 16], id: &[u8; 8]) -> Vec<u8> {
    let mut d = b"EASF".to_vec();
    d.extend_from_slice(&(plain.len() as u32).to_be_bytes());
    d.extend_from_slice(id);
    d.extend_from_slice(&Sha256::digest(&plain[..32]));
    d.extend_from_slice(&Aes128::new(key).cbc_encrypt_pkcs7(key, plain));
    d
}

/// an uncompressed NSO with `text` as its text segment
fn nso(text: &[u8]) -> Vec<u8> {
    let mut h = vec![0u8; 0x100];
    h[..4].copy_from_slice(b"NSO0");
    let end = ((text.len() + 0xfff) & !0xfff) as u32;
    for (i, (fo, mo, sz)) in [
        (0x100u32, 0u32, text.len() as u32),
        (0x100 + text.len() as u32, end, 0),
        (0x100 + text.len() as u32, end, 0),
    ]
    .into_iter()
    .enumerate()
    {
        h[0x10 + i * 0x10..0x14 + i * 0x10].copy_from_slice(&fo.to_le_bytes());
        h[0x14 + i * 0x10..0x18 + i * 0x10].copy_from_slice(&mo.to_le_bytes());
        h[0x18 + i * 0x10..0x1c + i * 0x10].copy_from_slice(&sz.to_le_bytes());
    }
    h.extend_from_slice(text);
    h
}

/// main's text: junk, a record of the key ID with a wrong key (a decoy the digest refuses), then the real record
fn main_text(with_key: bool) -> Vec<u8> {
    let mut t = lcg(1, 0x1000);
    t.extend_from_slice(KEY_ID);
    t.extend_from_slice(b"fedcba9876543210");
    t.extend_from_slice(&lcg(2, 0x40));
    if with_key {
        t.extend_from_slice(KEY_ID);
        t.extend_from_slice(&KEY);
    }
    t.extend_from_slice(&lcg(3, 0x100));
    t
}

/// the shader archive as FIFA 18 has it: an EASF of a chunkzip of a BIG4 of .fxo files
fn shader_archive(typ: u32) -> Vec<u8> {
    let inner = big4(&[
        ("a.fxo", fxo(&[(10, St::Vs), (20, St::Ps)])),
        ("b.fxo", fxo(&[(30, St::Vs), (20, St::Ps)])),
        ("readme.txt", lcg(4, 100)),
        ("c.fxo", fxo(&[(40, St::Cs)])),
    ]);
    easf_of(&chunked(b"chunkzip", &inner, 0x1000, typ), &KEY, KEY_ID)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-easf-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Run {
    shaders: usize,
    log: Vec<String>,
    out: PathBuf,
}

fn run(dir: &Path, files: &[(&str, Vec<u8>)], main: &[u8], jobs: usize, out: &str) -> Run {
    let romfs = dir.join("RomFS");
    let _ = std::fs::remove_dir_all(&romfs);
    for (rel, b) in files {
        let p = romfs.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b).unwrap();
    }
    let mainp = dir.join("main");
    std::fs::write(&mainp, nso(main)).unwrap();
    let prof = dir.join("profile");
    std::fs::create_dir_all(prof.join(GAME)).unwrap();
    std::fs::write(
        prof.join(GAME).join("game.tsv"),
        format!("title_id\t{GAME}\nshader_groups\t/data/=data\nshader_note\tsynthetic EA game\n"),
    )
    .unwrap();
    let o = dir.join(out);
    let _ = std::fs::remove_dir_all(&o);
    let (pkg, tables, rec) = (o.join("pkg"), o.join("tables"), o.join("split.record"));
    let mut opts = SplitOptions::single(&romfs, &mainp, &pkg, &tables, &prof, GAME);
    opts.jobs = jobs;
    opts.record = Some(&rec);
    let log = Mutex::new(Vec::new());
    let r = split(&opts, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let log = log.into_inner().unwrap();
    Run {
        shaders: r.unwrap_or_else(|e| panic!("{e}: {log:?}")).shaders,
        log,
        out: o,
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn game_files(typ: u32) -> Vec<(&'static str, Vec<u8>)> {
    let audio = lcg(9, 3000);
    vec![
        (
            "datax.big",
            big4(&[
                ("data/x/shader.big", shader_archive(typ)),
                ("audio/bank.sbs", audio.clone()),
                (
                    "data/x/config.ini",
                    easf_of(
                        &chunked(
                            b"chunzstd",
                            b"[section]\nkey=value\n0123456789012345678901234567890123",
                            16,
                            6,
                        ),
                        &KEY,
                        KEY_ID,
                    ),
                ),
            ]),
        ),
        (
            "data1.big",
            big4(&[
                ("tex/a.rx3", chunked(b"chunzstd", &lcg(5, 5000), 0x800, 6)),
                ("tex/b.rx3", chunked(b"chunlzma", &lcg(6, 5000), 0x800, 5)),
                ("tex/c.rx3", chunked(b"chunkref", &lcg(6, 5000), 0x800, 2)),
            ]),
        ),
        ("data/movie.vp6", audio),
    ]
}

#[test]
fn an_ea_shader_archive_is_split_end_to_end() {
    let dir = temp("e2e");
    let r = run(&dir, &game_files(1), &main_text(true), 4, "o4");
    assert_eq!(r.shaders, 4, "{:?}", r.log);
    let map = read(&r.out.join("tables/shader_map.tsv"));
    let labels: Vec<&str> = map
        .lines()
        .filter(|l| !l.starts_with('#'))
        .skip(1)
        .map(|l| l.rsplit('\t').next().unwrap())
        .collect();
    assert!(
        labels.contains(&"/datax.big#data/x/shader.big#a.fxo#NXSHADER#GLSLC@0x18#var"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"/datax.big#data/x/shader.big#c.fxo#NXSHADER#GLSLC@0x18"),
        "{labels:?}"
    );
    // the two programs pair as they are compiled: (vs 10, ps 20) and (vs 30, ps 20); the compute shader pairs with nothing
    let pairs = read(&r.out.join("tables/shader_pairs.tsv"));
    assert_eq!(
        pairs.lines().filter(|l| !l.starts_with('#')).count(),
        2,
        "{pairs}"
    );
    assert!(
        !r.log.iter().any(|l| l.contains("single-stage")),
        "{:?}",
        r.log
    );
    // the key is named by its fingerprint only, the decoy is a candidate the digest refuses
    let fp = easf::fingerprint(&KEY);
    let note = r
        .log
        .iter()
        .find(|l| l.contains("key ID \"testkey \""))
        .unwrap_or_else(|| panic!("{:?}", r.log));
    assert!(
        note.contains(&fp) && note.contains("2 candidate(s)"),
        "{note}"
    );
    let key_hex: String = KEY.iter().map(|b| format!("{b:02x}")).collect();
    for l in &r.log {
        assert!(
            !l.contains(&key_hex) && !l.contains("0123456789abcdef"),
            "{l}"
        );
    }
    // the RomFS's chunzstd / chunlzma / chunkref members are counted, not opened
    for m in ["chunzstd", "chunlzma", "chunkref"] {
        assert!(
            r.log.iter().any(|l| l.starts_with(&format!(
                "EA archive: 1 {m} container(s) in the RomFS not opened"
            ))),
            "{m}: {:?}",
            r.log
        );
    }
    assert!(!r.log.iter().any(|l| l.contains("[RED]")), "{:?}", r.log);
    // the record: the EA reader was used, its BIG4 files are no unknown kind
    let rec = read(&r.out.join("split.record"));
    assert!(rec.contains("uses\tea generic\n"), "{rec}");
    assert!(!rec.contains(".big#"), "{rec}");
    // the same bytes with one thread, and with zstd blocks instead of deflate
    let one = run(&dir, &game_files(1), &main_text(true), 1, "o1");
    let zs = run(&dir, &game_files(6), &main_text(true), 2, "oz");
    let st = run(&dir, &game_files(0), &main_text(true), 2, "os");
    for o in [&one, &zs, &st] {
        for f in [
            "pkg/vs_other.bin",
            "pkg/ps_other.bin",
            "pkg/cs_other.bin",
            "tables/shader_map.tsv",
            "tables/shader_pairs.tsv",
        ] {
            assert_eq!(
                std::fs::read(r.out.join(f)).unwrap(),
                std::fs::read(o.out.join(f)).unwrap(),
                "{f}"
            );
        }
    }
    assert_eq!(
        r.log
            .iter()
            .filter(|l| !l.starts_with("progress"))
            .collect::<Vec<_>>(),
        one.log
            .iter()
            .filter(|l| !l.starts_with("progress"))
            .collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_key_main_does_not_hold_or_an_lzma_archive_leaves_only_that_archive_out() {
    let dir = temp("nokey");
    let mut files = game_files(1);
    files.push(("loose.fxo", fxo(&[(50, St::Vs), (60, St::Ps)])));
    let r = run(&dir, &files, &main_text(false), 2, "o");
    assert_eq!(r.shaders, 2, "{:?}", r.log);
    let red: Vec<&String> = r.log.iter().filter(|l| l.starts_with("[RED]")).collect();
    assert!(
        red.iter().any(|l| l.contains(
            "/datax.big#data/x/shader.big: no record of EASF key ID \"testkey \" in main"
        ) && l.contains("(1 candidate(s))")),
        "{red:?}"
    );
    assert!(
        r.log
            .iter()
            .any(|l| l.contains("file(s) could not be read")),
        "{:?}",
        r.log
    );
    // an EASF archive whose payload is LZMA: refused with the reason, the loose program still read
    let mut files = vec![("loose.fxo", fxo(&[(50, St::Vs), (60, St::Ps)]))];
    let inner = big4(&[("a.fxo", fxo(&[(10, St::Vs), (20, St::Ps)]))]);
    let mut lz = chunked(b"chunkzip", &inner, 0x1000, 1);
    // the second block claims LZMA: the first block's head (BIG4) makes it worth decoding, the second cannot be
    let second = {
        let h = easf::chunk_header(&lz).unwrap();
        assert!(h.count >= 2);
        let mut o = 0x28usize;
        let sz = u32::from_be_bytes(lz[o..o + 4].try_into().unwrap()) as usize;
        o += 8 + sz;
        (o + 8).div_ceil(16) * 16 - 8
    };
    lz[second + 4..second + 8].copy_from_slice(&5u32.to_be_bytes());
    files.push(("datax.big", big4(&[("s.big", easf_of(&lz, &KEY, KEY_ID))])));
    let r = run(&dir, &files, &main_text(true), 2, "o2");
    assert_eq!(r.shaders, 2, "{:?}", r.log);
    assert!(
        r.log.iter().any(|l| l.starts_with("[RED]")
            && l.contains("chunkzip: block 1: block type 5 (LZMA) is not decoded by this port")),
        "{:?}",
        r.log
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn chunk_containers_decode_and_peek() {
    let d = lcg(11, 10_000);
    for typ in [0u32, 1, 6] {
        let c = chunked(b"chunkzip", &d, 0x1000, typ);
        assert_eq!(easf::chunk_decode(&c, &Ruzstd).unwrap(), d, "type {typ}");
        assert_eq!(
            easf::chunk_peek(&MemSource(&c), &Ruzstd).unwrap(),
            easf::Peek::Other
        );
    }
    let b = big4(&[("a", lcg(1, 100))]);
    let c = chunked(b"chunzstd", &b, 0x40, 6);
    assert_eq!(
        easf::chunk_peek(&MemSource(&c), &Ruzstd).unwrap(),
        easf::Peek::Archive
    );
    let c = chunked(b"chunlzma", &b, 0x40, 5);
    match easf::chunk_peek(&MemSource(&c), &Ruzstd).unwrap() {
        easf::Peek::Unread(why) => assert!(why.contains("LZMA (type 5)"), "{why}"),
        p => panic!("{p:?}"),
    }
    assert!(
        easf::chunk_decode(&c, &Ruzstd)
            .unwrap_err()
            .contains("block type 5 (LZMA) is not decoded by this port")
    );
    let c = chunked(b"chunkref", &b, 0x40, 2);
    assert!(
        easf::chunk_decode(&c, &Ruzstd)
            .unwrap_err()
            .contains("RefPack")
    );
}

#[test]
fn easf_and_its_key() {
    let plain = lcg(3, 1000);
    let d = easf_of(&plain, &KEY, KEY_ID);
    assert!(easf::confirms(&d, &KEY));
    assert!(!easf::confirms(&d, b"fedcba9876543210"));
    assert_eq!(easf::easf_decrypt(&d, &KEY).unwrap(), plain);
    assert!(easf::easf_decrypt(&d, b"fedcba9876543210").is_err());
    let ring = easf::Keyring::with_images(vec![main_text(true)]);
    assert_eq!(ring.resolve(0, &d).unwrap(), KEY);
    assert_eq!(ring.resolve(0, &d).unwrap(), KEY); // the second time from the cache
    let notes = ring.notes().join("\n");
    assert!(
        notes.contains(&easf::fingerprint(&KEY)) && !notes.contains("0123456789abcdef"),
        "{notes}"
    );
    let ring = easf::Keyring::with_images(vec![main_text(false)]);
    let e = ring.resolve(0, &d).unwrap_err();
    assert!(
        e.contains("1 candidate(s)") && !e.contains("fedcba9876543210"),
        "{e}"
    );
    let mut bad = d.clone();
    bad[8] = 0x01;
    assert!(
        easf::easf_header(&bad)
            .unwrap_err()
            .contains("not printable")
    );
}

#[test]
fn the_record_classifies_ea_files() {
    for head in [
        big4(&[("a", vec![0; 8])]),
        easf_of(&lcg(1, 64), &KEY, KEY_ID),
        fxo(&[(1, St::Cs)]),
        chunked(b"chunlzma", &lcg(1, 64), 32, 5),
    ] {
        assert_eq!(
            classify_source("data/x.big", &MemSource(&head)).unwrap(),
            ["ea"]
        );
    }
}

/// the container cut at lengths all along it, and with bytes flipped where the readers look
fn mutations(d: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for n in (0..d.len()).step_by((d.len() / 61).max(1)) {
        out.push(d[..n].to_vec());
    }
    for o in (0..0x80.min(d.len())).chain((0..d.len()).step_by(0x55)) {
        for x in [0xff, 0x80, 0x01] {
            let mut m = d.to_vec();
            m[o] ^= x;
            out.push(m);
        }
    }
    out
}

#[test]
fn damaged_containers_are_errors_never_a_panic() {
    let inner = big4(&[
        ("a.fxo", fxo(&[(10, St::Vs), (20, St::Ps)])),
        ("b", lcg(1, 50)),
    ]);
    let chunk = chunked(b"chunkzip", &inner, 0x400, 1);
    let ring = easf::Keyring::with_images(vec![main_text(true)]);
    for m in mutations(&chunk) {
        let _ = easf::chunk_decode(&m, &Ruzstd);
        let _ = easf::chunk_peek(&MemSource(&m), &Ruzstd);
    }
    for m in mutations(&inner) {
        if let Ok(es) = easf::big4_entries(&MemSource(&m)) {
            for e in es {
                assert!(e.offset + e.size <= m.len() as u64);
            }
        }
    }
    let f = fxo(&[(10, St::Vs), (20, St::Ps)]);
    for m in mutations(&f) {
        let _ = easf::fxo_shaders(&m, "/x");
    }
    let e = easf_of(&chunk, &KEY, KEY_ID);
    for m in mutations(&e) {
        if let Ok(k) = ring.resolve(0, &m) {
            let _ = easf::easf_decrypt(&m, &k);
        }
    }
    // split end to end with damaged archives: never a panic or a failed split. EASF verifies only the first 32 plaintext bytes (as the game does), so a flipped ciphertext byte in a
    // shader makes another shader of it: a handful more keys at most, never an unbounded number
    let dir = temp("damaged");
    let good = shader_archive(1);
    for m in mutations(&good).iter().step_by(23) {
        let r = run(
            &dir,
            &[("datax.big", big4(&[("s.big", m.clone())]))],
            &main_text(true),
            2,
            "o",
        );
        assert!(r.shaders <= 8, "{:?}", r.log);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
