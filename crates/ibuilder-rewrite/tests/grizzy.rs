//! The Rust rewrite of the three modules of Grizzy & the Lemmings: Crazy Party (the dump in `iOSBuilder/out/ref-out-grizzy/ExeFS`) against the Python oracle, kept as SHA-256s (the
//! 167 MB `main.patched.bin` does not belong in a repository). The oracle is the shipped engine's `patch_nso.py` (nxnative a3663659 + the NX2Apple patches + `engine-patches/0409`, the
//! stack-free conditional select: 36 / 21 / 68 `select` sites in sdk / subsdk0 / main) run with no game profile (`DM_GAME` unset), `DM_REWRITE_J=6`, from a copy of the engine's `dm/tools`
//! in a scratch folder, as `python -B patch_nso.py <module> <out> <module>` (the interpreter of the reference venv, Python 3.14.1, capstone 5.0.7); "log" is what it printed.
//! Recorded 2026-11-19, replacing the files of `out-py/pyrewrite` (`tests/oracle.rs`, which compares whole files when that folder exists, and skips when it does not): the
//! pre-0409 oracle's `patched.bin`, `gates.tsv` differ from these (the select sites no longer spill).
//! `build_src.sha256` (the hash of the Python sources folder) is not kept: it depends on where the tools were copied from.

use ibuilder_rewrite::{Options, rewrite};
use sha2::{Digest, Sha256};
use std::io::Write;

/// (module, file, SHA-256, size) of the Python's outputs; "log" is its stdout
// table rows are kept one per line as written (rustfmt would put every field of every row on its own line)
#[rustfmt::skip]
const ORACLE: &[(&str, &str, &str, usize)] = &[
    ("main", "patched.bin", "8abb1dec00628d81629272c2a9d8fb25d89272d0dfd2785dfc7dc997633ad0eb", 167018696),
    ("main", "fixups.tsv", "ef8d0d7069f73ba7edc784f1db5efffa0d63f61c291ba86a9cfb64542b0e0c28", 293),
    ("main", "sites.tsv", "7f26e009ae9251c926910476af3fbb3f4ed3c0c7835df5b740c11f977d268e11", 452554),
    ("main", "gates.tsv", "cc1d50d6eadb02530c0dcee7a0b0bbf0d6561621756fedf914c33fa9873acaf7", 2147),
    ("main", "cuts.tsv", "20b4c4bc564f6c05d903a2cc94af2624cf4497389ebd2690e5f469e81d83d04a", 44),
    ("main", "obs.tsv", "20b4c4bc564f6c05d903a2cc94af2624cf4497389ebd2690e5f469e81d83d04a", 44),
    ("main", "changed.tsv", "1846f824d3201e216d5c7d5bf45bd2111296b3d5c8f4467cbaccb9eb9d023491", 331640),
    ("main", "match.tsv", "60f9de1cc23a99890f98aa5c13d8dac2120af879db41a1fa9c9ec2a3248778da", 563),
    ("main", "log", "cd371684b371b832720a40d5df59f93badff680031801b15ab26bc9fdb30cac3", 708),
    ("subsdk0", "patched.bin", "15a34a00d30c432c4d00d7d89b718d28b1559ed610dab83c18a343c5c4cd0ad8", 7136812),
    ("subsdk0", "fixups.tsv", "84a4ff71728f6a6d80227d1b850aa693c01ac48a0247b0cae20fcf6c59afc373", 238),
    ("subsdk0", "sites.tsv", "1589e185129148b6ae3541ef948ac606a6c5bd407e486b3116181cbdaaa4950d", 123298),
    ("subsdk0", "gates.tsv", "6947f043cecadf27de0f69c26a347a7209e637a63927fc0a6a2dac0183c833dc", 268),
    ("subsdk0", "cuts.tsv", "f189600fd9be5f1fc483f730032f76ee6f3bf56a6daa7a9231057d6417bceebc", 44),
    ("subsdk0", "obs.tsv", "f189600fd9be5f1fc483f730032f76ee6f3bf56a6daa7a9231057d6417bceebc", 44),
    ("subsdk0", "changed.tsv", "a2cf77db9ea9cfa63644ace6f40b6be726c750e9454a881cad802f11ecc44e3c", 88297),
    ("subsdk0", "log", "24fca1a983f324926fe45dd57d5acfdc14896b8ee6d9705693ec6601bf67611e", 145),
    ("sdk", "patched.bin", "dfaaedccfe6639e73930f29007f6a68ac7a6e23b6984f6e75a20e8eb706de698", 13756020),
    ("sdk", "fixups.tsv", "4454fdc4b7ff6b4b319d853edea0584ac57798b5e848a723e06d4298fc2bf731", 238),
    ("sdk", "sites.tsv", "5db9594279e9c1f2bb2c244ba09a7e24d190748cc019c5d4c6866d04bd82cd0d", 209041),
    ("sdk", "gates.tsv", "4067c1149abaf32fe676ac70ce047e0e87b3005486bc8bcc1546283ecb9c65c2", 2852),
    ("sdk", "cuts.tsv", "1e5241d9df938201e86e1958b521f7601495ae762b0ea30f53b41bead64f04d0", 44),
    ("sdk", "obs.tsv", "d63d36455ddff8d2266ff61c72c815e50ef66c300b74548861e3c706c5ec7eb1", 584),
    ("sdk", "changed.tsv", "6b6d9b179e5b0788dfc1a68ac3e521d7c24c462ac22f7a38e7666ec63a7037cc", 149908),
    ("sdk", "match.tsv", "0f9b00b689668bfefd345a801247c393f72d0b4e9b4008c8a010ec2bab4da69e", 869),
    ("sdk", "log", "163a9e6155857b0143c39b6d1146bd68b520c94a5057d1e279159cda6d389cc6", 977),
];

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
    let path = ibuilder_testkit::grizzy_exefs(module);
    let Ok(file) = std::fs::File::open(&path) else {
        ibuilder_testkit::skip(&format!(
            "{module}: the Grizzy ExeFS ({path}) is not on this machine"
        ));
        return;
    };
    // SAFETY: a read-only map of a file nothing writes while the test runs
    let map = unsafe { memmap2::Mmap::map(&file) }.unwrap();
    let opts = Options {
        workers: threads,
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
            "{module}.{f} differs from the Python oracle ({threads} threads)"
        );
    }
    eprintln!(
        "{module}: {} files + log identical to the Python oracle ({threads} threads)",
        got.len() - 1
    );
}

#[test]
fn grizzy_subsdk0_and_sdk_match_the_0409_oracle() {
    check("subsdk0", 4);
    check("sdk", 4);
}

#[test]
fn grizzy_main_matches_the_0409_oracle_at_one_and_eight_threads() {
    check("main", 8);
    check("main", 1);
}
