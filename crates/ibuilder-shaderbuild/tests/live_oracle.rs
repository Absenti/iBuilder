//! The live path against the phone's own `shader_device.live_translate`: real Grizzy shaders (ps / vs / cs / gs, as `(key, ctl, code)`), three bad entries, the engine's real tools. The Python run
//! (`tools/run_live.py` -> `$S2/B/oracle/live`) left its result (`ok`, `bad`, the files) and its library image; here the same entries go through `live_package` / the merged tables / a partial
//! build with the stand-in-free driver and the finisher that replays the Python's image, so what is compared is the live driver: the key check and its messages, the stage names, the record
//! format, the table merge, the partial build, the file selection.
mod common;
use common::*;
use ibuilder_shader::tools::{ShaderTools, ToolRun};
use ibuilder_shaderbuild::finish::ReplayFinisher;
use ibuilder_shaderbuild::live::{LiveEntry, live_translate};
use ibuilder_shaderbuild::tools::FnTools;
use serde_json::Value;
use std::sync::Arc;

#[test]
fn live_translation_equals_the_phones_python() {
    let dir = s2("B/oracle/live");
    if !have(&dir) || !have(&pkg()) {
        return;
    }
    let img_path = dir.join("work/out/b-b24b25240cad.img");
    if !have(&img_path) {
        return;
    }
    let img = std::fs::read(&img_path).unwrap();
    let res: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("result.json")).unwrap()).unwrap();
    // the entries in the order the Python run got them (the live rows of the merged map follow it, and so does the stamp)
    let order = std::fs::read_to_string(dir.join("order.txt")).unwrap();
    let entries: Vec<LiveEntry> = order
        .lines()
        .map(|key| LiveEntry {
            ctl: std::fs::read(dir.join("entries").join(format!("{key}.ctl"))).unwrap(),
            code: std::fs::read(dir.join("entries").join(format!("{key}.code"))).unwrap(),
            key: key.to_string(),
        })
        .collect();
    // a stand-in translator: the replay finisher ignores what the tools write, but the driver wants a .spv for each job
    let xlate = |argv: &[String]| -> ToolRun {
        let jobs: Value =
            serde_json::from_str(&std::fs::read_to_string(&argv[0]).unwrap()).unwrap();
        for j in jobs.as_array().unwrap() {
            let k = j["key"].as_str().unwrap();
            std::fs::write(
                std::path::Path::new(&argv[1]).join(format!("{k}.spv")),
                b"spv",
            )
            .unwrap();
        }
        ToolRun::ok()
    };
    let tools = FnTools::new(Arc::new(xlate), Arc::new(|_: &[String]| ToolRun::ok()));
    let work = std::env::temp_dir().join(format!("ibsb-live-{}", std::process::id()));
    let make = |_: &std::path::Path,
                _: &str|
     -> Result<Box<dyn ibuilder_shaderbuild::Finisher + '_>, String> {
        Ok(Box::new(ReplayFinisher::new(&img)?))
    };
    let log = |_: &str| {};
    let t: &dyn ShaderTools = &tools;
    let out = live_translate(
        &entries,
        &profile().join(GAME),
        &profile().join(GAME),
        GAME,
        &work,
        CHAIN,
        t,
        &make,
        &log,
        None,
    )
    .unwrap();
    let want_ok: std::collections::BTreeMap<String, String> = res["ok"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    let want_bad: std::collections::BTreeMap<String, String> = res["bad"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    assert_eq!(
        out.ok, want_ok,
        "the shaders that were translated, with their stage names"
    );
    assert_eq!(
        out.bad, want_bad,
        "the refused entries and the Python's messages"
    );
    let want_files: Vec<String> = res["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(out.files.keys().cloned().collect::<Vec<_>>(), want_files);
    for (n, bytes) in &out.files {
        assert_eq!(
            bytes,
            &std::fs::read(dir.join("files").join(n)).unwrap(),
            "{n}"
        );
    }
    eprintln!(
        "live: {} ok ({:?} stages), {} bad, {} files byte-identical to the phone's Python",
        out.ok.len(),
        out.ok.values().collect::<std::collections::BTreeSet<_>>(),
        out.bad.len(),
        out.files.len()
    );
    // the live image of the run: the same bytes as the Python's (it is a partial build over the same packages)
    let mine = std::fs::read(work.join("out/b-b24b25240cad.img")).unwrap();
    if mine != img {
        let keep = s2("B/run/live-rs-image.img");
        let _ = std::fs::create_dir_all(keep.parent().unwrap());
        std::fs::write(&keep, &mine).unwrap();
        panic!(
            "the live build's library image differs from the Python's ({} vs {} bytes); kept at {}",
            mine.len(),
            img.len(),
            keep.display()
        );
    }
    let _ = std::fs::remove_dir_all(&work);
}

/// the same entries through the REAL finish port and the real tools: the files the host reads must be the phone's Python's, byte for byte
#[cfg(all(feature = "host-tools", feature = "f2-finish"))]
#[test]
fn live_translation_with_the_real_finish_and_tools_equals_the_phones_python() {
    use ibuilder_shaderbuild::finish_adapter::RustFinisher;
    use ibuilder_shaderbuild::tools::ProcessTools;
    let dir = s2("B/oracle/live");
    let engine = s2("B/py/engine-jobs");
    if !have(&dir) || !have(&engine) || !have(&pkg()) {
        return;
    }
    let res: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("result.json")).unwrap()).unwrap();
    let order = std::fs::read_to_string(dir.join("order.txt")).unwrap();
    let entries: Vec<LiveEntry> = order
        .lines()
        .map(|key| LiveEntry {
            ctl: std::fs::read(dir.join("entries").join(format!("{key}.ctl"))).unwrap(),
            code: std::fs::read(dir.join("entries").join(format!("{key}.code"))).unwrap(),
            key: key.to_string(),
        })
        .collect();
    let tools = ProcessTools::in_engine(&engine);
    let work = std::env::temp_dir().join(format!("ibsb-live-real-{}", std::process::id()));
    let make = |prof: &std::path::Path,
                gid: &str|
     -> Result<Box<dyn ibuilder_shaderbuild::Finisher>, String> {
        Ok(Box::new(RustFinisher::load(
            &ibuilder_shaderbuild::game::game_dir(prof, gid),
            gid,
            CHAIN,
            false,
        )?))
    };
    let log = |_: &str| {};
    let t: &dyn ShaderTools = &tools;
    let out = live_translate(
        &entries,
        &profile().join(GAME),
        &profile().join(GAME),
        GAME,
        &work,
        CHAIN,
        t,
        &make,
        &log,
        None,
    )
    .unwrap();
    assert_eq!(out.ok.len(), res["ok"].as_object().unwrap().len());
    assert_eq!(out.bad.len(), 3);
    let want_files: Vec<String> = res["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(out.files.keys().cloned().collect::<Vec<_>>(), want_files);
    for (n, bytes) in &out.files {
        assert_eq!(
            bytes,
            &std::fs::read(dir.join("files").join(n)).unwrap(),
            "{n}"
        );
    }
    eprintln!(
        "live with the real finish: {} files byte-identical to the phone's Python",
        out.files.len()
    );
    let _ = std::fs::remove_dir_all(&work);
}
