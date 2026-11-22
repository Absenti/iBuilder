//! Mods through the real adapter (`RustMods` over `ibuilder-mods`) and the pipeline's stage 2: `test_mods.py`'s "the hook in do_rewrite" block and "the boot's switches" run end to end, plus a
//! refused module through the REAL rewriter (`pyrewrite_driver.py`: a refused module's `sys.exit(msg)` is an ordinary error, here a `Result` the chain reports as "stage 2 FAILED").

use ibuilder_cli::adapters::RustMods;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::config::Config;
use ibuilder_pipeline::ctx::{Ctx, View};
use ibuilder_pipeline::env::{Env, MapEnv};
use ibuilder_pipeline::log::MemLog;
use ibuilder_pipeline::mods::apply_mods;
use ibuilder_pipeline::rewrite::{do_rewrite, rewrite_is_cached};
use ibuilder_pipeline::testutil::{TempDir, write};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

const TID: &str = "0100AAAA00000000";

fn bid() -> Vec<u8> {
    (0x10u8..0x30).collect()
}

/// An NSO like `test_mods.py`'s `make_nso`: three 0x1000-byte segments, LZ4-literal compressed, flags with hash bits.
fn make_nso(bid: &[u8]) -> Vec<u8> {
    let text: Vec<u8> = (0..16).flat_map(|_| 0u8..=255).collect();
    let segs = [text, b"ro-data-".repeat(512), vec![b'D'; 0x1000]];
    let mem = [0u32, 0x1000, 0x2000];
    let lit = |raw: &[u8]| {
        let n = raw.len();
        let mut out = vec![(n.min(15) << 4) as u8];
        let mut rest = n - 15;
        while rest >= 255 {
            out.push(255);
            rest -= 255;
        }
        out.push(rest as u8);
        out.extend_from_slice(raw);
        out
    };
    let stored: Vec<Vec<u8>> = segs.iter().map(|s| lit(s)).collect();
    let mut h = vec![0u8; 0x100];
    h[0..4].copy_from_slice(b"NSO0");
    h[0xC..0x10].copy_from_slice(&(0b111u32 | 0b111000).to_le_bytes());
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

fn ips_one(off: u32, data: &[u8]) -> Vec<u8> {
    let mut out = b"PATCH".to_vec();
    out.extend_from_slice(&off.to_be_bytes()[1..]);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(b"EOF");
    out
}

/// A rewriter that records the bytes it was given as input and writes the outputs the chain looks for.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<Vec<u8>>>,
}
impl Rewriter for Recorder {
    fn rewrite(&self, req: &RewriteRequest<'_>, _v: &View<'_>) -> ibuilder_pipeline::Result<()> {
        self.seen.lock().unwrap().push(fs::read(req.src)?);
        fs::write(
            req.out_dir.join(format!("{}.patched.bin", req.module)),
            b"rewritten",
        )?;
        Ok(())
    }
}

struct Fx {
    dir: TempDir,
    ctx: Ctx,
    log: Arc<MemLog>,
    env: Arc<MapEnv>,
    rw: Arc<Recorder>,
}

fn fx() -> Fx {
    let dir = TempDir::new("mods-chain");
    let log = MemLog::new();
    let env = Arc::new(MapEnv::new());
    let rw = Arc::new(Recorder::default());
    let mut ctx = Ctx::builder(dir.path())
        .env(env.clone())
        .log(log.clone())
        .build();
    ctx.adapters.rewriter = rw.clone();
    ctx.adapters.mods = Some(Arc::new(RustMods));
    let exe = dir.join("ExeFS");
    write(exe.join("main"), make_nso(&bid()));
    write(dir.join("profile.tsv"), format!("title_id\t{TID}\n"));
    write(
        dir.join(format!("profile-derived/{TID}/game.tsv")),
        "modules\tmain\n",
    );
    ctx.cfg = Config::parse(&format!(
        "app_build=b1\nmodules=main\nexe_dir={}\nextract_out={}\nprofile={}\n",
        exe.display(),
        dir.join("extract").display(),
        dir.join("profile.tsv").display()
    ));
    Fx {
        dir,
        ctx,
        log,
        env,
        rw,
    }
}

fn gdir(f: &Fx) -> std::path::PathBuf {
    f.dir.join(format!("games/{TID}"))
}

