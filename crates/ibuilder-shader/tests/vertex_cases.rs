//! Differential tests of the vertex entry points and libraries against the HEAD Python (`tools/finish/gen_diff_cases.py`, categories `vmask`, `vflow`, `vlayer`, `vsunit`,
//! `vsentries`): `vs_viewport_mask`, `finish()`'s vertex section (`vs::vertex_entries`), `vs_layer_entry(full, source)`, `dmsh_side.vs_unit` / `vs_entries`, and
//! `dmsh_build.vs_libraries` in source mode (the iOS image's files, the reflection's `pull` / `vin`, whether the base was cut, the red lines), with pulling on and off.
//! The file is looked up at `$FINISH_CASES`, else `tests/data/finish_cases.json`; a file without these categories, or no file, passes with a note.
use ibuilder_shader::PostError;
use ibuilder_shader::finish::pyjson::J;
use ibuilder_shader::{finish::vsunit, vs};
use serde_json::{Value, json};

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

/// the generator stores a long text as `sha256:<hex>:<length>`
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
/// `json.dumps(v)` (the generator's `dj`): ensure_ascii, so non-ASCII characters as `\uXXXX` (two for a character past the BMP)
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
fn js(t: &str) -> String {
    dumps(&J::from(t))
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
    fn ok(&mut self, id: usize, good: bool, what: impl FnOnce() -> String) {
        self.n += 1;
        if !good {
            self.bad.push(if self.bad.len() < 6 {
                format!("{} #{id}: {}", self.name, what())
            } else {
                String::new()
            });
        }
    }
    /// `expect`: {"ok": json text} or {"err": the ValueError's message}
    fn check(&mut self, id: usize, expect: &Value, got: Result<String, String>) {
        let good = match (expect.get("ok").and_then(|x| x.as_str()), &got) {
            (Some(e), Ok(g)) => same(e, g),
            (None, Err(g)) => expect.get("err").and_then(|x| x.as_str()) == Some(g.as_str()),
            _ => false,
        };
        self.ok(id, good, || {
            format!(
                "python {} / rust {}",
                expect.to_string().chars().take(300).collect::<String>(),
                format!("{got:?}").chars().take(300).collect::<String>()
            )
        });
    }
    fn done(self) -> usize {
        eprintln!("{}: {} cases, {} differ", self.name, self.n, self.bad.len());
        for b in self.bad.iter().filter(|b| !b.is_empty()) {
            eprintln!("  {b}");
        }
        self.bad.len()
    }
}

fn msg(e: PostError) -> String {
    match e {
        PostError::Failed { msg, .. } => msg,
        PostError::Unported(s) => format!("unported {s}"),
    }
}

fn strs(n: &vsunit::Names) -> J {
    let mut j = J::obj();
    for (k, v) in n {
        j.set(k, v.as_str());
    }
    j
}

const TF_TAIL: &str = "\nvertex void main0_tf(main0_in in [[stage_in]], constant float4& u [[buffer(0)]], uint dm_vid [[vertex_id]], device uint* dm_tf0 [[buffer(8)]])\n{\n    dm_tf0[dm_vid] = as_type<uint>(in.m_0.x + u.x);\n}\n";

/// the generator's `unit_full`: the build's source for vs_unit from the source after vs_init, then the refused shape `k`
fn unit_full(src: &str, with_tf: bool, two: bool, k: i64) -> String {
    let (raster, unculled) = vs::vs_viewport_mask(src).unwrap();
    let mut full = vs::vs_depth(&raster).unwrap().1 + &vs::capture(&unculled).unwrap();
    if with_tf {
        full.push_str(TF_TAIL);
    }
    if two {
        full = full.replacen(
            "    return out;\n}\n\nvertex void main0_gs",
            "    if (acc.w > 1.0) { return out; } else { return out; }\n}\n\nvertex void main0_gs",
            1,
        );
    }
    let i = full.find("vertex void main0_gs(").unwrap_or(full.len());
    let gs = |a: &str, b: &str| format!("{}{}", &full[..i], full[i..].replace(a, b));
    match k {
        1 => full.replacen(&format!("    {}\n", vsunit::VS_DEPTH), "", 1),
        2 => full.replace("vertex main0_out main0_z(", "vertex main0_out main0_y("),
        3 => full.replace(
            "main0_z(main0_in in [[stage_in]], ",
            "main0_z(main0_in in [[stage_in]], uint x [[vertex_id]], ",
        ),
        4 => {
            full.clone()
                + "\nvertex main0_out main0_p(main0_in in [[stage_in]])\n{\n    main0_out out = {};\n    return out;\n}\n"
        }
        5 => full.replace(
            "constant float4& u [[buffer(0)]]",
            "constant float4& u [[buffer(0), function_constant(c)]]",
        ),
        6 => gs("float4 acc = u;", "float4 acc = u + 1.0;"),
        7 => gs(" return; }", " }"),
        8 => gs(
            "constant float4& u [[buffer(0)]]",
            "constant float4& v [[buffer(0)]]",
        ),
        9 => full.replace("vertex main0_out main0_z(", "vertex float4 main0_z("),
        13 => full.clone() + "\nstatic int dm_wl_in(int x) { return x; }\n",
        _ => full,
    }
}

