//! Integration tests against the Python oracle (`tools/assemble.py`, `tools/macho_write.py`, `gen_embed.py`).
//!
//! The Python tools were run on identical inputs (see docs/crates/MACHO-RUST.md, "Oracle"); their outputs'
//! SHA-256 are recorded below, so the Rust output is checked against them without the 190 MB files. When the
//! Python output files are present (`IBUILDER_MACHO_ORACLE_DIR`, default: `$IBUILDER_SCRATCH_OLD/macho/oracle`) the
//! comparison is also made byte by byte and a mismatch names the first differing offset.
//!
//! Fixtures are referenced by absolute read-only path (iOSBuilder, never copied). A missing or changed
//! fixture (host or game input hash differs) makes the case SKIP with a message, never fail.

use ibuilder_macho::assemble::{Game, ModuleSource, assemble_files};
use ibuilder_macho::embed::{GenEmbedOptions, gen_embed};
use ibuilder_macho::writer::{ProbeParams, build_probe};
use ibuilder_testkit::{IOSBUILDER, grizzy_oracle, peak_rss_bytes};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `$IBUILDER_SCRATCH_OLD/macho`: the folder `scripts/macho-oracle.sh` writes
fn scratch_macho() -> PathBuf {
    ibuilder_testkit::scratch_old().join("macho")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// First differing offset of two files, or None when identical (streaming; never holds more than 2 MiB).
fn first_diff_files(a: &Path, b: &Path) -> std::io::Result<Option<u64>> {
    let (mut fa, mut fb) = (File::open(a)?, File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    let mut pos = 0u64;
    loop {
        let na = read_full(&mut fa, &mut ba)?;
        let nb = read_full(&mut fb, &mut bb)?;
        let n = na.min(nb);
        if let Some(i) = (0..n).find(|&i| ba[i] != bb[i]) {
            return Ok(Some(pos + i as u64));
        }
        if na != nb {
            return Ok(Some(pos + n as u64));
        }
        if na == 0 {
            return Ok(None);
        }
        pos += na as u64;
    }
}

fn read_full(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        let n = f.read(&mut buf[got..])?;
        if n == 0 {
            break;
        }
        got += n;
    }
    Ok(got)
}

/// A scratch path under the target dir (never /tmp), unique per call so concurrent tests cannot collide.
fn tmp(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ibuilder-macho");
    fs::create_dir_all(&d).unwrap();
    d.join(format!(
        "{}-{}-{name}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ))
}

fn oracle_dir() -> PathBuf {
    std::env::var_os("IBUILDER_MACHO_ORACLE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch_macho().join("oracle"))
}

fn hosts_dir() -> PathBuf {
    std::env::var_os("IBUILDER_MACHO_HOSTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch_macho().join("hosts"))
}

/// `(module, sha256 of .patched.bin, sha256 of .fixups.tsv)` of the rewriter oracle the Python run used.
// table rows are kept one per line as written (rustfmt would put every field of every row on its own line)
#[rustfmt::skip]
const GAME: [(&str, &str, &str); 3] = [
    ("main", "2f19ece90706c50796550b9da2cca61173103fe7a77ebadc4f51fb4261762c61", "ef8d0d7069f73ba7edc784f1db5efffa0d63f61c291ba86a9cfb64542b0e0c28"),
    ("sdk", "15c3de3c6373446b1bff568c3574127ede54708ebe59e2912ae15024cd7cb5aa", "4454fdc4b7ff6b4b319d853edea0584ac57798b5e848a723e06d4298fc2bf731"),
    ("subsdk0", "3ee37e9518b5464424f245cbcfe67529516857ad5a909bdc750588bfc35bf563", "84a4ff71728f6a6d80227d1b850aa693c01ac48a0247b0cae20fcf6c59afc373"),
];

/// Hash the game inputs once; Some(reason) when they are not the ones the oracle was produced from.
fn game_inputs_problem() -> &'static Option<String> {
    static P: OnceLock<Option<String>> = OnceLock::new();
    P.get_or_init(|| {
        for (m, bin_sha, fx_sha) in GAME {
            for (file, want) in [
                (format!("{m}.patched.bin"), bin_sha),
                (format!("{m}.fixups.tsv"), fx_sha),
            ] {
                let p = PathBuf::from(grizzy_oracle(m, &file));
                match sha256_file(&p) {
                    Err(e) => return Some(format!("{}: {e}", p.display())),
                    Ok(h) if h != want => {
                        return Some(format!("{} changed (sha256 {h})", p.display()));
                    }
                    Ok(_) => {}
                }
            }
        }
        None
    })
}

fn sources(names: &[&str]) -> Vec<ModuleSource> {
    names
        .iter()
        .map(|m| ModuleSource {
            name: m.to_string(),
            patched_bin: grizzy_oracle(m, &format!("{m}.patched.bin")).into(),
            fixups_tsv: grizzy_oracle(m, &format!("{m}.fixups.tsv")).into(),
        })
        .collect()
}

struct Case {
    id: &'static str,
    /// candidate locations of the prebuilt host (relative to iOSBuilder); the scratch copy `<id>.dylib` is tried last
    hosts: &'static [&'static str],
    host_sha: &'static str,
    modules: &'static [&'static str],
    tag: &'static str,
    synthetic: bool,
    size: u64,
    sha: &'static str,
}

const CASES: &[Case] = &[
    Case {
        id: "dev-4762784",
        hosts: &[
            "out-device/gameapp-build-device/bin/Release/net11.0-ios/ios-arm64/GameApp.app/host-prebuilt.dylib",
            "out-game/gameapp-out-device-3/gameapp-build-device/bin/Release/net11.0-ios/ios-arm64/GameApp.app/host-prebuilt.dylib",
        ],
        host_sha: "0d72694df478cfc4569f0a6a3b7d559bd5407c6ee9499ceb44ca7bc9e2a08e88",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 192703664,
        sha: "7cad2d17b92d234492316a7d1e757df4c009e2c5331104fa40e83f2760ff7101",
    },
    Case {
        id: "asm-sim",
        hosts: &["out-asm/host-prebuilt-sim.dylib"],
        host_sha: "4e620fa6d49ebb604e4081962201f61276a5633ad92ff8550d0a7290312fccc1",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 190820040,
        sha: "527baad8d2a10e2a17312a886f5eb3d72ed967922ee86b4f08de29a2ea35cc99",
    },
    Case {
        id: "load-ios",
        hosts: &["out-load/host-prebuilt-loadable-ios.dylib"],
        host_sha: "fefa424a4d4f257072f80a219f96a015683c7d21f88b8eed234e0cecdb203d0d",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 192703760,
        sha: "b9cc7517736c1d5d22a8842ac9255aeedba65da13dd17767210eefc71969c4d1",
    },
    Case {
        id: "load-sim",
        hosts: &["out-load/host-prebuilt-loadable-sim.dylib"],
        host_sha: "ba7100c738f63cf3c1d298be9cd45877da251929271b06d851c9bb55d7fa1df7",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 190838384,
        sha: "3a0ff563d25d713910f729c656155a0551bdc24a69380cd22f25b5162668d85f",
    },
    Case {
        id: "wt-ios",
        hosts: &["$IOSBUILDER_WORKTREE/out-load-ui/host-prebuilt-loadable-ios.dylib"],
        host_sha: "ab20ca04f5605f935d85ff9d1c832e93f44711627c97d8ace85d26b4f87491ab",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 194600976,
        sha: "ce1e845029b05a1dc550aeca54a754ca24088c4e605ca21e4648c86fa96549a3",
    },
    Case {
        id: "wt-sim",
        hosts: &["$IOSBUILDER_WORKTREE/out-load-ui/host-prebuilt-loadable-sim.dylib"],
        host_sha: "3128c0a56a4989b83a034f595c52433f92e6536a74335db460c4cf3ea63d815c",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 192375296,
        sha: "71726759bf9a94c1c26262bf5c333ba18aa11746a3cf6713c69fae707fa69d72",
    },
    Case {
        id: "wt-app-ios",
        hosts: &[
            "$IOSBUILDER_WORKTREE/out-game/gameapp-build-device/bin/Release/net11.0-ios/ios-arm64/GameApp.app/host-prebuilt.dylib",
        ],
        host_sha: "48b093ee151c62742757828a9f840900b38ea80c4833b345908aecc5468847d0",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: false,
        size: 194154600,
        sha: "0d7b36709c486ebcf6c3729b065f6b982a1ed700523de2cc585d42bdc6580363",
    },
    Case {
        id: "tag-sdk-sub",
        hosts: &["out-load/host-prebuilt-loadable-ios.dylib"],
        host_sha: "fefa424a4d4f257072f80a219f96a015683c7d21f88b8eed234e0cecdb203d0d",
        modules: &["sdk", "subsdk0"],
        tag: "T",
        synthetic: false,
        size: 25668872,
        sha: "12b20f5865bcbeaff1bd40dde6265d8c0f964981aec2391984940fd3e0f603ee",
    },
    Case {
        id: "main-only",
        hosts: &["$IOSBUILDER_WORKTREE/out-load-ui/host-prebuilt-loadable-ios.dylib"],
        host_sha: "ab20ca04f5605f935d85ff9d1c832e93f44711627c97d8ace85d26b4f87491ab",
        modules: &["main"],
        tag: "",
        synthetic: false,
        size: 173694984,
        sha: "941f61d50e6edca90c8e17270eecede1d2cfc0518018f7d5ab75275b52736ff6",
    },
    Case {
        id: "syn-asm-sim",
        hosts: &["out-asm/host-prebuilt-sim.dylib"],
        host_sha: "4e620fa6d49ebb604e4081962201f61276a5633ad92ff8550d0a7290312fccc1",
        modules: &["main", "subsdk0", "sdk"],
        tag: "",
        synthetic: true,
        size: 3026632,
        sha: "1dd0d1c087ee984f23cabf251b0d710759e90dc20adc0db3b5e37beb53e3156f",
    },
    Case {
        id: "syn-tag-wt-sim",
        hosts: &["$IOSBUILDER_WORKTREE/out-load-ui/host-prebuilt-loadable-sim.dylib"],
        host_sha: "3128c0a56a4989b83a034f595c52433f92e6536a74335db460c4cf3ea63d815c",
        modules: &["main", "sdk"],
        tag: "G",
        synthetic: true,
        size: 4532728,
        sha: "fef8243ac3b1f830d74928af32e161e9d8f3ecce15d723fad56c2e8919ea8de5",
    },
];

/// Locate the host: the iOSBuilder path (or the scratch copy) whose SHA-256 is the recorded one.
fn find_host(c: &Case) -> Result<PathBuf, String> {
    let mut tried = Vec::new();
    // a host path is relative to the original iOSBuilder checkout, or starts with `$IOSBUILDER_WORKTREE/` (the tree that holds the newest work)
    let mut cands: Vec<PathBuf> = c
        .hosts
        .iter()
        .map(|h| match h.strip_prefix("$IOSBUILDER_WORKTREE/") {
            Some(rest) => PathBuf::from(format!(
                "{}/{rest}",
                ibuilder_testkit::iosbuilder_worktree()
            )),
            None => PathBuf::from(format!("{IOSBUILDER}/{h}")),
        })
        .collect();
    cands.push(hosts_dir().join(format!("{}.dylib", c.id)));
    for p in cands {
        match sha256_file(&p) {
            Ok(h) if h == c.host_sha => return Ok(p),
            Ok(h) => tried.push(format!("{} (sha256 {h})", p.display())),
            Err(e) => tried.push(format!("{} ({e})", p.display())),
        }
    }
    Err(format!(
        "host with sha256 {} not found; tried {}",
        c.host_sha,
        tried.join(", ")
    ))
}

/// Returns false (after printing why) when the case's fixtures are absent.
fn run_case(c: &Case) -> bool {
    let host = match find_host(c) {
        Ok(h) => h,
        Err(m) => {
            ibuilder_testkit::skip(&format!("{}: {m}", c.id));
            return false;
        }
    };
    if let (false, Some(why)) = (c.synthetic, game_inputs_problem()) {
        ibuilder_testkit::skip(&format!("{}: game fixtures unavailable: {why}", c.id));
        return false;
    }
    let game = if c.synthetic {
        Game::Synthetic {
            modules: c.modules.iter().map(|s| s.to_string()).collect(),
        }
    } else {
        Game::Sources(sources(c.modules))
    };
    let out = tmp(&format!("{}.dylib", c.id));
    let report =
        assemble_files(&host, &out, &game, c.tag).unwrap_or_else(|e| panic!("{}: {e}", c.id));
    assert_eq!(report.info.size, c.size, "{}: size", c.id);
    assert_eq!(
        fs::metadata(&out).unwrap().len(),
        c.size,
        "{}: file size",
        c.id
    );
    let sha = sha256_file(&out).unwrap();
    // byte-level comparison with the Python output when it is around: names the first differing offset
    let py = oracle_dir().join(format!("{}.dylib", c.id));
    if py.exists() {
        let d = first_diff_files(&out, &py).unwrap();
        assert_eq!(
            d,
            None,
            "{}: first byte differing from the Python output {}",
            c.id,
            py.display()
        );
    }
    assert_eq!(
        sha, c.sha,
        "{}: sha256 of the Rust output differs from the Python oracle's",
        c.id
    );
    let _ = fs::remove_file(&out);
    eprintln!("OK   {}: {} bytes sha256 {}", c.id, c.size, c.sha);
    true
}

macro_rules! case_test {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            let c = CASES.iter().find(|c| c.id == $id).unwrap();
            run_case(c);
        }
    };
}

