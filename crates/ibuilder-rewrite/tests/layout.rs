//! `layout::relayout_module` (nxnative mod_layout.py, 17454fc3 + 65dfeb33: a module's writable data on a 16 KB page of its own) against the Python on synthetic modules:
//! `tests/fixtures/layout/` (tools/layoutcases.py) holds upstream test_mod_layout.py's images (FINAL FANTASY VIII's Math.nro shape, an NSO with MOD0 in text and DT_RELR,
//! the .eh_frame / LSDA variant, a module that already fits), hand-made edge and error cases, and 120 fuzzed variants (instructions of the function region, relocation
//! addends, symbol values, dynamic tags, MOD0 offsets), each with the image SHA-256, the segments, D and the notes the Python gave, or its error message.
//! No game files are needed: nothing is skipped.

use ibuilder_rewrite::layout::{relayout_module, shift_needed};
use sha2::{Digest, Sha256};
use std::path::Path;

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/layout")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn with_pokes(base: &[u8], pokes: &str) -> Vec<u8> {
    let mut b = base.to_vec();
    if pokes != "-" {
        for p in pokes.split(',') {
            let (o, w) = p.split_once('=').unwrap();
            let o = usize::from_str_radix(o.trim_start_matches("0x"), 16).unwrap();
            let w = u32::from_str_radix(w.trim_start_matches("0x"), 16).unwrap();
            b[o..o + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
    b
}

#[test]
fn relayout_is_the_pythons_on_every_synthetic_case() {
    let manifest = std::fs::read_to_string(dir().join("manifest.tsv")).unwrap();
    let (mut moved, mut same, mut errors, mut bad) = (0, 0, 0, Vec::new());
    for row in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let c: Vec<&str> = row.split('\t').collect();
        let (name, base, pokes) = (c[0], c[1], c[2]);
        let raw = with_pokes(
            &std::fs::read(dir().join(format!("base.{base}.bin"))).unwrap(),
            pokes,
        );
        let got = ibuilder_nso::load(&raw, name)
            .map_err(|e| format!("load: {}", e.0))
            .and_then(relayout_module);
        match (c[3], got) {
            ("error", Err(e)) if e == c[4] => errors += 1,
            ("crash", Err(_)) => errors += 1,
            (d, Ok((m, gd, notes))) if d != "error" && d != "crash" => {
                let seg = m
                    .segments
                    .iter()
                    .map(|(o, s)| format!("{o:#x}+{s:#x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let n = if notes.is_empty() {
                    "-".to_string()
                } else {
                    notes.join("; ")
                };
                let want = (c[3], c[4], c[5], c[6]);
                let have = (format!("{gd:#x}"), seg, hex(&Sha256::digest(&m.image)), n);
                if (
                    have.0.as_str(),
                    have.1.as_str(),
                    have.2.as_str(),
                    have.3.as_str(),
                ) == want
                {
                    if gd == 0 { same += 1 } else { moved += 1 }
                } else {
                    bad.push(format!("{name}: Python {want:?}, Rust {have:?}"));
                }
            }
            (want, got) => bad.push(format!(
                "{name}: Python {want} {:?}, Rust {:?}",
                c.get(4),
                got.map(|(_, d, n)| (d, n))
            )),
        }
    }
    assert!(
        bad.is_empty(),
        "{} cases differ:\n  {}",
        bad.len(),
        bad.join("\n  ")
    );
    assert!(
        moved >= 120 && same >= 1 && errors >= 5,
        "moved {moved}, unchanged {same}, errors {errors}"
    );
    println!("{moved} moved, {same} unchanged, {errors} stopped: all as the Python");
}

#[test]
fn a_moved_module_reloads_as_it_is_and_needs_no_second_move() {
    let raw = std::fs::read(dir().join("base.math.bin")).unwrap();
    let m = ibuilder_nso::load(&raw, "Math").unwrap();
    let (out, d, _) = relayout_module(m).unwrap();
    assert_eq!(d, 0x2000);
    assert_eq!(
        out.segments,
        [(0, 0x1000), (0x1000, 0x3000), (0x4000, 0x1000)]
    );
    assert!(
        out.image[0x2000..0x4000].iter().all(|&b| b == 0),
        "the gap is zero"
    );
    // the embedded NRO header stays consistent: the image loads to the same module
    let again = ibuilder_nso::load(&out.image, "Math").unwrap();
    assert_eq!((again.segments, &again.image), (out.segments, &out.image));
    assert_eq!(relayout_module(again).unwrap().1, 0);
}

#[test]
fn the_shift_is_whole_pages_up_to_the_code_s_16k_page() {
    assert_eq!(shift_needed(0x1000, 0x2000), 0x2000);
    assert_eq!(shift_needed(0x1000, 0x3000), 0x1000);
    assert_eq!(shift_needed(0x1000, 0x4000), 0);
    assert_eq!(shift_needed(0x11000, 0x12000), 0x2000);
    assert_eq!(shift_needed(0x4000, 0x4000), 0);
    assert_eq!(shift_needed(0x4001, 0x5000), 0x3000);
}

#[test]
fn a_module_that_fits_is_returned_untouched() {
    let raw = std::fs::read(dir().join("base.fitting.bin")).unwrap();
    let m = ibuilder_nso::load(&raw, "ok").unwrap();
    let (image, segments) = (m.image.clone(), m.segments);
    let (out, d, notes) = relayout_module(m).unwrap();
    assert_eq!((d, notes.len()), (0, 0));
    assert_eq!((out.image, out.segments), (image, segments));
}

/// the whole rewrite of a moved module (patched.bin, fixups.tsv, sites.tsv, gates.tsv, cuts.tsv, obs.tsv, changed.tsv and what it printed) against the engine's
/// patch_nso.py with mod_layout.py called after the load, as upstream does (`rewrite.tsv`, tools/layoutcases.py --rewrite-oracle)
#[test]
fn the_rewrite_of_a_moved_module_is_the_pythons() {
    use ibuilder_rewrite::{Options, rewrite};
    use std::collections::BTreeMap;
    let manifest = std::fs::read_to_string(dir().join("manifest.tsv")).unwrap();
    let inputs: BTreeMap<&str, (&str, &str)> = manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (c[0], (c[1], c[2]))
        })
        .collect();
    let oracle = std::fs::read_to_string(dir().join("rewrite.tsv")).unwrap();
    let mut want: BTreeMap<&str, BTreeMap<&str, &str>> = BTreeMap::new();
    for l in oracle
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let c: Vec<&str> = l.splitn(3, '\t').collect();
        want.entry(c[0]).or_default().insert(c[1], c[2]);
    }
    let sha = |b: &[u8]| hex(&Sha256::digest(b));
    let (mut same, mut stopped) = (0, 0);
    for (name, files) in &want {
        let (base, pokes) = inputs[name];
        let raw = with_pokes(
            &std::fs::read(dir().join(format!("base.{base}.bin"))).unwrap(),
            pokes,
        );
        let opts = Options {
            workers: 2,
            ..Default::default()
        };
        match (files.get("error"), rewrite(&raw, name, "nro0", &opts)) {
            (Some(line), Err(e)) => {
                if line.starts_with("[RED]") {
                    assert_eq!(
                        &e.msg, line,
                        "{name}: the message differs from the Python's"
                    );
                }
                stopped += 1;
            }
            (None, Ok(r)) => {
                let mut got: BTreeMap<&str, String> = BTreeMap::new();
                got.insert("stdout", sha(format!("{}\n", r.log.join("\n")).as_bytes()));
                let mut bin = Vec::new();
                r.write_patched(&mut bin).unwrap();
                got.insert("patched.bin", sha(&bin));
                for (k, v) in [
                    ("fixups.tsv", &r.fixups),
                    ("sites.tsv", &r.sites),
                    ("gates.tsv", &r.gates),
                    ("cuts.tsv", &r.cuts),
                    ("obs.tsv", &r.obs),
                    ("changed.tsv", &r.changed),
                ] {
                    got.insert(k, sha(v.as_bytes()));
                }
                if let Some(m) = &r.match_tsv {
                    got.insert("match.tsv", sha(m.as_bytes()));
                }
                let want: BTreeMap<&str, String> =
                    files.iter().map(|(k, v)| (*k, v.to_string())).collect();
                assert_eq!(got, want, "{name}: the rewrite differs from the Python's");
                same += 1;
            }
            (w, g) => panic!(
                "{name}: the Python {:?}, the Rust {:?}",
                w,
                g.map(|_| "finished").map_err(|e| e.msg)
            ),
        }
    }
    assert!(
        same >= 30 && stopped >= 5,
        "{same} rewrites, {stopped} stops"
    );
    println!(
        "{same} rewrites of moved modules identical to the Python's, {stopped} stopped as it did"
    );
}
