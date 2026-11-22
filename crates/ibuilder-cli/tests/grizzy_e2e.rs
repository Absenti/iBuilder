//! The real chain on Grizzy and the Lemmings (the reference game), through the real stage crates: profile -> rewrite -> assemble -> sign. Heavy (the 75 MB `main` rewrite, ~2 GB of
//! RAM under the Python; far less here) and it needs read-only fixtures that are not in the repo, so it is `#[ignore]`d and skips when they are absent:
//!
//!   cargo test --release -p ibuilder-cli --test grizzy_e2e -- --ignored --nocapture
//!
//! What it proves: every file the Rust rewrite writes for main / subsdk0 / sdk is byte-identical to the Python oracle's (`out-py/pyrewrite/out/<module>/`), the framework is assembled
//! and signed, and the signature verifies (`ibuilder_sign::verify_file`, and `codesign --verify --strict` on the Mach-O when codesign exists).

use ibuilder_cli::{CliArgs, adapters::wired, format_report, run};
use ibuilder_testkit::{IOSBUILDER, compare_files, grizzy_exefs, grizzy_oracle};
use std::path::{Path, PathBuf};

fn scratch() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("ibuilder-grizzy-e2e")
}

#[test]
#[ignore = "heavy: needs the Grizzy fixtures under $IOSBUILDER, the original iOSBuilder checkout (read only)"]
fn grizzy_end_to_end() {
    let exefs = PathBuf::from(grizzy_exefs(""));
    let beside = exefs.parent().unwrap().to_path_buf();
    let host = Path::new(&IOSBUILDER.to_string()).join("out-load/host-prebuilt-loadable-ios.dylib");
    // the engine copy of the reference environment (docs/CONVENTIONS.md): $SCRATCH/ref/engine/dm/{games,host,tools}
    let default_bundle = ibuilder_testkit::scratch_old().join("ref/engine");
    let bundle = std::env::var_os("IBUILDER_ENGINE_BUNDLE")
        .map(PathBuf::from)
        .or(Some(default_bundle))
        .filter(|b| b.join("dm/host").is_dir());
    let (Some(bundle), true, true) = (bundle, exefs.join("main").exists(), host.exists()) else {
        ibuilder_testkit::skip(&format!(
            "needs IBUILDER_ENGINE_BUNDLE (a folder with dm/host and dm/games), {} and {}",
            exefs.display(),
            host.display()
        ));
        return;
    };
    let out = scratch();
    let _ = std::fs::remove_dir_all(&out);
    let a = CliArgs {
        exefs: exefs.clone(),
        out: out.clone(),
        bundle: Some(bundle),
        host: Some(host),
        romfs: Some(beside.join("romfs_manifest.tsv")),
        ..Default::default()
    };
    eprintln!("stage workers: {}", wired().join(", "));
    let r = run(&a).expect("run");
    eprintln!("{}", format_report(&r));
    assert_eq!(r.chain.rc, 0, "{:?}", r.chain);
    assert_eq!(
        r.chain.status.get("rewrite").map(String::as_str),
        Some("ok")
    );
    assert_eq!(
        r.chain.status.get("assemble").map(String::as_str),
        Some("ok")
    );

    // byte-identical rewrite output (match.tsv is only written when the game table has build rows: the registered table does, the derived one does not)
    let mut compared = 0;
    for m in ["main", "subsdk0", "sdk"] {
        for ext in [
            "patched.bin",
            "fixups.tsv",
            "sites.tsv",
            "gates.tsv",
            "cuts.tsv",
            "obs.tsv",
            "changed.tsv",
        ] {
            let file = format!("{m}.{ext}");
            let ours = out.join("extract/rewrite").join(m).join(&file);
            compare_files(&ours, grizzy_oracle(m, &file)).unwrap_or_else(|e| panic!("{e}"));
            compared += 1;
        }
    }
    assert_eq!(compared, 21);

    // the framework is signed and the signature verifies
    let fw = out.join("DmGameProbe.framework/DmGameProbe");
    let v = ibuilder_sign::verify_file(&fw).expect("verify");
    assert!(
        v.pages_ok > 0 && v.pages_ok == v.n_code && v.matches,
        "{v:?}"
    );
}

