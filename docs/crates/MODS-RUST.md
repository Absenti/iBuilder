# `ibuilder-mods`: Atmosphere-style mods and cheats in Rust

Port of `gameapp/mods.py` (+ `test_mods.py`) of the iOSBuilder worktree `$IOSBUILDER_WORKTREE`, and the hooks `gameapp_pipeline.py` calls it from (`docs/chain/PIPELINE-RUST.md`, "Delta since first port").
Library crate `crates/ibuilder-mods` (`#![forbid(unsafe_code)]`, only `sha2` besides `std`, no process API: it builds for `aarch64-apple-ios` and `-ios-sim`); a Mac-only differential driver
`src/bin/mods_diff.rs` behind feature `diff` (adds `serde_json`); the harness in `crates/ibuilder-mods/tools/diff/`.

## What it does (Python function -> Rust)

The app's importer (C#, UI: not ported) keeps what belongs to a game in `Documents/games/<id>/mods/<mod>/{romfs,exefs,cheats}` and the player's choices in `Documents/games/<id>/mods.tsv`.

| `mods.py` | Rust (`ibuilder_mods::`) |
|---|---|
| `read_state` (mods.tsv: `mod` / `patch` / `cheat` rows), `list_mods`, `enabled_mods`, `_files` (`os.walk` semantics), `_sha`, `exefs_signature` | `store.rs` |
| `parse_ips` (IPS, IPS32, RLE, truncated = `ModError`), `Patch` (+ `digest`), `parse_pchtxt` (`@nsobid-`, `@flag offset_shift`, `@enabled/@disabled`, `@stop`, `//` and `/* */`, quoted string refused, implicit block), `bid_matches`, `stem_bid`, `exefs_patches` | `patch.rs` |
| `lz4_block`, `nso_build_id`, `nso_image`, `apply_writes`, `build_nso` (stored plain, flags `& ~0xBF`, hashes), `patch_nso` | `nso.rs`, `lz4.rs` |
| `record_build_ids`, `plan`, `apply_module`, `remove_patched`, `mark_applied` | `chain.rs` (`ModPlan`, `PlanEntry`) |
| `cheat_sections`, `cheat_file_bid`, `write_cheats_folder` (`cheats-run/<bid>.{txt,enabled,origin.tsv}`), `romfs_overlays`, `apply_boot` (`DM_ROMFS_OVERLAY`, `DM_CHEATS`, a switch the environment names wins) | `boot.rs` (`Environ` trait) |
| the Python `str` / `int` / `repr` / `OSError` texts the logs and branches depend on (`str.splitlines`, `isspace`, `int(x, 0)`, `repr`, `[Errno 2] ...: 'path'`, the `$` of `re` before a trailing newline) | `pyfmt.rs`, `pytables.rs` (generated from CPython 3.14.1, Unicode 16.0) |

Offsets: the 0x100 header rule (`module offset = ips offset - 0x100`; a pchtxt address + `offset_shift` is such an IPS offset). Offsets are Python ints: `i128` saturated at +-2^100, so
"outside every segment" and "not a valid `struct` field" come out as with unbounded integers.

**Errors are the Python's.** `ModsError::Mod` is `ModError` (caught where the Python catches it: the file is skipped and said, or the module "left as dumped"); `Os` is an `OSError` with Python's text;
`Unhandled { exc, msg }` is an exception `mods.py` does *not* catch and that therefore propagates to the chain. They are kept as they are, not "fixed" (byte/behaviour identity first):

* `Patch.digest()` packs `struct.pack('<QI', off, len)`: an offset below 0 or above 2^64 is a `struct.error`. A matching IPS that writes into the NSO header (IPS offset < 0x100), or a pchtxt address < 0x100 with no
  `offset_shift`, makes `plan()` raise: **stage 2 fails** ("mods: error: argument out of range") instead of skipping that mod. (A Python-side fix would be to catch it in `exefs_patches`.)
* a pchtxt address such as `0x0x12` passes the `HEX` test after removing every `0x` and then `int(addr, 16)` raises `ValueError`; `mods.tsv` that is not UTF-8 raises `UnicodeDecodeError`; a non-ASCII module name raises
  `UnicodeEncodeError` in `record_build_ids` (`encoding='ascii'`); a damaged LZ4 segment raises `LZ4BlockError` (not caught by `apply_module`); `patch_nso` on a non-NSO with an enabled patch raises `AttributeError` (`None.hex()`).
  `apply_mods` (the boot hook) catches all of them: "mods: not applied (UnicodeDecodeError: ...) -- the game starts without them".
* **ZBIC (NSO flag bit 7, system 22.0.0+) is not ported** (no ZBIC decoder in Rust anywhere in this repo): `ModsError::Unported`, never swallowed into "left as dumped". `plan` only reads headers, so it works for such modules; `apply_module` fails explicitly.

## Verification (exact commands and results; this Mac, 2026-11-19)

Python 3.14.1 + `lz4` 4.4.5 (`$SCRATCH_OLD/ref/venv`), a COPY of `mods.py`, `PYTHONDONTWRITEBYTECODE=1`, `python -I`.

