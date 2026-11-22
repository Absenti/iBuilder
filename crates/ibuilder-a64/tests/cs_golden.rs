//! Review r5 P10: an automatic gate on the Capstone text the rewriter reads (mnemonic, `op_str`, operands, groups, writeback, `regs_access()`).
//!
//! The expected dumps in `tests/fixtures/cs/*.capstone507.tsv` were written by the PYTHON `capstone` 5.0.7 the oracle used (`crates/ibuilder-rewrite/tools/cs_dump.py`, 2026-11-22), not by
//! this crate: the test is a parity check against the oracle, not only a regression guard. The inputs (`*.words`: little-endian (address, word) pairs, the `cs_probe uniq` / `fuzz` format) are
//!   * `synth.words`: every distinct text word of the synthetic fixtures `crates/ibuilder-rewrite/tests/fixtures/synth/*.nso` (`cs_probe uniq`, 951 words);
//!   * `fuzz.words`: `cs_probe fuzz 4000 12345 .../synth/tmpl_21.nso`: half uniform random words at random addresses, half one/two-bit flips of the fixture's words (4000 words, 1716 of
//!     them undecodable: the refusals are compared too).
//!
//! To replay against the Python: `python3 -I -B crates/ibuilder-rewrite/tools/cs_dump.py tests/fixtures/cs/<x>.words out.tsv && cmp out.tsv tests/fixtures/cs/<x>.capstone507.tsv`
//! (needs `pip install capstone==5.0.7`). The files hold no game code: synthetic fixtures and random words only.
use ibuilder_a64::cs::Decoder;
use std::path::Path;

fn check(name: &str) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cs");
    let words = std::fs::read(dir.join(format!("{name}.words"))).unwrap();
    let want = std::fs::read_to_string(dir.join(format!("{name}.capstone507.tsv"))).unwrap();
    let mut dec = Decoder::new(true);
    let mut got = String::new();
    for c in words.as_chunks::<8>().0 {
        let o = u32::from_le_bytes(c[..4].try_into().unwrap());
        let w = u32::from_le_bytes(c[4..].try_into().unwrap());
        got.push_str(&dec.probe_line(w, o));
        got.push('\n');
    }
    assert_eq!(
        got.lines().count(),
        words.len() / 8,
        "{name}: one line per word"
    );
    for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
        assert_eq!(
            g,
            w,
            "{name}: line {} differs from the Python capstone 5.0.7 dump",
            i + 1
        );
    }
    assert_eq!(
        got, want,
        "{name}: the dump differs from the Python capstone 5.0.7 dump"
    );
}

#[test]
fn the_synthetic_fixtures_decode_as_python_capstone_does() {
    check("synth");
}

#[test]
fn random_and_bit_flipped_words_decode_as_python_capstone_does() {
    check("fuzz");
}
