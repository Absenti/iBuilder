//! The later additions to `gameapp_pipeline.py`: the mods hooks, `release_mimalloc` and its use in the restart decision, the rows GameApp adds to the settings sheet (Pipeline cache, MetalFX, External display) and `DM_METALFX`'s expansion, the in-process boot's own line, and the refused module
//! of `pyrewrite_driver.py`. The mods themselves are `ibuilder-mods` (its own spec); here the pipeline is exercised against a stand-in `ModsHost` that records what the chain asks of it.

mod common;

use common::*;
use ibuilder_pipeline::adapters::*;
use ibuilder_pipeline::boot::{boot_after_build, prepare_boot};
use ibuilder_pipeline::chain::{RC_FAILED, run_chain};
use ibuilder_pipeline::ctx::{Ctx, View};
use ibuilder_pipeline::env::{Env, MapEnv};
use ibuilder_pipeline::log::{Log, MemLog};
use ibuilder_pipeline::markers::restart_for_shaders;
use ibuilder_pipeline::memory::{
    FakeMem, MemInfo, MemSource, mem_line, release_memory, release_mimalloc,
};
use ibuilder_pipeline::mods::apply_mods;
use ibuilder_pipeline::rewrite::{do_rewrite, rewrite_cache_dir, rewrite_is_cached, rewrite_key};
use ibuilder_pipeline::settings::{
    apply_game_settings, gameapp_rows, launcher_catalogue, write_launcher_settings,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const STID: &str = "0100AAAA00000000";
const CHAIN: &str = "cc0f2129e1de";

// ---- the allocator's memory given back in this process, before any restart (docs/DECISIONS.md 82) --------------------------------------------------------------------

/// "the patched runtime": the call hands `gives` MiB of the footprint back (and raises the memory left by as much).
struct Giving {
    gives: u64,
    rc: i32,
    calls: AtomicU64,
    footprint: AtomicU64,
    available: AtomicU64,
}

impl Giving {
    fn new(gives: u64, rc: i32) -> Arc<Giving> {
        Arc::new(Giving {
            gives,
            rc,
            calls: AtomicU64::new(0),
            footprint: AtomicU64::new(3000 * MIB),
            available: AtomicU64::new(600 * MIB),
        })
    }
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl MemSource for Giving {
    fn mem(&self) -> MemInfo {
        MemInfo {
            resident: 1 << 30,
            peak: 1 << 30,
            footprint: self.footprint.load(Ordering::SeqCst),
            available: self.available.load(Ordering::SeqCst),
            free: 1 << 40,
        }
    }
    fn release_mimalloc(&self) -> Option<i32> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.rc == 0 {
            self.footprint.fetch_sub(self.gives * MIB, Ordering::SeqCst);
            self.available.fetch_add(self.gives * MIB, Ordering::SeqCst);
        }
        Some(self.rc)
    }
}

fn restart_fixture() -> Fx {
    let mut fx = Fx::new();
    write(
        fx.p(&format!("profile-derived/{STID}/game.tsv")),
        format!("title_id\t{STID}\n"),
    );
    fx.profile(&format!("title_id\t{STID}\n"));
    write(fx.p("extract/RomFS.img"), "rom");
    write(fx.p("extract/ExeFS/main"), "main");
    for (k, v) in [
        ("shader_chain", CHAIN),
        ("shader_free_gib", "0"),
        ("shader_tables", "0"),
        ("game", "Grizzy.nsp"),
        ("app_build", "b1"),
        ("modules", "main"),
    ] {
        fx.set_cfg(k, v);
    }
    fx.set_cfg("romfs", &fx.p("extract/RomFS.img").to_string_lossy());
    fx.set_cfg("exe_dir", &fx.p("extract/ExeFS").to_string_lossy());
    fx.ctx.adapters.tools = Some(ibuilder_pipeline::mock::mock_tools());
    let key = rewrite_key(&fx.ctx, &fx.p("extract/ExeFS/main"), "")
        .unwrap()
        .unwrap();
    let c = rewrite_cache_dir(&fx.ctx, "main").unwrap();
    write(c.join("key"), format!("{key}\n"));
    write(c.join("files/main.patched.bin"), "x");
    fx
}

#[test]
fn the_allocators_memory_is_given_back_in_this_process_before_any_restart() {
    // py (test_resume.py, "the allocator's memory given back in this process"): each case below is one `check`
    let mut fx = restart_fixture();
    let run = |fx: &mut Fx, dm: Arc<Giving>| -> bool {
        fx.ctx.mem = dm;
        let _ = fs::remove_file(fx.p("gameapp.continue"));
        restart_for_shaders(&fx.ctx, true, 3200 * MIB)
    };
    let dm = Giving::new(2500, 0);
    fx.log.clear();
    let go = run(&mut fx, dm.clone());
    assert!(
        !go && dm.calls() == 1 && !fx.p("gameapp.continue").exists(),
        "600 MiB left, the call gives 2500 back: stage 3 goes on in this process, no restart: {:?}",
        fx.log.lines()
    );
    assert!(
        fx.log.contains("footprint 3000 -> 500 MiB"),
        "the log has the footprint before and after: {:?}",
        fx.log.lines()
    );
    assert!(
        fx.log
            .contains("3100 MiB left after giving the allocator's memory back (2400 needed)"),
        "{:?}",
        fx.log.lines()
    );
    let dm = Giving::new(300, 0);
    let go = run(&mut fx, dm.clone());
    assert!(
        go && fx.p("gameapp.continue").is_file() && dm.calls() == 1,
        "the call gives only 300 back (900 MiB left, 2400 needed): the restart stays as the fallback"
    );
    let dm = Giving::new(0, -1);
    fx.log.clear();
    let go = run(&mut fx, dm.clone());
    assert!(
        go && fx.log.contains("not in this runtime"),
        "a runtime without the patched function (rc -1): the restart decides as before: {:?}",
        fx.log.lines()
    );
    let dm = Giving::new(2500, 0);
    dm.available.store(3000 * MIB, Ordering::SeqCst);
    let go = run(&mut fx, dm.clone());
    assert!(
        !go && dm.calls() == 0,
        "enough memory left already: nothing is released and no restart"
    );
    // a runtime that has no such function at all (every Rust build): the restart decides, silently
    fx.ctx.mem = Arc::new(FakeMem::new(600 * MIB));
    fx.log.clear();
    let _ = fs::remove_file(fx.p("gameapp.continue"));
    assert!(
        restart_for_shaders(&fx.ctx, true, 3200 * MIB) && !fx.log.contains("mimalloc"),
        "no call, no line: {:?}",
        fx.log.lines()
    );
}

#[test]
fn release_mimalloc_returns_the_mib_the_footprint_fell_by_and_release_memory_calls_it() {
    let log = MemLog::new();
    let dm = Giving::new(1200, 0);
    // py: "release_mimalloc returns the MiB the footprint fell by"
    let freed = release_mimalloc("t", &*log, &*dm);
    assert!(
        freed == Some(1200) && log.contains("mimalloc release: footprint 3000 -> 1800"),
        "{freed:?} {:?}",
        log.lines()
    );
    // py: "release_memory calls it after the trim and the collect"
    release_memory("t", &*log, &*dm);
    assert_eq!(dm.calls(), 2);
    // the explicit no-op: Rust's allocator has no such call, the default `MemSource::release_mimalloc` is None and nothing is logged
    let log = MemLog::new();
    let plain = FakeMem::new(0);
    assert_eq!(release_mimalloc("t", &*log, &plain), None);
    assert!(log.lines().is_empty());
    release_memory("t", &*log, &plain);
    assert!(log.lines().iter().all(|l| !l.contains("mimalloc")));
    mem_line("t", &*log, &plain);
}

// ---- the mods hooks ----------------------------------------------------------------------------------------------------------------------------------------------

/// Records what the chain asks of the mods; `digest` is what `plan` reports for `main`.
#[derive(Default)]
struct FakeMods {
    digest: Mutex<String>,
    calls: Mutex<Vec<String>>,
    patched_copy: Mutex<Option<PathBuf>>,
    fail_plan: bool,
    fail_boot: bool,
}

impl FakeMods {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl ModsHost for FakeMods {
    fn plan(
        &self,
        _docs: &Path,
        tid: &str,
        _exe: &Path,
        modules: &[String],
        _log: &dyn Log,
        quiet: bool,
    ) -> Result<Vec<(String, ModPlanEntry)>, ModsFailure> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("plan {tid} quiet={quiet}"));
        if self.fail_plan {
            return Err(ModsFailure {
                exc: "error".into(),
                msg: "argument out of range".into(),
            });
        }
        let d = self.digest.lock().unwrap().clone();
        if d.is_empty() || !modules.iter().any(|m| m == "main") {
            return Ok(vec![]);
        }
        Ok(vec![(
            "main".into(),
            ModPlanEntry {
                digest: d,
                labels: vec!["m/x.ips [ips]".into()],
                token: Arc::new(()),
            },
        )])
    }
    fn apply_module(
        &self,
        docs: &Path,
        tid: &str,
        _exe: &Path,
        module: &str,
        _e: &ModPlanEntry,
        _log: &dyn Log,
    ) -> Result<Option<PathBuf>, ModsFailure> {
        self.calls.lock().unwrap().push(format!("apply {module}"));
        let dst = docs.join("games").join(tid).join("mods-exefs").join(module);
        write(&dst, "PATCHED COPY");
        *self.patched_copy.lock().unwrap() = Some(dst.clone());
        Ok(Some(dst))
    }
    fn remove_patched(&self, docs: &Path, tid: &str) {
        self.calls.lock().unwrap().push("remove".into());
        let _ = fs::remove_dir_all(docs.join("games").join(tid).join("mods-exefs"));
    }
    fn mark_applied(&self, docs: &Path, tid: &str, _log: &dyn Log) -> Result<(), ModsFailure> {
        self.calls.lock().unwrap().push("mark".into());
        write(docs.join("games").join(tid).join("mods.applied"), "sig");
        Ok(())
    }
    fn apply_boot(
        &self,
        _docs: &Path,
        tid: &str,
        _log: &dyn Log,
        env: &dyn Env,
    ) -> Result<BTreeMap<String, String>, ModsFailure> {
        self.calls.lock().unwrap().push(format!("boot {tid}"));
        if self.fail_boot {
            return Err(ModsFailure {
                exc: "UnicodeDecodeError".into(),
                msg: "'utf-8' codec can't decode byte 0xff in position 8: invalid start byte"
                    .into(),
            });
        }
        // like the real `apply_boot`: a switch the environment already names wins
        if env.get("DM_CHEATS").is_some() {
            return Ok(BTreeMap::new());
        }
        env.set("DM_CHEATS", "/cheats");
        Ok(BTreeMap::from([(
            "DM_CHEATS".to_string(),
            "/cheats".to_string(),
        )]))
    }
}

