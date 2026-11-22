# ibuilder-pack: the `.nxgame` packager and manifest comparator

Port of `iOSBuilder/tools/pack_nxgame.py` (489 lines) and `tools/compare_nxgame.py` (156 lines); main and the worktree copies
are byte-identical, ported either. Format: `iOSBuilder/docs/PACKAGE.md`. Crate: `crates/ibuilder-pack` (deps: `sha2`, `rayon`,
`libc`; `#![deny(unsafe_code)]` with one justified `unsafe` call, see below; no process is spawned).

A `.nxgame` is `<title ID>.nxgame/` holding `DmGame<title ID>.framework/` (executable, `game.manifest.json`, `profile.tsv`, `dm/`,
`msl/b-<chain>.img`, `icon.png`, `Info.plist`), `romfs.img`, `aoc/`, and `package.json` listing every framework file with its
SHA-256. The manifest is written like the engine's `gen_manifest.py` (`json.dumps(indent=2, ensure_ascii=False) + "\n"`), `package.json`
like the Mac app's Swift `JSONEncoder` (`indent=2, sort_keys, separators=(',', ' : ')`, **no** trailing newline).

## Source -> module

| Python | Rust |
|---|---|
| `read_profile`, `console_of`, `nro_count` | `src/profile.rs` |
| `romfs_identity_of_image`, `romfs_identity_short` (FNV-1a 64 over header + index) | `src/romfs_id.rs` |
| `build_manifest`, `write_manifest`, `framework_files`, `aoc_bytes`, `copy_file`, `main` | `src/pack.rs` (`pack`, `PackArgs`, `PackOutcome`) |
| `json.dumps` / `json.loads` spellings | `src/json.rs` (`Json`, `Dump::{compact, indented, swift_pretty}`, `loads`, `float_repr`) |
| `--clone` (`cp -c` through `subprocess`) | `src/clone.rs` (`libc::clonefile(2)`, the only `unsafe`; falls back to a copy with the same `[warn]`) |
| Python `str` semantics (`strip`, `split`, `splitlines`) | `src/pyutil.rs` |
| `compare_nxgame.py` (`find_manifest`, `compare`, `main`, `--json-out`) | `src/compare.rs` |
| the two `__main__`s | `src/bin/ibuilder-pack.rs`: subcommands `pack` and `compare` |

Why hand-rolled JSON (not `serde_json`): the files are compared byte for byte with Python's output, which needs insertion-ordered objects
and Python's spellings (`™` vs literal under `ensure_ascii`, `1e+16`, `\u007f`, `" : "`); `serde_json` without `preserve_order` sorts keys.
No dependency beyond the allow-list. One detail that matters and is tested: `package.json`'s `files` are sorted **component-wise**
(Python sorts `Path` objects, so `dm/a` sorts before `dm-x` and `dm.txt`; a string sort gives the opposite order).

## How to run

```
cargo build --release -p ibuilder-pack
ibuilder-pack pack --out <title>.nxgame --profile profile.tsv --framework <exe|dir> --dm-dir <dir> --msl b-<chain>.img \
      --icon icon.png --romfs-img RomFS.img --aoc <dir> --nro-table nro.tsv --package-version v0 [--clone] [--force] [--json] ...
ibuilder-pack pack --manifest-only --out <dir|file.json> --profile profile.tsv --package-version v0 --chain b-xxxx [--romfs-img ..]
ibuilder-pack compare <reference> <candidate> [--reason KEY=TEXT]... [--json-out FILE] [--label TEXT]
```
Same options, defaults, refusals (`[RED] pack_nxgame: ...`, exit 1) and report lines as the Python. Differences: `--opt value` / `--opt=value`
only (no argparse prefix abbreviations); raw OS errors print Rust messages instead of Python tracebacks (exit status 1 in both).
Library: `ibuilder_pack::{pack, PackArgs, compare, compare_files_cli, romfs_identity_of_image}`.

## Verification (commands and results, this Mac, 2026-11-19)

