//! The pipeline's `ShaderBackend` over `ibuilder-shaderbuild`: what the glue itself does (the build and the live translation are tested in the shaderbuild crate against the Python).
#![cfg(feature = "shaders")]
#![allow(unused_imports)] // some imports serve only the tests that exist without `shader-finish` / `shadersplit`
use ibuilder_cli::adapters::shaders::{RustShaders, fn_tools};
use ibuilder_pipeline::adapters::{
    BuildRequest, LiveEntry, LiveRequest, ShaderBackend, ShaderTools, ToolOutput,
};
use ibuilder_pipeline::log::NullLog;
use ibuilder_shaderbuild::tools::ShaderTools as BuildTools;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[test]
fn the_pipelines_tools_get_the_program_name_first_and_the_builds_do_not() {
    let seen: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(vec![]));
    let s2 = seen.clone();
    let t = ShaderTools {
        xlate: Arc::new(move |a: &[String]| {
            s2.lock().unwrap().push(a.to_vec());
            ToolOutput {
                rc: 3,
                stdout: "x".into(),
                stderr: "[RED] k: no".into(),
            }
        }),
        spirv_cross: Arc::new(|a: &[String]| ToolOutput {
            rc: 0,
            stdout: a.join(" "),
            stderr: String::new(),
        }),
        glslang: None,
    };
    let b = fn_tools(&t);
    let r = b.xlate(&["jobs.json".into(), "out".into()]);
    assert_eq!(
        (r.rc, r.stdout.as_str(), r.stderr.as_str()),
        (3, "x", "[RED] k: no")
    );
    assert_eq!(
        seen.lock().unwrap()[0],
        vec!["dmsh_xlate", "jobs.json", "out"]
    );
    assert_eq!(
        b.spirv_cross(&["--version".into()]).stdout,
        "spirv-cross --version"
    );
}

#[cfg(not(feature = "shader-finish"))]
#[test]
fn without_the_finish_port_the_build_refuses_in_words_not_with_a_stand_in_library() {
    let tools = ShaderTools {
        xlate: Arc::new(|_: &[String]| ToolOutput::default()),
        spirv_cross: Arc::new(|_: &[String]| ToolOutput::default()),
        glslang: None,
    };
    let p = Path::new("/nonexistent");
    let req = BuildRequest {
        pkg_dir: p,
        profile_dir: p,
        game_id: "g",
        out_dir: p,
        chain: "c",
        jobs: 1,
        tables: true,
        pulling: false,
        cas: p,
        tools: &tools,
    };
    let e = RustShaders
        .build(&req, &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("ibuilder_shader::finish") && e.contains("shader-finish"),
        "{e}"
    );
}

#[cfg(not(feature = "shadersplit"))]
#[test]
fn without_the_split_crate_the_split_refuses_in_words() {
    use ibuilder_pipeline::adapters::SplitRequest;
    let gate = Arc::new(ibuilder_pipeline::gate::ShaderMemoryGate::new(
        Arc::new(ibuilder_pipeline::memory::FakeMem::new(0)),
        Arc::new(NullLog),
        None,
        ibuilder_pipeline::thermal::RealClock::new(),
    ));
    let p = Path::new("/nonexistent");
    let req = SplitRequest {
        romfs: p,
        main_nso: p,
        pkg_dir: p,
        tables_dir: p,
        profile_dir: p,
        game_id: "g",
        dlc: &[],
        jobs: 1,
        memo_dir: p,
        gate: &gate,
    };
    let e = RustShaders.split(&req, &NullLog).unwrap_err().to_string();
    assert!(e.contains("ibuilder-shadersplit"), "{e}");
    assert_eq!(RustShaders.split_version(), "-");
}

