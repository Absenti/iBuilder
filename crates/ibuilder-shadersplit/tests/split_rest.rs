//! The readers ported from nxnative 033793f0 (SHARC, RAGE WAD, REDengine RDHS / FZLC, IPK v5, MT Framework ARC v9 / headed SPK, Telltale .ttarch2, agl BAHS, NXCX, PACx 403, VBF) and the
//! raw LZMA1 decoder on damaged input: the containers of tests/data/games/rest-* (tools/gen_split_rest.py) damaged in ~2,800 ways and ~1,300 LZMA streams, against what the engine's own
//! shader_split.py / vbf.py / lzma answered for each (tests/data/vectors-split-rest.jsonl.gz, tools/gen_split_rest_vectors.py). No game data.

use ibuilder_shadersplit::Ruzstd;
use ibuilder_shadersplit::scan_types::{Ctx, Shader};
use ibuilder_shadersplit::source::MemSource;
use ibuilder_shadersplit::{archive, lzma1, nvn, nxcx, rage_wad, redengine, sharc, telltale, vbf};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn games() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games")
}

fn h16(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<String>()[..16]
        .to_string()
}

fn shaders(v: &[Shader]) -> Value {
    Value::Array(
        v.iter()
            .map(|s| {
                json!([
                    s.label,
                    s.st.short(),
                    h16(&[s.ctl.clone(), s.code.clone()].concat())
                ])
            })
            .collect(),
    )
}

/// The decoders' own words are not the port's to copy (as tests/sync5.rs): zlib's codes, lz4's error codes, libzstd's messages
fn norm(s: &str) -> String {
    let mut s = s
        .replace("invalid literal/length/distance code", "invalid code")
        .replace("invalid literal/length code", "invalid code")
        .replace("invalid distance code", "invalid code");
    for m in [
        "Decompression failed: corrupt input or insufficient space in destination buffer",
        "invalid zstd frame (",
        "LZ4BlockError: ",
        "ZstdError: ",
    ] {
        if let Some(i) = s.find(m) {
            s.replace_range(i + m.len().., "<decoder>");
        }
    }
    s
}

fn norm_v(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(norm(s)),
        Value::Array(a) => Value::Array(a.iter().map(norm_v).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm_v(v))).collect()),
        v => v.clone(),
    }
}

fn res<T>(r: Result<T, ibuilder_shadersplit::Failure>, f: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!({"ok": f(v)}),
        Err(e) => json!({"err": e.to_string()}),
    }
}

fn damaged(base: &[u8], r: &Value) -> Vec<u8> {
    let mut b = base.to_vec();
    for m in r["muts"].as_array().unwrap() {
        b[m[0].as_u64().unwrap() as usize] = m[1].as_u64().unwrap() as u8;
    }
    if let Some(c) = r["cut"].as_u64() {
        b.truncate(c as usize);
    }
    b
}

const FILES: &[(&str, &[&str])] = &[
    (
        "sharc",
        &["rest-sharc/RomFS/Data/shared/shader_switch.sharc"],
    ),
    ("wad", &["rest-wad/RomFS/Data/a/out.wad.nx"]),
    (
        "rdhs",
        &["rest-redengine/RomFS/content/content0/shadernx.cache"],
    ),
    (
        "fzlc",
        &["rest-redengine/RomFS/content/content0/staticshadernx.cache"],
    ),
    (
        "ipk",
        &[
            "rest-ipk/RomFS/Data/bundle_nx.ipk",
            "rest-ipk/RomFS/Data/legacy.ipk",
        ],
    ),
    (
        "mtarc",
        &[
            "rest-mtarc/RomFS/Data/re/root.arc",
            "rest-mtarc/RomFS/Data/sf/root.arc",
        ],
    ),
    (
        "tt",
        &[
            "rest-telltale/RomFS/Data/SM3_nx_Shaders.ttarch2",
            "rest-telltale/RomFS/Data/Patch.ttarch2",
        ],
    ),
    (
        "bahs",
        &[
            "rest-bahs/RomFS/Data/payload.sharcb",
            "rest-bahs/RomFS/Data/nvn1.sharcb",
        ],
    ),
    ("nxcx", &["rest-nxcx/RomFS/Data/w_allpac.mrg"]),
    (
        "pacx",
        &["rest-pacx/RomFS/Data/d.pac", "rest-pacx/RomFS/Data/c.pac"],
    ),
    ("vbf", &["rest-vbf-zip-play/RomFS/Data/ffx.vbf"]),
];

