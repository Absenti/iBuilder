//! The Rust rewrite of the three modules of The Legend of Zelda: Ocarina of Time (Switch 2, 0400486024DEC000, v131072) against the Python oracle: the SHA-256 of every
//! file `patch_nso.py` wrote for them (a copy of the engine's tools, `DM_GAME` the game's profile, `DM_REWRITE_J=6`) and of what it printed. `main` is a ZBIC module (flags
//! 0xff, system 22.0.0+): loading it needs `ibuilder_nso::zbic`; its build ID is registered in `FRAME_STEP` (the frame ratio, the dynamic resolution and the screen shape stubs of
//! `stubs_games::ocarina`, with the `ANAMORPHIC_ASPECT` capability) and its profile has the four movie `MATCH_CUTS`, whose output this checks end to end. The oracle is the shipped engine's
//! `patch_nso.py` (nxnative a3663659 + the NX2Apple patches + engine-patches/0409, the stack-free conditional select: 44 / 10 / 71 `select` sites in sdk / subsdk0 / main) run on the three real modules with `DM_GAME=0400486024DEC000` and no `game.tsv` for the profile (the Rust side gets no games folder). Skipped with a message when the extracted game is not there (set `IBUILDER_OCARINA_EXEFS`).

use ibuilder_rewrite::{Options, rewrite};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;

/// (module, file, SHA-256, size) of the Python's outputs; "log" is its stdout
// table rows are kept one per line as written (rustfmt would put every field of every row on its own line)
#[rustfmt::skip]
const ORACLE: &[(&str, &str, &str, usize)] = &[
    ("main", "patched.bin", "de498b73a84c641dcf0d2c86c9aeb2dd784531b10b94120de94a73d2019cfc51", 84941420),
    ("main", "fixups.tsv", "2eeaffc1ad4bfbf40f09b6917b00b00678b54305c2ead3b720f42ff10e52a5ec", 314),
    ("main", "sites.tsv", "10d0ef0a259b683afa0443defd0ece56518478aad043dd3772bd940114f5beaf", 718201),
    ("main", "gates.tsv", "eccb3edb696af6ace1d7c2046bc55dd21c76bffe9e956ad49f4f16d70ab916de", 1767),
    ("main", "cuts.tsv", "32426ca1335df73da1559e301059eb61577153be03ee7d4e57b030a04b372c5f", 2181),
    ("main", "obs.tsv", "99e7b4f486617cb5eab3b745c7b921ccd92051e1ebd2f3d21db5e94a0fb08916", 44),
    ("main", "changed.tsv", "3b64dcabd9a5c5506b96e4038d5cc569fa11e10c2fe2729ca24f4c940d31c395", 528291),
    ("main", "match.tsv", "c2fb661b22a084295c32999e8573bc2eb5bf1dbc97d15988ed664e5a8f7ce32c", 1030),
    ("main", "log", "681c6bc4dbc19bfb536cf5cc33921b4c71e91a57478545c2d728bee3c569db0a", 1051),
    ("subsdk0", "patched.bin", "a4cd3eb0d8417ccedde3d8fd016ba5117b62668a0a611b59aefd705e609e57fd", 1117992),
    ("subsdk0", "fixups.tsv", "c3ee00c129b52140b4f0c7cac94ea31aff7af6a72c51bf7d8227cb96dd246cfd", 233),
    ("subsdk0", "sites.tsv", "eb60534677e761fd2253328a5b2f92fedb72e41746eb2359d10c098586332c09", 40462),
    ("subsdk0", "gates.tsv", "e60cd6ae132a9c7ccd948cc67dbda2ca2e51554fd718dc8954727f13f7bfe498", 48),
    ("subsdk0", "cuts.tsv", "76dbd397a3ad4c9203508e20238fd561a33738f7a7d2c979e866edf08f21645e", 44),
    ("subsdk0", "obs.tsv", "76dbd397a3ad4c9203508e20238fd561a33738f7a7d2c979e866edf08f21645e", 44),
    ("subsdk0", "changed.tsv", "a39134393f11af8afc88563cb9bb7ed38da0e8f8ca2962bdd0d746a063a0aba5", 28777),
    ("subsdk0", "log", "b3f811b166e8aa80d06cad5bb4b8f9ac317e98d60e857562e3cd4eca2387df40", 140),
    ("sdk", "patched.bin", "dd05c2c222fb857255ec3ba30bb075cf970cc50fbf206cfba021cee85d343e7f", 18404432),
    ("sdk", "fixups.tsv", "3d09fd5f54fc327b51c1684a407681230452345ee5f7776aedc4710e3bbecb3b", 240),
    ("sdk", "sites.tsv", "e1185b2db573b9895e518e148a138bb22ee7a8894a7cbc955725b3b66872a280", 336606),
    ("sdk", "gates.tsv", "27bed2e5ca65f9b01eff734cc0d42595d65468e73c42d6346026c1a2b189a104", 3070),
    ("sdk", "cuts.tsv", "fbb8b4f707299dc7182208fb158aff7434ab039d96d1259221216a6c4a144b31", 355),
    ("sdk", "obs.tsv", "7a30313410665183bf04dd1adfe60b8499160dff734e6dbe4fcced91cbfac8d8", 584),
    ("sdk", "changed.tsv", "4a65ca7322680d65eb5a57e835fc80c438546f02bf09d86cf3f3bf9d68062d65", 236553),
    ("sdk", "match.tsv", "782c12bbf90f6e40704c6928ef599dcd20b1771bf9cffa4db0b4e3bc76682b7e", 539),
    ("sdk", "log", "400f6c52904785cc12b1ab3cfeb6728491d9a1304a9fb11668cfe443359ce67d", 618),
];

