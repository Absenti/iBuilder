//! Differential tests of `ibuilder_shader::finish` against the Python (the engine's own modules) on synthetic inputs: the parts of the finishing step the Grizzy library does not exercise
//! (bindless textures, computed driver-segment indices, vertex raster facts, readable-tile samples through gl_FragCoord, vertex libraries, SPIR-V rewriting, geometry text helpers).
//! The cases are made by `tools/finish/gen_diff_cases.py` (hand-made MSL / SPIR-V, the Python's answers stored as the text `json.dumps` gives). The file is looked up at
//! `$FINISH_CASES`, else `tests/data/finish_cases.json`; when it is absent the test says so and passes.
use ibuilder_shader::finish::{entry, fetch, geometry, pyjson::J, reflect, spv, unit, vin};
use ibuilder_shader::{fsuint, side};
use serde_json::Value;

fn cases() -> Option<Value> {
    let p = std::env::var("FINISH_CASES").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/finish_cases.json").to_string()
    });
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).ok(),
        Err(_) => {
            ibuilder_testkit::skip(&format!(
                "no differential cases at {p} (generate them with tools/finish/gen_diff_cases.py)"
            ));
            None
        }
    }
}

/// the generator stores a long expectation as `sha256:<hex>:<length>`
fn same(expect: &str, got: &str) -> bool {
    use sha2::{Digest, Sha256};
    match expect.strip_prefix("sha256:") {
        Some(r) => {
            let (h, n) = r.split_once(':').unwrap_or((r, ""));
            n == got.len().to_string()
                && h == Sha256::digest(got.as_bytes())
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
        }
        None => expect == got,
    }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}
fn jstrs(v: Vec<String>) -> J {
    J::Arr(v.into_iter().map(J::Str).collect())
}

struct Tally {
    name: &'static str,
    n: usize,
    bad: Vec<String>,
}
impl Tally {
    fn new(name: &'static str) -> Tally {
        Tally {
            name,
            n: 0,
            bad: vec![],
        }
    }
    /// `expect` is the generator's `attempt` result: {"ok": json text} or {"err": ...}; `got` the Rust result (None = an error)
    fn check(&mut self, id: usize, expect: &Value, got: Option<String>) {
        self.n += 1;
        let ok = match (expect.get("ok").and_then(|x| x.as_str()), &got) {
            (Some(e), Some(g)) => same(e, g),
            (None, None) => true,
            _ => false,
        };
        if !ok && self.bad.len() < 6 {
            self.bad.push(format!(
                "{} #{id}: python {} / rust {}",
                self.name,
                expect.to_string().chars().take(300).collect::<String>(),
                got.map(|g| g.chars().take(300).collect::<String>())
                    .unwrap_or("error".into())
            ));
        } else if !ok {
            self.bad.push(String::new());
        }
    }
    fn exact(&mut self, id: usize, expect: &str, got: String) {
        self.n += 1;
        if !same(expect, &got) {
            if self.bad.len() < 6 {
                self.bad.push(format!(
                    "{} #{id}: python {} / rust {}",
                    self.name,
                    expect.chars().take(300).collect::<String>(),
                    got.chars().take(300).collect::<String>()
                ));
            } else {
                self.bad.push(String::new());
            }
        }
    }
    fn done(self) -> usize {
        eprintln!("{}: {} cases, {} differ", self.name, self.n, self.bad.len());
        for b in self.bad.iter().filter(|b| !b.is_empty()) {
            eprintln!("  {b}");
        }
        self.bad.len()
    }
}

