//! A pixel shader's typed-output entry point and side libraries on real shaders, against the Python: `$FRAGMENT_REAL` names a folder that a local script fills from a
//! game's build (each shader's MSL just before `fs_uint` as `<key>.src.metal`, and `manifest.json` with the sha256 of what the Python made of it: `fs_uint`, the source
//! the chain writes, `unit`, `ps_libraries`' files and reflection, and the image's `_bx` / `_bf`). No game data is kept in the repository: without the folder the test
//! says so and passes.
use ibuilder_shader::finish::{pyjson::J, unit};
use ibuilder_shader::{fsuint, side};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn h(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hashes(files: &[(String, Vec<u8>)], key: &str) -> Value {
    let mut o = serde_json::Map::new();
    for (n, b) in files {
        o.insert(
            n[key.len()..].to_string(),
            Value::String(h(std::str::from_utf8(b).unwrap())),
        );
    }
    Value::Object(o)
}

#[test]
fn typed_entry_and_side_libraries_of_real_shaders() {
    let Ok(dir) = std::env::var("FRAGMENT_REAL") else {
        ibuilder_testkit::skip("no $FRAGMENT_REAL folder (made locally from a game's build)");
        return;
    };
    let man: Vec<Value> =
        serde_json::from_slice(&std::fs::read(format!("{dir}/manifest.json")).unwrap()).unwrap();
    let names = [
        "fs_uint",
        "typed_ops",
        "source",
        "unit",
        "ps_libraries",
        "image",
    ];
    let mut bad = [0usize; 6];
    let mut shown = 0;
    for x in &man {
        let key = x["key"].as_str().unwrap();
        let src = std::fs::read_to_string(format!("{dir}/{key}.src.metal")).unwrap();
        let fu = fsuint::fs_uint(&src);
        let mut miss = [false; 6];
        miss[0] = fu.as_deref().map(h).as_deref() != x["fu"].as_str();
        let ops = fu.as_deref().map(fsuint::typed_ops);
        let want_ops = x["ops"]
            .as_object()
            .map(|o| (o["alpha"].as_bool().unwrap(), o["logic"].as_bool().unwrap()));
        miss[1] = ops != want_ops;
        let full = match fu.as_deref().and_then(|_| side::shared_body(&src)) {
            Some(sh) => sh + fu.as_deref().unwrap(),
            None => src.clone(),
        };
        miss[2] = Some(h(&full).as_str()) != x["full"].as_str();
        let (u, info) = unit::unit(&full);
        let info_j = match &info.why {
            Some(w) => J::obj().with("why", w.as_str()),
            None => J::obj()
                .with(
                    "alpha",
                    J::Arr(info.alpha.iter().map(|s| J::from(s.as_str())).collect()),
                )
                .with(
                    "logic",
                    J::Arr(info.logic.iter().map(|s| J::from(s.as_str())).collect()),
                ),
        };
        let uj = J::Arr(vec![u.map(J::Str).unwrap_or(J::Null), info_j]).dumps_compact();
        miss[3] = Some(h(&uj).as_str()) != x["unit"].as_str();
        let binds: Vec<J> = x["binds"]
            .as_array()
            .unwrap()
            .iter()
            .map(J::from_value)
            .collect();
        let (files, sj) = unit::ps_libraries(key, &full, &binds, false);
        let got = hashes(&files, key);
        miss[4] = got != x["files"] || Some(sj.dumps_compact().as_str()) != x["side"].as_str();
        miss[5] = got != x["img_files"];
        for (i, m) in miss.iter().enumerate() {
            if *m {
                bad[i] += 1;
                if shown < 10 {
                    shown += 1;
                    eprintln!("{key}: {} differs", names[i]);
                }
            }
        }
    }
    for (n, b) in names.iter().zip(bad) {
        eprintln!("{n}: {} shaders, {b} differ", man.len());
    }
    assert_eq!(bad, [0; 6]);
}