fn exefs() -> Option<PathBuf> {
    let candidates = [
        std::env::var("IBUILDER_OCARINA_EXEFS").ok(),
        Some(
            ibuilder_testkit::scratch()
                .join("ocarina/extract/ExeFS")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|p| p.join("main").is_file())
}

fn hex(h: impl AsRef<[u8]>) -> String {
    h.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

struct Hasher(Sha256, usize);
impl Write for Hasher {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.update(b);
        self.1 += b.len();
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn check(module: &str, threads: usize) {
    let Some(dir) = exefs() else {
        ibuilder_testkit::skip(&format!(
            "{module}: no extracted Ocarina of Time ExeFS on this machine"
        ));
        return;
    };
    let file = std::fs::File::open(dir.join(module)).unwrap();
    // SAFETY: a read-only map of a file nothing writes while the test runs
    let map = unsafe { memmap2::Mmap::map(&file) }.unwrap();
    let opts = Options {
        workers: threads,
        game: Some("0400486024DEC000".into()),
        ..Default::default()
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let r = pool
        .install(|| rewrite(&map, module, module, &opts))
        .unwrap_or_else(|e| panic!("{module}: {e}"));
    let mut got: Vec<(&str, String, usize)> = vec![];
    let mut h = Hasher(Sha256::new(), 0);
    r.write_patched(&mut h).unwrap();
    got.push(("patched.bin", hex(h.0.finalize()), h.1));
    let texts = [
        ("fixups.tsv", &r.fixups),
        ("sites.tsv", &r.sites),
        ("gates.tsv", &r.gates),
        ("cuts.tsv", &r.cuts),
        ("obs.tsv", &r.obs),
        ("changed.tsv", &r.changed),
    ];
    for (ext, t) in texts {
        got.push((ext, hex(Sha256::digest(t.as_bytes())), t.len()));
    }
    if let Some(m) = &r.match_tsv {
        got.push(("match.tsv", hex(Sha256::digest(m.as_bytes())), m.len()));
    }
    let log = format!("{}\n", r.log.join("\n"));
    got.push(("log", hex(Sha256::digest(log.as_bytes())), log.len()));
    let want: Vec<&(&str, &str, &str, usize)> = ORACLE.iter().filter(|o| o.0 == module).collect();
    assert_eq!(
        want.len(),
        got.len(),
        "{module}: the oracle has {} files, this run {}",
        want.len(),
        got.len()
    );
    for (f, sha, n) in &got {
        let o = want
            .iter()
            .find(|o| o.1 == *f)
            .unwrap_or_else(|| panic!("{module}: no oracle for {f}"));
        assert_eq!(
            (o.3, o.2),
            (*n, sha.as_str()),
            "{module}.{f} differs from the Python oracle"
        );
    }
    eprintln!(
        "{module}: {} files + log identical to the Python oracle ({threads} threads)",
        got.len() - 1
    );
}

#[test]
fn ocarina_subsdk0_and_sdk_match_the_oracle() {
    check("subsdk0", 4);
    check("sdk", 4);
}

#[test]
fn ocarina_main_matches_the_oracle_at_any_thread_count() {
    // the ZBIC module with the build-registered frame step, dynamic-resolution and screen shape stubs and the movie cuts; one and eight threads give the same bytes
    check("main", 8);
    check("main", 1);
}