#[test]
fn reflection_and_driver_reads() {
    let Some(c) = cases() else { return };
    let mut t = Tally::new("reflect");
    for (i, x) in c["reflect"].as_array().unwrap().iter().enumerate() {
        let got = reflect::reflect("k", &s(x, "stage"), &s(x, "msl"), &x["res"])
            .ok()
            .map(|b| J::Arr(b).dumps_compact());
        t.check(i, &x["expect"], got);
    }
    let mut bad = t.done();
    let mut t = Tally::new("drv_reads / unfillable");
    for (i, x) in c["drv"].as_array().unwrap().iter().enumerate() {
        let msl = s(x, "msl");
        let dr = reflect::drv_reads(&msl);
        t.exact(i, &s(x, "expect"), dr.dumps_compact());
        for st in ["ps", "vs", "cs", "gs", "ts", "ms"] {
            t.exact(
                i,
                x["unfillable"][st].as_str().unwrap(),
                jstrs(reflect::unfillable(&dr, st)).dumps_compact(),
            );
        }
    }
    bad += t.done();
    // gptr_sites: the sites list, or the ValueError's message
    let mut t = Tally::new("gptr_sites");
    for (i, x) in c["gptr"].as_array().into_iter().flatten().enumerate() {
        let (expect, got) = match (
            x["expect"].get("ok"),
            reflect::gptr_sites(&s(x, "stage"), &x["res"]),
        ) {
            (Some(e), Ok(g)) => (e.as_str().unwrap_or("").to_string(), g.dumps_compact()),
            (_, Ok(g)) => (format!("error {}", x["expect"]["err"]), g.dumps_compact()),
            (_, Err(m)) => (
                x["expect"]
                    .get("err")
                    .map(|e| format!("error {e}"))
                    .unwrap_or_else(|| x["expect"].to_string()),
                format!("error {}", Value::String(m)),
            ),
        };
        t.exact(i, &expect, got);
    }
    bad += t.done();
    assert_eq!(bad, 0);
}

#[test]
fn raster_facts_and_screen_varyings() {
    let Some(c) = cases() else { return };
    let mut t = Tally::new("raster facts (vs)");
    for (i, x) in c["vs"].as_array().unwrap().iter().enumerate() {
        let msl = s(x, "msl");
        t.exact(
            i,
            &s(x, "raster"),
            reflect::raster_facts("vs", &msl).dumps_compact(),
        );
        t.exact(
            i,
            &s(x, "screen"),
            J::Arr(
                fetch::screen_varyings(&msl)
                    .into_iter()
                    .map(J::Int)
                    .collect(),
            )
            .dumps_compact(),
        );
        t.exact(
            i,
            &s(x, "attributes"),
            J::Arr(
                reflect::attributes(&msl)
                    .into_iter()
                    .map(|(a, b, l)| J::Arr(vec![J::Str(a), J::Str(b), J::Int(l)]))
                    .collect(),
            )
            .dumps_compact(),
        );
    }
    let mut bad = t.done();
    let mut t = Tally::new("raster facts (ps)");
    for (i, x) in c["ps"].as_array().unwrap().iter().enumerate() {
        t.exact(
            i,
            &s(x, "raster"),
            reflect::raster_facts("ps", &s(x, "msl")).dumps_compact(),
        );
    }
    bad += t.done();
    assert_eq!(bad, 0);
}

