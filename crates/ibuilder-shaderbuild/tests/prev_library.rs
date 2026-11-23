//! Speed-up B6: a rebuild of the same chain takes the shaders whose digest did not change from the previous library (`<out_dir>/prev/b-<chain>.img` and its digests table) instead of
//! translating them again, and makes the image a build without the previous library makes, byte for byte. A synthetic game, no engine.
mod synth;
use ibuilder_shader::romfs;
use ibuilder_shader::tools::ShaderTools;
use ibuilder_shaderbuild::cache::{self, Meta, PREV_DIR, REUSE_FILE};
use ibuilder_shaderbuild::finish::{Finished, Finisher};
use ibuilder_shaderbuild::library::{Source, digests_path, write_image};
use ibuilder_shaderbuild::{BuildEnv, BuildOptions, BuildReport, build};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use synth::{CHAIN, key_of, scratch, tools};

/// a finisher whose files depend on the shader's code (`<work>/<key>.bin`), so a changed shader is a changed file; it counts its calls and can raise a cancel at its n-th
struct Coded {
    fp: &'static str,
    calls: AtomicUsize,
    cancel_at: Option<(usize, &'static AtomicBool)>,
}

impl Coded {
    fn new(fp: &'static str) -> Coded {
        Coded {
            fp,
            calls: AtomicUsize::new(0),
            cancel_at: None,
        }
    }
}

impl Finisher for Coded {
    fn fingerprint(&self) -> String {
        self.fp.into()
    }
    fn finish(
        &self,
        key: &str,
        work: &Path,
        _job: &mut Value,
        c1: &[u8],
        _tools: &dyn ShaderTools,
        _paired: &dyn Fn(&str) -> Option<BTreeSet<u32>>,
    ) -> Finished {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((at, flag)) = self.cancel_at
            && n == at
        {
            flag.store(true, Ordering::SeqCst);
        }
        let code = fs::read(work.join(format!("{key}.bin"))).expect("the shader's code file");
        let mut metal = format!("// shader {key}\n").into_bytes();
        metal.extend(&code);
        metal.extend(format!("\n// c1 {}\n", c1.len()).bytes());
        Finished {
            status: "ok".into(),
            detail: String::new(),
            refl: None,
            outputs: vec![
                (format!("{key}_b.metal"), metal),
                (
                    format!("{key}_b.json"),
                    format!("{{\"n\": {}}}\n", code.len()).into_bytes(),
                ),
            ],
            ps_locs: BTreeSet::new(),
            log: Vec::new(),
        }
    }
}

/// shader `i`'s split record, its code text `code of <key> <variant>`
fn record(key: &str, variant: &str) -> Vec<u8> {
    let mut r = b"BTCDBTCD".to_vec();
    r.extend((0..8).map(|i| u8::from_str_radix(&key[2 * i..2 * i + 2], 16).unwrap()));
    r.extend([0u8; 0x28]);
    r.extend(format!("code of {key} {variant} ").repeat(6).bytes());
    let mut ctl = vec![0u8; 0x800];
    ctl[..4].copy_from_slice(&[0x34, 0x12, 0x76, 0x98]);
    r.extend(ctl);
    r
}

/// `<root>/pkg/ps_main.bin` and `<root>/profile/G/...` of `n` shaders, shader `i` in variant `variant(i)`
fn write_game(root: &Path, n: usize, variant: impl Fn(usize) -> &'static str) {
    let _ = fs::remove_dir_all(root.join("pkg"));
    let _ = fs::remove_dir_all(root.join("profile"));
    let g = root.join("profile").join("G");
    fs::create_dir_all(&g).unwrap();
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::write(g.join("game.tsv"), "title_id\tG\n").unwrap();
    let mut bin = Vec::new();
    for i in 0..n {
        bin.extend(record(&key_of(i), variant(i)));
    }
    fs::write(root.join("pkg").join("ps_main.bin"), bin).unwrap();
    fs::write(g.join("shader_map.tsv"), synth::map_text(n)).unwrap();
    for t in ["shader_facts.tsv", "shader_pairs.tsv", "pso_pairs.tsv"] {
        fs::write(g.join(t), "").unwrap();
    }
}

fn options(root: &Path, out: &str, cas: &str, chain: &str) -> BuildOptions {
    options_t(root, out, cas, chain, false)
}

/// `tables`: the build writes c1_full.tsv / cb_extent.tsv, whose first line holds the build's time (two builds never agree on it: `same_but_the_time` compares those)
fn options_t(root: &Path, out: &str, cas: &str, chain: &str, tables: bool) -> BuildOptions {
    let mut o = BuildOptions::new(
        &root.join("pkg"),
        &root.join("profile"),
        "G",
        &root.join(out),
        chain,
        &root.join(cas),
    );
    o.jobs = 4;
    o.tables = tables;
    o.pulling = false;
    o.follow_stop = false; // the process-wide request is other tests' (stop_build.rs); this one stops through `cancel`
    o
}

fn run(o: &BuildOptions, fin: &Coded, cancel: Option<&AtomicBool>) -> Result<BuildReport, String> {
    let t = tools(0);
    let lines = Mutex::new(Vec::<String>::new());
    let log = |l: &str| lines.lock().unwrap().push(l.to_string());
    build(
        o,
        &BuildEnv {
            tools: &t,
            finisher: fin,
            log: &log,
            cancel,
        },
    )
}

/// `<out_dir>/prev/` gets the library of `from` (out folder) and its digests table, as stage 3 of the pipeline puts them there
fn place_previous(root: &Path, from: &str, out: &str, chain: &str) -> PathBuf {
    let img = root.join(from).join(format!("b-{chain}.img"));
    let prev = root.join(out).join(PREV_DIR);
    let _ = fs::remove_dir_all(&prev);
    fs::create_dir_all(&prev).unwrap();
    // hard links, as the pipeline makes them (a copy is another file to the table: its time may differ)
    fs::hard_link(&img, prev.join(format!("b-{chain}.img"))).unwrap();
    fs::hard_link(
        digests_path(&img),
        digests_path(&prev.join(format!("b-{chain}.img"))),
    )
    .unwrap();
    prev
}

fn reuse(prev: &Path) -> Vec<(String, String)> {
    fs::read_to_string(prev.join(REUSE_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn reuse_row(prev: &Path, k: &str) -> String {
    reuse(prev)
        .into_iter()
        .find(|r| r.0 == k)
        .map(|r| r.1)
        .unwrap_or_default()
}

/// the two images hold the same files at the same places with the same bytes, but for the time in the first line of the two tables
fn same_but_the_time(a: &[u8], b: &[u8]) {
    let (ia, ib) = (
        romfs::Image::open(a).unwrap(),
        romfs::Image::open(b).unwrap(),
    );
    let layout = |i: &romfs::Image| {
        i.files
            .iter()
            .map(|f| (f.name.clone(), f.off, f.size))
            .collect::<Vec<_>>()
    };
    assert_eq!(layout(&ia), layout(&ib), "the images' layouts differ");
    let mut tables = 0;
    for (fa, fb) in ia.files.iter().zip(&ib.files) {
        let (x, y) = (ia.bytes(fa), ib.bytes(fb));
        if fa.name == "c1_full.tsv" || fa.name == "cb_extent.tsv" {
            let cut = |t: &[u8]| {
                let s = String::from_utf8(t.to_vec()).unwrap();
                let (head, rest) = s.split_once('\n').unwrap();
                (
                    head.rsplit_once(' ').unwrap().0.to_string(),
                    rest.to_string(),
                )
            };
            assert_eq!(cut(x), cut(y), "{}", fa.name);
            tables += 1;
        } else {
            assert!(x == y, "{} differs", fa.name);
        }
    }
    assert_eq!(tables, 2);
}

fn image(root: &Path, out: &str, chain: &str) -> Vec<u8> {
    fs::read(root.join(out).join(format!("b-{chain}.img"))).unwrap()
}

const N: usize = 60;

#[test]
fn an_unchanged_shader_comes_from_the_previous_library_and_the_image_is_the_same() {
    let root = scratch("prev-reuse");
    write_game(&root, N, |_| "v1");
    let fin = Coded::new("coded-1");
    let first = run(&options_t(&root, "out1", "cas1", CHAIN, true), &fin, None).unwrap();
    assert_eq!((first.ok, first.cache_miss), (N, N));
    let table = fs::read_to_string(digests_path(
        &root.join("out1").join(format!("b-{CHAIN}.img")),
    ))
    .unwrap();
    assert_eq!(
        table.lines().count(),
        N + 1,
        "a line per shader under the image's line"
    );

    // the game changes: shader 7 is another shader now, shader N is new
    write_game(&root, N + 1, |i| if i == 7 { "v2" } else { "v1" });
    let prev = place_previous(&root, "out1", "out2", CHAIN);
    let fin2 = Coded::new("coded-1");
    let r = run(&options_t(&root, "out2", "cas2", CHAIN, true), &fin2, None).unwrap(); // an empty cache: every hit is the previous library's
    assert_eq!(r.ok, N + 1);
    assert_eq!((r.cache_hit, r.cache_miss), (N - 1, 2));
    assert_eq!(
        fin2.calls.load(Ordering::SeqCst),
        2,
        "only the changed and the new shader are finished"
    );
    assert_eq!(reuse_row(&prev, "state"), "used");
    assert_eq!(reuse_row(&prev, "entries"), N.to_string());
    assert_eq!(reuse_row(&prev, "hits"), (N - 1).to_string());
    assert_eq!(reuse_row(&prev, "looked"), (N + 1).to_string());

    // the same game built with no previous library and no cache: the same image, the same table
    let fin3 = Coded::new("coded-1");
    let fresh = run(&options_t(&root, "out3", "cas3", CHAIN, true), &fin3, None).unwrap();
    assert_eq!(fresh.cache_miss, N + 1);
    same_but_the_time(&image(&root, "out2", CHAIN), &image(&root, "out3", CHAIN));
    // and with no tables (no time in the image), byte for byte
    let prev4 = place_previous(&root, "out1", "out4", CHAIN);
    let r4 = run(
        &options(&root, "out4", "cas4", CHAIN),
        &Coded::new("coded-1"),
        None,
    )
    .unwrap();
    assert_eq!(
        (r4.cache_hit, reuse_row(&prev4, "hits")),
        (N - 1, (N - 1).to_string())
    );
    run(
        &options(&root, "out5", "cas5", CHAIN),
        &Coded::new("coded-1"),
        None,
    )
    .unwrap();
    assert!(
        image(&root, "out4", CHAIN) == image(&root, "out5", CHAIN),
        "the image with reuse differs from the one without"
    );
    // the shaders' lines (the first line is the image's own: its time)
    let lines = |out: &str| {
        let t = fs::read_to_string(digests_path(&root.join(out).join(format!("b-{CHAIN}.img"))))
            .unwrap();
        t.split_once('\n').unwrap().1.to_string()
    };
    assert_eq!(
        lines("out2"),
        lines("out3"),
        "the digests table made from a previous library is the one a full build makes"
    );
    assert_eq!(lines("out2").lines().count(), N + 1);
    // the changed shader really is another file
    assert_ne!(image(&root, "out1", CHAIN), image(&root, "out2", CHAIN));
    // the previous library is read, never changed
    assert_eq!(
        fs::read(prev.join(format!("b-{CHAIN}.img"))).unwrap(),
        image(&root, "out1", CHAIN)
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn another_finisher_or_chain_or_a_table_of_another_image_takes_nothing() {
    let root = scratch("prev-none");
    write_game(&root, N, |_| "v1");
    run(
        &options(&root, "out1", "cas1", CHAIN),
        &Coded::new("coded-1"),
        None,
    )
    .unwrap();
    run(
        &options(&root, "ref2", "casr2", CHAIN),
        &Coded::new("coded-2"),
        None,
    )
    .unwrap();

    // another finisher (as another CODE_FP or tool version would be): every digest differs, nothing is served, the image is the full build's
    let prev = place_previous(&root, "out1", "out2", CHAIN);
    let fin = Coded::new("coded-2");
    let r = run(&options(&root, "out2", "cas2", CHAIN), &fin, None).unwrap();
    assert_eq!((r.cache_hit, fin.calls.load(Ordering::SeqCst)), (0, N));
    assert_eq!(reuse_row(&prev, "hits"), "0");
    assert!(image(&root, "out2", CHAIN) == image(&root, "ref2", CHAIN));

    // another chain: the previous library of this chain is not even looked at (its name is `b-<chain>.img`)
    let other = "0123456789ab";
    let prev3 = place_previous(&root, "out1", "out3", CHAIN);
    let fin3 = Coded::new("coded-1");
    let r3 = run(&options(&root, "out3", "cas3", other), &fin3, None).unwrap();
    assert_eq!((r3.cache_hit, fin3.calls.load(Ordering::SeqCst)), (0, N));
    assert!(!prev3.join(REUSE_FILE).exists());

    // a table whose image was replaced by another one of the same names and sizes (the same game, another finisher's bytes): not believed, every shader is translated
    write_game(&root, N, |_| "v3");
    run(
        &options(&root, "ref4", "casr4", CHAIN),
        &Coded::new("coded-1"),
        None,
    )
    .unwrap();
    write_game(&root, N, |_| "v1");
    let prev4 = place_previous(&root, "out1", "out4", CHAIN);
    let other_img = image(&root, "ref4", CHAIN);
    assert_eq!(other_img.len(), image(&root, "out1", CHAIN).len());
    fs::remove_file(prev4.join(format!("b-{CHAIN}.img"))).unwrap();
    fs::write(prev4.join(format!("b-{CHAIN}.img")), &other_img).unwrap();
    let fin4 = Coded::new("coded-1");
    let r4 = run(&options(&root, "out4", "cas4", CHAIN), &fin4, None).unwrap();
    assert_eq!((r4.cache_hit, fin4.calls.load(Ordering::SeqCst)), (0, N));
    assert!(
        reuse_row(&prev4, "state").starts_with("not used: the digests table is of another image"),
        "{:?}",
        reuse(&prev4)
    );
    assert!(image(&root, "out4", CHAIN) == image(&root, "out1", CHAIN));

    // a damaged line: the whole table is not believed
    let prev5 = place_previous(&root, "out1", "out5", CHAIN);
    let t = digests_path(&prev5.join(format!("b-{CHAIN}.img")));
    let text = fs::read_to_string(&t)
        .unwrap()
        .replacen("_b.json\t", "_b.json\tx", 1);
    fs::write(&t, text).unwrap();
    let fin5 = Coded::new("coded-1");
    let r5 = run(&options(&root, "out5", "cas5", CHAIN), &fin5, None).unwrap();
    assert_eq!(r5.cache_hit, 0);
    assert!(
        reuse_row(&prev5, "state").starts_with("not used: a damaged line"),
        "{:?}",
        reuse(&prev5)
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_build_stopped_with_a_previous_library_resumes_to_the_same_image() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    let root = scratch("prev-stop");
    write_game(&root, N, |_| "v1");
    run(
        &options(&root, "out1", "cas1", CHAIN),
        &Coded::new("coded-1"),
        None,
    )
    .unwrap();
    // 20 shaders change; the build is stopped at the 5th it finishes
    let variant = |i: usize| if i.is_multiple_of(3) { "v2" } else { "v1" };
    write_game(&root, N, variant);
    let prev = place_previous(&root, "out1", "out2", CHAIN);
    let mut fin = Coded::new("coded-1");
    fin.cancel_at = Some((5, &CANCEL));
    let e = run(&options(&root, "out2", "cas2", CHAIN), &fin, Some(&CANCEL)).unwrap_err();
    assert_eq!(e, "cancelled");
    assert_eq!(
        reuse_row(&prev, "hits"),
        (N - 20).to_string(),
        "the cache check went through before the stop"
    );
    let finished = fin.calls.load(Ordering::SeqCst);
    assert!(finished < 20, "{finished}");
    // the run that follows: what finished is in the cache, the rest of the unchanged shaders still come from the previous library
    CANCEL.store(false, Ordering::SeqCst);
    let fin2 = Coded::new("coded-1");
    let r = run(&options(&root, "out2", "cas2", CHAIN), &fin2, None).unwrap();
    assert_eq!(r.cache_hit + fin2.calls.load(Ordering::SeqCst), N);
    assert_eq!(fin2.calls.load(Ordering::SeqCst) + finished, 20);
    assert_eq!(reuse_row(&prev, "hits"), (N - 20).to_string());
    let fresh = run(
        &options(&root, "out3", "cas3", CHAIN),
        &Coded::new("coded-1"),
        None,
    )
    .unwrap();
    assert_eq!(fresh.cache_miss, N);
    assert!(image(&root, "out2", CHAIN) == image(&root, "out3", CHAIN));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_shader_the_cache_must_not_take_gets_no_line_and_a_new_image_drops_an_old_table() {
    let d = scratch("prev-table");
    let meta = Meta {
        c1: vec![],
        cb: None,
        tables: true,
        locs: vec![1],
    };
    let (da, db) = ("a".repeat(64), "b".repeat(64));
    let stage = d.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let mut sources = Vec::new();
    for (path, name) in [
        (stage.join(&da), "ka"),
        (stage.join(format!("{db}.nc")), "kb"),
    ] {
        let files = vec![(format!("{name}_b.metal"), name.repeat(40).into_bytes())];
        fs::write(&path, cache::encode(&meta, &files)).unwrap();
        let st = cache::read_header(&path).unwrap();
        for (n, off, len) in st.files {
            sources.push((
                n,
                Source::Range {
                    path: path.clone(),
                    off,
                    len,
                },
            ));
        }
    }
    sources.push(("stamp".into(), Source::Bytes(b"x".to_vec())));
    let img = d.join("b-x.img");
    write_image(sources, &img).unwrap();
    let table = fs::read_to_string(digests_path(&img)).unwrap();
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 2, "{table}");
    assert!(lines[1].starts_with(&format!("{da}\t")) && lines[1].contains("\tka_b.metal\t"));
    // an image with no shader of a cache entry: no table, and the old one is gone
    write_image(vec![("a".into(), Source::Bytes(vec![1; 10]))], &img).unwrap();
    assert!(!digests_path(&img).exists());
    fs::remove_dir_all(&d).unwrap();
}
