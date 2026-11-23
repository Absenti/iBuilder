//! TT Games' CC4 DAT archives (`ttdat.rs`) and Frostbite's superbundles (`frostbite.rs`): the containers of tests/data/games/sync-ttdat and sync-frostbite damaged in 549 ways, against what
//! the engine's `ttdat.py` / `frostbite.py` / `shader_split.py` answered for each (tests/data/vectors-ttdat-frostbite.jsonl.gz, tools/gen_ttdat_frostbite_vectors.py); main's new loose
//! control section rules (`adjacent_shader`, `referenced_shader_copy`, upstream's test_split_adjacent.py cases); and the refusals the Python has no counterpart for (Oodle members and
//! blocks, which the Python decodes with the game's own decoder in the Mac host). No game data.

use ibuilder_shadersplit::ctl::{St, check, glslc_stage};
use ibuilder_shadersplit::error::Failure;
use ibuilder_shadersplit::frostbite::{self, COMPILED_BYTECODE, FbRoot};
use ibuilder_shadersplit::glslc::{adjacent_shader, raw_shaders, referenced_shader_copy};
use ibuilder_shadersplit::image::walk_root;
use ibuilder_shadersplit::scan::FileRef;
use ibuilder_shadersplit::source::MemSource;
use ibuilder_shadersplit::ttdat;
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitError, SplitOptions, split};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn sha16(b: &[u8]) -> String {
    let h = Sha256::digest(b);
    h.iter().map(|x| format!("{x:02x}")).collect::<String>()[..16].to_string()
}

fn text(e: &Failure) -> String {
    match e {
        Failure::Unsupported(_) => "OODLE".into(),
        e => e.to_string(),
    }
}

fn tt_outcome(b: &[u8]) -> Value {
    let src = MemSource(b);
    let es = match ttdat::entries("P", &src) {
        Err(e) => return json!({"entries": {"err": e}}),
        Ok(None) => return json!({"entries": null}),
        Ok(Some(es)) => es,
    };
    let mut members = Vec::new();
    for e in &es {
        let d = match ttdat::read(&src, e) {
            Ok(d) => d,
            Err(x) => {
                members.push(json!({"err": text(&x)}));
                continue;
            }
        };
        members.push(match ttdat::programs(&d, &format!("/x/{}", e.0)) {
            Ok(found) => json!({"read": sha16(&d), "ok": found
                .iter()
                .map(|s| json!([s.label, s.st.short(), sha16(&[s.ctl.clone(), s.code.clone()].concat())]))
                .collect::<Vec<_>>()}),
            Err(x) => json!({"read": sha16(&d), "err": text(&x)}),
        });
    }
    json!({"entries": {"ok": es.iter().map(|e| json!([e.0, e.1, e.2, e.3])).collect::<Vec<_>>()}, "members": members})
}

/// `os.path.relpath` of a path below the root
fn norm(rel: &str) -> String {
    let mut v: Vec<&str> = Vec::new();
    for c in rel.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if v.pop().is_none() {
                    v.push("..");
                }
            }
            c => v.push(c),
        }
    }
    v.join("/")
}

fn fb_outcome(root: &Path) -> Value {
    let files: Vec<FileRef> = walk_root(root)
        .unwrap()
        .into_iter()
        .map(|e| FileRef {
            root: 0,
            rel: e.rel,
            loc: e.loc,
        })
        .collect();
    let r = FbRoot::new(&files, 0, String::new());
    let Ok(res) = r.resources(COMPILED_BYTECODE, &|f| f.rel.clone()) else {
        return json!({"resources": "err"});
    };
    let mut list: Vec<Value> = res
        .iter()
        .map(|((c, o, s), (z, n))| json!([norm(c), o, s, z, n]))
        .collect();
    list.sort_by_key(|v| v.to_string());
    let mut items = Vec::new();
    for ((cas, off, stored), (size, _)) in &res {
        let Some(f) = files.iter().find(|f| f.rel == norm(cas)) else {
            items.push(json!({"err": "FileNotFoundError"}));
            continue;
        };
        let src = f.loc.open().unwrap();
        let b = match frostbite::read(&src, *off, *stored, *size) {
            Ok(b) => b,
            Err(x) => {
                items.push(json!({"err": text(&x)}));
                continue;
            }
        };
        let (code, ctl) = match frostbite::nvn_shader(&b) {
            Ok(Some(s)) => s,
            Ok(None) => {
                items.push(json!({"red": "no VNVN record"}));
                continue;
            }
            Err(x) => {
                items.push(json!({"err": text(&x)}));
                continue;
            }
        };
        let st = glslc_stage(ctl, code).unwrap();
        let why = match st {
            Some(st) => check(st, ctl, code).unwrap(),
            None => Some("no stage in the code area header"),
        };
        items.push(match why {
            Some(w) => json!({"red": w}),
            None => json!({"ok": [st.unwrap().short(), sha16(&[ctl, code].concat())]}),
        });
    }
    json!({"resources": list, "items": items})
}

