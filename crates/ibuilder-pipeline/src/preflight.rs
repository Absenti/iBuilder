//! Checks of a game's ExeFS BEFORE stage 2 spends time on it, and the hints that turn a rewriter's low-level failure into something a player can act on.
//!
//! Why this exists (device log, 2026-11-19): Mario Kart 8 Deluxe from a merged XCI failed at stage 2 with `KeyError: 2`. The package carries the 4.0.0 update beside the
//! base game; the extractor read the base game (1.0.0), whose `main` is a 32-bit AArch32 program (`main.npdm` Is64BitInstruction = 0). The rewriter (and the engine's host)
//! handle AArch64 only, so it tripped on the first thing it parses (the unwind table pointer encodings) and said nothing useful. A 32-bit program cannot run on an iPhone at all
//! (Apple's A-series cores have no AArch32), so the right answer is an early, plain refusal that says what the file is and what to try, not a parser error minutes later.
//!
//! * `check` looks at `main.npdm` (the process flags) and the modules' NSO headers (build ids against the profile's `build` rows) and returns findings; a `fatal` one stops the chain.
//! * `rewrite_hint` adds a sentence to a rewriter failure whose text is a known low-level signature (`KeyError: <n>` from the unwind table, a missing MOD0, ...).
//!
//! Nothing here changes a byte of any output: a game that passes is rewritten exactly as before.

use std::fs;
use std::path::Path;

/// One result of `check`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// the chain stops on a fatal finding; a warning is only logged
    pub fatal: bool,
    /// a short stable name (`aarch32`, `unknown-build`, ...), for tests and for the app to key on
    pub code: &'static str,
    pub text: String,
}

/// The process flags of a `main.npdm` (META): `(is_64bit_instruction, address_space_type)`; `None` when it is not an NPDM.
/// Offset 0x0C holds the flags: bit 0 Is64BitInstruction, bits 1..3 the address space (0: 32 bit, 1: 64 bit with the old layout, 2: 32 bit without the reserved region, 3: 64 bit without it).
pub fn npdm_flags(npdm: &[u8]) -> Option<(bool, u8)> {
    if npdm.len() < 0x10 || &npdm[..4] != b"META" {
        return None;
    }
    let f = npdm[0x0C];
    Some((f & 1 != 0, (f >> 1) & 7))
}

/// The first 16 bytes of an NSO's build id (header 0x40..0x50) as lower-case hex: what the profile's `build` rows list; `None` when it is not an NSO0.
pub fn nso_build_id(nso: &[u8]) -> Option<String> {
    if nso.len() < 0x50 || &nso[..4] != b"NSO0" {
        return None;
    }
    Some(nso[0x40..0x50].iter().map(|b| format!("{b:02x}")).collect())
}

/// The builds a game table knows: `(version label, [(module, build id)])` from its `build<TAB>v<N><TAB><label><TAB>main=<id> subsdk0=<id> ...` rows.
pub fn known_builds(game_tsv: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    for line in game_tsv.lines() {
        let Some(rest) = line.strip_prefix("build\t") else {
            continue;
        };
        let rest = rest.split('#').next().unwrap_or("");
        let mut cols = rest.split('\t');
        let (Some(v), Some(label)) = (cols.next(), cols.next()) else {
            continue;
        };
        let mods: Vec<(String, String)> = cols
            .flat_map(|c| c.split_whitespace())
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_ascii_lowercase()))
            .collect();
        out.push((format!("{label} ({v})"), mods));
    }
    out
}

