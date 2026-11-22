//! The per-shader digest: the key of the translation cache (`dmsh_build.shader_key`, `pair_keyed`) as this crate defines it.
//!
//! **Definition.** The Python hashes the text of its own source files into every key (`CODE_FP`, `TOOL_FP` through `dmsh_chain`), so a key made here can never equal one of its keys, and
//! none is meant to: what has to be identical is the *output*, which the tests prove file by file. Here the digest is the sha256 of, in this order, each part preceded by its tag and a
//! NUL: the cache format and this crate's version (`CODE_FP`), the chain id the host takes (it hashes the engine's tool sources and the translation options), the finish code's
//! fingerprint (`Finisher::fingerprint`), what the tools say they are (`spirv-cross --version`), whether the pulling libraries are built, the translator's options, the package name, the shader's
//! record bytes, its row of the shader map and of the conflict table, its row of the answer table (compact, key-sorted JSON), the profile's `nvn_*` / `shader_*` rows, its pairs
//! (a vertex shader), whether it is in `dual_want` / `bf_want`. A vertex shader with pairs then takes its pixel shaders' digests too (`pair_keyed`), since `vs_pad` reads their MSL.
//!
//! **Nothing to bump by hand.** `CODE_FP` carries a hash of the sources that decide an output (`build.rs`: this crate's `src/*.rs`, `ibuilder-shader`'s `src/**`), as the Python's `CODE_FP` hashed its
//! own; a vendored crate without `ibuilder-shader` beside it falls back to the crate version (`IBSB_SRC_FP=none`), and then a release that changes outputs must change a version. `Finisher::fingerprint`
//! is for finishers that are not in those sources (a test finisher, a future out-of-tree one).
use crate::game::Game;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Cache format, the crate version and a hash of the sources that decide an output (this crate's `src/*.rs` and `ibuilder-shader`'s `src/**`, computed by `build.rs`): an edit to the finish
/// code or to the driver gives every shader a new digest, so no entry made by other code can be served. Bump the format number when the entry layout changes without a source change that
/// would show (a dependency that changes behaviour).
pub const CODE_FP: &str = concat!(
    "ibuilder-shaderbuild cas 1 / ",
    env!("CARGO_PKG_VERSION"),
    " / ",
    env!("IBSB_SRC_FP")
);

pub fn hex_of(d: &[u8]) -> String {
    crate::records::hex(d)
}

/// What every shader's digest shares
pub struct DigestBase {
    prefix: Sha256,
    pub profile_rows: Vec<u8>,
}

impl DigestBase {
    pub fn new(
        chain: &str,
        finisher_fp: &str,
        tools_fp: &str,
        pulling: bool,
        xlate: &crate::jobs::XlateOpts,
        game: &Game,
    ) -> DigestBase {
        let mut h = Sha256::new();
        for (tag, v) in [
            ("code", CODE_FP.to_string()),
            ("chain", chain.to_string()),
            ("finish", finisher_fp.to_string()),
            ("tools", tools_fp.to_string()),
            ("pulling", (pulling as u8).to_string()),
            (
                "xlate",
                format!("debug={} reduced={}", xlate.debug, xlate.reduced),
            ),
        ] {
            h.update(tag.as_bytes());
            h.update([0u8]);
            h.update(v.as_bytes());
            h.update([0u8]);
        }
        DigestBase {
            prefix: h,
            profile_rows: game.profile_rows(),
        }
    }

    /// `shader_key(key, name, rec)`
    #[allow(clippy::too_many_arguments)]
    pub fn shader_digest(
        &self,
        name: &str,
        rec: &[u8],
        map_row: Option<&[u8]>,
        conflict_row: Option<&[u8]>,
        facts: Option<(&str, &Value)>,
        pairs: Option<&BTreeSet<String>>,
        dual: bool,
        bf_want: bool,
    ) -> String {
        let mut h = self.prefix.clone();
        let part = |h: &mut Sha256, tag: &str, v: &[u8]| {
            h.update(tag.as_bytes());
            h.update([0u8]);
            h.update(v);
            h.update([0u8]);
        };
        part(&mut h, "name", name.as_bytes());
        part(&mut h, "record", rec);
        part(&mut h, "map", map_row.unwrap_or(b"missing"));
        part(&mut h, "conflict", conflict_row.unwrap_or(b""));
        let facts_row = match facts {
            Some((k, v)) => format!(
                "{k}\t{}",
                crate::pyjson::dumps(v, &crate::pyjson::Opts::COMPACT_SORTED)
            )
            .into_bytes(),
            None => b"missing".to_vec(),
        };
        part(&mut h, "facts", &facts_row);
        part(&mut h, "profile", &self.profile_rows);
        let pairs_text = pairs
            .map(|p| p.iter().cloned().collect::<Vec<_>>().join(","))
            .unwrap_or_default();
        part(&mut h, "pairs", pairs_text.as_bytes());
        part(&mut h, "dual", if dual { b"1" } else { b"0" });
        part(&mut h, "bf", if bf_want { b"1" } else { b"0" });
        hex_of(&h.finalize())
    }
}

/// `pair_keyed(digest, pixel_digests)`: a vertex shader's digest with its pixel shaders' digests (`vs_pad`)
pub fn pair_keyed(digest: &str, pixel_digests: &[String]) -> String {
    let mut h = Sha256::new();
    h.update(digest.as_bytes());
    h.update([0u8]);
    h.update(pixel_digests.join(",").as_bytes());
    hex_of(&h.finalize())
}
