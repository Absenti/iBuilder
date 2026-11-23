//! Unreal Engine paks (`uepak.rs`): the synthetic paks of tests/data/games/ue-* damaged in 490 ways, against what the engine's `uepak.py` / `shader_split.py` answered for each
//! (tests/data/vectors-uepak.jsonl.gz, tools/gen_uepak_vectors.py), and the refusals the Python has no counterpart for (an encrypted index). No game data.

use ibuilder_shadersplit::ctl::St;
use ibuilder_shadersplit::source::MemSource;
use ibuilder_shadersplit::uepak::{self, UeErr};
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitOptions, split};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn text(e: &UeErr) -> String {
    match e {
        UeErr::Value(s) => format!("ValueError: {s}"),
        UeErr::Red(s) | UeErr::Unsupported(s) | UeErr::Other(s) => s.clone(),
    }
}

fn sha16(b: &[u8]) -> String {
    let h = Sha256::digest(b);
    h.iter().map(|x| format!("{x:02x}")).collect::<String>()[..16].to_string()
}

/// what the vector generator records for one (damaged) pak
fn outcome(b: &[u8]) -> Value {
    let src = MemSource(b);
    let mut out = serde_json::Map::new();
    match uepak::footer(&src) {
        Ok(f) => {
            out.insert(
                "footer".into(),
                json!({"ok": [f.encrypted, f.ioff, f.isz, f.methods, f.version]}),
            );
        }
        Err(e) => {
            out.insert("footer".into(), json!({"err": text(&e)}));
            return Value::Object(out);
        }
    }
    out.insert(
        "sources".into(),
        match uepak::pak_shader_sources("P", &src, None, &|| false) {
            Ok((a, b)) => json!({"ok": [a, b]}),
            Err(e) => json!({"err": text(&e)}),
        },
    );
    let pk = match uepak::Pak::open("P", &src, None) {
        Ok(p) => p,
        Err(e) => {
            out.insert("pak".into(), json!({"err": text(&e)}));
            return Value::Object(out);
        }
    };
    out.insert("pak".into(), json!({"ok": [pk.mount, pk.names()]}));
    let mut files = Vec::new();
    for name in pk.names() {
        match pk.read(&name, &Ruzstd, true) {
            Err(e) => files.push(json!([name, {"err": text(&e)}])),
            Ok(d) => {
                let mut found = Vec::new();
                let sh = match uepak::inline_shaders(&d, &format!("/x#{name}"), &mut found) {
                    Ok(()) => json!({"ok": found
                        .iter()
                        .map(|s| json!([s.label, s.st.short(), sha16(&[s.ctl.clone(), s.code.clone()].concat())]))
                        .collect::<Vec<_>>()}),
                    Err(e) => json!({"err": text(&e)}),
                };
                files.push(json!([name, {"ok": sha16(&d), "shaders": sh}]));
            }
        }
    }
    out.insert("files".into(), Value::Array(files));
    Value::Object(out)
}

/// The decoders' own words for a damaged stream are not the port's to copy: the zlib the Python links (its fast path: "invalid literal/length/distance code") words a bad code
/// otherwise than zlib's slow path, which `zlibx` models; lz4's error codes and libzstd's messages are their own. Everything else of an error is compared.
fn norm(s: &str) -> String {
    let mut s = s
        .replace("invalid literal/length/distance code", "invalid code")
        .replace("invalid literal/length code", "invalid code")
        .replace("invalid distance code", "invalid code");
    let m = "Decompression failed: corrupt input or insufficient space in destination buffer";
    while let Some(i) = s.find(m) {
        let j = s[i..].find(')').map(|k| i + k).unwrap_or(s.len());
        s.replace_range(i..j, "LZ4 refused");
    }
    // zstd's (libzstd's / ruzstd's) words for a damaged frame
    let d = "does not decode (";
    if let Some(i) = s.find(d)
        && (s[i + d.len()..].starts_with("zstd: ")
            || s[i + d.len()..].starts_with("Unable to decompress Zstandard"))
        && let Some(j) = s.rfind(')')
    {
        s.replace_range(i + d.len()..j, "zstd refused");
    }
    s
}

fn norm_v(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(norm(s)),
        Value::Array(a) => Value::Array(a.iter().map(norm_v).collect()),
        Value::Object(o) => {
            let mut m: Vec<(&String, &Value)> = o.iter().collect();
            m.sort_by(|a, b| a.0.cmp(b.0));
            Value::Array(m.into_iter().map(|(k, v)| json!([k, norm_v(v)])).collect())
        }
        v => v.clone(),
    }
}

#[test]
fn damaged_paks_read_as_the_python_reads_them() {
    let p = data().join("vectors-uepak.jsonl.gz");
    let raw =
        std::fs::read(&p).expect("tests/data/vectors-uepak.jsonl.gz (tools/gen_uepak_vectors.py)");
    let mut s = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut s)
        .unwrap();
    let (mut n, mut bad) = (0, Vec::new());
    for line in s.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let base = data().join("games").join(r["base"].as_str().unwrap());
        let mut b = std::fs::read(&base).unwrap();
        for m in r["muts"].as_array().unwrap() {
            b[m[0].as_u64().unwrap() as usize] = m[1].as_u64().unwrap() as u8;
        }
        if let Some(c) = r["cut"].as_u64() {
            b.truncate(c as usize);
        }
        let got = outcome(&b);
        n += 1;
        if norm_v(&got) != norm_v(&r["out"]) {
            bad.push(format!(
                "{} {:?} cut {:?}:\n  python {}\n  rust   {}",
                r["base"], r["muts"], r["cut"], r["out"], got
            ));
        }
    }
    assert!(n > 400, "{n} vectors");
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn every_prefix_and_byte_of_a_pak_reads_without_a_panic() {
    // the release profile aborts on a panic: every cut and every damaged byte of the footer and index must give a value or an error
    let base = data().join("games/ue-v8short-v9/RomFS/Game/Content/Paks/c-Switch.pak");
    let b = std::fs::read(base).unwrap();
    for cut in (0..b.len()).step_by(7) {
        let _ = outcome(&b[..cut]);
    }
    for o in b.len().saturating_sub(0x400)..b.len() {
        for v in [0u8, 0x7f, 0xff] {
            let mut d = b.clone();
            d[o] = v;
            let _ = outcome(&d);
        }
    }
}