fn hexu(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

#[test]
fn stage_2_rewrites_the_patched_copy_caches_by_the_patches_and_marks_what_it_saw() {
    let mut f = fx();
    let ips_path = format!("mods/Cheater/exefs/{}.ips", hexu(&bid()));
    write(gdir(&f).join(&ips_path), ips_one(0x110, &[1, 2, 3, 5]));
    write(gdir(&f).join("mods.tsv"), "mod\tCheater\t1\n");
    let out = do_rewrite(&mut f.ctx).unwrap();
    // py: "do_rewrite: stage 2 reads the patched copy, not the dumped module"
    let seen = f.rw.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{:?}", f.log.lines());
    let (_h, segs) = ibuilder_mods::nso_image(&seen[0]).unwrap();
    assert_eq!(segs[0].1[0x10..0x14], [1, 2, 3, 5]);
    assert_eq!(
        fs::read(f.dir.join("ExeFS/main")).unwrap(),
        make_nso(&bid()),
        "the dumped module is untouched"
    );
    // py: "do_rewrite: the copy is removed after stage 2 and the applied signature is written"
    assert!(!gdir(&f).join("mods-exefs").exists());
    assert_eq!(
        fs::read_to_string(gdir(&f).join("mods.applied")).unwrap(),
        ibuilder_mods::exefs_signature(f.dir.path(), TID).unwrap()
    );
    assert!(
        fs::read_to_string(gdir(&f).join("build_ids.txt"))
            .unwrap()
            .starts_with("main\t")
    );
    // py: "do_rewrite: the same set again is taken from the cache (no second rewrite)"
    fs::remove_dir_all(&out).unwrap();
    f.log.clear();
    do_rewrite(&mut f.ctx).unwrap();
    assert_eq!(f.rw.seen.lock().unwrap().len(), 1, "{:?}", f.log.lines());
    assert!(f.log.contains("taken from the cache"));
    // py: "rewrite_is_cached: true for that set"
    assert!(rewrite_is_cached(&f.ctx));
    // py: "rewrite_is_cached: false once the patch changed"
    write(gdir(&f).join(&ips_path), ips_one(0x110, &[9]));
    assert!(!rewrite_is_cached(&f.ctx));
    // py: "do_rewrite: a changed patch rewrites again"
    do_rewrite(&mut f.ctx).unwrap();
    let seen = f.rw.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(ibuilder_mods::nso_image(&seen[1]).unwrap().1[0].1[0x10], 9);
    // the game with no mods enabled keeps the cache key it always had: switching the mod off rewrites the plain module again
    write(gdir(&f).join("mods.tsv"), "mod\tCheater\t0\n");
    assert!(!rewrite_is_cached(&f.ctx));
    do_rewrite(&mut f.ctx).unwrap();
    let seen = f.rw.seen.lock().unwrap().clone();
    assert_eq!(seen[2], make_nso(&bid()), "the dumped bytes");
}

#[test]
fn the_boots_switches_come_from_the_store() {
    let f = fx();
    write(gdir(&f).join("mods/Skins/romfs/ui/icon.bin"), "icon");
    write(gdir(&f).join("mods/Zed/romfs/ui/icon.bin"), "other icon");
    write(
        gdir(&f).join(format!("mods/Cheater/cheats/{}.txt", hexu(&bid()[..8]))),
        "[Infinite HP]\n04000000 00200000 000003E7\n",
    );
    write(
        gdir(&f).join("mods.tsv"),
        "mod\tSkins\t1\nmod\tZed\t1\nmod\tCheater\t1\ncheat\tCheater\tInfinite HP\t1\n",
    );
    let got = apply_mods(&f.ctx);
    let want = format!(
        "{}:{}",
        gdir(&f).join("mods/Skins/romfs").display(),
        gdir(&f).join("mods/Zed/romfs").display()
    );
    assert_eq!(got.get("DM_ROMFS_OVERLAY"), Some(&want));
    assert_eq!(f.env.get("DM_ROMFS_OVERLAY"), Some(want));
    assert_eq!(
        f.env.get("DM_CHEATS"),
        Some(gdir(&f).join("cheats-run").to_string_lossy().into_owned())
    );
    assert!(
        f.log
            .contains("mods: RomFS overlay: 2 file(s) from 2 mod(s)"),
        "{:?}",
        f.log.lines()
    );
    // py: "boot: nothing enabled sets nothing" / "the pipeline hook ... never raises": a broken mods.tsv is said and the game starts without mods
    write(gdir(&f).join("mods.tsv"), b"mod\tSkins\t1\n\xff");
    let f2 = fx();
    write(gdir(&f2).join("mods.tsv"), b"mod\tSkins\t1\n\xff");
    assert!(apply_mods(&f2.ctx).is_empty());
    assert!(f2.log.contains("mods: not applied (UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 12: invalid start byte) -- the game starts without them"), "{:?}", f2.log.lines());
    let f3 = fx();
    write(gdir(&f3).join("mods.tsv"), "mod\tSkins\t0\n");
    assert!(apply_mods(&f3.ctx).is_empty());
}

#[cfg(feature = "rewrite")]
#[test]
fn a_module_the_real_rewriter_refuses_is_a_stage_2_error() {
    // py (pyrewrite_driver.run_once): "a rewrite that exits with a message is a RuntimeError, not a SystemExit" -- the real rewriter on bytes that are not a module returns an error, the stage fails
    let mut f = fx();
    f.ctx.adapters.rewriter = Arc::new(ibuilder_cli::adapters::RustRewriter);
    write(f.dir.join("ExeFS/main"), vec![0u8; 0x400]);
    let err = do_rewrite(&mut f.ctx).unwrap_err().to_string();
    assert!(!err.is_empty(), "the refusal carries its message");
    eprintln!("refused: {err}");
    // and a module whose NSO header is real but whose segments are cut short
    let nso = make_nso(&bid());
    write(f.dir.join("ExeFS/main"), &nso[..0x180]);
    assert!(do_rewrite(&mut f.ctx).is_err());
    let _ = Path::new("");
}

#[cfg(feature = "rewrite")]
#[test]
fn zero_rewrite_threads_are_refused_as_paches_0301_does() {
    // paches/engine/0301: `THREADS < 1` -> sys.exit('[RED] DM_REWRITE_JOBS must be a positive integer') -> pyrewrite_driver: RuntimeError('patch_nso.py stopped: ...')
    let f = fx();
    let v = f.ctx.view();
    let out = f.dir.join("o");
    fs::create_dir_all(&out).unwrap();
    let req = RewriteRequest {
        src: &f.dir.join("ExeFS/main"),
        out_dir: &out,
        module: "main",
        jobs: 0,
    };
    let err = ibuilder_cli::adapters::RustRewriter
        .rewrite(&req, &v)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("DM_REWRITE_JOBS must be a positive integer"),
        "{err}"
    );
}