fn outcome(kind: &str, d: &[u8], cx: &Ctx<'_>) -> Value {
    let src = MemSource(d);
    match kind {
        "sharc" => res(sharc::sharc_shaders(d, "/x.sharc"), |v| shaders(&v)),
        "wad" => res(rage_wad::noire_wad_shaders(&src, "/out.wad.nx"), |v| {
            shaders(&v)
        }),
        "rdhs" if d.len() < 16 => json!({"skip": true}),
        "rdhs" => res(redengine::rdhs_shaders(cx, &src, "/c"), |v| shaders(&v)),
        "fzlc" => res(redengine::fzlc_shaders(cx, &src, "/c"), |v| shaders(&v)),
        "ipk" => {
            let mut out = Vec::new();
            let r = archive::ipk_shaders(cx, &src, "x.ipk", &mut out);
            res(r, |_| shaders(&out))
        }
        "mtarc" => {
            let mut out = Vec::new();
            let r = archive::mtarc_shaders(d, "/x.arc", &mut out);
            res(r, |_| shaders(&out))
        }
        // the split asks the source first whether to read the archive at all; the answer must be the Python's either way
        "tt" => match telltale::ttarch2_named(&src) {
            Err(e) => json!({"err": e.to_string()}),
            Ok(false) => json!({"ok": []}),
            Ok(true) => res(telltale::ttarch2_shaders(d, "/x.ttarch2"), |v| shaders(&v)),
        },
        "bahs" => {
            cx.bahs_var.borrow_mut().clear();
            let mut r = res(nvn::bahs_shaders(cx, d, "/x.sharcb"), |v| shaders(&v));
            let mut var: Vec<(String, String)> = cx.bahs_var.borrow().clone().into_iter().collect();
            var.sort();
            r["var"] = json!(
                var.into_iter()
                    .map(|(k, v)| json!([k, v]))
                    .collect::<Vec<_>>()
            );
            r
        }
        "nxcx" => res(nxcx::nxcx_shaders(&src, "/x.mrg"), |v| match v {
            None => Value::Null,
            Some(v) => shaders(&v),
        }),
        "pacx" => res(archive::pacx4_parts(d, "/x.pac"), |v| {
            json!(
                v.iter()
                    .map(|(n, p)| json!([n, h16(p)]))
                    .collect::<Vec<_>>()
            )
        }),
        "vbf" => match vbf::entries(&src) {
            Err(e) => json!({"err": e}),
            Ok(es) => json!({"ok": es.iter().map(|e| {
                let m = match vbf::read(&src, e) { Ok(b) => json!({"ok": h16(&b)}), Err(x) => json!({"err": x}) };
                json!([e.0, e.1, e.2, e.3, m])
            }).collect::<Vec<_>>()}),
        },
        k => panic!("kind {k}"),
    }
}

#[test]
fn damaged_rest_readers_read_as_the_python_reads_them() {
    let raw = std::fs::read(games().join("../vectors-split-rest.jsonl.gz"))
        .expect("tests/data/vectors-split-rest.jsonl.gz (tools/gen_split_rest_vectors.py)");
    let mut s = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut s)
        .unwrap();
    let rules = ibuilder_shadersplit::ctl::KeyRules::parse("0+0").unwrap();
    let files = ibuilder_shadersplit::split::FileIndex::empty();
    let (empty, dicts) = (Default::default(), Default::default());
    let cx = Ctx {
        rules: &rules,
        zstd: &Ruzstd,
        common: None,
        borrowed: &empty,
        zdicts: &dicts,
        log: &|_| {},
        files: &files,
        bahs_var: Default::default(),
    };
    let base: std::collections::HashMap<&str, Vec<Vec<u8>>> = FILES
        .iter()
        .map(|(k, ps)| {
            (
                *k,
                ps.iter()
                    .map(|p| std::fs::read(games().join(p)).unwrap())
                    .collect(),
            )
        })
        .collect();
    let (mut n, mut bad) = (0usize, Vec::new());
    for line in s.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let kind = r["kind"].as_str().unwrap();
        let got = if kind == "lzma" {
            let d = ibuilder_shadersplit::bytes::unhex(r["data"].as_str().unwrap()).unwrap();
            let u = |k: &str| r[k].as_u64().unwrap();
            match lzma1::decompress_raw(
                &d,
                u("lc") as u32,
                u("lp") as u32,
                u("pb") as u32,
                u("ds") as u32,
                u("max") as usize,
            ) {
                Ok(x) => json!({"ok": [x.out.len(), h16(&x.out), x.eof]}),
                Err(e) => json!({"err": e}),
            }
        } else {
            let b = &base[kind][r["file"].as_u64().unwrap() as usize];
            outcome(kind, &damaged(b, &r), &cx)
        };
        n += 1;
        if norm_v(&got) != norm_v(&r["want"]) {
            bad.push(format!(
                "{kind} {} {}:\n  python {}\n  rust   {}",
                r["muts"], r["cut"], r["want"], got
            ));
        }
    }
    assert!(n > 4000, "{n} vectors");
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// The NXCX search reads 16 MiB windows that overlap by the longest magic less one byte: a GLSLC output that starts 2 bytes before a window's end is found once, as the Python finds it
#[test]
fn nxcx_output_across_the_search_window() {
    let g = std::fs::read(games().join("rest-nxcx/RomFS/Data/w_allpac.mrg")).unwrap();
    let at = g
        .windows(4)
        .position(|w| w == [0x91, 0x68, 0x86, 0x19])
        .unwrap();
    let mut d = b"NXCX".to_vec();
    d.resize((16 << 20) - 2, 0);
    d.extend_from_slice(&g[at..]);
    d[100..104].copy_from_slice(b"BAHS");
    let found = nxcx::nxcx_shaders(&MemSource(&d), "/w.mrg")
        .unwrap()
        .unwrap();
    assert!(!found.is_empty());
    let labels: Vec<&str> = found.iter().map(|s| s.label.as_str()).collect();
    assert!(
        labels[0].ends_with(&format!("#GLSLC@{:#x}", (16 << 20) - 2))
            || labels[0].contains(&format!("@{:#x}", (16 << 20) - 2)),
        "{labels:?}"
    );
}
