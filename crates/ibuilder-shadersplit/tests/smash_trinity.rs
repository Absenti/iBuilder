//! Super Smash Bros. Ultimate's data.arc and Game Freak's Trinity packs on damaged input: the containers of the synthetic games tests/data/games/sync-smash and sync-trinity
//! (tools/gen_smash_trinity.py, no game data), damaged as each vector of tests/data/vectors-smash-trinity.jsonl.gz says (tools/gen_smash_trinity_vectors.py) and split end to end.
//! The split must end where the engine's own split ended (with its text when that is a `[RED]` line of the engine), or write the same bytes and leave out as many files.
//! The one difference on purpose: a Trinity file the damage turned into Oodle (compression 3) is the engine's red line for its range (no Mac host), and this port's refusal.

use flate2::read::GzDecoder;
use ibuilder_shadersplit::source::{FileSource, MemSource};
use ibuilder_shadersplit::trinity::{View, packs_of};
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitError, SplitOptions, Zstd, split};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const GAME: &str = "0100TESTTEST0000";

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn games() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/games")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// gen_smash_trinity_vectors.py `rawframe`: one zstd frame of raw blocks
fn rawframe(t: &[u8]) -> Vec<u8> {
    let mut out = vec![0x28, 0xb5, 0x2f, 0xfd, 0xa0];
    out.extend_from_slice(&(t.len() as u32).to_le_bytes());
    let blocks: Vec<&[u8]> = if t.is_empty() {
        vec![&[]]
    } else {
        t.chunks(0x20000).collect()
    };
    for (i, b) in blocks.iter().enumerate() {
        let h = ((b.len() as u32) << 3) | u32::from(i == blocks.len() - 1);
        out.extend_from_slice(&h.to_le_bytes()[..3]);
        out.extend_from_slice(b);
    }
    out
}

fn u32at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// gen_smash_trinity_vectors.py `apply`
fn apply(base: &[u8], v: &Value) -> Vec<u8> {
    let mut d = base.to_vec();
    let num = |k: &str| v[k].as_u64().unwrap() as usize;
    match v["t"].as_str().unwrap() {
        "flip" => {
            d[num("at")] ^= num("x") as u8;
            d
        }
        "cut" => {
            d.truncate(num("n"));
            d
        }
        t => {
            let fs_off = u64::from_le_bytes(d[0x20..0x28].try_into().unwrap()) as usize;
            let (mut size, stored) = (u32at(&d, fs_off + 4), u32at(&d, fs_off + 8) as usize);
            let mut tab = Ruzstd
                .decompress(&d[fs_off + 16..fs_off + 16 + stored], None, None)
                .unwrap();
            if t == "tflip" {
                tab[num("at")] ^= num("x") as u8;
            } else {
                tab.truncate(num("n"));
                if v["fix"].as_bool().unwrap() {
                    size = tab.len() as u32;
                }
            }
            let f = rawframe(&tab);
            let mut out = d[..fs_off].to_vec();
            for w in [0x10, size, f.len() as u32, 16 + f.len() as u32] {
                out.extend_from_slice(&w.to_le_bytes());
            }
            out.extend_from_slice(&f);
            out
        }
    }
}