#[test]
fn fragment_units_and_readable_tile_versions() {
    let Some(c) = cases() else { return };
    let (mut tu, mut tf, mut ts, mut te) = (
        Tally::new("unit"),
        Tally::new("fold"),
        Tally::new("shared_body"),
        Tally::new("entries"),
    );
    for (i, x) in c["frag"].as_array().unwrap().iter().enumerate() {
        let msl = s(x, "msl");
        let (u, info) = unit::unit(&msl);
        let info_j = match &info.why {
            Some(w) => J::obj().with("why", w.as_str()),
            None => J::obj()
                .with("alpha", jstrs(info.alpha.clone()))
                .with("logic", jstrs(info.logic.clone())),
        };
        tu.exact(
            i,
            &s(x, "unit"),
            J::Arr(vec![u.clone().map(J::Str).unwrap_or(J::Null), info_j]).dumps_compact(),
        );
        let binds: Vec<J> = x["binds"]
            .as_array()
            .unwrap()
            .iter()
            .map(J::from_value)
            .collect();
        let f = fetch::fold(
            u.as_deref().unwrap_or(&msl),
            &binds,
            x["om_only_ok"].as_bool().unwrap(),
        );
        let fj = match f {
            Ok((t, m)) => J::Arr(vec![J::Str(t), m]),
            Err(r) => J::Arr(vec![J::Null, J::Str(r)]),
        };
        tf.check(i, &x["fold"], Some(fj.dumps_compact()));
        ts.exact(
            i,
            &s(x, "shared"),
            unit::shared_body(&msl)
                .map(J::Str)
                .unwrap_or(J::Null)
                .dumps_compact(),
        );
        let ej = unit::entries(&msl).ok().map(|es| {
            J::Arr(
                es.into_iter()
                    .map(|e| {
                        J::Arr(vec![
                            J::Str(e.attr),
                            J::Str(e.ret),
                            J::Str(e.name),
                            jstrs(e.params),
                            J::Str(e.decl),
                            J::Str(e.args),
                        ])
                    })
                    .collect(),
            )
            .dumps_compact()
        });
        te.check(i, &x["entries"], ej);
    }
    let mut bad = tu.done() + tf.done() + ts.done() + te.done();
    // the typed-output entry point main0_u (fs_uint, typed_ops), the source the chain writes with it (shared body + main0_u + main0_d), its unit and readable-tile
    // version, and ps_libraries as shader_device runs it (source mode)
    let (mut tfu, mut tops, mut tfull, mut tu, mut tf, mut tp) = (
        Tally::new("typed fs_uint"),
        Tally::new("typed typed_ops"),
        Tally::new("typed source"),
        Tally::new("typed unit"),
        Tally::new("typed fold"),
        Tally::new("typed ps_libraries"),
    );
    for (i, x) in c["typed"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        let msl = s(x, "msl");
        let fu = fsuint::fs_uint(&msl);
        let fd = if x["dual"].as_bool().unwrap() {
            fsuint::fs_dual(&msl)
        } else {
            None
        };
        tfu.exact(
            i,
            &s(x, "fu"),
            fu.clone().map(J::Str).unwrap_or(J::Null).dumps_compact(),
        );
        tops.exact(
            i,
            &s(x, "ops"),
            fu.as_deref()
                .map(|f| {
                    let (a, l) = fsuint::typed_ops(f);
                    J::obj().with("alpha", a).with("logic", l)
                })
                .unwrap_or(J::Null)
                .dumps_compact(),
        );
        let shared = if fu.is_some() || fd.is_some() {
            side::shared_body(&msl)
        } else {
            None
        };
        let full = match shared {
            Some(sh) => sh + fu.as_deref().unwrap_or("") + fd.as_deref().unwrap_or(""),
            None => msl.clone(),
        };
        tfull.exact(i, &s(x, "full"), J::Str(full.clone()).dumps_compact());
        let (u, info) = unit::unit(&full);
        let info_j = match &info.why {
            Some(w) => J::obj().with("why", w.as_str()),
            None => J::obj()
                .with("alpha", jstrs(info.alpha.clone()))
                .with("logic", jstrs(info.logic.clone())),
        };
        tu.exact(
            i,
            &s(x, "unit"),
            J::Arr(vec![u.clone().map(J::Str).unwrap_or(J::Null), info_j]).dumps_compact(),
        );
        let binds: Vec<J> = x["binds"]
            .as_array()
            .unwrap()
            .iter()
            .map(J::from_value)
            .collect();
        let om = x["om_only_ok"].as_bool().unwrap();
        let fj = match fetch::fold(u.as_deref().unwrap_or(&full), &binds, om) {
            Ok((t, m)) => J::Arr(vec![J::Str(t), m]),
            Err(r) => J::Arr(vec![J::Null, J::Str(r)]),
        };
        tf.check(i, &x["fold"], Some(fj.dumps_compact()));
        let (mut files, side_j) = unit::ps_libraries("k0", &full, &binds, om);
        files.sort();
        let mut fo = J::obj();
        for (n, b) in files {
            fo.set(&n, String::from_utf8(b).unwrap());
        }
        tp.check(
            i,
            &x["pslib"],
            Some(J::Arr(vec![fo, side_j]).dumps_compact()),
        );
    }
    bad += tfu.done() + tops.done() + tfull.done() + tu.done() + tf.done() + tp.done();
    assert_eq!(bad, 0);
}

#[test]
fn vertex_libraries() {
    let Some(c) = cases() else { return };
    let (mut ti, mut tv, mut tp) = (
        Tally::new("int_inputs"),
        Tally::new("vin variant"),
        Tally::new("pull variant"),
    );
    for (i, x) in c["vlib"].as_array().unwrap().iter().enumerate() {
        let msl = s(x, "msl");
        // a dict: compared as a set of pairs (the Python's order is its insertion order)
        let want: Value = serde_json::from_str(x["int_inputs"].as_str().unwrap()).unwrap();
        let got: Value = Value::Object(
            entry::int_inputs(&msl)
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect(),
        );
        ti.n += 1;
        if want != got {
            ti.bad.push(format!("int_inputs #{i}: {want} / {got}"));
        }
        let vj = match vin::vin_variant(&msl) {
            Ok((t, sh)) => J::Arr(vec![J::Str(t), J::Arr(sh)]),
            Err(r) => J::Arr(vec![J::Null, J::Str(r)]),
        };
        tv.check(i, &x["vin"], Some(vj.dumps_compact()));
        let pj = match vin::pull_variant(&msl) {
            Ok((t, info)) => J::Arr(vec![J::Str(t), info]),
            Err(r) => J::Arr(vec![J::Null, J::Str(r)]),
        };
        tp.check(i, &x["pull"], Some(pj.dumps_compact()));
    }
    let bad = ti.done() + tv.done() + tp.done();
    assert_eq!(bad, 0);
}