/// `check(exe_dir, modules, game_tsv)`: the findings for the ExeFS in `exe_dir` (the game table's text is optional: without it the build ids are not compared).
pub fn check(exe_dir: &Path, modules: &[String], game_tsv: Option<&str>) -> Vec<Finding> {
    let mut out = Vec::new();
    let npdm = fs::read(exe_dir.join("main.npdm")).ok();
    let main_id = fs::read(exe_dir.join("main"))
        .ok()
        .and_then(|d| nso_build_id(&d));

    if let Some((is64, space)) = npdm.as_deref().and_then(npdm_flags)
        && !is64
    {
        let id = main_id
            .as_deref()
            .map(|i| format!(", main build id {i}"))
            .unwrap_or_default();
        out.push(Finding {
                fatal: true,
                code: "aarch32",
                text: format!(
                    "this game's code is a 32-bit (AArch32) program (main.npdm: Is64BitInstruction = 0, address space type {space}{id}); the rewriter and the engine's host handle AArch64 only, and Apple's A-series cores \
                     cannot run 32-bit ARM code at all. If this package also carries an update, that update was probably left out and only the base game was read: import the update as a file of its own \
                     (a newer build of the same title may be 64-bit); otherwise this title cannot be built here"
                ),
            });
        return out; // nothing after this is meaningful for a 32-bit program
    }

    if let (Some(tsv), Some(id)) = (game_tsv, main_id.as_deref()) {
        let builds = known_builds(tsv);
        if !builds.is_empty() {
            let known = builds.iter().any(|(_, mods)| {
                mods.iter().any(|(m, i)| {
                    m == "main" && i.starts_with(id)
                        || (m == "main" && id.starts_with(i.as_str()) && !i.is_empty())
                })
            });
            if !known && modules.iter().any(|m| m == "main") {
                let list = builds
                    .iter()
                    .map(|(label, _)| label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(Finding {
                    fatal: false,
                    code: "unknown-build",
                    text: format!(
                        "main's build id {id} is not one of the builds this game's profile knows ({list}): this is another version (the base game without its update, or a newer update); the profile's \
                         build-specific settings and patches may not apply to it"
                    ),
                });
            }
        }
    }
    out
}

/// A sentence for a rewriter failure whose text is a known low-level signature, or `None`.
pub fn rewrite_hint(error: &str) -> Option<&'static str> {
    let e = error.trim();
    if e.starts_with("KeyError:") {
        return Some(
            "the module's unwind table (eh_frame_hdr) is not in the form the rewriter reads: it is not a standard 64-bit module of a supported build (a 32-bit program, a damaged file, or an unsupported build)",
        );
    }
    if e.contains("== b'MOD0'") {
        return Some(
            "the module has no MOD0 header where it says it does: the file is damaged or is not an NSO of a supported system version",
        );
    }
    if e.contains("not an NSO or NRO") {
        return Some(
            "a module of the ExeFS is not an NSO: the extraction may be damaged; extract the game again",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npdm(flags: u8) -> Vec<u8> {
        let mut v = vec![0u8; 0x80];
        v[..4].copy_from_slice(b"META");
        v[0x0C] = flags;
        v
    }

    fn nso(id: [u8; 16]) -> Vec<u8> {
        let mut v = vec![0u8; 0x100];
        v[..4].copy_from_slice(b"NSO0");
        v[0x40..0x50].copy_from_slice(&id);
        v
    }

    fn dir(files: &[(&str, Vec<u8>)]) -> tempdir::T {
        let d = tempdir::T::new();
        for (n, b) in files {
            fs::write(d.path().join(n), b).unwrap();
        }
        d
    }

    /// a tiny temp dir without a dependency
    mod tempdir {
        use std::path::{Path, PathBuf};
        pub struct T(PathBuf);
        impl T {
            pub fn new() -> T {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let p = std::env::temp_dir().join(format!(
                    "ibuilder-preflight-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                ));
                std::fs::create_dir_all(&p).unwrap();
                T(p)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for T {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn npdm_flags_read_the_instruction_set_and_the_address_space() {
        assert_eq!(npdm_flags(&npdm(0x4)), Some((false, 2))); // Mario Kart 8 Deluxe 1.0.0: 32-bit, address space 2
        assert_eq!(npdm_flags(&npdm(0x3)), Some((true, 1)));
        assert_eq!(npdm_flags(&npdm(0x7)), Some((true, 3)));
        assert_eq!(npdm_flags(b"NOPE0000000000000000"), None);
        assert_eq!(npdm_flags(&[0u8; 4]), None);
    }

    #[test]
    fn a_32_bit_program_is_refused_at_once_with_a_plain_message() {
        let d = dir(&[
            ("main.npdm", npdm(0x4)),
            (
                "main",
                nso([
                    0xb5, 0xc3, 0x9b, 0x5b, 0x7c, 0x62, 0xa8, 0x8e, 0xc2, 0x5f, 0xa8, 0x97, 0x85,
                    0x8c, 0x87, 0x4e,
                ]),
            ),
        ]);
        let f = check(d.path(), &["main".into()], None);
        assert_eq!(f.len(), 1);
        assert!(f[0].fatal && f[0].code == "aarch32");
        assert!(
            f[0].text.contains("32-bit (AArch32)")
                && f[0].text.contains("b5c39b5b7c62a88ec25fa897858c874e")
                && f[0].text.contains("update"),
            "{}",
            f[0].text
        );
    }

    #[test]
    fn a_64_bit_program_passes_and_an_unknown_build_is_only_a_warning() {
        let d = dir(&[
            ("main.npdm", npdm(0x3)),
            (
                "main",
                nso([
                    0x2c, 0x33, 0x6a, 0x9b, 0xcf, 0x79, 0xc3, 0x04, 0x0c, 0xe5, 0x06, 0xcd, 0xd3,
                    0x91, 0xb5, 0x78,
                ]),
            ),
        ]);
        let tsv = "build\tv1441792\t4.0.0\tmain=2c336a9bcf79c3040ce506cdd391b578 sdk=ba520906609c503227871a314a94fb5f\nbuild\tv1507328\t4.0.1\tmain=158c0c7399aabd7bf83795c2ef3484c7\n";
        assert!(
            check(d.path(), &["main".into()], Some(tsv)).is_empty(),
            "the registered 4.0.0 main is known"
        );
        let other = dir(&[("main.npdm", npdm(0x3)), ("main", nso([9; 16]))]);
        let f = check(other.path(), &["main".into()], Some(tsv));
        assert_eq!(f.len(), 1);
        assert!(
            !f[0].fatal && f[0].code == "unknown-build" && f[0].text.contains("4.0.0 (v1441792)"),
            "{}",
            f[0].text
        );
        // a table with no build rows, or no table, never warns
        assert!(check(other.path(), &["main".into()], Some("name\tX\n")).is_empty());
        assert!(check(other.path(), &["main".into()], None).is_empty());
    }

    #[test]
    fn missing_files_never_make_a_finding() {
        let d = dir(&[]);
        assert!(check(d.path(), &["main".into()], Some("build\tv1\tx\tmain=00\n")).is_empty());
    }

    #[test]
    fn known_low_level_failures_get_a_hint_and_others_do_not() {
        assert!(
            rewrite_hint("KeyError: 2")
                .unwrap()
                .contains("unwind table")
        );
        assert!(
            rewrite_hint("assert IMG[m0:m0 + 4] == b'MOD0'")
                .unwrap()
                .contains("MOD0")
        );
        assert!(
            rewrite_hint("main: not an NSO or NRO (magic b'\\x00')")
                .unwrap()
                .contains("extract the game again")
        );
        assert_eq!(
            rewrite_hint("no padding word for a hop from 0x1 to 0x2"),
            None
        );
    }

    /// the real Mario Kart 8 Deluxe 1.0.0 files of the 2026-11-19 device log (kept out of the repository: used when present)
    #[test]
    fn the_real_mario_kart_base_game_is_refused() {
        let Ok(base) = std::env::var("PREFLIGHT_MK8D_EXEFS") else {
            ibuilder_testkit::skip(
                "set PREFLIGHT_MK8D_EXEFS to the Mario Kart 8 Deluxe 1.0.0 ExeFS folder",
            );
            return;
        };
        let f = check(Path::new(&base), &["main".into()], None);
        assert!(f.iter().any(|x| x.fatal && x.code == "aarch32"), "{f:?}");
    }
}