/// a copy of `game` with `rel` replaced, split with `jobs` threads: (the result, the log, the output folder, the copy's RomFS)
fn split_copy(
    game: &str,
    rel: &str,
    bytes: &[u8],
    tag: &str,
    jobs: usize,
) -> (
    Result<ibuilder_shadersplit::SplitStats, SplitError>,
    Vec<String>,
    PathBuf,
    PathBuf,
) {
    let g = std::env::temp_dir().join(format!(
        "ibuilder-smash-trinity-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&g);
    let src = games().join(game);
    copy_dir(&src.join("RomFS"), &g.join("RomFS"));
    copy_dir(&src.join("profile"), &g.join("profile"));
    std::fs::copy(src.join("main"), g.join("main")).unwrap();
    std::fs::write(g.join("RomFS").join(rel), bytes).unwrap();
    let (romfs, main, prof) = (g.join("RomFS"), g.join("main"), g.join("profile"));
    let (pkg, tables) = (g.join("out/pkg"), g.join("out/tables"));
    let mut o = SplitOptions::single(&romfs, &main, &pkg, &tables, &prof, GAME);
    o.jobs = jobs;
    let log = Mutex::new(Vec::new());
    let r = split(&o, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    (r, log.into_inner().unwrap(), g.join("out"), romfs)
}

fn outputs(out: &Path) -> Vec<(String, String)> {
    let mut got = Vec::new();
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
    got
}

fn unreadable(log: &[String]) -> u64 {
    log.iter()
        .find(|l| l.starts_with("[RED] ") && l.contains(" file(s) could not be read"))
        .and_then(|l| l.split_whitespace().nth(1))
        .map(|n| n.parse().unwrap())
        .unwrap_or(0)
}

/// One vector: Ok(0) the same bytes, Ok(1) both ended, Ok(2) both ended with the engine's text, Ok(3) refused as Oodle; Err the difference
fn one(i: usize, line: &str) -> Result<usize, String> {
    let r: Value = serde_json::from_str(line).unwrap();
    let (v, py) = (&r["v"], &r["py"]);
    let (game, rel) = match v["c"].as_str().unwrap() {
        "arc" => ("sync-smash", "data.arc"),
        "trpfs" => ("sync-trinity", "arc/data.trpfs"),
        _ => ("sync-trinity", "arc/data.trpfd"),
    };
    let base = std::fs::read(games().join(game).join("RomFS").join(rel)).unwrap();
    let d = apply(&base, v);
    let (res, log, out, romfs) = split_copy(game, rel, &d, &i.to_string(), 1);
    let romfs = romfs.display().to_string();
    let got = match (&res, py["rc"].as_i64().unwrap()) {
        (Err(e), 0) => {
            if py["oodle"].as_bool() == Some(true)
                && matches!(e, SplitError::Unsupported(m) if m.starts_with("Oodle Mermaid (Trinity pack"))
            {
                Ok(3)
            } else {
                Err(format!(
                    "this port ended with {e}, the engine's split did not"
                ))
            }
        }
        (Ok(_), 0) => {
            let want: Vec<(String, String)> = py["files"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect();
            if want != outputs(&out) {
                Err("other bytes".to_string())
            } else if unreadable(&log) != py["unreadable"].as_u64().unwrap() {
                Err(format!(
                    "{} files unreadable, the engine {} ({log:?})",
                    unreadable(&log),
                    py["unreadable"]
                ))
            } else {
                Ok(0)
            }
        }
        (Ok(s), _) => Err(format!(
            "this port split it ({} shaders), the engine ended with {}",
            s.shaders, py["err"]
        )),
        (Err(e), _) => {
            let want = py["err"].as_str().unwrap();
            let got = format!("[RED] {e}").replace(&romfs, "@ROMFS@");
            if !want.starts_with("[RED] ") {
                Ok(1) // an exception of the engine's reader (its traceback), this port's own text
            } else if want == got {
                Ok(2)
            } else {
                Err(format!("ended with {got:?}, the engine with {want:?}"))
            }
        }
    };
    let _ = std::fs::remove_dir_all(out.parent().unwrap());
    got.map_err(|w| format!("vector {i} {v}: {w}"))
}

#[test]
fn damaged_containers_split_as_the_engine_splits_them() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/vectors-smash-trinity.jsonl.gz");
    let Ok(f) = std::fs::File::open(&p) else {
        ibuilder_testkit::skip(
            "tests/data/vectors-smash-trinity.jsonl.gz is not there (run tools/gen_smash_trinity_vectors.py)",
        );
        return;
    };
    let lines: Vec<String> = std::io::BufReader::new(GzDecoder::new(f))
        .lines()
        .map(|l| l.unwrap())
        .collect();
    // 8 threads, each splitting its vectors with 1 job: (same, ended, ended with the engine's text, refused as Oodle), the differences
    let parts: Vec<([u32; 4], Vec<String>)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..8)
            .map(|t| {
                let lines = &lines;
                sc.spawn(move || {
                    let (mut n, mut bad) = ([0u32; 4], Vec::new());
                    for (i, line) in lines.iter().enumerate().skip(t).step_by(8) {
                        match one(i, line) {
                            Ok(k) => n[k] += 1,
                            Err(w) => bad.push(w),
                        }
                    }
                    (n, bad)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let (mut n, mut bad) = ([0u32; 4], Vec::new());
    for (k, b) in parts {
        for j in 0..4 {
            n[j] += k[j];
        }
        bad.extend(b);
    }
    bad.sort();
    let [same, failed, red_text, oodle] = n;
    eprintln!(
        "{same} same, {failed} ended ({red_text} more with the engine's text), {oodle} refused as Oodle"
    );
    assert!(
        bad.is_empty(),
        "{} vectors differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(
        same > 500 && failed + red_text > 100 && red_text > 10,
        "{same} same, {failed} failed ({red_text} with the engine's text), {oodle} refused"
    );
}

/// A Trinity file of compression 3 (Oodle) ends the split as an unsupported container: the game's decoder is not here
#[test]
fn an_oodle_file_in_a_trinity_pack_is_refused() {
    let dir = games().join("sync-trinity/RomFS/arc");
    let (fs, fd) = (dir.join("data.trpfs"), dir.join("data.trpfd"));
    let (src, desc) = (FileSource::open(&fs).unwrap(), std::fs::read(&fd).unwrap());
    let packs = packs_of("arc/data.trpfs", &MemSource(&desc), &src).unwrap();
    let v = View::new(&src);
    let pr = v.root(packs[0].0 as i128).unwrap();
    let t = v.refs(pr, 1).unwrap()[0];
    let c = v.field(t, 1).unwrap().unwrap() as usize;
    let mut d = std::fs::read(&fs).unwrap();
    assert_eq!(d[c], 0);
    d[c] = 3;
    for jobs in [1, 4] {
        let (r, _, out, _) = split_copy(
            "sync-trinity",
            "arc/data.trpfs",
            &d,
            &format!("oodle{jobs}"),
            jobs,
        );
        match r {
            Err(SplitError::Unsupported(m)) => assert!(
                m.starts_with(
                    "Oodle Mermaid (Trinity pack /arc/data.trpfs#arc/pack_a.trpak: file 0,"
                ),
                "{m}"
            ),
            r => panic!("{:?}", r.map(|s| s.shaders)),
        }
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }
}

/// The record names both families for the files they take
#[test]
fn the_record_names_the_two_families() {
    use ibuilder_shadersplit::record::classify_source;
    for (game, rel, fam) in [
        ("sync-smash", "data.arc", "smash-arc"),
        ("sync-trinity", "arc/data.trpfs", "trinity"),
        ("sync-trinity", "arc/data.trpfd", "trinity"),
    ] {
        let d = std::fs::read(games().join(game).join("RomFS").join(rel)).unwrap();
        assert_eq!(classify_source(rel, &MemSource(&d)), Ok(vec![fam]), "{rel}");
    }
    let rec = ibuilder_shadersplit::record::revisions();
    assert!(
        rec.contains("smash-arc=1") && rec.contains("trinity=1"),
        "{rec}"
    );
}