/// the Python's list sorted as the port sorts it (both sort the same rows; JSON text order only for the comparison)
fn sorted_resources(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(a) = v.get_mut("resources").and_then(|r| r.as_array_mut()) {
        a.sort_by_key(|x| x.to_string());
    }
    v
}

/// zlib's fast path words a bad code otherwise than its slow path, which `zlibx` models (as tests/uepak.rs)
fn norm_text(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(
            s.replace("invalid literal/length/distance code", "invalid code")
                .replace("invalid literal/length code", "invalid code")
                .replace("invalid distance code", "invalid code"),
        ),
        Value::Array(a) => Value::Array(a.iter().map(norm_text).collect()),
        Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), norm_text(v))).collect())
        }
        v => v.clone(),
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

#[test]
fn damaged_containers_read_as_the_python_reads_them() {
    let p = data().join("vectors-ttdat-frostbite.jsonl.gz");
    let raw = std::fs::read(&p).expect(
        "tests/data/vectors-ttdat-frostbite.jsonl.gz (tools/gen_ttdat_frostbite_vectors.py)",
    );
    let mut s = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut s)
        .unwrap();
    let work = std::env::temp_dir().join(format!("ibuilder-fb-vectors-{}", std::process::id()));
    let fb_src = data().join("games/sync-frostbite/RomFS");
    let (mut n, mut bad, mut kinds) = (0, Vec::new(), [0usize; 2]);
    for line in s.lines() {
        let r: Value = serde_json::from_str(line).unwrap();
        let base = r["base"].as_str().unwrap();
        let fb = r["kind"] == "fb";
        let orig = if fb {
            fb_src.join(base)
        } else {
            data().join("games").join(base)
        };
        let mut b = std::fs::read(&orig).unwrap();
        for m in r["muts"].as_array().unwrap() {
            b[m[0].as_u64().unwrap() as usize] = m[1].as_u64().unwrap() as u8;
        }
        if let Some(c) = r["cut"].as_u64() {
            b.truncate(c as usize);
        }
        let got = if fb {
            let _ = std::fs::remove_dir_all(&work);
            copy_dir(&fb_src, &work);
            std::fs::write(work.join(base), &b).unwrap();
            kinds[1] += 1;
            fb_outcome(&work)
        } else {
            kinds[0] += 1;
            tt_outcome(&b)
        };
        n += 1;
        let want = sorted_resources(&r["out"]);
        if norm_text(&got) != norm_text(&want) {
            bad.push(format!(
                "{base} {:?} cut {:?}:\n  python {want}\n  rust   {got}",
                r["muts"], r["cut"]
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    assert!(
        n > 500 && kinds[0] > 200 && kinds[1] > 250,
        "{n} vectors {kinds:?}"
    );
    assert!(
        bad.is_empty(),
        "{} of {n} differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

#[test]
fn every_prefix_and_byte_of_a_dat_index_reads_without_a_panic() {
    let b = std::fs::read(data().join("games/sync-ttdat/RomFS/Data/GAME.DAT")).unwrap();
    for cut in (0..b.len()).step_by(5) {
        let _ = tt_outcome(&b[..cut]);
    }
    let off = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
    for o in (0..8).chain(off..b.len()) {
        for v in [0u8, 0x7f, 0xff] {
            let mut d = b.clone();
            d[o] = v;
            let _ = tt_outcome(&d);
        }
    }
}

// ---- Oodle: refused ------------------------------------------------------------------------------------------------------------------

fn split_copy(
    game: &str,
    tag: &str,
    patch: &dyn Fn(&Path),
) -> (Result<(), SplitError>, Vec<String>) {
    let g = data().join("games").join(game);
    let out =
        std::env::temp_dir().join(format!("ibuilder-ttfb-{game}-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    copy_dir(&g.join("RomFS"), &out.join("RomFS"));
    patch(&out.join("RomFS"));
    let (romfs, main, prof) = (out.join("RomFS"), g.join("main"), g.join("profile"));
    let (pkg, tables) = (out.join("pkg"), out.join("tables"));
    let mut o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, "0100TESTTEST0000");
    o.jobs = 2;
    let log = std::sync::Mutex::new(Vec::new());
    let r = split(&o, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    let _ = std::fs::remove_dir_all(&out);
    (r.map(|_| ()), log.into_inner().unwrap())
}

#[test]
fn an_oodle_member_or_block_ends_the_split_with_its_name() {
    // a TT member of OODL chunks whose packed length is not its raw length
    let mut payload = b"OODL".to_vec();
    payload.extend_from_slice(&3u32.to_le_bytes());
    payload.extend_from_slice(&10u32.to_le_bytes());
    payload.extend_from_slice(b"cmp");
    let e = (
        "shaders/o.nnx_shaders".to_string(),
        0,
        payload.len() as u64,
        10,
    );
    match ttdat::read(&MemSource(&payload), &e) {
        Err(Failure::Unsupported(m)) => {
            assert!(m.contains("Oodle") && m.contains("o.nnx_shaders"), "{m}")
        }
        r => panic!("{r:?}"),
    }
    // a stored OODL chunk (packed == raw) needs no decoder
    let mut stored = b"OODL".to_vec();
    stored.extend_from_slice(&4u32.to_le_bytes());
    stored.extend_from_slice(&4u32.to_le_bytes());
    stored.extend_from_slice(b"last");
    let e = ("s.nnx_shaders".to_string(), 0, stored.len() as u64, 4);
    assert_eq!(ttdat::read(&MemSource(&stored), &e).unwrap(), b"last");
    // a Frostbite item whose block is codec 0x19 (Oodle Leviathan): the whole split ends, as `unsupported container`
    let cas = "Data/NX/superbundlelayout/installpackage_00/cas_01.cas";
    let (r, _) = split_copy("sync-frostbite", "oodle", &|root| {
        let p = root.join(cas);
        let mut d = std::fs::read(&p).unwrap();
        // the first block of the file (after 0x30 filler bytes): the stored copy shaders/bytecode/0001 and 0001b name
        let at = 0x30;
        assert_eq!(d[at + 5] & 0xf0, 0x70);
        d[at + 4] = 0x19;
        std::fs::write(p, d).unwrap();
    });
    match r {
        Err(SplitError::Unsupported(m)) => assert!(m.contains("Oodle Leviathan"), "{m}"),
        r => panic!("{r:?}"),
    }
}

#[test]
fn a_tt_or_frostbite_game_is_read_not_refused() {
    for g in ["sync-ttdat", "sync-frostbite"] {
        let (r, log) = split_copy(g, "plain", &|_| {});
        r.unwrap_or_else(|e| panic!("{g}: {e}"));
        assert!(
            log.iter().any(|l| l.starts_with("shaders: ")),
            "{g}: {log:?}"
        );
    }
}

// ---- main's loose control sections (upstream's test_split_adjacent.py) ------------------------------------------------------------------

/// a control section (0x880 bytes, `+0x20` its length, `+0x704` the code's) and a headed vertex code area with an SPH
fn vs() -> (Vec<u8>, Vec<u8>) {
    let mut code = vec![0u8; 0x80];
    code[..4].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
    code[0x30..0x34].copy_from_slice(&((1u32 << 10) | 0x41).to_le_bytes());
    for (i, b) in code.iter_mut().enumerate().skip(0x34) {
        *b = (i * 7 + 1) as u8;
    }
    let mut ctl = vec![0u8; 0x880];
    ctl[..4].copy_from_slice(&[0x34, 0x12, 0x76, 0x98]);
    ctl[0x20..0x24].copy_from_slice(&0x880u32.to_le_bytes());
    ctl[0x704..0x708].copy_from_slice(&(code.len() as u32).to_le_bytes());
    (ctl, code)
}

fn compute(exit_at: Option<usize>) -> (Vec<u8>, Vec<u8>) {
    let (mut ctl, _) = vs();
    let mut code = vec![0u8; 0x100];
    ctl[0x704..0x708].copy_from_slice(&0x100u32.to_le_bytes());
    for (i, v) in [64u32, 1, 1].iter().enumerate() {
        ctl[0x748 + 4 * i..0x74c + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    code[..8].copy_from_slice(&1u64.to_le_bytes());
    if let Some(x) = exit_at {
        code[x..x + 8].copy_from_slice(&0xe30000000007000fu64.to_le_bytes());
    }
    (ctl, code)
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn adj(d: &[u8], c: usize) -> Option<(St, Vec<u8>, Vec<u8>)> {
    adjacent_shader(d, c, 0, None)
        .unwrap()
        .map(|(_, st, a, b)| (st, a.to_vec(), b.to_vec()))
}

#[test]
fn adjacent_code_areas_as_upstream_tests_them() {
    let (ctl, code) = vs();
    assert_eq!(
        adj(&cat(&[&ctl, &code]), 0),
        Some((St::Vs, ctl.clone(), code.clone()))
    );
    assert_eq!(
        adj(&cat(&[&code, &ctl]), code.len()),
        Some((St::Vs, ctl.clone(), code.clone()))
    );
    let (cctl, ccode) = compute(Some(8));
    let (_, noexit) = compute(None);
    assert_eq!(adj(&cat(&[&cctl, &ccode]), 0).unwrap().0, St::Cs);
    for (d, at) in [
        (cat(&[&noexit, &cctl, &ccode]), ccode.len()),
        (cat(&[&ccode, &cctl, &noexit]), ccode.len()),
    ] {
        assert_eq!(adj(&d, at).unwrap().2, ccode, "the EXIT breaks the tie");
    }
    assert!(
        adj(&cat(&[&ccode, &cctl, &ccode]), ccode.len()).is_none(),
        "two exits"
    );
    assert!(
        adj(&cat(&[&noexit, &cctl, &noexit]), ccode.len()).is_none(),
        "no exit"
    );
    let (_, sched) = compute(Some(0));
    assert!(
        adj(&cat(&[&sched, &cctl, &noexit]), ccode.len()).is_none(),
        "a scheduling word is not an exit"
    );
    assert!(
        adj(&cat(&[&cctl, &vec![0u8; 0x100]]), 0).is_none(),
        "zero padding"
    );
    let mut meta = ccode.clone();
    meta[32..40].copy_from_slice(&(1u64 << 63).to_le_bytes());
    assert_eq!(
        adj(&cat(&[&ccode, &cctl, &meta]), ccode.len()).unwrap().2,
        ccode
    );
    assert!(adj(&cat(&[&cctl, &ccode[..0xff]]), 0).is_none());
    assert!(adj(&cctl[..0x720], 0).is_none());
    let mut headed = ccode.clone();
    headed[..4].copy_from_slice(&[0x79, 0x56, 0x34, 0x12]);
    let d = cat(&[&vec![0u8; 0x104], &cctl, &headed]);
    assert_eq!(
        adj(&d, 0x104).unwrap().2,
        headed,
        "an unaligned control, then headed compute"
    );
    // a DAT member with no module header
    let found = ttdat::programs(&cat(&[&cctl, &ccode]), "shaders").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].st, &found[0].code), (St::Cs, &ccode));
    // reflection data between the control section and the code; a second code area is ambiguous; the next control bounds the search
    let (ctl, code) = vs();
    let older = ttdat::programs(&cat(&[&ctl, &vec![0u8; 0x5a8], &code]), "older").unwrap();
    assert_eq!(
        older
            .iter()
            .map(|s| (s.st, s.code.clone()))
            .collect::<Vec<_>>(),
        [(St::Vs, code.clone())]
    );
    assert!(ttdat::programs(&cat(&[&ctl, &vec![0u8; 0x5a8], &code, &code]), "ambiguous").is_err());
    assert!(ttdat::programs(&cat(&[&ctl, &vec![0u8; 0x5a8], &ctl, &code]), "next record").is_err());
}

#[test]
fn a_code_copy_built_by_address_is_taken_before_an_adjacent_one() {
    let (ctl, code) = vs();
    let mut control = cat(&[&ctl, b"!!NVvp5.0"]);
    let n = control.len() as u32;
    control[0x14..0x18].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
    control[0x20..0x24].copy_from_slice(&n.to_le_bytes());
    let mut d = vec![0u8; 0x1000 + code.len()];
    d[0x1000..].copy_from_slice(&code);
    let seq = [
        0xb0000001u32,
        0x91000021,
        0x52800002 | (code.len() as u32) << 5,
        0x94000000,
    ];
    for (i, w) in seq.iter().enumerate() {
        d[0x80 + 4 * i..0x84 + 4 * i].copy_from_slice(&w.to_le_bytes());
    }
    let got = referenced_shader_copy(&d, &control, &[0x70]).unwrap();
    assert_eq!(
        got.map(|(st, c)| (st, c.to_vec())),
        Some((St::Vs, code.clone()))
    );
    d[0x8c..0x90].copy_from_slice(&0xaa0503e1u32.to_le_bytes()); // MOV x1, x5 before the call
    d[0x90..0x94].copy_from_slice(&0x94000000u32.to_le_bytes());
    assert!(
        referenced_shader_copy(&d, &control, &[0x70])
            .unwrap()
            .is_none()
    );
    // in main: the copy wins over the code area after the control section; a call between the control's use and the copy leaves the adjacent one
    let mut other = code.clone();
    *other.last_mut().unwrap() ^= 1;
    let mut image = cat(&[&vec![0u8; 0x2000], &control, &other]);
    image[0x1000..0x1000 + code.len()].copy_from_slice(&code);
    for (i, w) in [0xd0000008u32, 0x91000108].iter().enumerate() {
        image[0x70 + 4 * i..0x74 + 4 * i].copy_from_slice(&w.to_le_bytes());
    }
    for (i, w) in seq.iter().enumerate() {
        image[0x80 + 4 * i..0x84 + 4 * i].copy_from_slice(&w.to_le_bytes());
    }
    let prof = ibuilder_shadersplit::game::Profile::from_game(
        ibuilder_shadersplit::game::Game::load(
            &data().join("games/sync-ttdat/profile"),
            "0100TESTTEST0000",
        )
        .unwrap(),
    )
    .unwrap();
    let files = ibuilder_shadersplit::split::FileIndex::empty();
    let (empty, dicts) = (Default::default(), Default::default());
    let cx = ibuilder_shadersplit::scan_types::Ctx {
        rules: &prof.rules,
        zstd: &Ruzstd,
        common: None,
        borrowed: &empty,
        zdicts: &dicts,
        log: &|_| {},
        files: &files,
        bahs_var: Default::default(),
    };
    let known = Default::default();
    let codes = |img: &[u8]| -> Vec<Vec<u8>> {
        raw_shaders(&cx, img, "main", &known)
            .unwrap()
            .into_iter()
            .map(|s| s.code)
            .collect()
    };
    assert_eq!(codes(&image), std::slice::from_ref(&code));
    image[0x78..0x7c].copy_from_slice(&0x94000000u32.to_le_bytes());
    assert_eq!(codes(&image), [other]);
    // a compute shader with no relocation in main
    let (cctl, ccode) = compute(Some(8));
    let found = raw_shaders(
        &cx,
        &cat(&[&vec![0u8; 0x2000], &cctl, &ccode]),
        "main",
        &known,
    )
    .unwrap();
    assert_eq!(
        found
            .iter()
            .map(|s| (s.st, s.code.clone()))
            .collect::<Vec<_>>(),
        [(St::Cs, ccode)]
    );
}