#[test]
fn live_entries_the_host_got_wrong_are_refused_with_the_splits_messages() {
    let dir = std::env::temp_dir().join(format!("ibsb-glue-{}", std::process::id()));
    let (tables, derived) = (dir.join("pack"), dir.join("derived"));
    std::fs::create_dir_all(&tables).unwrap();
    std::fs::create_dir_all(&derived).unwrap();
    std::fs::write(derived.join("game.tsv"), "title_id\t0x0100DFA027504000\n").unwrap();
    std::fs::write(
        tables.join("shader_map.tsv"),
        "package\tindex\tstage\tdm_ctl_sha16\tsource\n",
    )
    .unwrap();
    let tools = ShaderTools {
        xlate: Arc::new(|_: &[String]| ToolOutput::default()),
        spirv_cross: Arc::new(|_: &[String]| ToolOutput::default()),
        glslang: None,
    };
    let entries = vec![
        LiveEntry {
            key: "1234567890abcdef".into(),
            ctl: vec![0; 0x40],
            code: vec![0; 0x20],
        },
        LiveEntry {
            key: "fedcba0987654321".into(),
            ctl: vec![0; 0x100],
            code: vec![0; 0x40],
        },
    ];
    let work = dir.join("work");
    let r = RustShaders
        .live_translate(
            &LiveRequest {
                entries: &entries,
                pack: &tables,
                derived: &derived,
                game_id: "g",
                work: &work,
                chain: "c",
                tools: &tools,
                cas: None,
            },
            &NullLog,
        )
        .unwrap();
    assert!(r.ok.is_empty() && r.files.is_empty());
    assert_eq!(
        r.bad["1234567890abcdef"],
        "control section of 64 bytes, code area of 32"
    );
    assert!(
        r.bad["fedcba0987654321"].starts_with("its bytes give key "),
        "{:?}",
        r.bad
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_pipelines_cache_folder_is_the_one_the_build_makes() {
    // `ibuilder_pipeline::shaderpack::shader_cache_folder` keeps `cas-<chain>.dmsb` and removes every other `cas*`: it must be the very folder the build creates, or the cache would be
    // swept at the start of each build (and the pipeline's own name must be what the build's sweep of other chains matches)
    let dir = std::env::temp_dir().join(format!("ibsb-cachename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let chain = "6c53446391cc";
    let store = ibuilder_shaderbuild::cache::Store::open(&dir, chain, &dir.join("stage")).unwrap();
    assert_eq!(
        store.cache,
        dir.join(ibuilder_pipeline::shaderpack::shader_cache_name(chain))
    );
    assert!(store.cache.is_dir());
    // another chain's folder: the build's own sweep removes the same ones the pipeline's does
    let other = dir.join(ibuilder_pipeline::shaderpack::shader_cache_name(
        "19d183a2b462",
    ));
    std::fs::create_dir_all(other.join("ab")).unwrap();
    assert_eq!(
        ibuilder_shaderbuild::cache::Store::sweep_other_chains(&dir, chain),
        vec![ibuilder_pipeline::shaderpack::shader_cache_name(
            "19d183a2b462"
        )]
    );
    assert!(!other.exists() && store.cache.is_dir());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The GL / Vulkan layer tools: the pipeline's glslang (the app's `gameapp_glslang`) gets its program name first, as SPIRV-Cross does; without one, and without the
/// Mac's child-process fallback, every glslang call is rc 127 with `NO_GLSLANG`
#[cfg(feature = "glvk")]
#[test]
fn the_layer_tools_take_glslang_from_the_pipeline() {
    use ibuilder_cli::adapters::shaders::layer_tools;
    use ibuilder_glvk::LayerTools;
    let echo = |rc: i32| -> ibuilder_pipeline::adapters::ToolFn {
        Arc::new(move |a: &[String]| ToolOutput {
            rc,
            stdout: a.join(" "),
            stderr: String::new(),
        })
    };
    let mut t = ShaderTools {
        xlate: echo(9),
        spirv_cross: echo(0),
        glslang: Some(echo(2)),
    };
    let l = layer_tools(&t);
    let g = l.glslang(&["-E".into(), "-S".into(), "vert".into(), "a.in".into()]);
    assert_eq!(
        (g.rc, g.stdout.as_str()),
        (2, "glslangValidator -E -S vert a.in")
    );
    assert_eq!(
        l.spirv_cross(&["--revision".into()]).stdout,
        "spirv-cross --revision"
    );
    t.glslang = None;
    let g = layer_tools(&t).glslang(&["--version".into()]);
    if cfg!(feature = "shader-host-tools") {
        assert_ne!(
            g.stdout, "glslangValidator --version",
            "the Mac's own glslangValidator, not the pipeline's"
        );
    } else {
        assert_eq!(
            (g.rc, g.stderr.as_str()),
            (127, ibuilder_glvk::tools::NO_GLSLANG)
        );
    }
}