case_test!(assemble_dev_host_matches_python, "dev-4762784");
case_test!(assemble_asm_sim_matches_python, "asm-sim");
case_test!(assemble_load_ios_matches_python, "load-ios");
case_test!(assemble_load_sim_matches_python, "load-sim");
case_test!(assemble_worktree_ios_matches_python, "wt-ios");
case_test!(assemble_worktree_sim_matches_python, "wt-sim");
case_test!(assemble_worktree_app_host_matches_python, "wt-app-ios");
case_test!(assemble_tag_and_module_order_match_python, "tag-sdk-sub");
case_test!(assemble_single_module_matches_python, "main-only");
case_test!(assemble_synthetic_matches_python, "syn-asm-sim");
case_test!(assemble_synthetic_tag_matches_python, "syn-tag-wt-sim");

/// The framework the original author's pipeline stored (unsigned, `out-load/device/...`) is byte-identical to the
/// Rust output for the host it was built from -- an oracle that does not depend on this session's Python run.
#[test]
fn assemble_matches_the_stored_device_framework() {
    let stored = PathBuf::from(format!(
        "{IOSBUILDER}/out-load/device/DmGame0100DFA027504000.framework/DmGame0100DFA027504000"
    ));
    let c = CASES.iter().find(|c| c.id == "dev-4762784").unwrap();
    let host = match find_host(c) {
        Ok(h) => h,
        Err(m) => return ibuilder_testkit::skip(&format!("stored framework: {m}")),
    };
    if !stored.exists() {
        return ibuilder_testkit::skip(&format!("stored framework: {} absent", stored.display()));
    }
    if let Some(why) = game_inputs_problem() {
        return ibuilder_testkit::skip(&format!(
            "stored framework: game fixtures unavailable: {why}"
        ));
    }
    let out = tmp("stored-check.dylib");
    assemble_files(&host, &out, &Game::Sources(sources(c.modules)), "").unwrap();
    let d = first_diff_files(&out, &stored).unwrap();
    assert_eq!(d, None, "first byte differing from {}", stored.display());
    let _ = fs::remove_file(&out);
    eprintln!(
        "OK   stored framework {} is byte-identical",
        stored.display()
    );
}