/// The same chain with stage 3: `--shaders` through the library entry point (`run`), the engine's real `dmsh_xlate` / `spirv-cross` as child processes, on the 21,990 shaders of Grizzy. About a minute and a half
/// and ~3.5 GB of disk (the library 1.7 GB + the cache 1.6 GB), so it is `#[ignore]`d and needs `--features shader-host-tools`:
///
///   cargo test --release -p ibuilder-cli --features shader-host-tools --test grizzy_e2e -- --ignored --nocapture grizzy_end_to_end_with_shaders
///
/// It proves the statuses, the 21 rewrite files, the pack the pipeline installs (the library of the Python build's exact size, the tables beside it, `pack.txt`), the cache folder name, and that a second run builds nothing.
/// What it does NOT do is the file-by-file comparison of the library with the Python's image (`libdiff`, 1.7 GB each): `scripts/e2e-grizzy.sh verify` does, see docs/chain/E2E-RUST.md.
#[cfg(feature = "shader-host-tools")]
#[test]
#[ignore = "heavy: ~1.5 min and ~3.5 GB of disk; needs the Grizzy fixtures and the engine's tools (read only)"]
fn grizzy_end_to_end_with_shaders() {
    let exefs = PathBuf::from(grizzy_exefs(""));
    let ios_dir = IOSBUILDER.to_string();
    let ios = Path::new(&ios_dir);
    let (romfs, host) = (
        ios.join("out/ref-img-grizzy/RomFS.img"),
        ios.join("out-load/host-prebuilt-loadable-ios.dylib"),
    );
    let engine = std::env::var_os("IBUILDER_ENGINE")
        .map(PathBuf::from)
        .unwrap_or_else(|| ios.join("engine"));
    let (xlate, spirv) = ibuilder_cli::engine_tool_paths(&engine);
    let missing: Vec<String> = [&exefs.join("main"), &romfs, &host, &xlate, &spirv]
        .into_iter()
        .filter(|p| !p.exists())
        .map(|p| p.display().to_string())
        .collect();
    if !missing.is_empty() {
        ibuilder_testkit::skip(&format!("missing {}", missing.join(", ")));
        return;
    }
    let out = scratch().with_file_name("ibuilder-grizzy-e2e-shaders");
    let _ = std::fs::remove_dir_all(&out);
    let a = CliArgs {
        exefs: exefs.clone(),
        out: out.clone(),
        bundle: Some(engine.clone()),
        host: Some(host),
        romfs: Some(romfs),
        shaders: true,
        shader_chain: Some("b24b25240cad".into()),
        engine: Some(engine),
        ..Default::default()
    };
    let r = run(&a).expect("run");
    eprintln!("{}", format_report(&r));
    assert_eq!(r.chain.rc, 0, "{:?}", r.chain);
    for stage in ["profile", "rewrite", "shaders", "assemble"] {
        assert_eq!(
            r.chain.status.get(stage).map(String::as_str),
            Some("ok"),
            "{stage}: {:?}",
            r.chain.status
        );
    }
    let mut compared = 0;
    for m in ["main", "subsdk0", "sdk"] {
        for ext in [
            "patched.bin",
            "fixups.tsv",
            "sites.tsv",
            "gates.tsv",
            "cuts.tsv",
            "obs.tsv",
            "changed.tsv",
        ] {
            let file = format!("{m}.{ext}");
            compare_files(
                out.join("extract/rewrite").join(m).join(&file),
                grizzy_oracle(m, &file),
            )
            .unwrap_or_else(|e| panic!("{e}"));
            compared += 1;
        }
    }
    assert_eq!(compared, 21);
    // the pack stage 3 installs, where the host looks for it
    let pack = out.join("shaders/0100DFA027504000");
    let lib = pack.join("b-b24b25240cad.img");
    assert_eq!(
        std::fs::metadata(&lib).expect("the library").len(),
        1_706_277_556,
        "the size of the Python build's image of the same 21,990 shaders"
    );
    for t in ["c1_full.tsv", "cb_extent.tsv", "shader_map.tsv", "game.tsv"] {
        assert!(pack.join(t).is_file(), "{t} beside the library");
    }
    let note = std::fs::read_to_string(pack.join("pack.txt")).unwrap();
    assert!(
        note.contains("shaders\t21990\n") && note.contains("chain\tb24b25240cad\n"),
        "{note}"
    );
    assert!(
        !out.join("shaders/0100DFA027504000.work").exists(),
        "the work folder goes after a finished build"
    );
    // the cache is the folder the pipeline names, with an entry per shader (22k files in <hh>/ sub-folders)
    let cache = out
        .join("shadercache")
        .join(ibuilder_pipeline::shaderpack::shader_cache_name(
            "b24b25240cad",
        ));
    assert!(
        cache.is_dir() && ibuilder_pipeline::util::folder_bytes(&cache) > 1 << 30,
        "{}",
        cache.display()
    );
    // the framework is signed and verifies
    let v =
        ibuilder_sign::verify_file(&out.join("DmGameProbe.framework/DmGameProbe")).expect("verify");
    assert!(
        v.pages_ok > 0 && v.pages_ok == v.n_code && v.matches,
        "{v:?}"
    );
    // a second run with everything installed builds nothing
    let again = run(&a).expect("second run");
    assert!(again.chain.all_ok(), "{:?}", again.chain);
    assert!(
        again
            .segments
            .iter()
            .any(|s| s.label == "3 shaders already built")
            && again.wall_s < 30.0,
        "{}",
        format_report(&again)
    );
    let _ = std::fs::remove_dir_all(&out);
}