fn rewrite_fixture(mods: Option<Arc<FakeMods>>) -> (Fx, Arc<CountingRewriter>) {
    let mut fx = Fx::new();
    write(
        fx.p(&format!("profile-derived/{STID}/game.tsv")),
        "modules\tmain\nheap\t1\n",
    );
    fx.profile(&format!("title_id\t{STID}\n"));
    write(fx.p("exe/main"), "MAIN-1");
    for (k, v) in [("modules", "main"), ("app_build", "B1")] {
        fx.set_cfg(k, v);
    }
    fx.set_cfg("exe_dir", &fx.p("exe").to_string_lossy());
    fx.set_cfg("extract_out", &fx.p("extract").to_string_lossy());
    let rw = Arc::new(CountingRewriter::default());
    fx.ctx.adapters.rewriter = rw.clone();
    fx.ctx.adapters.mods = mods.map(|m| m as Arc<dyn ModsHost>);
    (fx, rw)
}

#[test]
fn the_rewrite_key_holds_the_patches_digest_and_without_one_is_what_it_was() {
    // py: "rewrite_key: the patches' digest is in it; without one it is what it was"
    let (fx, _rw) = rewrite_fixture(None);
    let src = fx.p("exe/main");
    let k0 = rewrite_key(&fx.ctx, &src, "").unwrap().unwrap();
    let k1 = rewrite_key(&fx.ctx, &src, "d1").unwrap().unwrap();
    let k2 = rewrite_key(&fx.ctx, &src, "d2").unwrap().unwrap();
    assert!(k0 != k1 && k1 != k2 && k0 != k2);
    assert_eq!(rewrite_key(&fx.ctx, &src, "").unwrap().unwrap(), k0);
    // the key before mods existed: sha256("app_build=B1\0" + module + "\0" + table name + table bytes), byte for byte what the Python's rewrite_key made
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"app_build=B1\0");
    h.update(b"MAIN-1");
    h.update(b"\0game.tsv\0");
    h.update(fs::read(fx.p(&format!("profile-derived/{STID}/game.tsv"))).unwrap());
    let want: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(k0, want);
}

