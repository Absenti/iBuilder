# NSO rewriter (`ibuilder-rewrite`, `ibuilder-a64`, `ibuilder-nso`)

Rust port of the engine's NSO patcher (`patch_nso.py` and the modules it imports; external, read-only). Given a
Switch NSO module it rewrites the code so it runs under the iOS loader (guest register x18 uses, exclusive
monitors, `mrs`/`svc` sites, jump tables, per-game frame-rate and screen patches) and writes the patched image plus
side tables. The target is byte-identical output to the Python oracle, then speed and memory. No embedded
interpreter and no process spawning.

## Crates and module map

| Python source | Rust |
| --- | --- |
| `nso.py` (LZ4 segments, SHA-256 checks, NRO; ZBIC = flag bit 7) | `ibuilder-nso/src/lib.rs`, `zbic.rs` (own ZBIC/zstd decoder; ZBIC-compressed NRO is an error) |
| `nso_asm.py` | `ibuilder-a64/src/asm.rs` (same asserts as `Err(AsmError)`) |
| Capstone Python binding | `ibuilder-a64/src/cs.rs` (safe wrapper over `capstone-sys`, `arch_arm64` only) |
| `patch_nso.py` driver, layouts (FAR / BIG_TEXT / TWO / GAP), outputs | `rewriter.rs`, `output.rs` |
| linear scan, exclusive ordering, per-function loop | `scan.rs`; per-site rewrites `sites.rs`; windows/retry blocks `windows.rs` |
| `jump_solver.py` (offset jump tables by dataflow) | `jump_solver.rs`, `pyset.rs` (CPython int-set iteration order, which the work list follows) |
| tail calls, far jumps | `jumps.rs`; spin waits `spin.rs`; conditional select with x18 `select.rs` |
| `nso_code.py`, `nso_syms`, `loader_census` | `fde.rs`; `nso_match.py` -> `matching.rs` |
| detectors (UE cvar/fps/malloc/ring/region, Unity fps/bucket, Monolith, Snowdrop) | `detect/*.rs` |
| `sdk_patches.py`, `engine_patches.py`, `games/*/patches.py` | `tables.rs` (generated), `patches.rs` (registry), `stubs.rs`, `stubs_games/<game>.rs` |
| game profile `game.tsv` rows | `frame.rs` |

Python `re` patterns over Capstone operand text (tail-call recognisers) are kept verbatim through the `regex` crate;
hot scans use word-level tests instead.

## Run

```sh
cargo build --release -p ibuilder-rewrite
ibuilder-rewrite [--threads N] [--game ID] [--games-dir DIR] [--py-tools DIR] [--spin-list] <module> <out dir> <name>
ibuilder-rewrite --list-ported
```

Environment equivalents: `DM_REWRITE_J`, `DM_GAME`, `DM_GAMES_DIR`, `DM_SPIN_LIST`. The log lines the Python prints go
to stdout; failures go to stderr with exit 1. `IBR_TIMING=1` prints per-phase times. `--py-tools` additionally writes
`build_src.sha256` (hashes of the Python sources of the oracle copy; only meaningful against that folder).
Output files: `<name>.patched.bin` plus `fixups/sites/gates/cuts/obs/changed/match.tsv`. The Python's `os.execve`
re-run (patch area too large for the gap before the base) is a loop inside `rewrite()`.

Library: `ibuilder_rewrite::rewrite(&file, name, name, &Options)`; `ibuilder_nso::{load, read}`; `ibuilder_a64::{asm, cs}`.

## Per-game patches

The Python loads `games/<id>/patches.py` dynamically. Rust uses a static registry instead:

* `tools/gen_patch_tables.py <engine>/dm [--coverage coverage.tsv] > src/tables.rs` reads the engine's patch files once at
  development time and emits all data (`CUTS`, `OBS`, `CAPABILITIES`, `MATCH_CUTS`, build IDs, match rules) plus a code
  digest per patch file that has code-valued tables.
* Stub functions are code and are ported by hand, one file per game in `stubs_games/` (13 games). Each `GamePort`
  records the digest it was verified against. A registration with no port, or an older digest, stops the rewrite with an
  "update the app" message; it never rewrites silently without the stub.