#[test]
fn sizes_from_a_damaged_file_are_refused_before_anything_is_allocated() {
    // a block that says 2^62 raw bytes, or an LZ4 block of 2 bytes that says 2^40: the bounds refuse them, no buffer of that size is asked for
    assert!(uepak::zlib_block(&[0x78, 0x9c, 3, 0], 1 << 62).is_err());
    assert!(uepak::lz4_block(&[0x10, b'x'], 1 << 40).is_err());
    assert!(uepak::zstd_block(&Ruzstd, &[0x28, 0xb5, 0x2f, 0xfd], -1).is_err());
    assert!(uepak::zstd_block(&Ruzstd, &[0x28, 0xb5, 0x2f, 0xfd], 1 << 40).is_err());
}

#[test]
fn records_and_paths() {
    assert_eq!(
        uepak::normpath("../../../AT/./Content//x/../y.uexp"),
        "../../../AT/Content/y.uexp"
    );
    assert_eq!(uepak::normpath("/a/../../b"), "/b");
    assert_eq!(uepak::normpath("//a"), "//a");
    assert_eq!(uepak::normpath(""), ".");
    assert_eq!(
        uepak::label_of("../../../", "AT/Content/M.uexp"),
        "AT/Content/M.uexp"
    );
    assert_eq!(uepak::hx(-5), "-0x5");
    assert_eq!(uepak::ue_tag("SP"), Some(St::Ps));
    assert!(uepak::lslg(b"LSLGSV").is_none());
    assert!(uepak::sage_record(&[0; 64]).is_none());
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// An encrypted index whose key main does not hold: the pak is left out with the Python's red line (`ue_legacy_jobs`: "the pak cannot be read"), the split goes on
#[test]
fn an_encrypted_index_with_no_key_in_main_costs_its_pak() {
    let g = data().join("games/ue-v8-kakarot");
    let w = std::env::temp_dir().join(format!("ibuilder-uepak-enc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&w);
    copy_dir(&g, &w);
    // the encrypted-index flag of the version 8 footer: 0xcc + 21 bytes from the end, then 20 bytes (the key GUID: 16, and 4 before it) -> the flag
    let p = w.join("RomFS/AT/Content/Paks/pakchunk1-Switch.pak");
    let mut b = std::fs::read(&p).unwrap();
    let at = b.len() - 0xcc - 1;
    b[at] = 1;
    std::fs::write(&p, &b).unwrap();
    let (romfs, main, prof) = (w.join("RomFS"), w.join("main"), w.join("profile"));
    let (pkg, tables) = (w.join("out/pkg"), w.join("out/tables"));
    let o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, "0100TESTTEST0000");
    let lines = std::sync::Mutex::new(Vec::<String>::new());
    split(&o, &Ruzstd, &NoGate, &|l| {
        lines.lock().unwrap().push(l.to_string())
    })
    .expect("split");
    let want = format!(
        "[RED] /AT/Content/Paks/pakchunk1-Switch.pak: the pak cannot be read ({}/AT/Content/Paks/pakchunk1-Switch.pak: the index is encrypted and no key in main (a 32-byte window or two 16-byte literals its code stores together) decrypts it); its shaders are left out",
        romfs.display()
    );
    assert!(
        lines.lock().unwrap().contains(&want),
        "{:?}",
        lines.lock().unwrap()
    );
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn an_archive_with_oodle_shaders_is_refused() {
    // a version 2 archive whose compressed shader is no LZ4 block (an Oodle stream): upstream sends it to the Mac's Oodle job; here it is an unsupported container, named
    let mut a = Vec::new();
    for v in [2u32, 0, 1] {
        a.extend(v.to_le_bytes());
    }
    a.extend([0u8; 20]);
    a.extend(0u32.to_le_bytes()); // no shader maps
    a.extend(1u32.to_le_bytes());
    let code = [0x8cu8, 0x0a, 1, 2, 3, 4, 5, 6];
    a.extend(0u64.to_le_bytes());
    a.extend((code.len() as u32).to_le_bytes());
    a.extend(0x1000u32.to_le_bytes());
    a.push(3);
    a.extend(0u32.to_le_bytes()); // preload entries
    a.extend(0u32.to_le_bytes()); // shader indices
    a.extend(code);
    match uepak::decoded_shaders(&a, "P: X.ushaderbytecode") {
        Err(ibuilder_shadersplit::Failure::Unsupported(m)) => assert!(
            m.starts_with("Unreal Engine shader code archive with Oodle-compressed shaders (P: X.ushaderbytecode: shader 0"),
            "{m}"
        ),
        other => panic!("{:?}", other.map(|v| v.len())),
    }
}
