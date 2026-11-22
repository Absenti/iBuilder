# Shader split (`ibuilder-shadersplit`)

Step 1 of stage 3: read the game's files and cut out every shader program with its control section. Overview of the whole stage: [shaders.md](shaders.md).

Crate `crates/ibuilder-shadersplit`: a library (`#![forbid(unsafe_code)]`, no process is spawned, builds for `aarch64-apple-ios` and `-ios-sim`) and the Mac CLI `shadersplit`.

## What it ports

The Python original is the engine's `shader_split.py` (plus its Unity reader, the RomFS image reader, the NSO reader and the game profile) and the phone-side wrapper around it (job memo, live package check).

| Python | Rust (`src/`) |
|---|---|
| `main` (version loop, key merge, pair detection, record / map / facts / pair writers) | `split.rs`, `scan_types.rs` |
| control-section helpers (`skey`, `same_shader`, `check`, stage detection, record cut, compute facts) | `ctl.rs` |
| `scan_file*`, `scan_blob`, key and pair helpers | `scan.rs` |
| Unity bundles and serialized files, type trees (16 built-in layouts, generated from the Python module by `tools/gen_known_trees.py`) | `unity.rs`, `unity_trees.rs` |
| RomFS image reader, game profile | `image.rs`, `game.rs` |
| GLSLC, BNSH, BAHS and the other NVN container readers | `glslc.rs`, `nvn.rs` |
| archive and compression formats (Yaz0, SARC, gzip, zstd, zip, game-specific paks) | `archive.rs`, `inflate.rs`, `zlibx.rs`, `zstd.rs`, `kit.rs` |
| archives that get jobs of their own | `jobs.rs` |
| job memo and thread pool | `memo.rs`, `split.rs` |
| NVN2 ray-tracing module check | `rt.rs` |
| live shader package check | `live.rs` |

New in Rust: `spill.rs` keeps the merged shaders' bytes in a temporary file instead of a map in memory.

## Input and output

Input: the game's RomFS (a `DMROMFS1` image or a folder), the `main` NSO, optional update and DLC roots, the profile folder, the title id.

Output, byte-identical to the Python's:
- `pkg/{ps,vs,cs,gs}_{data,main}.bin`: `BTCDBTCD` records (key, code area, control section). Other stages and ray-tracing modules get their own package files (`rt_<group>.bin`).
- `tables/{shader_map,shader_facts,shader_pairs,pso_pairs}.tsv`.

## API and CLI

```rust
let mut o = SplitOptions::single(romfs, main_nso, pkg_dir, tables_dir, profile_dir, game_id);
o.versions.push((update_romfs, update_main));   // optional: output is the union
o.dlc = vec![aoc_1_img];                        // optional
o.jobs = 6; o.memo_dir = Some(&work.join("splitjobs"));
let stats = ibuilder_shadersplit::split(&o, &Ruzstd, &gate, &|line| log.line(line))?;
```

- `split_version()` returns `rs<10 hex>` from the crate version and `SPLIT_REV` (bump it when a change can alter the output).
- zstd is a trait (`Zstd`); the CLI and tests use the pure-Rust `Ruzstd`, the app may pass its own callback.
- `JobGate` lets the caller throttle each job (the pipeline's memory/thermal gate).
- `live::package` checks entries captured during play (same refusal texts as the Python).

```
cargo run --release -p ibuilder-shadersplit --bin shadersplit -- \
  --romfs RomFS.img --main ExeFS/main --pkg out/pkg --tables out/tables \
  --profile <profile dir> --game <title id> [--jobs 8] [--memo <dir>] [--dlc a.img,b.img] \
  [--update-romfs R --update-main M]
```

## Design points

- **Order is part of the output.** Results are merged in the Python's job order (main, archive jobs, plain-file batches, deferred Unity files); the first copy of a key wins. The output is the same for any thread count and for a resumed memo.
- **Files are not read whole.** A file is a `Source` (positioned reads, no memory map). Large files with no recognised magic are only searched in 1 MiB windows; Unity serialized files are read through their tables and the objects of the wanted classes.
- **Resume.** Each finished job is written to `<memo_dir>/<digest>.job` (temp file + rename, own format `SSMEMO01`); damaged files are deleted and the job runs again.
- **Unity decoder budget.** One decode may produce at most 8 values per byte of the object plus 4,096; past that the object is reported as a `[RED]` error (the Python would run out of memory). Real layouts stay far below.
- **Unsupported containers fail loudly.** A container the Python reads and this port does not ends the split with `unsupported container: <name> (<file>)`. A damaged file is left out with a `[RED]` log line and counted in `SplitStats::unreadable`.
- **In-process zstd.** The port decodes `.zs` files itself; the original's newer code shells out to a `zstd` command, which does not exist on a phone.
- Allocator: the `shadersplit` binary (and the `ibuilder` CLI) run with `ibuilder-alloc`, which returns big blocks to the OS. The library does not install it; the host must.

## Verification

```
cargo test --release -p ibuilder-shadersplit
```

| suite | what it checks |
|---|---|
| `tests/grizzy.rs` | full split of a real game against the Python's output (11 files equal) for 1, 2 and 8 threads and with a memo fresh / complete / partly damaged. Skips without the game files (`SHADERSPLIT_ROMFS`, `SHADERSPLIT_MAIN`). |
| `tests/differential.rs` | 3,312 vectors made with the engine's own functions: control sections, keys, stages, containers, Yaz0 / SARC, the Unity type-tree interpreter, Unity files and bundles, live package. |
| `tests/games.rs`, `tests/synthetic.rs` | 14 synthetic games (generated, no game data) split by both implementations; output hashes and log lines compared at several thread counts. |
| `tests/rebase.rs`, `tests/snowdrop.rs` | newer engine behaviour: ray-tracing control sections, newer container revisions, Snowdrop zlib caches (including 2,400 zlib streams against zlib itself and 1,057 mutated slices). |

Measured on 2026-11-19 on an M1 Pro (shared machine, indicative), one real game with 21,990 shaders and a 2.5 GB RomFS: Python 6 threads about 19 s wall and 2 GB resident; Rust 6 threads 0.9 - 1.4 s wall, peak footprint 160 MiB. The iOS simulator run produced identical output; nothing was measured on a device.

## Known limits

- **Only one real game was available.** Many container readers (see the list in the source) are ported by reading and checked only against synthetic files built to the layouts the Python expects.
- Refused with `unsupported container`: XNB with LZX compression, Next Level Games `.dict` + `.data`, Unreal Engine paks / IoStore, Oodle-compressed entries (Kraken, Mermaid, Leviathan), zip64 / bzip2 / LZMA zip members and encrypted members, Game Freak Trinity archives, containers whose readers live in other engine modules (CPK, CASC, LSPK, RPF6, Frostbite, and similar), and NSO files with ZBIC compression.
- libzstd and `ruzstd` differ on a few damaged frames; one of 1,057 mutated slices differs in how it ends.
- A BERG data slice is read whole (as in the Python).
- The split's output is only as current as the Python it follows; an engine change to `shader_split.py` needs the vector generators in `tools/` re-run and `SPLIT_REV` bumped.
- No device run: jetsam limits and the memory gate with this crate are untested.