#[test]
fn stage_2_asks_the_mods_plan_keys_the_cache_by_it_and_rewrites_the_patched_copy() {
    let mods = Arc::new(FakeMods::default());
    *mods.digest.lock().unwrap() = "digest-one".into();
    let (mut fx, rw) = rewrite_fixture(Some(mods.clone()));
    let out = do_rewrite(&mut fx.ctx).unwrap();
    // py: "do_rewrite: stage 2 reads the patched copy, not the dumped module"
    assert_eq!(
        mods.calls(),
        vec![
            format!("plan {STID} quiet=false"),
            "apply main".into(),
            "remove".into(),
            "mark".into()
        ]
    );
    assert_eq!(
        fs::read(out.join("main/main.patched.bin")).unwrap(),
        b"PATCHED COPY.patched.bin"
    );
    // py: "do_rewrite: the copy is removed after stage 2 and the applied signature is written"
    assert!(
        !fx.p(&format!("games/{STID}/mods-exefs")).exists()
            && fx.p(&format!("games/{STID}/mods.applied")).is_file()
    );
    // py: "do_rewrite: the same set again is taken from the cache (no second rewrite)"
    std::fs::remove_dir_all(&out).unwrap();
    fx.log.clear();
    mods.calls.lock().unwrap().clear();
    do_rewrite(&mut fx.ctx).unwrap();
    assert_eq!(rw.runs.lock().unwrap().len(), 1, "{:?}", fx.log.lines());
    assert!(fx.log.contains("taken from the cache"));
    assert_eq!(
        mods.calls(),
        vec![
            format!("plan {STID} quiet=false"),
            "remove".into(),
            "mark".into()
        ],
        "no patched copy is made for a cache hit"
    );
    // py: "rewrite_is_cached: true for that set; false once the patch changed" (the digest is what changes)
    mods.calls.lock().unwrap().clear();
    assert!(rewrite_is_cached(&fx.ctx));
    assert_eq!(mods.calls(), vec![format!("plan {STID} quiet=true")]);
    *mods.digest.lock().unwrap() = "digest-two".into();
    assert!(!rewrite_is_cached(&fx.ctx));
    // py: "do_rewrite: a changed patch rewrites again"
    do_rewrite(&mut fx.ctx).unwrap();
    assert_eq!(rw.runs.lock().unwrap().len(), 2);
    // py: "plan: mods with no exefs part leave the plan empty (the cache key is the old one)": an empty plan is the cache of a game with no mods
    *mods.digest.lock().unwrap() = String::new();
    assert!(
        !rewrite_is_cached(&fx.ctx),
        "its key has no mods part any more: the earlier rewrite was made with patches"
    );
    do_rewrite(&mut fx.ctx).unwrap();
    assert!(rewrite_is_cached(&fx.ctx));
}

