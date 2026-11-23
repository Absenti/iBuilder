//! The vertex entry points and libraries on real shaders, against the HEAD Python: a folder made by a replay of a game's build (`$VERTEX_REAL`: `manifest.json` and
//! `<key>.src.metal`, each shader's source after vs_pad recovered from the build's library image, with the sha256 of what the Python made of it: the vertex section's source, its
//! analysis source, `plain`'s length, the viewport mask, discard and layer facts, and `vs_libraries`' files, reflection, cut flag and red lines). Game data stays out of the
//! repository: without `$VERTEX_REAL` the test says so and passes.
use ibuilder_shader::finish::pyjson::J;
use ibuilder_shader::{finish::vsunit, vs};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn h(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `json.dumps(v)`: ensure_ascii
fn dumps(v: &J) -> String {
    let mut o = String::new();
    for c in v.dumps_compact().chars() {
        if c.is_ascii() {
            o.push(c);
        } else {
            let mut b = [0u16; 2];
            for u in c.encode_utf16(&mut b) {
                o.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    o
}

#[test]
fn real_vertex_shaders() {
    let Ok(dir) = std::env::var("VERTEX_REAL") else {
        return ibuilder_testkit::skip(
            "VERTEX_REAL is not set (a replay folder of real vertex shaders)",
        );
    };
    let man: Vec<Value> =
        serde_json::from_slice(&std::fs::read(format!("{dir}/manifest.json")).unwrap()).unwrap();
    let (mut n, mut bad_sec, mut bad_lib) = (0, Vec::new(), Vec::new());
    for x in &man {
        n += 1;
        let key = x["key"].as_str().unwrap();
        let src = std::fs::read_to_string(format!("{dir}/{key}.src.metal")).unwrap();
        let tf = x.get("tf").filter(|v| !v.is_null());
        let e = match vs::vertex_entries(&src, &x["res"], tf.is_some(), tf) {
            Ok(e) => e,
            Err(err) => {
                bad_sec.push(format!("{key}: {err}"));
                continue;
            }
        };
        let lay = dumps(
            &e.layer_from
                .map(|(l, c)| J::obj().with("loc", l).with("comp", c.to_string()))
                .unwrap_or(J::Null),
        );
        if x["full"].as_str() != Some(&h(&e.full))
            || x["single"].as_str() != Some(&h(&e.single))
            || x["plain_len"].as_u64() != Some(e.plain_len as u64)
            || x["viewport_mask"].as_bool() != Some(e.viewport_mask)
            || x["discard"].as_bool() != Some(e.discard)
            || x["lay"].as_str() != Some(&lay)
        {
            bad_sec.push(key.to_string());
            continue;
        }
        for (want, pulling) in [(x, true), (&x["nopull"], false)] {
            let got = vsunit::vs_libraries(key, &e.full, pulling);
            let files: serde_json::Map<String, Value> = got
                .outputs
                .iter()
                .map(|(f, b)| {
                    (
                        f[key.len()..].to_string(),
                        Value::String(h(std::str::from_utf8(b).unwrap())),
                    )
                })
                .collect();
            let mut side = J::obj();
            if let Some(p) = &got.pull {
                side.set("pull", p.clone());
            }
            if let Some(v) = &got.vin {
                side.set("vin", v.clone());
            }
            let log: Vec<String> = want["log"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap().to_string())
                .collect();
            if Value::Object(files) != want["files"]
                || want["side"].as_str() != Some(&dumps(&side))
                || want["cut"].as_bool() != Some(got.cut)
                || log != got.log
            {
                bad_lib.push(format!("{key} (pulling {pulling})"));
            }
        }
    }
    eprintln!(
        "real vertex shaders: {n}, vertex section differs {}, vs_libraries differs {}",
        bad_sec.len(),
        bad_lib.len()
    );
    for b in bad_sec.iter().chain(&bad_lib).take(10) {
        eprintln!("  {b}");
    }
    assert!(n > 0 && bad_sec.is_empty() && bad_lib.is_empty());
}