`cargo test --release -p ibuilder-pack`: 6 unit tests (JSON spellings incl. Python float repr and astral escapes, `splitlines`, RomFS short id) and 6
oracle tests (`tests/oracle.rs`), all pass. Each oracle case runs the Python and the Rust packer on the same inputs and requires identical
**output trees (every file, byte for byte)**, identical stdout (including `--json` summaries) and identical refusal messages / exit status:

* 10 success cases on a synthetic tree (full, manifest-only with and without `--romfs-img`, Switch 2 profile, `--no-console`, no profile with every value
  given, framework as a directory, `--framework-name`, `--aoc` holding only Finder junk, `--clone`) and 11 refusal cases (no package version, no/bad chain,
  bad `--game`, no name, underscore module, no `--framework`, missing framework, a non-RomFS image, bad `--extra`, a folder without the executable):
  `OK` for all 21 (`cargo test ... --test oracle -- --nocapture`). The inputs include a `™`/`"`/`\`/`é` name (ensure_ascii off), `.DS_Store` / `._*`
  files (listed nowhere, copied by `copytree`), and sort-order traps (`dm/`, `dm-x`, `dm.txt`).
* `compare`: 8 candidate manifests (same, value, list, missing key, extra key, type mismatch incl. `true` vs `1`, key order, `0` vs `0.0`) x 2 reason sets
  (none / named): stdout, exit code and `--json-out` file identical to `compare_nxgame.py`.
* `json.dumps` in 4 configurations on 5 documents (nested, big ints, floats, escapes, astral code points): identical.
* **The real Grizzy package** (`iOSBuilder/out-pkg/0100DFA027504000.nxgame`, written by the Python on 2026-11-16 from
  `out-asm/DmGameProbe.framework/DmGameProbe` + its own profile / `dm/` / icon + the 2,498,033,693-byte `romfs.img`) re-packed from its own
  pieces with `--clone`: `package.json` equals the original byte for byte, the whole framework tree (24 files) equals the original, and the
  `romfs_identity` is `DMROMFSID1 title=0100DFA027504000 version=1.0.0 build=v0 size=2498033693 index=16253e096bc20e18`, the line `docs/PACKAGE.md`
  section 4.0 prints. The Python and Rust trees of that run are also identical (`diff -rq`).
* The three `*.nxgame` references of iOSBuilder's `docs/PACKAGE.md` are **not on the machine this was written on**, so the Grizzy package above is the only real `.nxgame` oracle. The Swift `PackageVerifier` (`tools/verify_package_swift.swift`)
  was not run.

## Numbers (indicative: shared Mac, `/usr/bin/time -l`, median of 5, outputs identical)

| run | Python | Rust |
|---|---|---|
| 191 MB framework + 19 `dm/` tables + icon, real copy, hash all 24 files | 0.18 s, 25.9 MiB RSS | 0.12 s, 6.1 MiB RSS |
| same + 2.5 GB `romfs.img` via APFS clone (`--clone`) | 0.19 s, 28.3 MiB | 0.11 s, 6.0 MiB |

Most of the time is the 191 MB copy and its SHA-256 (hashed in parallel across files, one file at a time per thread).

## iOS portability

Builds for `aarch64-apple-ios` and `aarch64-apple-ios-sim`. Run inside the booted iPhone 17 simulator
(`xcrun simctl spawn <SIMULATOR-UDID> .../ibuilder-pack pack ...` with absolute paths) the full synthetic case produced a tree
**byte-identical** to the host run's (`diff -r`), and `--clone` ran. `clonefile` exists on iOS (APFS) and is cfg-gated to Apple targets with a
copy fallback elsewhere. Simulator timings are the Mac's CPU. Not run on a physical iPhone.

## Not done / not verified

* JSON: integers beyond 128 bits are refused (Python is unbounded); a lone surrogate escape in input JSON becomes U+FFFD (a Rust `String` cannot hold one);
  `NaN`/`Infinity` parse but no manifest contains them. Python's `$` accepting a trailing newline in the title/chain regexes is not reproduced.
* `copytree` follows symlinks like the Python's default (`symlinks=False`) but directory metadata (mtimes, modes of directories) is not copied; file modes are.
* The Swift verifier and a phone import of the produced package were not run.