#[test]
fn a_mods_plan_the_python_lets_through_fails_stage_2_and_a_game_without_a_host_says_so() {
    let mods = Arc::new(FakeMods {
        fail_plan: true,
        ..Default::default()
    });
    let (mut fx, rw) = rewrite_fixture(Some(mods.clone()));
    let err = do_rewrite(&mut fx.ctx).unwrap_err().to_string();
    assert!(err.contains("mods: error: argument out of range"), "{err}");
    assert!(
        rw.runs.lock().unwrap().is_empty(),
        "nothing is rewritten when the plan fails (an uncaught struct.error in the Python)"
    );
    assert!(
        !rewrite_is_cached(&fx.ctx),
        "and a plan that fails is not 'cached'"
    );
    // no host in this build, a game that has mods: said once; a game with no mods folder: nothing said
    let (mut fx, rw) = rewrite_fixture(None);
    write(
        fx.p(&format!("games/{STID}/mods/Some mod/exefs/x.ips")),
        "PATCH",
    );
    do_rewrite(&mut fx.ctx).unwrap();
    assert_eq!(
        fx.log
            .lines()
            .iter()
            .filter(|l| l.contains("no mods support"))
            .count(),
        1,
        "{:?}",
        fx.log.lines()
    );
    assert_eq!(rw.runs.lock().unwrap().len(), 1);
    let (mut fx, _rw) = rewrite_fixture(None);
    do_rewrite(&mut fx.ctx).unwrap();
    assert!(!fx.log.contains("mods:"), "{:?}", fx.log.lines());
}