#[test]
fn spirv_rewriting() {
    let Some(c) = cases() else { return };
    let (mut ts, mut tf) = (Tally::new("spv_single_lane"), Tally::new("flatten_sets"));
    for (i, x) in c["spv"].as_array().unwrap().iter().enumerate() {
        let d = unhex(x["spv"].as_str().unwrap());
        let got = spv::spv_single_lane(&d).ok().map(|r| match r {
            Some(n) => format!("[true, {:?}]", hex(&n)),
            None => format!("[false, {:?}]", hex(&d)),
        });
        ts.check(i, &x["expect"], got);
    }
    for (i, x) in c["flatten"].as_array().unwrap().iter().enumerate() {
        let d = unhex(x["spv"].as_str().unwrap());
        tf.check(
            i,
            &x["expect"],
            spv::flatten_sets(&d).ok().map(|n| format!("{:?}", hex(&n))),
        );
    }
    let bad = ts.done() + tf.done();
    assert_eq!(bad, 0);
}

#[test]
fn geometry_text_helpers() {
    let Some(c) = cases() else { return };
    let (mut tp, mut tc, mut tr, mut tl) = (
        Tally::new("topology_pointer"),
        Tally::new("capture_input"),
        Tally::new("reader"),
        Tally::new("output_layout"),
    );
    let q = |t: String| J::Str(t).dumps_compact();
    for (i, x) in c["geo"].as_array().unwrap().iter().enumerate() {
        let msl = s(x, "msl");
        tp.check(
            i,
            &x["topology_pointer"],
            geometry::topology_pointer(&msl).ok().map(q),
        );
        tc.check(
            i,
            &x["capture_input"],
            geometry::capture_input(&msl).ok().map(q),
        );
    }
    for (i, x) in c["reader"].as_array().unwrap().iter().enumerate() {
        tr.check(i, &x["expect"], geometry::reader(&s(x, "msl")).ok().map(q));
    }
    for (i, x) in c["output_layout"].as_array().unwrap().iter().enumerate() {
        let code = unhex(x["code"].as_str().unwrap());
        for t in 0..6 {
            tl.check(
                i * 10 + t,
                &x["expect"][t],
                geometry::output_layout(&code, t)
                    .ok()
                    .map(|j| j.dumps_compact()),
            );
        }
    }
    // gs_slice reads the GLSL a (fake) spirv-cross writes
    struct Glsl(String);
    impl ibuilder_shader::tools::ShaderTools for Glsl {
        fn xlate(&self, _: &[String]) -> ibuilder_shader::tools::ToolRun {
            ibuilder_shader::tools::ToolRun::ok()
        }
        fn spirv_cross(&self, _: &[String]) -> ibuilder_shader::tools::ToolRun {
            ibuilder_shader::tools::ToolRun {
                rc: 0,
                stdout: self.0.clone(),
                stderr: String::new(),
            }
        }
    }
    let mut tg = Tally::new("gs_slice");
    for (i, x) in c["gs_slice"].as_array().unwrap().iter().enumerate() {
        let got = geometry::gs_slice(&Glsl(s(x, "glsl")), std::path::Path::new("x.spv")).map(
            |(l, ch)| {
                J::obj()
                    .with("loc", l)
                    .with("comp", ch.to_string())
                    .dumps_compact()
            },
        );
        // the Python's answer is None (-> `{"ok": "null"}`) or the dict
        let want_none = x["expect"]["ok"].as_str() == Some("null");
        if want_none {
            tg.n += 1;
            if got.is_some() {
                tg.bad
                    .push(format!("gs_slice #{i}: python None / rust {got:?}"));
            }
        } else {
            tg.check(i, &x["expect"], got);
        }
    }
    let bad = tp.done() + tc.done() + tr.done() + tl.done() + tg.done();
    assert_eq!(bad, 0);
}