#[test]
fn viewport_mask() {
    let Some(c) = cases() else { return };
    let Some(all) = c["vmask"].as_array() else {
        return ibuilder_testkit::skip("no vmask cases");
    };
    let mut t = Tally::new("vs_viewport_mask");
    for (i, x) in all.iter().enumerate() {
        let got = vs::vs_viewport_mask(&s(x, "msl"))
            .map(|(a, b)| dumps(&J::Arr(vec![J::Str(a), J::Str(b)])))
            .map_err(msg);
        t.check(i, &x["expect"], got);
    }
    assert_eq!(t.done(), 0);
}

#[test]
fn layer_entry_with_source() {
    let Some(c) = cases() else { return };
    let Some(all) = c["vlayer"].as_array() else {
        return ibuilder_testkit::skip("no vlayer cases");
    };
    let mut t = Tally::new("vs_layer_entry(full, source)");
    for (i, x) in all.iter().enumerate() {
        let src = s(x, "msl");
        let (raster, unculled) = vs::vs_viewport_mask(&src).unwrap();
        let full = vs::vs_depth(&raster).unwrap().1 + &vs::capture(&unculled).unwrap();
        t.ok(i, same(&s(x, "full"), &js(&full)), || "the source".into());
        let source = x["with_source"]
            .as_bool()
            .unwrap()
            .then(|| vs::vs_depth(&unculled).unwrap().1);
        let got = vs::vs_layer_entry_from(&full, source.as_deref())
            .map(|(f, lay)| {
                dumps(&J::Arr(vec![
                    J::Str(f),
                    lay.map(|(l, c)| J::obj().with("loc", l).with("comp", c.to_string()))
                        .unwrap_or(J::Null),
                ]))
            })
            .map_err(msg);
        t.check(i, &x["expect"], got);
    }
    assert_eq!(t.done(), 0);
}

#[test]
fn finish_vertex_section_and_libraries() {
    let Some(c) = cases() else { return };
    let Some(all) = c["vflow"].as_array() else {
        return ibuilder_testkit::skip("no vflow cases");
    };
    let res = json!({"sbufs": [], "textures": [], "images": [], "notes": []});
    let tfj = json!([{"stride": 16, "outputs": [[1, 4]]}, [], [], []]);
    let (mut t, mut tl) = (
        Tally::new("finish vertex section"),
        Tally::new("vs_libraries (source mode)"),
    );
    for (i, x) in all.iter().enumerate() {
        let src = s(x, "msl");
        let with_tf = x["tf"].as_bool().unwrap();
        let pre = vs::vs_base_instance(&vs::vs_layer(vs::vs_init(&src)).unwrap());
        let r = vs::vertex_entries(&pre, &res, with_tf, with_tf.then_some(&tfj));
        let e = match (&r, x.get("status")) {
            (Err(e), Some(st)) => {
                let (step, m) = match e {
                    PostError::Failed { step, msg } => (*step, msg.clone()),
                    _ => ("?", String::new()),
                };
                let good = step == "vs_viewport_mask"
                    && st == "vertex viewport mask failed"
                    && m == s(x, "detail");
                t.ok(i, good, || {
                    format!("python {} / rust {e:?}", s(x, "detail"))
                });
                continue;
            }
            (Ok(e), None) => e,
            _ => {
                t.ok(i, false, || {
                    format!("python {:?} / rust {:?}", x.get("status"), r.as_ref().err())
                });
                continue;
            }
        };
        let lay = dumps(
            &e.layer_from
                .map(|(l, c)| J::obj().with("loc", l).with("comp", c.to_string()))
                .unwrap_or(J::Null),
        );
        let good = same(&s(x, "full"), &js(&e.full))
            && same(&s(x, "single"), &js(&e.single))
            && x["plain_len"].as_u64() == Some(e.plain_len as u64)
            && x["viewport_mask"].as_bool() == Some(e.viewport_mask)
            && x["discard"].as_bool() == Some(e.discard)
            && s(x, "lay") == lay;
        t.ok(i, good, || {
            format!(
                "full {} single {} plain_len {}/{} mask {}/{} discard {}/{} lay {}/{}",
                same(&s(x, "full"), &js(&e.full)),
                same(&s(x, "single"), &js(&e.single)),
                x["plain_len"],
                e.plain_len,
                x["viewport_mask"],
                e.viewport_mask,
                x["discard"],
                e.discard,
                s(x, "lay"),
                lay
            )
        });
        for (want, pulling) in x["libs"].as_array().unwrap().iter().zip([true, false]) {
            let got = vsunit::vs_libraries("k", &e.full, pulling);
            let mut files: Vec<(String, String)> = got
                .outputs
                .iter()
                .map(|(n, b)| (n.clone(), js(std::str::from_utf8(b).unwrap())))
                .collect();
            files.sort();
            let wf = want["files"].as_object().unwrap();
            let files_ok = files.len() == wf.len()
                && files.iter().all(|(n, t)| {
                    wf.get(n)
                        .and_then(|v| v.as_str())
                        .is_some_and(|w| same(w, t))
                });
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
            let good = files_ok
                && same(&s(want, "side"), &dumps(&side))
                && want["cut"].as_bool() == Some(got.cut)
                && log == got.log;
            tl.ok(i, good, || {
                format!(
                    "pulling {pulling}: files {files_ok} {:?} / {:?}; side {} / {}; cut {} / {}; log {:?} / {:?}",
                    wf.keys().collect::<Vec<_>>(),
                    files.iter().map(|f| &f.0).collect::<Vec<_>>(),
                    s(want, "side"),
                    dumps(&side),
                    want["cut"],
                    got.cut,
                    log,
                    got.log
                )
            });
        }
    }
    assert_eq!(t.done() + tl.done(), 0);
}

