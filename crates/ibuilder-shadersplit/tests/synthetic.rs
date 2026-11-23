//! The split end to end on a synthetic game (a folder RomFS with a BNSH file, a SARC and junk, and an NSO whose text holds a GLSLC output): no game data needed.

use ibuilder_shadersplit::ctl::*;
use ibuilder_shadersplit::{NoGate, Ruzstd, SplitError, SplitOptions, split};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ibuilder-shadersplit-syn-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lcg(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

/// a control section + code area of stage `st`, as `mk_shader` of the vector generator makes them
fn shader(seed: u64, st: St) -> (Vec<u8>, Vec<u8>) {
    let (n, ncode) = (0x850usize, 0x100usize);
    let mut code = lcg(seed, ncode);
    let mut ctl = lcg(seed + 1, n);
    ctl[..4].copy_from_slice(&CTL_MAGIC);
    ctl[0x20..0x24].copy_from_slice(&(n as u32).to_le_bytes());
    ctl[0x704..0x708].copy_from_slice(&(ncode as u32).to_le_bytes());
    for buf in [&mut ctl[4..], &mut code[..]] {
        let mut i = 0;
        while let Some(p) = buf[i..].windows(4).position(|w| w == CTL_MAGIC) {
            buf[i + p] ^= 0x55;
            i += p + 1;
        }
    }
    if st == St::Cs {
        code[..4].copy_from_slice(&CS_MAGIC);
        ctl[0x748..0x758].copy_from_slice(&[64, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]); // the thread group, then 0 bytes of shared memory
    } else {
        code[..4].copy_from_slice(&GFX_MAGIC);
        let w: u32 = if st == St::Vs { 0x461 } else { 1 | (5 << 10) };
        code[0x30..0x34].copy_from_slice(&w.to_le_bytes());
    }
    (ctl, code)
}

fn bnsh(seed: u64) -> Vec<u8> {
    let mut d = vec![0u8; 0x100];
    d[..8].copy_from_slice(b"BNSH\0\0\0\0");
    d[0x60..0x64].copy_from_slice(b"grsc");
    d[0x7c..0x80].copy_from_slice(&1u32.to_le_bytes());
    d[0x80..0x88].copy_from_slice(&0x100u64.to_le_bytes());
    d.extend_from_slice(&[0u8; 0x40]);
    let prog = d.len();
    d.extend_from_slice(&[0u8; 0x50]);
    d[0x110..0x118].copy_from_slice(&(prog as u64).to_le_bytes());
    for (si, st) in [(0usize, St::Vs), (4, St::Ps)] {
        let (ctl, code) = shader(seed + si as u64, st);
        let sc = d.len();
        d.extend_from_slice(&[0u8; 0x20]);
        let pctl = d.len();
        d.extend_from_slice(&ctl);
        let pcode = d.len();
        d.extend_from_slice(&code);
        d[sc + 8..sc + 16].copy_from_slice(&(pctl as u64).to_le_bytes());
        d[sc + 16..sc + 24].copy_from_slice(&(pcode as u64).to_le_bytes());
        d[sc + 24..sc + 28].copy_from_slice(&(code.len() as u32).to_le_bytes());
        d[sc + 28..sc + 32].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
        d[prog + 8 + 8 * si..prog + 16 + 8 * si].copy_from_slice(&(sc as u64).to_le_bytes());
    }
    let n = d.len() as u32;
    d[0x1c..0x20].copy_from_slice(&n.to_le_bytes());
    d
}

fn sarc(members: &[(&str, &[u8])]) -> Vec<u8> {
    let n = members.len();
    let (mut names, mut offs) = (Vec::new(), Vec::new());
    for (nm, _) in members {
        offs.push(names.len() / 4);
        names.extend_from_slice(nm.as_bytes());
        names.push(0);
        while names.len() % 4 != 0 {
            names.push(0);
        }
    }
    let (mut data, mut ents) = (Vec::new(), Vec::new());
    for (i, (_, m)) in members.iter().enumerate() {
        while data.len() % 4 != 0 {
            data.push(0);
        }
        let s = data.len() as u32;
        data.extend_from_slice(m);
        for v in [
            i as u32,
            0x0100_0000 | offs[i] as u32,
            s,
            s + m.len() as u32,
        ] {
            ents.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut sfat = b"SFAT".to_vec();
    sfat.extend_from_slice(&12u16.to_le_bytes());
    sfat.extend_from_slice(&(n as u16).to_le_bytes());
    sfat.extend_from_slice(&0x65u32.to_le_bytes());
    let mut sfnt = b"SFNT".to_vec();
    sfnt.extend_from_slice(&[8, 0, 0, 0]);
    sfnt.extend_from_slice(&names);
    let mut doff = 0x14 + sfat.len() + ents.len() + sfnt.len();
    doff += (4 - doff % 4) % 4;
    let mut out = b"SARC".to_vec();
    out.extend_from_slice(&0x14u16.to_le_bytes());
    out.extend_from_slice(&0xfeffu16.to_le_bytes());
    out.extend_from_slice(&((doff + data.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(doff as u32).to_le_bytes());
    out.extend_from_slice(&0x100u32.to_le_bytes());
    out.extend_from_slice(&sfat);
    out.extend_from_slice(&ents);
    out.extend_from_slice(&sfnt);
    out.resize(doff, 0);
    out.extend_from_slice(&data);
    out
}

/// a GLSLC output of one stage
fn glslc(seed: u64, st: St) -> Vec<u8> {
    let (ctl, code) = shader(seed, st);
    let hdr = 0x90 + 0x90;
    let total = hdr + ctl.len() + code.len();
    let mut d = vec![0u8; total];
    d[..4].copy_from_slice(&GLSLC_MAGIC);
    d[0x44..0x48].copy_from_slice(&(total as u32).to_le_bytes());
    d[0x4c..0x50].copy_from_slice(&1u32.to_le_bytes());
    d[0x90..0x94].copy_from_slice(&((ctl.len() + code.len()) as u32).to_le_bytes());
    d[0x94..0x98].copy_from_slice(&(hdr as u32).to_le_bytes());
    d[0x90 + 0x34..0x90 + 0x38].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
    d[0x90 + 0x38..0x90 + 0x3c].copy_from_slice(&(code.len() as u32).to_le_bytes());
    d[0x90 + 0x3c..0x90 + 0x40].copy_from_slice(&(ctl.len() as u32).to_le_bytes());
    d[hdr..hdr + ctl.len()].copy_from_slice(&ctl);
    d[hdr + ctl.len()..].copy_from_slice(&code);
    d
}

/// an uncompressed NSO with `text` as its text segment
fn nso(text: &[u8]) -> Vec<u8> {
    let mut h = vec![0u8; 0x100];
    h[..4].copy_from_slice(b"NSO0");
    let end = ((text.len() + 0xfff) & !0xfff) as u32;
    for (i, (fo, mo, sz)) in [
        (0x100u32, 0u32, text.len() as u32),
        (0x100 + text.len() as u32, end, 0),
        (0x100 + text.len() as u32, end, 0),
    ]
    .into_iter()
    .enumerate()
    {
        h[0x10 + i * 0x10..0x14 + i * 0x10].copy_from_slice(&fo.to_le_bytes());
        h[0x14 + i * 0x10..0x18 + i * 0x10].copy_from_slice(&mo.to_le_bytes());
        h[0x18 + i * 0x10..0x1c + i * 0x10].copy_from_slice(&sz.to_le_bytes());
    }
    h.extend_from_slice(text);
    h
}

struct Game {
    dir: PathBuf,
    romfs: PathBuf,
    main: PathBuf,
    prof: PathBuf,
}

fn game(name: &str) -> Game {
    let dir = temp(name);
    let romfs = dir.join("RomFS");
    std::fs::create_dir_all(romfs.join("Data")).unwrap();
    std::fs::create_dir_all(romfs.join("Pack")).unwrap();
    std::fs::write(romfs.join("Data/a.bnsh"), bnsh(100)).unwrap();
    std::fs::write(
        romfs.join("Data/b.sarc"),
        sarc(&[
            ("x.bnsh", &bnsh(200)),
            ("y.bnsh", &bnsh(100)),
            ("z.bin", &lcg(5, 300)),
        ]),
    )
    .unwrap();
    std::fs::write(romfs.join("Pack/junk.bin"), lcg(9, 5000)).unwrap();
    std::fs::write(romfs.join("Pack/c.bnsh"), bnsh(300)).unwrap();
    let main = dir.join("main");
    let mut text = lcg(1, 0x2000);
    text.extend_from_slice(&glslc(400, St::Ps));
    text.extend_from_slice(&lcg(2, 0x100));
    text.extend_from_slice(&glslc(500, St::Cs));
    std::fs::write(&main, nso(&text)).unwrap();
    let prof = dir.join("profile");
    std::fs::create_dir_all(prof.join("0100TESTTEST0000")).unwrap();
    std::fs::write(prof.join("0100TESTTEST0000/game.tsv"), "title_id\t0100TESTTEST0000\nshader_groups\t/Data/=data /Pack/=pack\nshader_note\tsynthetic game\n").unwrap();
    Game {
        dir,
        romfs,
        main,
        prof,
    }
}

fn run(
    g: &Game,
    out: &str,
    jobs: usize,
    memo: Option<&Path>,
) -> (
    Result<ibuilder_shadersplit::SplitStats, SplitError>,
    Vec<String>,
    PathBuf,
) {
    let o = g.dir.join(out);
    let (pkg, tables) = (o.join("pkg"), o.join("tables"));
    let mut opts = SplitOptions::single(
        &g.romfs,
        &g.main,
        &pkg,
        &tables,
        &g.prof,
        "0100TESTTEST0000",
    );
    opts.jobs = jobs;
    opts.memo_dir = memo;
    let log = Mutex::new(Vec::new());
    let r = split(&opts, &Ruzstd, &NoGate, &|l| {
        log.lock().unwrap().push(l.to_string())
    });
    (r, log.into_inner().unwrap(), o)
}

fn read_dir_bytes(d: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

/// the regular files of `d` only (a directory put in the way is not one)
fn read_dir_bytes_files(d: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn a_synthetic_game_is_split_and_the_records_are_what_the_map_says() {
    let g = game("a");
    let (r, log, o) = run(&g, "o1", 3, None);
    let stats = r.unwrap();
    // a.bnsh, x.bnsh, c.bnsh: vs + ps each; y.bnsh = a.bnsh's keys again; main: a ps and a cs
    assert_eq!(stats.shaders, 3 * 2 + 2, "{log:?}");
    assert_eq!(stats.unreadable, 0);
    assert!(log.iter().any(|l| l.starts_with("shaders: 8 (")), "{log:?}");
    let map = std::fs::read_to_string(o.join("tables/shader_map.tsv")).unwrap();
    let lines: Vec<&str> = map.lines().collect();
    assert!(
        lines[0]
            .starts_with("# kind: evidence  synthetic game (BNSH / BAHS / GLSLC in RomFS + main)"),
        "{}",
        lines[0]
    );
    assert_eq!(lines[2], "package\tindex\tstage\tdm_ctl_sha16\tsource");
    assert_eq!(lines.len(), 3 + 8);
    // every row names a package that holds a record of that key, whose control section and code area give the key again
    let rules = KeyRules::default();
    let mut seen = 0;
    for row in &lines[3..] {
        let c: Vec<&str> = row.split('\t').collect();
        let pkg = std::fs::read(o.join("pkg").join(c[0])).unwrap();
        let (mut at, idx) = (0usize, c[1].parse::<usize>().unwrap());
        for _ in 0..idx {
            let (nctl, ncode) = (
                u32::from_le_bytes(pkg[at + 0x28..at + 0x2c].try_into().unwrap()) as usize,
                u32::from_le_bytes(pkg[at + 0x2c..at + 0x30].try_into().unwrap()) as usize,
            );
            at += 0x38 + ncode + nctl;
        }
        assert_eq!(&pkg[at..at + 8], b"BTCDBTCD");
        let (nctl, ncode) = (
            u32::from_le_bytes(pkg[at + 0x28..at + 0x2c].try_into().unwrap()) as usize,
            u32::from_le_bytes(pkg[at + 0x2c..at + 0x30].try_into().unwrap()) as usize,
        );
        let (code, ctl) = (
            &pkg[at + 0x38..at + 0x38 + ncode],
            &pkg[at + 0x38 + ncode..at + 0x38 + ncode + nctl],
        );
        assert_eq!(skey(&rules, ctl, code).unwrap(), c[3]);
        assert_eq!(
            ibuilder_shadersplit::bytes::hex(&pkg[at + 8..at + 16]),
            c[3]
        );
        assert_eq!(
            c[0],
            format!(
                "{}_{}.bin",
                if c[2] == "vertex" {
                    "vs"
                } else if c[2] == "pixel" {
                    "ps"
                } else {
                    "cs"
                },
                if c[4].starts_with("main") {
                    "main"
                } else if c[4].starts_with("/Data/") {
                    "data"
                } else {
                    "pack"
                }
            )
        );
        seen += 1;
    }
    assert_eq!(seen, 8);
    // the BNSH pairs: one vertex / pixel pair per variation, y.bnsh's equal to a.bnsh's
    let pairs = std::fs::read_to_string(o.join("tables/shader_pairs.tsv")).unwrap();
    assert_eq!(pairs.lines().count(), 1 + 3);
    let facts = std::fs::read_to_string(o.join("tables/shader_facts.tsv")).unwrap();
    assert!(
        facts
            .lines()
            .nth(1)
            .unwrap()
            .ends_with("\t{\"local\":[64,1,1],\"shared\":0}"),
        "{facts}"
    );
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn threads_and_a_resumed_memo_give_the_same_bytes() {
    let g = game("b");
    let (r1, _, o1) = run(&g, "t1", 1, None);
    r1.unwrap();
    let want = (
        read_dir_bytes(&o1.join("pkg")),
        read_dir_bytes(&o1.join("tables")),
    );
    let memo = g.dir.join("memo");
    for (i, j) in [8usize, 2, 8].into_iter().enumerate() {
        let (r, _, o) = run(&g, &format!("t{}", i + 2), j, Some(&memo));
        let s = r.unwrap();
        if i == 0 {
            assert_eq!((s.jobs_reused, s.jobs_ran), (0, 1));
        } else {
            assert_eq!((s.jobs_reused, s.jobs_ran), (1, 0));
        }
        assert_eq!(
            want,
            (
                read_dir_bytes(&o.join("pkg")),
                read_dir_bytes(&o.join("tables"))
            )
        );
    }
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn a_container_this_port_does_not_read_is_an_explicit_error() {
    let g = game("c");
    // (CRI CPK archives and Smash's data.arc, the examples before, are read now: cpk.rs, smash_arc.rs) a Next Level Games archive index
    let mut dict = vec![0x58, 0x24, 0xf3, 0xa9];
    dict.extend_from_slice(&lcg(1, 60));
    std::fs::write(g.romfs.join("Data/stage.dict"), dict).unwrap();
    let (r, _, _) = run(&g, "o", 2, None);
    match r {
        Err(SplitError::Unsupported(m)) => {
            assert!(
                m.contains("Next Level Games") && m.contains("stage.dict"),
                "{m}"
            );
            assert!(
                SplitError::Unsupported(m)
                    .to_string()
                    .starts_with("unsupported container: ")
            );
        }
        other => panic!(
            "expected an unsupported container, got {:?}",
            other.map(|s| s.shaders)
        ),
    }
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn a_file_no_reader_can_read_is_left_out_and_the_split_goes_on() {
    let g = game("d");
    // a BNSH whose variation table points outside the file
    let mut bad = bnsh(700);
    bad.truncate(0x120);
    std::fs::write(g.romfs.join("Data/bad.bnsh"), &bad).unwrap();
    let (r, log, _) = run(&g, "o", 2, None);
    let s = r.unwrap();
    assert_eq!(
        s.unreadable, 0,
        "a BNSH header the size check rejects is not a BNSH: {log:?}"
    );
    let mut bad = bnsh(800);
    let n = bad.len();
    bad[0x1c..0x20].copy_from_slice(&(n as u32).to_le_bytes());
    bad[0x80..0x88].copy_from_slice(&(1u64 << 40).to_le_bytes()); // the variation table is not in the file
    std::fs::write(g.romfs.join("Data/bad.bnsh"), &bad).unwrap();
    let (r, log, _) = run(&g, "o2", 2, None);
    let s = r.unwrap();
    assert_eq!(s.unreadable, 1, "{log:?}");
    assert_eq!(s.shaders, 8);
    assert!(
        log.iter()
            .any(|l| l.starts_with("[RED] /Data/bad.bnsh")
                && l.ends_with("(its shaders are left out)")),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|l| l.starts_with("[RED] 1 file(s) could not be read")),
        "{log:?}"
    );
    let _ = std::fs::remove_dir_all(&g.dir);
}

#[test]
fn a_table_that_cannot_be_written_whole_stays_as_it_was() {
    // review r2 N7: the tables a later step reads are replaced whole or not at all. The failure is a DIRECTORY named like the `.part` a table is written to (it fails for a root too)
    let g = game("e");
    let (r, _, o) = run(&g, "o", 2, None);
    r.unwrap();
    let old = read_dir_bytes(&o.join("tables"));
    std::fs::write(g.romfs.join("Data/d.bnsh"), bnsh(900)).unwrap(); // one more shader pair: shader_map.tsv and shader_pairs.tsv would change
    for blocked in ["shader_map.tsv", "shader_pairs.tsv"] {
        let part = o.join("tables").join(format!("{blocked}.part"));
        std::fs::create_dir(&part).unwrap();
        let (r, _, _) = run(&g, "o", 2, None);
        assert!(
            matches!(r, Err(SplitError::Io(_))),
            "{blocked}: {:?}",
            r.map(|s| s.shaders)
        );
        let now = read_dir_bytes_files(&o.join("tables"));
        assert!(
            now.contains(old.iter().find(|(n, _)| n == blocked).unwrap()),
            "{blocked} was changed"
        );
        assert!(
            now.iter().all(|(n, _)| !n.ends_with(".part")),
            "{:?}",
            now.iter().map(|x| &x.0).collect::<Vec<_>>()
        );
        std::fs::remove_dir(&part).unwrap();
    }
    let (r, _, _) = run(&g, "o", 2, None);
    assert_eq!(r.unwrap().shaders, 10);
    assert_ne!(old, read_dir_bytes_files(&o.join("tables")));
    assert!(
        read_dir_bytes_files(&o.join("tables"))
            .iter()
            .all(|(n, _)| !n.ends_with(".part"))
    );
    let _ = std::fs::remove_dir_all(&g.dir);
}