#[test]
fn the_boot_hook_sets_the_switches_never_raises_and_runs_between_env_txt_and_the_shader_pack() {
    let mods = Arc::new(FakeMods::default());
    let (fx, _rw) = rewrite_fixture(Some(mods.clone()));
    // py: "boot: the pipeline hook sets the same switches and never raises"
    assert_eq!(
        apply_mods(&fx.ctx),
        BTreeMap::from([("DM_CHEATS".to_string(), "/cheats".to_string())])
    );
    assert_eq!(fx.env.get("DM_CHEATS").as_deref(), Some("/cheats"));
    let failing = Arc::new(FakeMods {
        fail_boot: true,
        ..Default::default()
    });
    let (mut fx2, _rw) = rewrite_fixture(Some(failing.clone()));
    fx2.ctx.adapters.mods = Some(failing);
    assert!(apply_mods(&fx2.ctx).is_empty());
    assert!(fx2.log.contains("mods: not applied (UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 8: invalid start byte) -- the game starts without them"), "{:?}", fx2.log.lines());
    // no title id: nothing asked
    let bare = Fx::new();
    let m = Arc::new(FakeMods::default());
    let mut ctx = bare.ctx;
    ctx.adapters.mods = Some(m.clone());
    assert!(apply_mods(&ctx).is_empty() && m.calls().is_empty());
    // both boots go through prepare_boot: env.txt first (so a switch it names wins over the mods'), then the mods
    let (fx3, _rw) = rewrite_fixture(Some(mods.clone()));
    write(fx3.p("env.txt"), "DM_CHEATS=/from-env-txt\n");
    mods.calls.lock().unwrap().clear();
    prepare_boot(&fx3.ctx);
    assert_eq!(mods.calls(), vec![format!("boot {STID}")]);
    assert_eq!(
        fx3.env.get("DM_CHEATS").as_deref(),
        Some("/from-env-txt"),
        "the environment's own switch is set before the hook: it is the hook's to respect"
    );
}

// ---- the settings sheet: rows GameApp added, DM_METALFX -------------------------------------------------------------------------------------------------------

#[test]
fn gameapp_adds_the_pipeline_cache_metalfx_and_external_display_rows() {
    let rows = gameapp_rows();
    let envs: Vec<&str> = rows.iter().map(|r| r["env"].as_str().unwrap()).collect();
    assert_eq!(
        envs,
        vec![
            "DM_PRESENT_DRAIN",
            "DM_LIVE_SHADERS",
            "DM_PSO_BINARY_ARCHIVE",
            "DM_METALFX",
            "DM_EXTERNAL_DISPLAY"
        ]
    );
    let by = |e: &str| rows.iter().find(|r| r["env"] == e).unwrap().clone();
    let pso = by("DM_PSO_BINARY_ARCHIVE");
    assert!(
        pso["kind"] == "toggle"
            && pso["section"] == "Performance"
            && pso["label"] == "Pipeline cache"
            && pso["default"] == "1"
            && pso["on"] == "1"
            && pso["off"] == "0"
    );
    let fx = by("DM_METALFX");
    assert_eq!(
        fx["options"],
        serde_json::json!([
            ["off", "Off"],
            ["quality", "Quality"],
            ["performance", "Performance"]
        ])
    );
    assert!(fx["level"] == "basic" && fx["default"] == "off");
    let ext = by("DM_EXTERNAL_DISPLAY");
    assert_eq!(
        ext["options"],
        serde_json::json!([
            ["tv", "Game on the TV, iPhone as controller"],
            ["mirror", "Mirror the iPhone"]
        ])
    );
    assert!(
        ext["note"]
            .as_str()
            .unwrap()
            .contains("\"Playing on the TV\"")
    );
    // keys come in the Python dict's order (json.dump keeps it: the sheet reads the file)
    let keys: Vec<&String> = pso.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "env", "kind", "section", "label", "default", "on", "off", "note"
        ]
    );
    let keys: Vec<&String> = fx.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "env", "kind", "section", "label", "default", "level", "options", "note"
        ]
    );
}

