//! The GL port's text rewrites against the Python's own functions (`gl_shaders.fixups`, `promote_legacy`, `legacy_desktop_io`, `function_semicolons`,
//! `explicit_sampler_bindings`, `is_standalone_glsl`, `rename_identifier`, `active_default_uniforms`, `game_names`) on synthetic texts: `tests/fixtures/gl/text_cases.json`,
//! written by `tools/make_gl_fixtures.py`. Results and the names each call renamed are equal; where the Python raises `ValueError` the Rust fails.
use ibuilder_glvk::gl;
use serde_json::Value;
use std::collections::HashMap;

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn pairs(m: &HashMap<String, String>) -> Value {
    let mut v: Vec<(&String, &String)> = m.iter().collect();
    v.sort();
    Value::Array(
        v.into_iter()
            .map(|(a, b)| Value::Array(vec![Value::String(a.clone()), Value::String(b.clone())]))
            .collect(),
    )
}

#[test]
fn text_rewrites_as_python() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/gl/text_cases.json")).unwrap();
    let mut n = 0;
    for c in &cases {
        let a = c["in"].as_array().unwrap();
        let want = &c["out"];
        let mut renamed = HashMap::new();
        let got: Result<Value, String> = match s(&c["fn"]) {
            "fixups" => gl::fixups(s(&a[0]), s(&a[1]), &mut renamed)
                .map(Value::String)
                .map_err(|e| e.0),
            "promote_legacy" => Ok(Value::String(gl::promote_legacy(
                s(&a[0]),
                s(&a[1]),
                &mut renamed,
                a[2].as_bool().unwrap(),
            ))),
            "legacy_desktop_io" => Ok(Value::String(gl::legacy_desktop_io(s(&a[0]), s(&a[1])))),
            "function_semicolons" => Ok(Value::String(gl::function_semicolons(s(&a[0])))),
            "explicit_sampler_bindings" => gl::explicit_sampler_bindings(s(&a[0]))
                .map(|m| {
                    let mut v: Vec<(String, u128)> = m.into_iter().collect();
                    v.sort();
                    Value::Array(
                        v.into_iter()
                            .map(|(k, u)| {
                                Value::Array(vec![Value::String(k), Value::from(u as u64)])
                            })
                            .collect(),
                    )
                })
                .map_err(|e| e.0),
            "is_standalone_glsl" => {
                let b = ibuilder_glvk::py::to_latin1(s(&a[0])).unwrap();
                Ok(Value::Bool(gl::is_standalone_glsl(&b)))
            }
            "rename_identifier" => Ok(Value::String(gl::rename_identifier(
                s(&a[0]),
                s(&a[1]),
                s(&a[2]),
                &mut renamed,
                a[3].as_bool().unwrap(),
            ))),
            "active_default_uniforms" => Ok(match gl::active_default_uniforms(s(&a[0])) {
                None => Value::Null,
                Some(set) => {
                    let mut v: Vec<String> = set.into_iter().collect();
                    v.sort();
                    Value::from(v)
                }
            }),
            "game_names" => {
                let f: Vec<String> = a[0]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| s(x).to_string())
                    .collect();
                let r: HashMap<String, String> = a[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (s(&p[0]).to_string(), s(&p[1]).to_string()))
                    .collect();
                Ok(Value::from(gl::game_names(&f, &r)))
            }
            other => panic!("unknown case {other}"),
        };
        match s(&want[0]) {
            "ok" => assert_eq!(got.as_ref().ok(), Some(&want[1]), "{c}"),
            "ValueError" => assert!(got.is_err(), "{c}: Python raises, Rust gave {got:?}"),
            other => panic!("{other}"),
        }
        if let Some(r) = c.get("renamed") {
            assert_eq!(&pairs(&renamed), r, "{c}");
        }
        n += 1;
    }
    assert_eq!(n, cases.len());
    assert!(n > 200);
}
