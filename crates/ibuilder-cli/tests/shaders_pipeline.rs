//! The pipeline's `ShaderBackend` end to end on a small game made of Grizzy's `keys-s300` shaders (401 shaders): the real finish port, the engine's real tools as child processes, through the
//! request the pipeline sends (`BuildRequest`). The library must hold the Python build's files for those shaders byte for byte (its stamp differs: this profile holds only these shaders), the
//! progress lines must be the ones the pipeline parses, the tables must be where the pipeline looks for them. Needs `--features shader-finish,shader-host-tools` and the fixtures (skips without).
#![cfg(all(feature = "shader-finish", feature = "shader-host-tools"))]
use ibuilder_cli::adapters::shaders::{RustShaders, process_tools};
use ibuilder_pipeline::adapters::{BuildRequest, ShaderBackend};
use ibuilder_pipeline::shaders::parse_progress;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GAME: &str = "0100DFA027504000";
const CHAIN: &str = "b24b25240cad";

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

/// the DMROMFS1 index: (name, bytes) of every file (a tiny reader: this crate does not depend on ibuilder-shader)
fn image_files(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let u64at = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap()) as usize;
    let (ioff, isz, count) = (u64at(8), u64at(16), u64at(24));
    let idx = &data[ioff..ioff + isz];
    let pool = count * 24;
    (1..count)
        .map(|k| {
            let o = k * 24;
            let (a, b) = (
                u64::from_le_bytes(idx[o..o + 8].try_into().unwrap()) as usize,
                u64::from_le_bytes(idx[o + 8..o + 16].try_into().unwrap()) as usize,
            );
            let (no, nl) = (
                u32::from_le_bytes(idx[o + 16..o + 20].try_into().unwrap()) as usize,
                u16::from_le_bytes(idx[o + 20..o + 22].try_into().unwrap()) as usize,
            );
            (
                String::from_utf8_lossy(&idx[pool + no..pool + no + nl]).into_owned(),
                data[a..a + b].to_vec(),
            )
        })
        .collect()
}

#[test]
fn the_pipelines_build_request_gives_the_pythons_files() {
    let (old, s2) = (ibuilder_testkit::scratch_old(), ibuilder_testkit::scratch());
    let py_img = old.join("shader/runs/s300-j1/b-b24b25240cad.img");
    let (pkg, prof, engine) = (
        old.join("shader/split-out/pkg"),
        old.join("shader/profile").join(GAME),
        s2.join("B/py/engine-jobs"),
    );
    for p in [&py_img, &pkg, &prof, &engine] {
        if !p.exists() {
            ibuilder_testkit::skip(&format!("fixture {} is absent", p.display()));
            return;
        }
    }
    let want: Vec<(String, Vec<u8>)> = image_files(&read(&py_img));
    let keys: BTreeSet<String> = want
        .iter()
        .filter_map(|(n, _)| n.strip_suffix("_b.json").map(str::to_string))
        .collect();
    assert_eq!(keys.len(), 401);
    // a small game: the profile's tables cut to these shaders
    let work = std::env::temp_dir().join(format!("ibsb-pipe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let mini = work.join("profile").join(GAME);
    std::fs::create_dir_all(&mini).unwrap();
    for e in std::fs::read_dir(&prof).unwrap().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let text = String::from_utf8_lossy(&read(&e.path())).into_owned();
        let keep = |ln: &str| -> bool {
            match n.as_str() {
                "shader_map.tsv" => {
                    ln.starts_with('#')
                        || ln.starts_with("package\t")
                        || ln
                            .split('\t')
                            .nth(3)
                            .map(|k| keys.contains(k))
                            .unwrap_or(false)
                }
                "shader_facts.tsv" => {
                    ln.starts_with('#')
                        || ln
                            .split('\t')
                            .next()
                            .map(|k| keys.contains(k))
                            .unwrap_or(false)
                }
                "shader_pairs.tsv" | "pso_pairs.tsv" => {
                    ln.starts_with('#') || ln.split_whitespace().all(|k| keys.contains(k))
                }
                _ => true,
            }
        };
        let kept: String = text
            .split_inclusive('\n')
            .filter(|l| keep(l.trim_end_matches('\n')))
            .collect();
        std::fs::write(mini.join(&n), kept).unwrap();
    }
    let tools = process_tools(
        &engine.join("dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate"),
        &engine.join("dm/gfx/tools/spirv_cross_bin/spirv-cross"),
    );
    let (out, cas) = (work.join("out"), work.join("cas"));
    std::fs::create_dir_all(&cas).unwrap();
    let req = BuildRequest {
        pkg_dir: &pkg,
        profile_dir: &work.join("profile"),
        game_id: GAME,
        out_dir: &out,
        chain: CHAIN,
        jobs: 4,
        tables: true,
        pulling: false,
        cas: &cas,
        tools: &tools,
    };
    let lines = Mutex::new(Vec::<String>::new());
    let img = RustShaders
        .build(&req, &mut |l| lines.lock().unwrap().push(l.to_string()))
        .unwrap();
    assert_eq!(img, out.join(format!("b-{CHAIN}.img")));
    let got = image_files(&read(&img));
    let names: BTreeSet<&str> = got.iter().map(|g| g.0.as_str()).collect();
    for (n, bytes) in &want {
        if n == "inputs.tsv" {
            continue; // this profile holds fewer shaders: another digest
        }
        let g = got
            .iter()
            .find(|g| &g.0 == n)
            .unwrap_or_else(|| panic!("{n} is not in the library"));
        assert!(&g.1 == bytes, "{n} differs from the Python's");
    }
    assert!(
        names.contains("c1_full.tsv")
            && names.contains("cb_extent.tsv")
            && names.contains("inputs.tsv")
    );
    // the tables where the pipeline looks for them
    for t in ["c1_full.tsv", "cb_extent.tsv"] {
        assert!(
            out.join("_work/profile").join(GAME).join(t).is_file(),
            "{t}"
        );
    }
    let progress: Vec<(u64, u64)> = lines
        .lock()
        .unwrap()
        .iter()
        .filter_map(|l| parse_progress(l))
        .collect();
    assert!(progress.iter().any(|p| *p == (401, 401)), "{progress:?}");
    eprintln!(
        "pipeline build: {} files, {} progress lines parsed by the pipeline's own parser; the 401 shaders' files equal the Python's",
        got.len(),
        progress.len()
    );
    let _ = std::fs::remove_dir_all(&work);
}