struct SheetBase;
impl LauncherBase for SheetBase {
    fn base_document(
        &self,
        _v: &View<'_>,
        tid: &str,
        _m: &Path,
        _c: &Path,
        gfx: &str,
        _d: Option<&Path>,
    ) -> ibuilder_pipeline::Result<serde_json::Value> {
        Ok(
            serde_json::json!({"schema": 1, "game": tid, "gfxapi": gfx, "settings": [
            {"env": "DM_FPS_TARGET", "kind": "choice", "label": "fps"},
            {"env": "DM_PRESENT_UPSCALE", "kind": "choice", "label": "Upscaling", "options": [["off", "Off"], ["metalfx", "MetalFX"]]},
            {"env": "DM_FRAMEGEN", "kind": "toggle", "label": "Frame generation"}]}),
        )
    }
}

#[test]
fn the_nvn_sheet_has_one_metalfx_row_and_dm_metalfx_becomes_two_engine_switches() {
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{STID}\n"));
    fx.set_cfg("extract_out", &fx.p("extract").to_string_lossy());
    fx.ctx.adapters.launcher = Some(Arc::new(SheetBase));
    let main_dir = fx.p("extract/rewrite/main");
    write(
        main_dir.join("main.patched.bin"),
        "x\0nvnBootstrapLoader\0y",
    );
    let out = write_launcher_settings(&fx.ctx, &main_dir).expect("written");
    let doc: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let envs: Vec<&str> = doc["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["env"].as_str().unwrap())
        .collect();
    // py: "MetalFX upscaling has one row (DM_METALFX); the engine's Off / MetalFX "Upscaling" row would be a second, competing one"
    assert!(
        envs.contains(&"DM_METALFX") && !envs.contains(&"DM_PRESENT_UPSCALE"),
        "{envs:?}"
    );
    assert!(
        envs.contains(&"DM_PSO_BINARY_ARCHIVE")
            && envs.contains(&"DM_EXTERNAL_DISPLAY")
            && envs.contains(&"DM_FRAMEGEN")
    );
    // a Vulkan game keeps the engine's own rows and none of GameApp's
    write(main_dir.join("main.patched.bin"), "a\0vkCreateInstance\0b");
    let out = write_launcher_settings(&fx.ctx, &main_dir).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&fs::read_to_string(&out).unwrap()).unwrap();
    let envs: Vec<&str> = doc["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["env"].as_str().unwrap())
        .collect();
    assert_eq!(
        envs,
        vec!["DM_FPS_TARGET", "DM_PRESENT_UPSCALE", "DM_FRAMEGEN"]
    );

    // DM_METALFX: the player's choice reaches the boot as the present's scaler and the render scale; an environment that names one keeps it
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{STID}\n"));
    let offered =
        serde_json::json!({"settings": gameapp_rows(), "catalogue": launcher_catalogue(&fx.ctx)});
    write(
        fx.p(&format!("games/{STID}/launcher.json")),
        offered.to_string(),
    );
    for (choice, scale) in [
        ("quality", Some("0.75")),
        ("performance", Some("0.5")),
        ("off", None),
    ] {
        let mut fx = Fx::new();
        fx.profile(&format!("title_id\t{STID}\n"));
        write(
            fx.p(&format!("games/{STID}/launcher.json")),
            offered.to_string(),
        );
        write(
            fx.p(&format!("games/{STID}/settings.tsv")),
            format!("DM_METALFX\t{choice}\n"),
        );
        let got = apply_game_settings(&fx.ctx);
        let mut want = BTreeMap::from([("DM_METALFX".to_string(), choice.to_string())]);
        if let Some(s) = scale {
            want.insert("DM_PRESENT_UPSCALE".into(), "metalfx".into());
            want.insert("DM_RENDER_SCALE".into(), s.into());
        }
        assert_eq!(got, want, "{choice}");
    }
    let mut fx = Fx::new();
    fx.profile(&format!("title_id\t{STID}\n"));
    write(
        fx.p(&format!("games/{STID}/launcher.json")),
        offered.to_string(),
    );
    write(
        fx.p(&format!("games/{STID}/settings.tsv")),
        "DM_METALFX\tquality\n",
    );
    fx.env.set("DM_RENDER_SCALE", "1");
    let got = apply_game_settings(&fx.ctx);
    assert_eq!(got.get("DM_RENDER_SCALE"), None, "{got:?}");
    assert_eq!(fx.env.get("DM_RENDER_SCALE").as_deref(), Some("1"));
    assert_eq!(
        got.get("DM_PRESENT_UPSCALE").map(String::as_str),
        Some("metalfx")
    );
}

// ---- the in-process boot's own line --------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_in_process_boot_says_the_guest_ran_in_this_process() {
    let mut fx = Fx::new();
    fx.set_cfg("boot_after_build", "1");
    fx.set_cfg("boot_seconds", "0");
    fx.ctx.adapters.boot = Some(FakeBoot::new(fx.p("gameapp.boot-only"), "ok"));
    let fw = fx.p("fw/DmGameProbe.framework/DmGameProbe");
    write(&fw, "x");
    write(fx.p("extract/flat/main.patched.bin"), "x");
    let flat = fx.p("extract/flat");
    boot_after_build(&mut fx.ctx, &fw, &flat);
    assert!(fx.log.contains("stage 5 boot returned 0, the guest returned 0 (stage 0 = the guest ran in this process and came back; 3 = dlopen failed, 4 = dm_host_probe_boot not found, guest -1 = never called)"), "{:?}", fx.log.lines());
}

