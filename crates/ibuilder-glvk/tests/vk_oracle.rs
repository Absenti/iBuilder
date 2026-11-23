//! `vk::build` against the engine's own `vk_shaders.py` (tests/fixtures/vk/expected, made by tools/make_vk_fixtures.sh through tools/oracle.py) on synthetic shaders:
//! the table and every MSL file, byte for byte and in the Python's order.
//!
//! Runs the real SPIRV-Cross: `$SPIRV_CROSS`, else `spirv-cross` on PATH; skipped with a message when neither runs. The oracle used the engine's pinned build
//! (`dm/gfx/tools/spirv_cross_bin/spirv-cross`, f11ba9f + its patch); another build may write other MSL, so point `SPIRV_CROSS` at that binary.
#![cfg(feature = "host-tools")]

use ibuilder_glvk::LayerTools;
use ibuilder_glvk::tools::ProcessLayerTools;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vk")
}

fn tools() -> Option<ProcessLayerTools> {
    let t = ProcessLayerTools::from_env();
    let r = t.spirv_cross(&["--revision".to_string()]);
    if r.rc == 127 {
        eprintln!(
            "SKIP: no spirv-cross ({}): set SPIRV_CROSS",
            r.stderr.trim()
        );
        return None;
    }
    Some(t)
}

fn work(name: &str) -> PathBuf {
    let w = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&w);
    std::fs::create_dir_all(&w).unwrap();
    w
}

#[test]
fn vk_build_is_the_python_s() {
    let Some(t) = tools() else { return };
    let f = fixtures();
    let roots = [f.join("in"), f.join("in2")];
    let w = work("vk_oracle");
    let layer = ibuilder_glvk::vk::build(&roots, &t, &w, 4).unwrap();
    let exp = f.join("expected/vk");
    let want = std::fs::read_to_string(exp.join("vk_shaders.tsv")).unwrap();
    if layer.table != want {
        let (g, e): (Vec<&str>, Vec<&str>) =
            (layer.table.lines().collect(), want.lines().collect());
        let rev = |l: &str| l.starts_with("# spirv-cross ");
        let (gr, er) = (g.iter().find(|l| rev(l)), e.iter().find(|l| rev(l)));
        if gr != er {
            eprintln!(
                "NOTE: another spirv-cross ({gr:?}, the oracle's {er:?}): its revision line is not compared"
            );
        }
        let strip = |v: &[&str]| {
            v.iter()
                .filter(|l| !rev(l))
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(strip(&g), strip(&e), "table");
    }
    let order: Vec<String> = std::fs::read_to_string(exp.join("msl.order"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let names: Vec<String> = layer.msl.iter().map(|(n, _)| n.clone()).collect();
    assert_eq!(names, order, "the MSL files and their order");
    for (name, text) in &layer.msl {
        let e = std::fs::read_to_string(exp.join("msl").join(name)).unwrap();
        assert!(*text == e, "{name} differs from the Python's");
    }
    assert_eq!(layer.shaders, 8);
    assert_eq!(
        layer.log[0],
        "[RED] skipped unsupported shader stages: geom 1"
    );
    // the Layer's files on disk
    let out = work("vk_oracle_out");
    layer.write(&out).unwrap();
    assert_eq!(
        std::fs::read_to_string(out.join("vk/vk_shaders.tsv")).unwrap(),
        layer.table
    );
    assert_eq!(
        std::fs::read_dir(out.join("vk/msl")).unwrap().count(),
        order.len()
    );
}

#[test]
fn jobs_do_not_change_the_output() {
    let Some(t) = tools() else { return };
    let f = fixtures();
    let roots = [f.join("in"), f.join("in2")];
    let a = ibuilder_glvk::vk::build(&roots, &t, &work("vk_jobs1"), 1).unwrap();
    let b = ibuilder_glvk::vk::build(&roots, &t, &work("vk_jobs8"), 8).unwrap();
    assert_eq!(a, b);
}

#[test]
fn red_cases() {
    let Some(t) = tools() else { return };
    let w = work("vk_red");
    // a root that does not exist
    let missing = w.join("nothing-here");
    let e = ibuilder_glvk::vk::build(std::slice::from_ref(&missing), &t, &w, 1).unwrap_err();
    assert_eq!(
        e.0,
        format!("[RED] {}: no such file or folder", missing.display())
    );
    // a folder without SPIR-V
    let empty = w.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(empty.join("x.txt"), b"hello").unwrap();
    let e = ibuilder_glvk::vk::build(std::slice::from_ref(&empty), &t, &w, 1).unwrap_err();
    assert_eq!(e.0, format!("[RED] no SPIR-V files in {}", empty.display()));
    // a SPIR-V file whose length is not whole words
    let odd = w.join("odd");
    std::fs::create_dir_all(&odd).unwrap();
    let mut b = std::fs::read(fixtures().join("in/a.vert")).unwrap();
    b.push(0);
    std::fs::write(odd.join("a.vert"), &b).unwrap();
    let e = ibuilder_glvk::vk::build(std::slice::from_ref(&odd), &t, &w, 1).unwrap_err();
    assert_eq!(
        e.0,
        format!(
            "[RED] /a.vert: SPIR-V length {} is not a multiple of 4",
            b.len()
        )
    );
}