1. **`test_mods.py` ported**: all 64 checks. 55 in `crates/ibuilder-mods/tests/mods_spec.rs` (each carries its Python check name in a `// py:` comment), 8 through the pipeline
   (`rewrite_key`, `do_rewrite` x4, `rewrite_is_cached` x2, `apply_mods`: `crates/ibuilder-pipeline/tests/delta_spec.rs` with a stand-in `ModsHost`, and `crates/ibuilder-cli/tests/mods_chain.rs` with the real
   `RustMods` + `ibuilder-mods`), 1 (the engine's own `nso.load` of the patched NSO) as a cross-check with the Rust port of `nso.py` (`ibuilder-nso`).
   `cargo test --release -p ibuilder-mods -p ibuilder-pipeline -p ibuilder-cli` -> all pass (mods: 5 unit + 7 spec; delta_spec 10; mods_chain 4).
2. **Differential harness** (`crates/ibuilder-mods/tools/diff/run.sh <scratch> <python> <mods.py copy> 300 "11 12 13"`): per seed, 300 randomized scenarios (synthetic mod folders: IPS/IPS32 valid, cut, no end, wrong
   header, junk; pchtxt with good and bad flags, comments, `\r\n` / `\r` / U+2028 line ends, quoted strings, `0x0x12`; cheat files with duplicate sections, bad UTF-8; romfs; hidden files; a symlink; mods.tsv with CRLF, junk,
   bad UTF-8; synthetic NSOs, LZ4-compressed or plain, damaged, truncated, not NSO, a directory, missing; tid empty) each run through 13 steps (`list, state, patches, record, plan, apply, mark, signature, boot, cheats,
   plan(quiet), remove, signature`), comparing every step's result, **every log line** and the **final file tree (sha256 of every file)**; 13,500 micro operations (IPS, pchtxt, `repr`, `bid_matches`, `stem_bid`,
   `cheat_sections`, `nso_image`, `patch_nso` with negative/huge/boundary offsets); 40,000 LZ4 streams (valid at several sizes, mutated, truncated, extended, garbage; sizes true/smaller/larger).
   Result on the final code, seeds 21 and 22: **2 x (300 scenarios = 3,900 steps, 13,500 micro operations, 40,000 LZ4 streams): 0 differences**. Exceptions the Python raised, matched in class and (for `ModError` /
   `OSError`) message: scenarios `struct.error` 68, `ValueError` 48, `UnicodeDecodeError` 36, `UnicodeEncodeError` 6, `LZ4BlockError` 2; micro operations `ModError` 2,271, `AttributeError` 166, `ValueError` 116,
   `LZ4BlockError` 11. Before the last two small optimizations (one buffer in `build_nso`, `extend_from_within` for non-overlapping LZ4 matches) seeds 11, 12 and 13 (3 x the same sizes) also gave 0 differences, and the
   development runs before them (scenario seeds 1 and 7: 300 and 600 cases; micro seed 5) too. The comparator is checked to fail on a tampered result. The LZ4 decoder needed two fixes found by this fuzz (the reference
   decoder's "two-stage shortcut", offset 0); **9 of the 200,000 LZ4 streams of the two runs have an offset of 0**: the reference decoder then copies bytes of its output buffer it never wrote (undefined; zeros in
   practice), this port calls them corrupt and the comparator counts them apart.
   **Real size**: Grizzy's `main` (75,035,192 B, LZ4) patched with an IPS (read-only input, outputs in the scratch folder): the 165,679,076-byte patched copy is byte-identical to the Python's, plan digest and log equal;
   Rust 0.36-0.62 s wall, 408 MB max RSS; Python 0.31 s, 1.0 GB (shared loaded Mac: indicative; memory is the number that matters on the phone).
3. **iOS**: `PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-mods` (and `aarch64-apple-ios`, and `-p ibuilder-pipeline`): Finished. The `mods_diff` simulator binary
   (platform IOSSIMULATOR, minos 14.0) ran in the booted iPhone 17 simulator (`xcrun simctl spawn <SIMULATOR-UDID> mods_diff ops.json out.json`): 300 scenarios (seed 41) with results, logs and final file trees, and 13,500 micro operations (byte-identical output file), identical to the Mac binary
   (which equals the Python), on the final code. `ibuilder-ffi --features real-stages` (mods wired through `ibuilder-cli`) builds for `-ios-sim`; its 29 exported symbols are unchanged (the C ABI did not change: mods need no new entry point,
   the store is files and the C# importer keeps writing them).

## Differences from the Python and what is NOT done

* ZBIC modules (above). Python's `lz4` is used by the real app interpreter (`import lz4.block`); this crate follows `LZ4_decompress_safe` (the same as `lz4.block.decompress(raw, uncompressed_size=n)`), not the pure-Python fallback decoder
  of `mods.py` (which accepts other malformed streams: only used when `lz4` is missing).
* File names that are not UTF-8 (APFS has none) are skipped, Python would carry surrogate-escaped names. `int(x, 0)` for `offset_shift` does not accept Unicode decimal digits (Arabic-Indic digits are valid in Python).
  `repr` uses a table of CPython 3.14.1's non-printable code points: a newer Unicode may differ in a log text only. The `UnicodeDecodeError` text uses absolute byte positions (Python reports them relative to the
  8 KiB chunk its text reader decodes: identical below 8 KiB).
* Not ported: the C# importer (zip / folder layouts, UI), `GameMods.ExefsSignature` (C#; the Rust `exefs_signature` computes the same text, held by `mods_spec`), the in-game cheat menu.
* Not verified: a ZBIC module; a real game's mod set (none available: only synthetic mods); on a physical iPhone.