#[test]
fn vertex_units() {
    let Some(c) = cases() else { return };
    let Some(all) = c["vsunit"].as_array() else {
        return ibuilder_testkit::skip("no vsunit cases");
    };
    let mut t = Tally::new("vs_unit");
    let mut fulls = Vec::new();
    for (i, x) in all.iter().enumerate() {
        let full = unit_full(
            &s(x, "msl"),
            x["tf"].as_bool().unwrap(),
            x["two"].as_bool().unwrap(),
            x["k"].as_i64().unwrap(),
        );
        t.ok(i, same(&s(x, "full"), &js(&full)), || "the source".into());
        let edit = |v: String| v.replace("float4 acc = u;", "float4 acc = u + 1.0;");
        let pv = match s(x, "pulled").as_str() {
            "none" => None,
            k => ibuilder_shader::finish::vin::pull_variant(&full)
                .ok()
                .map(|(v, _)| if k == "edited" { edit(v) } else { v }),
        };
        let vv = match s(x, "shadowed").as_str() {
            "none" => None,
            "full" => Some(full.clone()),
            k => ibuilder_shader::finish::vin::vin_variant(&full)
                .ok()
                .map(|(v, _)| if k == "edited" { edit(v) } else { v }),
        };
        let got = vsunit::vs_unit(&full, pv.as_deref(), vv.as_deref())
            .map(|(u, pn, vn)| dumps(&J::Arr(vec![J::Str(u), strs(&pn), strs(&vn)])));
        t.check(i, &x["expect"], got);
        fulls.push((
            s(x, "msl"),
            x["tf"].as_bool().unwrap(),
            x["two"].as_bool().unwrap(),
            x["k"].as_i64().unwrap(),
        ));
    }
    let mut bad = t.done();
    let mut t = Tally::new("vs_entries");
    for (i, x) in c["vsentries"].as_array().into_iter().flatten().enumerate() {
        let (src, a, b, k) = &fulls[x["case"].as_u64().unwrap() as usize];
        let m = unit_full(src, *a, *b, *k) + &s(x, "tail");
        let m = &m[..x["len"].as_u64().unwrap() as usize];
        let got = vsunit::vs_entries(m).map(|es| {
            let mut j = J::obj();
            for (n, e) in es {
                j.set(
                    &n,
                    J::Arr(vec![
                        J::Str(e.ret),
                        J::Str(e.params),
                        J::Int(e.start as i64),
                        J::Int(e.body as i64),
                        J::Int(e.end as i64),
                    ]),
                );
            }
            dumps(&j)
        });
        t.check(i, &x["expect"], got);
    }
    bad += t.done();
    assert_eq!(bad, 0);
}