// ---- a tool's sys.exit inside the app is an error, not the app's end (trap 58) ----------------------------------------------------------------------------------------

/// What `patch_nso.py` ends a refused module with (`sys.exit("<message>")`): a rewriter that says no.
struct Refusing(&'static str);
impl Rewriter for Refusing {
    fn rewrite(&self, _req: &RewriteRequest<'_>, _v: &View<'_>) -> ibuilder_pipeline::Result<()> {
        Err(ibuilder_pipeline::PipelineError::failed(self.0))
    }
}

#[test]
fn a_refused_module_is_a_stage_2_error_not_an_exit_and_the_chain_reports_it() {
    // py: "a rewrite that exits with a message is a RuntimeError, not a SystemExit" -> here a Result the chain logs as "stage 2 FAILED", with the message, and the process lives on
    let (mut fx, _rw) = rewrite_fixture(None);
    fx.ctx.adapters.rewriter = Arc::new(Refusing("not a module this rewrite can read"));
    let err = do_rewrite(&mut fx.ctx).unwrap_err().to_string();
    assert!(err.contains("not a module this rewrite can read"), "{err}");
    // the whole chain: result 1, the line the app greps, nothing written for stage 3 and later
    let mut ctx: Ctx = Ctx::builder(fx.docs())
        .env(Arc::new(MapEnv::new()))
        .log(fx.log.clone())
        .build();
    ctx.cfg = fx.ctx.cfg.clone();
    ctx.cfg
        .set("extract_out", fx.p("extract").to_string_lossy().to_string());
    ctx.adapters = ibuilder_pipeline::mock::mock_adapters(STID);
    ctx.adapters.rewriter = Arc::new(Refusing("not a module this rewrite can read"));
    write(fx.p("extract/ExeFS/main"), "x");
    ctx.cfg.set(
        "exe_dir",
        fx.p("extract/ExeFS").to_string_lossy().to_string(),
    );
    ctx.cfg.remove("profile");
    fx.log.clear();
    let rep = run_chain(&mut ctx);
    assert_eq!(rep.rc, RC_FAILED, "{:?}", fx.log.lines());
    assert!(
        fx.log.contains("stage 2 FAILED") && fx.log.contains("not a module this rewrite can read"),
        "{:?}",
        fx.log.lines()
    );
    assert!(rep.framework.is_none());
}