/// `ibuilder_testkit::peak_rss_bytes` treats every non-macOS target as Linux (ru_maxrss in KiB), but iOS reports bytes
/// like macOS: on iOS it returns 1024x too much. Undo that here (reported to the orchestrator, testkit is not mine).
fn peak_rss() -> u64 {
    let r = peak_rss_bytes();
    if cfg!(target_os = "ios") { r / 1024 } else { r }
}

/// Peak RSS of a full 190 MB assembly stays tiny (Python holds ~4x the image: ~790 MB RSS).
#[test]
fn assembly_memory_is_bounded() {
    let c = CASES.iter().find(|c| c.id == "asm-sim").unwrap();
    if !run_case(c) {
        return;
    }
    let rss = peak_rss();
    eprintln!(
        "peak RSS of this test process after a full assembly: {:.1} MiB",
        rss as f64 / 1048576.0
    );
    assert!(
        rss < 96 << 20,
        "peak RSS {rss} B: the assembler is not streaming"
    );
}

// ---- macho_write.py: deterministic once LC_UUID is pinned ---------------------------------------------------
fn probe(
    sim: bool,
    f: impl FnOnce(&mut ProbeParams),
) -> (Vec<u8>, ibuilder_macho::writer::ProbeInfo) {
    let mut p = ProbeParams::for_platform(sim);
    p.uuid = Some([
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ]);
    f(&mut p);
    build_probe(&p).unwrap()
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

#[test]
fn macho_write_matches_python() {
    // sha256 of `macho_write.py` runs with uuid.uuid4 pinned to 00112233-4455-6677-8899-aabbccddeeff
    let (b, i) = probe(true, |_| {});
    assert_eq!((b.len(), i.text_off, i.platform), (65720, 1424, 7));
    assert_eq!(
        sha(&b),
        "ee670240359e08b26967171b85ffdc36ad747802b346d35b08b9cad020704864",
        "--platform sim"
    );
    let (b, i) = probe(false, |p| {
        p.name = "DmGameX".into();
        p.magic = 0x12345678;
        p.minos = "26.1".into();
        p.sdk = "27.2.1".into();
    });
    assert_eq!(
        (b.len(), i.text_off, i.platform, i.magic),
        (65720, 1424, 2, 305419896)
    );
    assert_eq!(i.id_dylib, "@rpath/DmGameX.framework/DmGameX");
    assert_eq!(
        sha(&b),
        "4ce4f53bc1ccf2c92f77190c92c5d9b75962c69a8cf96d0d59ec76af037031a7",
        "--platform ios --name DmGameX ..."
    );
    let (b, i) = probe(true, |p| {
        p.name = "A".into();
        p.magic = 0xFFFF_FFFF;
        p.minos = "27.0.5".into();
        p.sdk = "28".into();
    });
    assert_eq!(i.text_off, 1408, "a shorter LC_ID_DYLIB moves __text down");
    assert_eq!(
        sha(&b),
        "0216f3def68385c9a72afd6f07544d3a1db3b18e91043bf8636429d031ba5aa8",
        "--name A --magic 0xFFFFFFFF ..."
    );
}

// ---- gen_embed.py: the generated assembly is reproduced exactly ---------------------------------------------
fn expect_text(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

#[test]
fn gen_embed_matches_python() {
    if let Some(why) = game_inputs_problem() {
        return ibuilder_testkit::skip(&format!("gen_embed: game fixtures unavailable: {why}"));
    }
    // a directory holding <name>.patched.bin / .fixups.tsv under the names gen_embed.py expects
    let dir = tmp("flat");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (m, _, _) in GAME {
        for f in [format!("{m}.patched.bin"), format!("{m}.fixups.tsv")] {
            std::os::unix::fs::symlink(grizzy_oracle(m, &f), dir.join(&f)).unwrap();
        }
    }
    let dir_s = dir.to_string_lossy().into_owned();
    let jit = tmp("jit.img");
    fs::write(&jit, vec![b'J'; 5000]).unwrap();
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let check = |label: &str,
                 d: &Path,
                 mods: &[&str],
                 opts: GenEmbedOptions,
                 fixture: &str,
                 flags: &str| {
        let (asm, fl) = gen_embed(d, &names(mods), &opts).unwrap();
        let want = expect_text(fixture)
            .replace("@DIR@", &d.to_string_lossy())
            .replace("@JIT@", &jit.to_string_lossy());
        assert_eq!(asm, want, "{label}: assembly");
        assert_eq!(fl, flags, "{label}: flags");
    };
    check(
        "base",
        &dir,
        &["main", "subsdk0", "sdk"],
        GenEmbedOptions::default(),
        "ge-base.S.tmpl",
        "-Wl,-segprot,__DMmainW,rw,rw -Wl,-segprot,__DMsubsdk0W,rw,rw -Wl,-segprot,__DMsdkW,rw,rw",
    );
    check(
        "tag",
        &dir,
        &["main", "sdk"],
        GenEmbedOptions {
            tag: "T".into(),
            jit: None,
        },
        "ge-tag.S.tmpl",
        "-Wl,-segprot,__DMTmainW,rw,rw -Wl,-segprot,__DMTsdkW,rw,rw",
    );
    check(
        "jit",
        &dir,
        &["main"],
        GenEmbedOptions {
            tag: String::new(),
            jit: Some(jit.clone()),
        },
        "ge-jit.S.tmpl",
        "-Wl,-segprot,__DMmainW,rw,rw",
    );
    // NRO table: packed pieces + the __DATA table, from a directory that also holds nro.tsv
    let nro_dir = tmp("flat-nro");
    let _ = fs::remove_dir_all(&nro_dir);
    fs::create_dir_all(&nro_dir).unwrap();
    for e in fs::read_dir(&dir).unwrap() {
        let e = e.unwrap();
        std::os::unix::fs::symlink(
            fs::read_link(e.path()).unwrap(),
            nro_dir.join(e.file_name()),
        )
        .unwrap();
    }
    fs::write(
        nro_dir.join("nro.tsv"),
        "# comment\nsdk\tx\nrefused\tfoo\nsubsdk0\ty\n\n",
    )
    .unwrap();
    check(
        "nro",
        &nro_dir,
        &["main"],
        GenEmbedOptions {
            tag: "Q".into(),
            jit: Some(jit.clone()),
        },
        "ge-nro.S.tmpl",
        "-Wl,-segprot,__DMQmainW,rw,rw -Wl,-segprot,__DMQ_NRO_W,rw,rw",
    );
    let _ = dir_s;
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&nro_dir);
    let _ = fs::remove_file(&jit);
}

/// Output == input is refused (streaming would truncate the host before reading it) and leaves the input intact.
#[test]
fn output_that_is_an_input_is_refused() {
    let c = CASES.iter().find(|c| c.id == "asm-sim").unwrap();
    let host = match find_host(c) {
        Ok(h) => h,
        Err(m) => return ibuilder_testkit::skip(&format!("output==input: {m}")),
    };
    let copy = tmp("host-copy.dylib");
    fs::copy(&host, &copy).unwrap();
    let before = sha256_file(&copy).unwrap();
    let game = Game::Synthetic {
        modules: vec!["main".into()],
    };
    let e = assemble_files(&copy, &copy, &game, "").unwrap_err();
    assert!(e.to_string().contains("also an input"), "{e}");
    assert_eq!(sha256_file(&copy).unwrap(), before);
    // and a failing plan (a non-Mach-O host) does not create or truncate the output
    let junk = tmp("junk.bin");
    fs::write(&junk, vec![0u8; 100]).unwrap();
    let out = tmp("never-created.dylib");
    assert!(assemble_files(&junk, &out, &game, "").is_err());
    assert!(
        !out.exists(),
        "a failed layout must not leave an output file"
    );
    let _ = fs::remove_file(&copy);
    let _ = fs::remove_file(&junk);
}
