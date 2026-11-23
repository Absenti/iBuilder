//! End to end on small synthetic games (`tests/data/games/<name>/`: a RomFS folder, an NSO, a profile, DLC folders): the Rust split must write the same bytes as the engine's own
//! split (`shader_device.split`), whose sha256 per file is in `expected.json` (`tools/gen_games.py` writes both). Every game exercises one family of container readers end to end:
//! the job planning, the reader, the merge order, the tables.

use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn games() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games");
    let mut v: Vec<PathBuf> = match std::fs::read_dir(&root) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.join("expected.json").is_file())
            .collect(),
        Err(_) => vec![],
    };
    v.sort();
    v
}

fn run_game(
    g: &Path,
    jobs: usize,
) -> (
    Result<ibuilder_shadersplit::SplitStats, ibuilder_shadersplit::SplitError>,
    Vec<String>,
    PathBuf,
) {
    let exp: Value =
        serde_json::from_slice(&std::fs::read(g.join("expected.json")).unwrap()).unwrap();
    let out = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-game-{}-{}-{jobs}",
        g.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    let dlc: Vec<PathBuf> = exp["dlc"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| g.join("dlc").join(d.as_str().unwrap()))
        .collect();
    let (romfs, main, prof) = (g.join("RomFS"), g.join("main"), g.join("profile"));
    let (pkg, tables) = (out.join("pkg"), out.join("tables"));
    let updates: Vec<(PathBuf, PathBuf)> = exp["updates"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|u| {
                    (
                        g.join(u.as_str().unwrap()).join("RomFS"),
                        g.join(u.as_str().unwrap()).join("main"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, "0100TESTTEST0000");
    for (r, m) in &updates {
        o.versions.push((r.as_path(), m.as_path()));
    }
    o.dlc = dlc.iter().map(|p| p.as_path()).collect();
    o.jobs = jobs;
    // `DM_SPLIT_REGISTERED_MAP` (tools/gen_registered.py): a registered shader map beside the game
    let registered = exp["registered"].as_str().map(|r| g.join(r));
    o.registered = registered.as_deref();
    // `DM_NRO_DIR`: the NROs the game decodes at run time (a CRA archive's file table is in one)
    let nro_dir = exp["nro_dir"].as_str().map(|r| g.join(r));
    o.nro_dir = nro_dir.as_deref();
    // `sys.argv[9]`: the play captures beside the game (tools/gen_split_rest.py), listed in a manifest of absolute paths written here
    let play = exp["glslc_play"].as_array().map(|ps| {
        let list: Vec<String> = ps
            .iter()
            .map(|p| g.join(p.as_str().unwrap()).display().to_string())
            .collect();
        std::fs::create_dir_all(&out).unwrap();
        let m = out.join("play.json");
        std::fs::write(&m, serde_json::to_string(&list).unwrap()).unwrap();
        m
    });
    o.glslc_play = play.as_deref();
    let log = Mutex::new(Vec::new());
    let r = split(&o, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    (r, log.into_inner().unwrap(), out)
}

#[test]
fn every_synthetic_game_is_split_to_the_bytes_of_the_python() {
    let gs = games();
    if gs.is_empty() {
        ibuilder_testkit::skip("tests/data/games is empty (run tools/gen_games.py)");
        return;
    }
    for g in &gs {
        let name = g.file_name().unwrap().to_string_lossy().into_owned();
        let exp: Value =
            serde_json::from_slice(&std::fs::read(g.join("expected.json")).unwrap()).unwrap();
        for jobs in [1usize, 4, 8] {
            let (r, log, out) = run_game(g, jobs);
            let stats = r.unwrap_or_else(|e| panic!("{name}: the split failed: {e}"));
            assert_eq!(
                stats.shaders as u64,
                exp["shaders"].as_u64().unwrap(),
                "{name}: shader count ({log:?})"
            );
            let mut got: Vec<(String, String)> = Vec::new();
            for sub in ["pkg", "tables"] {
                let mut names: Vec<_> = std::fs::read_dir(out.join(sub))
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                for n in names {
                    got.push((
                        format!("{sub}/{n}"),
                        sha(&std::fs::read(out.join(sub).join(&n)).unwrap()),
                    ));
                }
            }
            let want: Vec<(String, String)> = exp["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            assert_eq!(
                want, got,
                "{name} ({jobs} threads): the files and their sha256"
            );
            // the Python's own stdout lines (bar the progress counter and the unordered compute-source lines)
            // (a KIT member's message names its file by the path the engine opened: the generator wrote the RomFS part as @ROMFS@)
            let romfs = g.join("RomFS").display().to_string();
            let game = g.display().to_string();
            let log: Vec<String> = log
                .iter()
                .map(|l| l.replace(&romfs, "@ROMFS@").replace(&game, "@GAME@"))
                .collect();
            for l in exp["log"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap())
            {
                if l.starts_with("shaders: ")
                    || l.starts_with("single-stage programs")
                    || l.starts_with("[RED]")
                    || l.starts_with("[WARN]")
                {
                    assert!(
                        log.iter().any(|m| m == l),
                        "{name}: the Python logged {l:?}, this port: {log:?}"
                    );
                }
            }
            let _ = std::fs::remove_dir_all(&out);
        }
    }
}