* `scripts/check-patch-coverage.sh [<engine>/dm]` regenerates tables and coverage and fails listing every build or
  rule the Rust cannot rewrite; it prints `SKIP` and exits 0 without an engine tree. `tests/patch_coverage.rs` runs it.
* A game with no `patches.py` gets no registered cuts; engine-wide `MATCH_CUTS` that miss are reported in
  `<name>.match.tsv` and the log.
* After an engine update: regenerate the tables, port what the script lists, check with `tools/stubtest.py --export`
  against the engine's Python, put the new digest in the `GamePort`.

## Design points

* Memory: the image is allocated once, LZ4 segments decompress straight into it, the input is memory-mapped,
  the original text is not copied (a log of changed words replaces it), functions are decoded one at a time and dropped,
  and the patch area is written straight to the output.
* The x18 scan only asks Capstone about words whose register fields can name x18/w18 (a superset, tested against
  Capstone on millions of words).
* Parallelism (rayon) is read-only and results are keyed by address, so output does not depend on `--threads`.
* `jump_solver.rs` follows the shipped engine: where the Python stops with an assertion (e.g. a 32-bit guard on a 64-bit
  index), so does the port.
* `unsafe` is confined to `cs.rs` (FFI) and the memory map in `ibuilder_nso::read`. Builds for `aarch64-apple-ios`
  and `aarch64-apple-ios-sim`.

## Verification

```sh
cargo test --release -p ibuilder-a64 -p ibuilder-nso -p ibuilder-rewrite
```

Fixture-dependent tests skip (with a message) without game files or the engine tree:

* `tests/oracle.rs`, `tests/grizzy.rs`, `tests/ocarina.rs`: real game modules against the oracle's file hashes and log
  (needs the extracted game modules). Output identical at 1, 2 and 8 threads.
* `tests/synthetic.rs`: 21 synthetic modules with the oracle's hashes in `tests/fixtures/synth/manifest.tsv`
  (no game code; rebuilt by `tools/export_fixtures.py` + `tools/synth_manifest.py`). 17 are rewritten file for
  file; 4 stop with the Python's message.
* `tests/select.rs` (CSEL/x18 fixtures), `stubs_games::difftest` (84 stub cases: build-keyed live-compared in
  development, matched rules as fixtures), `tests/damaged_module.rs`, `tests/patch_coverage.rs`.
* `ibuilder-a64` tests: encoders against values generated by `tools/gen_asm_tests.py`; `tests/cs_golden.rs` and
  `examples/cs_probe.rs` compare Capstone decoding with the Python binding (bundled Capstone 5.0.6 matches 5.0.7
  on every tested word).
* `ibuilder-nso/tests/zbic.rs`, `zstd_machinery.rs`: ZBIC/zstd decoder.
* Development-only differential fuzzers against the Python (not cargo tests): `tools/difftest.py`, `jsfuzz.py`,
  `selecttest.py`, `stubtest.py`, `bigtext.py`.

Oracle comparisons were run on Grizzy and Ocarina of Time (Switch 2) modules; everything else is covered by the
synthetic and fuzz tests, not by game files.

## Performance

Measured in November 2026 on an M1 Pro (8 cores, shared machine, indicative): Grizzy `main` (75 MB) takes about
2.5 s at 8 threads and 7.5 s at 1 thread, with about 350 MiB peak footprint; `sdk` and `subsdk0` take
under 2 s. The Python needs about 200 s serial and 4 GB for the same module. Most remaining time is Capstone.

## Known limits

* Layouts needing text beyond 128 MB (hops / GAP / TWO) are not checked against an oracle.
* The jump solver and landing pads on real exception tables are verified only on the ten solved functions of the two
  available games and on synthetic code.
* ZBIC-compressed NRO modules are refused.
* `nso_match.validate` is not ported; patch files outside the repository are not loaded (static registry).
* Python exceptions on odd operand shapes (a `ValueError` from `int()` on a shifted compare, negative struct offsets) are
  treated as "not recognised".
