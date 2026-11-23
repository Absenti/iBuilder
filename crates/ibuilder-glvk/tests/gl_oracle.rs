//! `gl::build` against the Python's `gl_shaders.py` on the synthetic fixtures (`tests/fixtures/gl/input/` + `main.nso`): the table and every MSL file, byte for byte.
//!
//! `expected/` was written by `tools/oracle.py gl <input as a DMROMFS1 image> main.nso expected` (an image, so that the Python walks the folders sorted as the phone's
//! RomFS image is; over a folder the Python takes the file system's order). Needs the tools the oracle ran: `SPIRV_CROSS` (the engine's pinned
//! `dm/gfx/tools/spirv_cross_bin/spirv-cross`, else `spirv-cross` on PATH) and `GLSLANG` (else `glslangValidator`); without them the test says so and passes.
#![cfg(feature = "host-tools")]

use ibuilder_glvk::LayerTools;
use ibuilder_glvk::tools::ProcessLayerTools;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gl")
}

fn tools_or_skip() -> Option<ProcessLayerTools> {
    let t = ProcessLayerTools::from_env();
    if t.spirv_cross(&["--revision".into()]).rc == 127 || t.glslang(&["--version".into()]).rc == 127
    {
        eprintln!(
            "SKIP: spirv-cross ({}) or glslangValidator ({}) not found (set SPIRV_CROSS / GLSLANG)",
            t.spirv_cross_bin.display(),
            t.glslang_bin.display()
        );
        return None;
    }
    Some(t)
}

#[test]
fn gl_shaders_as_python() {
    let Some(tools) = tools_or_skip() else {
        return;
    };
    let fx = fixtures();
    let work = std::env::temp_dir().join(format!("glvk-gl-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let layer = ibuilder_glvk::gl::build(
        &[fx.join("input"), fx.join("main.nso")],
        &tools,
        &work,
        4,
        false,
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(&work);
    let exp = fx.join("expected/gl");
    let want = std::fs::read_to_string(exp.join("gl_shaders.tsv")).unwrap();
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), layer.table.lines().collect());
    assert_eq!(w.len(), g.len(), "table lines\n{}", layer.table);
    for (k, (a, b)) in w.iter().zip(&g).enumerate() {
        if k == 2 && a != b && a.starts_with("# Glslang") {
            eprintln!(
                "NOTE: the tools differ from the oracle's: {b:?} (expected {a:?}); the other lines are compared"
            );
            continue;
        }
        assert_eq!(a, b, "table line {}", k + 1);
    }
    assert!(layer.table.ends_with('\n'));
    let order: Vec<String> = std::fs::read_to_string(exp.join("msl.order"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let names: Vec<String> = layer.msl.iter().map(|m| m.0.clone()).collect();
    assert_eq!(names, order);
    for (name, text) in &layer.msl {
        let want = std::fs::read_to_string(exp.join("msl").join(name)).unwrap();
        assert!(want == *text, "{name} differs");
    }
    assert_eq!(layer.shaders, 14);
    assert!(
        layer.log.iter().any(|l| l.starts_with(
            "[RED] /bad/b.frag (80fefd25de58827d) does not translate: frag: glslang: ERROR: "
        )),
        "{:?}",
        layer.log
    );
}

#[test]
fn fixed_function_and_empty_inputs() {
    let Some(tools) = tools_or_skip() else {
        return;
    };
    let fx = fixtures();
    let work = std::env::temp_dir().join(format!("glvk-gl-fixed-{}", std::process::id()));
    let empty = work.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    // a fixed-function build: no source, an empty table and the placeholder library
    let l = ibuilder_glvk::gl::build(
        std::slice::from_ref(&empty),
        &tools,
        &work.join("w"),
        1,
        true,
    )
    .unwrap();
    let tsv = std::fs::read_to_string(fx.join("expected/gl/gl_shaders.tsv")).unwrap();
    let comment = tsv.lines().nth(1).unwrap();
    assert_eq!(
        l.table,
        format!(
            "# GL shader ABI 2\n{comment}\n# Fixed-function OpenGL uses the host Metal shaders\n"
        )
    );
    assert_eq!(
        l.msl,
        vec![(
            "empty.metal".to_string(),
            "#include <metal_stdlib>\n".to_string()
        )]
    );
    // sources in a fixed-function build, and no source in a shader build: the Python's red()
    let e = ibuilder_glvk::gl::build(&[fx.join("input")], &tools, &work.join("w"), 1, true)
        .unwrap_err();
    assert_eq!(e.0, "[RED] GLSL sources found in a fixed-function build");
    let e = ibuilder_glvk::gl::build(
        std::slice::from_ref(&empty),
        &tools,
        &work.join("w"),
        1,
        false,
    )
    .unwrap_err();
    assert_eq!(e.0, format!("[RED] no GLSL shaders in {}", empty.display()));
    let _ = std::fs::remove_dir_all(&work);
}
