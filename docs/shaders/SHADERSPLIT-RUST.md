# The shader split in Rust: `ibuilder-shadersplit`

Crate `crates/ibuilder-shadersplit` (library, `#![forbid(unsafe_code)]`, no process is spawned, builds for `aarch64-apple-ios` and `-ios-sim`) plus the Mac CLI `shadersplit` (`src/bin/`).
It is package **S** of the round in `docs/shaders/SHADER-BACKEND-PLAN.md`: step 1 of stage 3 (`docs/shaders/SHADER-STAGE-ANALYSIS.md`, section 1, row 1), the reading of the game's shaders.

**Input** the game's RomFS (a `DMROMFS1` image or a folder), its `main` NSO (+ updates, + DLC roots, the profile folder, the game id).
**Output** `pkg/{ps,vs,cs,gs}_{data,main}.bin` (`BTCDBTCD` records: key, code area, control section) and `tables/{shader_map,shader_facts,shader_pairs,pso_pairs}.tsv`, **byte-identical to the Python's**.
**Round 4 (package SNOWDROP)** brought the port to the engine the app now ships (`out-stage/out-engine`, rebased to nxnative a3663659 + the NX2Apple patches): Snowdrop's bare zlib caches (`paches/engine/0104`) and the other changes of its `shader_split.py`: section 10.

## 1. What was ported (versions)

| Python | size, mtime | read as |
|---|---|---|
| `iOSBuilder/engine/dm/gfx/tools/shader_split.py` (`diff` empty against `iPorterNX/third_party/NX2Apple/engine/dm/...`) | 286,158 B, 2026-11-16 01:28:04 | the engine's split (3,821 lines) |
| `gfx/tools/unityfs.py`, `tools/romfs.py`, `games/game.py`, `tools/nso.py` (same copies) | 48,116 / 17,292 / ... B | the Unity reader, the image reader, the profile |
| worktree `$IOSBUILDER_WORKTREE/gameapp/shader_device.py` (re-read just before the last run; `diff` against the harness copy empty) | 37,785 B, 2026-11-19 00:48:32 | `split` (l.336-377), `JobMemo` (l.287-335), `live_package` (l.378-410), the zstd shim (l.216-283) |

Round 4 (section 10) reads the shipped engine's copy: `$IOSBUILDER_WORKTREE/out-stage/out-engine/engine/dm/gfx/tools/shader_split.py` (295,158 B, 3,941 lines, 2026-11-19 02:28; 284 lines of `diff` against the copy above), and its `kit.py` and `dmsh_rt.py`;
`unityfs.py`, `romfs.py`, `nso.py` are byte-identical to the older copies, `game.py` differs in the `screen_aspect` define only (not the split's).

Oracle: `$OLD/shader/split-out/{pkg,tables}` = the output of `crates/ibuilder-shader/tools/measure/run_split.py` (`shader_device.split` over `shader_split.py`, Python 3.14.1).
I re-ran that harness here (`/usr/bin/time -l`, scratch copy of the engine): its 11 files are `cmp`-equal to the stored oracle.

### Python to Rust

| Python | Rust (`crates/ibuilder-shadersplit/src/`) |
|---|---|
| `shader_split.main` (the version loop, `merge_keys`, `separable_pairs`, the record / map / facts / pair writers) | `split.rs` (`split`, `Merged`, `pixel_cover`), `scan_types.rs` |
| `CTL_MAGIC`... `skey`, `key_ctl`, `ctl_len`, `same_shader`, `check`, `sph_stage`, `nvn2_stage`, `glslc_stage`, the record cut, the compute facts | `ctl.rs` (`record`, `compute_facts` are the lines of `main` / `live_package`) |
| `scan_file`, `scan_file_in`, `scan_blob`, `uniq_keys`, `var_pairs`, `BAHS_VAR` | `scan.rs`, `scan_types.rs` (`BAHS_VAR` is per job: its keys are labels of one file) |
| `unity_shaders`, `unity_program`, `unity_stages`, `unity_code_blobs`, `unity_compute`, `unity_pass_pairs`, `unity_text_glslc`; `unityfs.py` (bundles, serialized files, type trees, `KNOWN_TREES`) | `scan.rs`, `unity.rs`, `unity_trees.rs` (generated: `tools/gen_known_trees.py` writes the 16 layouts from the Python module itself) |
| `romfs.py` `Image` (every check), `os.walk` | `image.rs` |
| `game.py` `Game` (`shader_groups`, `shader_note`, `shader_key_skip`, `shader_facts_manual.tsv`) | `game.rs` |
| `glslc_*`, `rgxd_shaders`, `tfxo_*`, `shad_*`, `raw_shaders`, `rel_targets`, `adrp_targets`, Retro `snvn_*` / `retro_*` / `rfrm_shaders` | `glslc.rs` |
| `bnsh_shaders`, `bahs_shaders`, `bahs_variations`, `sead_prim_shaders`, `ndsr_shaders`, `ndve_shaders`, `mshd_shaders`, `needle_shaders`, `civbsf_shaders`, `shn_shaders`, `sumo_*`, `phyre_shaders`, `pg*`, `viper_shader`, `mgfx_*`, `hcps_shaders`, `slct_shaders` | `nvn.rs` |
| `yaz0`, `sarc_members`, `ungzip`, `unzs*`, `unsized`, `zs_dicts`, `pacx4_*`, `mtarc_*`, `bea_members`, `gfpak_members`, `bnd2_*`, `heaps_*`, `kt_gz`, `gust_gz`, `kt_shaders`, `monolith_*`, `xnb_decompress`, `idrk_*`, `ipk_shaders`, `d11_pack_shaders`, `snowdrop_*`, `ue3_*` | `archive.rs` |
| `linkdata_jobs`, `kpka_*`, `sdf_*`, `ocio_shaders`, `bdt_jobs` / `bhd5_entries` / `undcx` / `bnd3_members`, `pkz_*`, `zip_jobs` / `zip_member`, `arh_*` | `jobs.rs` (planning + running of the archives the Python gives jobs of their own) |
| `shader_device.JobMemo`, the thread pool, the `gate` | `memo.rs`, `split.rs` (`run_pool`, `JobGate`) |
| (new, memory: the Python holds every first copy of a key in a dict) | `spill.rs`: the merged shaders' bytes in a temporary file (section 9) |
| `shader_device.live_package` + the helpers it imports (`skey`, `glslc_stage`, `STAGE_NAME`, `nvn2_ctl`, `CTL_MAGIC`) | `live.rs` (**package B's helpers**) |
| `compression.zstd` / the app's `zstd(data, dict, max_length)` callable | `zstd.rs` (trait `Zstd`, default `Ruzstd`) |
| `json.dumps(..., sort_keys=True, separators=(',', ':'))`, ensure_ascii strings | `pyjson.rs` |
| (round 4) `snowdrop_objects`, `snowdrop_zlib` (`#SDFZLIB@`) | `archive.rs`; zlib's own `inflate` behaviour (`decompressobj().decompress(data, max_length)`, its error texts) in `zlibx.rs` |
| (round 4) `kit.py`, `kit_jobs`, the KIT job of `scan_file_in` | `kit.rs` (planning in `split.rs`: `Work::Kit`) |
| (round 4) `dmsh_rt.parse_module` (the metadata check behind `check(.., 'rt', ..)`) | `rt.rs` |

## 2. API and how the pipeline calls it

```rust
let mut o = SplitOptions::single(romfs, main_nso, pkg_dir, tables_dir, profile_dir, game_id);
o.versions.push((update_romfs, update_main));   // optional: the output is the union of the versions
o.dlc = vec![aoc_1_img, aoc_2_img];             // optional
o.jobs = 6; o.memo_dir = Some(&work.join("splitjobs"));
let stats = ibuilder_shadersplit::split(&o, &Ruzstd, &gate, &|line| log.line(line))?;   // SplitStats { shaders, jobs_reused, jobs_ran, unreadable, by_stage }
```

* `ibuilder_shadersplit::split_version()`: `"rs<10 hex>"`, from the crate version and `SPLIT_REV` (bump it when a change can change the output); this is what `ShaderBackend::split_version` returns.
* **Mapping `SplitRequest` (package B wires it in `ibuilder-cli`)**: `romfs`, `main_nso`, `pkg_dir`, `tables_dir`, `profile_dir`, `game_id` map to the same-named arguments; `dlc: &[PathBuf]` to `o.dlc`; `jobs` to `o.jobs`;
  `memo_dir` to `Some(memo_dir)`; `gate: &Arc<ShaderMemoryGate>` through
  `struct G<'a>(&'a ShaderMemoryGate); impl JobGate for G<'_> { fn run(&self, f: &mut dyn FnMut()) { self.0.run(|| f()) } }`;
  `log` to `&|l| log.line(l)` (the lines are the Python's stdout: `progress: n/total`, `[RED] ...`, `single-stage programs: ...`, `shaders: ...`, `unity compute source: [...]`);
  the result to `SplitResult { shaders: stats.shaders, jobs_reused: stats.jobs_reused, jobs_ran: stats.jobs_ran }`; an `Err(SplitError)` to `PipelineError::failed(e.to_string())`.
  `SplitError::Unsupported(..)` displays as `unsupported container: <name> (/<file>)`.
* **zstd**: the trait is `decompress(&self, data, dict: Option<&[u8]>, max_length: Option<usize>) -> Result<Vec<u8>, String>`, the shape of the phone's callable. The Mac CLI and the tests use `Ruzstd` (the pure-Rust `ruzstd`, dictionaries included, no C);
  the app may pass its own callback behind the same trait. No file of Grizzy is zstd; the zstd paths were checked against the Python's own `compression.zstd` on synthetic data (section 3: the `misc`, `kpka`, `bdt-pkz` and `ipk-d11-berg-ue3` games).
* **Live shaders** (`live` module): `live::package(rules, entries, pkg_dir, tables_dir)` is `live_package`, `live::package_entry` its per-entry checks (same refusal texts), and `skey`, `glslc_stage`, `stage_name`, `nvn2_ctl`, `CTL_MAGIC`, `record`, `compute_facts`
  are re-exported. `KeyRules::parse("0x770+0x78")` is the profile's `shader_key_skip`.
* **Resume**: a finished job is written to `memo_dir/<digest>.job` (tmp + rename; own format `SSMEMO01`, not a pickle: a folder the Python app left is not read, and `split_version` differs anyway).
  A damaged file is deleted and its job runs again; half-written `.tmp` files are removed on open. `SplitStats::{jobs_reused, jobs_ran}` count them as `split_stats` did.
* **Threads**: `jobs` worker threads take jobs in order; the results are merged **in job order**, so 1, 2 or 8 threads, and a resumed memo, give the same bytes. Every job goes through the gate.

### How a file is read

The Python reads every file whole (1.8-2.2 GB resident for Grizzy's 2.5 GB RomFS). Here a file is a [`Source`] (`pread` of a range of a file or of an image member; no memory map, the crate forbids `unsafe`).
A file over 16 MiB that no magic at its start opens is only **searched** in 1 MiB chunks for the container magics (BNSH, BAHS, GLSLC, MGFX, Needle, Ninty_NX, NVNsh, `Yaz0` in a SARC) and read whole only when one is in it;
a Unity serialized file **of any size** is read through its metadata and the objects of the classes asked for (Shader, ComputeShader, TextAsset), never whole (before package MEM only the files over 16 MiB took this path:
the 190 smaller `.assets` / `level` files were each read whole by whichever of the six threads had them). So a 380 MB `.resS` costs a search, and a 138 MB `.assets` its shader objects.

## 3. Verification (what I ran)

All on this Mac (M1 Pro, 8 cores, shared with other agents: **load average 40-55 while I measured**). `CARGO_TARGET_DIR=<scratch>/target-shadersplit`, a private workspace (symlinks to the three crates) so the shared `Cargo.lock` was not touched.

```
cargo test --release -p ibuilder-shadersplit          # 44 tests, all pass (round 4: 27 before, 17 new; Grizzy's 2 skip when no fixture is there)
```
(Round 3: `tests/grizzy.rs` reads the RomFS image and `main` from `SHADERSPLIT_ROMFS` / `SHADERSPLIT_MAIN` when set, else the checkout's `out/ref-img-grizzy/RomFS.img`: that file was deleted from the checkout during the round, and the
test was run against a private copy of the same image, which splits to the oracle byte for byte. Without a fixture the test still skips with a message.)

| suite | what | result |
|---|---|---|
| `tests/grizzy.rs` | the full Grizzy split (RomFS image + `main`, profile text written by the test) vs `$OLD/shader/split-out`: **11 files `compare_files`-equal** for 1, 2 and 8 threads; with a memo: fresh (44 jobs ran), complete (44 reused, 0 ran), half deleted + one damaged + one `.tmp` (22 reused / 22 ran) | pass (skips when a fixture is absent) |
| manual | `diff -r <out>/pkg $OLD/shader/split-out/pkg && diff -r <out>/tables ...` for `--jobs 1`, `2`, `8`, with the task's profile folder `$OLD/shader/profile/0100DFA027504000`, and in the iOS simulator | all identical |
| `tests/differential.rs` | **3,312 vectors** (`tests/data/vectors-skip0.jsonl.gz` 1,912, `vectors-skip770.jsonl.gz` 1,400; `tools/gen_vectors.py` writes them with the engine's own functions): control sections / codes / keys / stages / `check` for both key rules (`shader_key_skip` `0+0` and `0x770+0x78`, 800 each), `same_shader` (300 each), the record cut and compute facts (300 each), containers through `scan_blob` / `glslc_shaders` (GLSLC single / multi / NVN2, BNSH, BAHS with its variations, SARC + Yaz0 nesting, MSHD, TFXO, PGSO, NDSR: 205 inputs, valid and mutated), `yaz0` (60), `sarc_members` (40), `pixel_input_cover` (30), the type-tree interpreter on random objects of all 16 `KNOWN_TREES` layouts (96, half of them mutated), Unity files (56: tree in the file, no tree and none borrowed = `NoTree`, tree borrowed) and UnityFS bundles (12: stored, LZ4, LZMA blocks), and `shader_device.live_package` (25 vectors, 59 entries accepted, 17 refused) | pass: every value equal, and an error exactly where the Python raised |
| `tests/games.rs` | **10 synthetic games** (`tests/data/games/*`, 0.6 MB; `tools/gen_games.py` builds them and runs the engine's split over them for `expected.json`): `plain` (BNSH, SARC, Yaz0, gzip, BAHS, DLC with a shared and an own shader), `misc` (zstd `.zs`, size-prefixed `.cmp`, MSHD, tfxo, PGSO, XNB-LZ4, NDSR), `zip` (stored / deflate, a prefix before the archive), `kpka` (3 SDF layouts, stored / deflate / zstd, a patch pak replacing an entry, OCIO, a block table), `bdt-pkz`, `arh`, `linkdata`, `idrk` (zlib, new zlib, LZ4 chunks, PDRK), `ipk-d11-berg-ue3`, `update-dlc` (a base game, an update, a DLC): sha256 of all `pkg/` and `tables/` files, 1 and 4 threads, and the Python's `shaders:` / `single-stage` / `[RED]` log lines | pass |
| `tests/synthetic.rs` | a game built in Rust: records vs the map (every row's record gives the key again), 1 / 3 / 8 threads and a resumed memo give the same bytes, a container the port does not read is `unsupported container: ...`, an unreadable file is left out and logged as the Python does | pass |
| unit tests | the 16 layouts parse, alignment flags, `int(x, 0)`, base64, zlib `eof` / `unused_data` semantics, Python JSON spelling, `live` files | pass |
| `tests/snowdrop.rs` (round 4) | the engine's `test_snowdrop.py` (its 2 new cases, the 9 older ones, and the label / order of the bare caches), `zlibx` against zlib on **2,400 streams** (stored, fixed and dynamic blocks, windows 9-15, truncated, bit-flipped, preset-dictionary headers; whole and cut at 1, 7, 20, 100 bytes of output: the same bytes, `eof`, bytes used, error text), and `snowdrop_shaders` against the engine on **1,057 mutated BERG slices** (506 found the same shaders, 509 ended in the same `[RED]` text, 41 are errors of the older record readers' libraries, 1 differs in a damaged zstd frame: section 10) | pass |
| `tests/rebase.rs` (round 4) | **2,711 vectors** of `tools/gen_rebase.py` against the rebased engine, the error texts exact: `check` / `nvn2_stage` / `same_shader` of NVN2 ray tracing (640), BNSH with the RT pointer (410), CIVBSF 0x1118 / 0x1218 (455), bnd2 v2 / v3 (455), Phyre and `scan_blob`'s RYHP fall-through (549), KIT index + members (202); and 6 games that end in an error (5 malformed KIT indexes, `main`'s `uniq_keys` red) | pass |
| `tests/games.rs` (round 4: 14 games, 1 / 4 / 8 threads) | + `snowdrop` (bare caches, c5 / c6 records, SDF zstd chunks, 8 files each with one defect: 24 shaders, 8 `[RED]` lines), `rebase-misc` (23), `rebase-kit` (10), `rebase-kit-bad` (2 + 3 `[RED]`) | pass |

The synthetic inputs are PRNG bytes in the layouts the readers expect, then mutated; nothing comes from a game (`tests/data` is 5.7 MB).

**`shader_map.tsv` against the stored one** (`$OLD/shader/profile/0100DFA027504000/shader_map.tsv`): it differs in line 2 only. Stored: `# column 5 records where each key first appears; no PC shader pairing supplied`;
split-out and this port: `# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied`. The stored map was written by an older `shader_split.py`;
the engine's current text has the add-on rule (`merge_keys`) in it. The other three tables are `cmp`-equal to the stored ones.

## 4. Performance (Grizzy: 227 files, 2.5 GB RomFS, 75 MB `main`, 21,990 shaders)

`/usr/bin/time -l`, release build, load average ~45-55 (indicative; the orchestrator's quiet-machine run is the official one). Python = `run_split.py` (`shader_device.split`, 6 threads, Python 3.14.1).

| | wall | user + sys CPU | max RSS | peak footprint |
|---|---|---|---|---|
| Python, 6 threads (2 runs) | **19.2 / 19.7 s** (13.9 s in the first harness run of the earlier part) | 13.0-13.4 + 1.3-1.7 s | 2.02 / 2.24 GB (1.81 GB earlier) | 2.0 / 2.5 GB |
| Rust, 6 threads (5 runs) | **0.89 - 1.42 s** | 1.07-1.17 + 0.48-0.80 s | 554 - 637 MB | 595 - 636 MB |
| Rust, 1 thread (4 runs) | 1.33 - 2.60 s | 0.96-1.05 + 0.33-0.35 s | 566 - 586 MB | 565 - 585 MB |
| Rust, 8 threads, complete memo (nothing to scan) | 0.44 s | 0.31 + 0.10 s | 538 MB | |

About **16x less wall time, 9x less CPU, 3.5x less memory** than the Python harness on this machine. The 560-640 MB is the merged shaders (127 MB of records) and the decoded Unity programs of the files in flight;
it is a measurement, not a floor (a streaming merge that keeps only the first copy of each key would be the next step).

**Round 3 (package MEM): the memory column of this table is out of date** (the 554-687 MB above is the first port; the split's peak is now 160 MiB of footprint / 232 MiB resident, section 9).

**iOS.** `PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-shadersplit` and `--target aarch64-apple-ios`: both `Finished` (CLI + library; `LC_BUILD_VERSION` platform 7 / 2, minos 14.0),
no `fork`/`exec`/`posix_spawn` symbol in the device binary, no `unsafe` and no `Command` in the source.
Run inside the booted iPhone 17 simulator: `xcrun simctl spawn <SIMULATOR-UDID> <shadersplit> --romfs RomFS.img --main main --pkg .. --tables .. --profile .. --game 0100DFA027504000 --jobs 6`:
`shaders 21990 wall 1.1-1.5 s`, `diff -r` of `pkg/` and `tables/` against the oracle empty (the data paths are reachable: shared host file system). The simulator runs on the Mac's CPU and `time -l` measures the `simctl` wrapper,
so there is no simulator RSS figure and nothing here says how the phone behaves (jetsam, the gate, A17 timings): **not measured on a device**.

## 5. Containers: what is ported, and what is not (explicitly)

A container the Python reads and this port does not is **never skipped silently**: the split ends with `SplitError::Unsupported`, displayed as `unsupported container: <name> (/<file>)`
(detection is the Python's own predicate: names beside each other, magics, footers). A file that is *damaged* is the Python's case, not this one: it is left out, `[RED] <file>: <why> (its shaders are left out)` is logged,
`SplitStats::unreadable` counts it and the split goes on (`scan_file`).

**Ported** (the rest of the Python's readers; verification level in section 3):
Unity (UnityFS with LZMA / LZ4 / stored blocks, serialized files 17-22, type trees in the file or borrowed, the 16 `KNOWN_TREES`, ComputeShader, TextAsset GLSLC), BNSH, agl BAHS, sead primitive drawer, NDSR / NDVE, MSHD, Needle, SHN!, CIVBSF, Sumo, Phyre,
PlatinumGames DAT / shader objects, Star Fox shader binaries, MonoGame MGFX (+ XNB with LZ4), Monolith (ARH1/ARH2 + XBC1 / cmp-zlib / HCPS / SLCT), Hedgehog PACx 403, MT Framework ARC / SPK, Bezel Engine BEA, Game Freak GFPAK (stored / zlib / LZ4), Criterion bnd2,
Heaps PAK (+ base64), Koei Tecmo (LINKDATA / chunked zlib / gust / RDB IDRK + PDRK), UbiArt IPK, Double Eleven packs, Snowdrop BERG (zlib / zstd records, zstd-wrapped chunks), Unreal Engine 3 caches (stored and zlib blocks), Retro Studios (RFRM / SNVN / LZSS in main),
RAGE rgxd, YEBIS tfxo (+ the zlib stream in main), Diablo II SHAD, control sections loose in main (`raw_shaders`), Capcom RE Engine (KPKA, SDF, OCIO), Snowdrop's bare zlib caches (round 4), Unravel TWO's KIT archives (round 4), FromSoftware dvdbnd (BHD5 + DCX + BND3), PlatinumGames PKZ (stored / zstd), zip (stored / deflate, prefixed archives),
Yaz0 / SARC / gzip / zstd (with Tears of the Kingdom's dictionaries) / HAL's size-prefixed frames (zstd and LZ4 frame), the glslc capture folder and the boot NROs.
Round 4's containers (Snowdrop bare zlib, KIT, CIVBSF 0x1118, bnd2 v2, NVN2 RT, Phyre's headerless compute) are checked against the engine **on synthetic data only** (section 10): no South Park, Unravel TWO, Civilization VI, Burnout Paradise or Switch 2 ray-tracing file was read.
Ported **by reading and checked only through the Grizzy run or not at all** (no synthetic vector or game exercises them): SHN!, CIVBSF, sead primitive, NDVE, Needle, Sumo, Phyre, `pg_sized_shader`, Star Fox binaries, MGFX, HCPS / SLCT, rgxd, SHAD, PACx, MTARC, BEA, GFPAK, bnd2, Heaps, RFRM / SNVN / LZSS,
`raw_shaders` beyond what Grizzy's `main` runs through it, `tfxo_streams`, `unsized` with an LZ4 frame, `ZsDic.pack.zs` dictionaries. The Grizzy run executes the main-scan paths of several of them (no hit), which proves they do not misfire, not that they read a real file.

**Not supported: ends the split with `unsupported container: ...`**

| container | why |
|---|---|
| XNB with LZX compression (8 files in Stardew Valley) | the LZX decoder is not ported (no LZX encoder here to test it; stored blocks only would verify a fraction): any LZX XNB is refused rather than guessed |
| Next Level Games `.dict` + `.data` archives (Luigi's Mansion 3, Mario Strikers) | not ported (the revision-4 reader shells out to the `zstd` command) |
| Camelot NARC / CRA (`cra.py`), CRI CPK (`cpk.py`), Diablo III CPK (`d3cpk.py`), Blizzard CASC (`casc.py`), Smash Ultimate `data.arc` (`smash_arc.py`), Larian LSPK (`lspk.py`), Rockstar RPF6 (`rpf6.py`, AES key from main), Frostbite (`frostbite.py`), Phyre VBF (`vbf.py`) | the readers live in other engine modules, not in `shader_split.py` |
| Game Freak Trinity `data.trpfs`, phx `PDAT` | not ported yet (FlatBuffers / zstd stream peeks; Trinity's entries are Oodle) |
| Unreal Engine paks, IoStore `.utoc`, decoded shader code archives (`ue_shaders/*.ushaderbytecode`) | `uepak.py` (AES, pak / IoStore readers) is not ported |
| **Oodle** (Kraken / Mermaid / Leviathan): GFPAK entries, PKZ `OodleKraken`, UE3 blocks, Trinity, Frostbite, Unreal | the Python asks the game's own decoder in the Mac host through a child process; none exists here |
| zip64 archives, zip members compressed with bzip2 / LZMA, encrypted members, Star Fox's LZ4-chunk members | zip64 / bzip2 / LZMA are not read (a zip that cannot be read is scanned as one file with the Python's `[RED]` line, as there); the others are `unsupported` |
| NSO with ZBIC compression (system 22.0.0 and later) | `ibuilder-nso` has no ZBIC decoder (an error from there, not a wrong image) |

Not reproduced (diagnostics only): the `[RED] main holds GLSL sources and imports glslcCompile, but there are no captured GLSLC outputs` line (needs `profile_guess.glsl_sources` and `nso_syms`), and the Python's `uepak.unreal_paks` decision beyond the footer test.

## 6. Decisions and deviations (each is visible in a test or a measurement above)

1. **Order is part of the output.** The Python's results are `[main, special archive jobs..., plain-file batches of 16..., deferred Unity files]`, and the first copy of a key wins (`where` is in the map): the merge here is in that order for any thread count.
   The archive jobs run before the plain files (`big + files`), in the Python's kind order (linkdata, kpka, bdt, pkz, zip, arh); the Unity files without type trees (`NoTree` in the first pass) are scanned again after everything, with the borrowed trees (`KNOWN_TREES` last).
2. **`BAHS_VAR` per job**, not a process global: its keys are file labels, so the pairs are the same.
3. **`dict` semantics reproduced where they show**: main's results keep the *last* copy of a key at the first copy's position (`{skey: ... for ...}`), the merge keeps the first; the BNSH / Unity labels, `Counter.most_common()` order in the summary line, `int.from_bytes` of short slices, a `bytearray` slice assignment that appends (found by a vector).
4. **Memo format** is this crate's (`SSMEMO01`), not a pickle; the key is a SHA-256 of the job's files (root, path, size) and `split_version()`.
5. **The order of `unity compute source:` lines** follows the threads (the Python's did too); the lines themselves are `json.dumps` text.
6. **Facts JSON** goes through `serde_json` and a Python-spelling writer (sorted keys, `,` / `:`, ensure_ascii strings); a hand-written fact with a float in exponent form would print as Rust prints it.
7. Error texts of exceptions the Python raises (`struct.error`, `IndexError`, ...) are this crate's wording, only in the log lines; a file fails in exactly the cases a vector showed.
8. `ZsDic.pack.zs` is read from every root (the Python's `zs_dicts(r)` for `romfs_v + dlc_v`).

## 7. Dependencies (beyond the conventions' list)

| crate | why | builds for iOS |
|---|---|---|
| `ibuilder-nso` (workspace) | the NSO loader (`nso.py`) | yes |
| `memchr` (already in the lock through `serde_json`) | `memmem` for the magic searches the Python does with `bytes.find` / `re` over whole files | yes |
| `flate2` (`rust_backend` = `miniz_oxide`, no C) + `crc32fast` | zlib / raw deflate / gzip with Python's `decompressobj` semantics (`inflate.rs`), zip CRCs | yes |
| `ruzstd` 0.8 (`std`, `hash`) | the default `Zstd` (dictionaries, concatenated frames); pure Rust | yes |
| `lzma-rs` 0.3 | raw LZMA1 of Unity bundles | yes |
| `serde_json` | the facts rows and the test vectors | yes |
| `sha2`, `lz4_flex` (workspace) | keys, memo digests; LZ4 blocks and frames | yes |
| `ibuilder-alloc` (workspace, round 3) | the global allocator of the `shadersplit` binary only (`BigBlocks`); the library does not use it; dev-dependency with feature `profile` for `examples/memprofile.rs` | yes (`libc` only) |

The three crates are new to `Cargo.lock` (`flate2`, `ruzstd`, `lzma-rs` + their dependencies); I built in a private workspace and did not write the shared lock file.

## 8. Not verified, not done

* No device run: iOS numbers are for the simulator on the Mac's CPU; memory under jetsam (3.3 GiB) and the gate's behaviour with this crate are untested. `JobGate` is exercised only with `NoGate`.
* Games other than Grizzy and the synthetic ones: every container listed as "by reading" above, and the whole of section 5's refusals, have no real file behind them.
* The Python source is the shipped engine's as of 2026-11-19 (round 4, section 10; `SPLIT_REV` 2). A later engine change that touches `shader_split.py` needs a new run of `tools/gen_vectors.py` / `gen_games.py` / `gen_snowdrop.py` / `gen_rebase.py` and a `SPLIT_REV` bump.
* Round 4: the Grizzy test was not re-run (its RomFS image is no longer in the checkout), and `main`'s `uniq_keys` (first copy wins) is the one change that could touch its output.
* ~~A streaming merge and a `Source`-based read of Unity programs would lower the 560-640 MB; not done.~~ Done in round 3 (package MEM, section 9): the merge keeps the bytes in a spill file, Unity serialized files are read through their tables at any size;
  the split's peak is 160 MiB of footprint (232 MiB resident) in a chain process that runs `ibuilder-alloc`'s allocator, down from 600-760 MiB. What is left is `main`'s decoded image (158 MiB, held while `main` is scanned): section 9.4.
* Reproduce the fixtures: `SHADER_DEVICE_DIR=<worktree>/gameapp PYTHONDONTWRITEBYTECODE=1 python3 -I tools/gen_vectors.py <engine>/dm/gfx/tools 0+0 tests/data/vectors-skip0.jsonl.gz` (and `0x770+0x78 ... --ctl-only` for the second file),
  `... tools/gen_games.py <engine> tests/data/games`, `python3 -I tools/gen_known_trees.py <engine>/dm/gfx/tools > src/unity_trees.rs`.
  Round 4 (the shipped engine, `python3` = 3.14 with `lz4`): `... tools/gen_snowdrop.py <engine root> <games out> tests/data/vectors-snowdrop.jsonl.gz` (the `snowdrop` game goes to `tests/data/games`), `... tools/gen_rebase.py <engine root> <games out> tests/data/vectors-rebase.jsonl.gz`
  (`rebase-*` to `tests/data/games`, `err-*` to `tests/data/games-err`), and `tools/gen_vectors.py` again for `vectors-skip0` / `vectors-skip770` (2 vectors of each changed: `nvn2_stage` of a control with +0x56 = 8).

## 9. Memory (package MEM, round 3, 2026-11-19)

Full account, profiles and the allocator experiments: `docs/chain/MEMORY-RUST.md`. Grizzy, 6 threads, `/usr/bin/time -l` on the Mac CLI (shared machine, load 20-60 during these runs; footprint = `peak memory footprint`):

| | peak footprint | max RSS | wall (indicative) |
|---|---|---|---|
| first port (system allocator) | 603-631 MiB (memo on: 720) | 604-631 MiB | 0.67-1.2 s |
| now (`shadersplit`, whose global allocator is `ibuilder-alloc`'s), fresh / memo fresh / memo complete / 8 threads | **160 MiB** in each | **232 MiB** | 0.6-1.4 s |

Where the 440-470 MiB went (the program's live heap was never above 195 MiB: libmalloc keeps the freed buffers, docs/chain/MEMORY-RUST.md section 3), and what each part of it was:

1. **Big blocks stay in the footprint** under the system allocator, so every whole-file read, decoded Unity object and search window of a thread was still there when the next one was allocated. `ibuilder-alloc` (a global allocator that `mmap`s blocks of 128 KiB and more) returns
   them at `free`; the library cannot install it for its host: the `shadersplit` binary and the `ibuilder` CLI do, the app's FFI archive must (docs/chain/MEMORY-RUST.md section 6). **The 160 / 232 MiB above need it**: the same code with the system allocator (`--features system-alloc`) peaks at 358-398 MiB of footprint (memo resumed: 267), the first port at 603-631.
2. **`spill.rs`**: the first copy of every key used to stay in a `HashMap<String, Shader>` (127 MB for 21,990 shaders) until the package was written. The bytes now go to a temporary file in the package folder as they are merged
   (`.split-spill-<pid>`, 4 MiB write buffer, removed when the split ends or fails, swept at the start of the next split after a kill); the map keeps `(offset, lengths)`. They are read back for the `same_shader` check of a later copy of a key,
   for the pixel cover (only the first 0x68 bytes of each pixel shader's code area: `PS_INPUTS_PREFIX`, the cover is computed from `pixel_cover_of_inputs`) and when each record is cut (`record`) and written, in the same sorted order as before,
   so errors come in the same order too. A game's own copy that replaces an add-on's is appended again; the dead bytes stay in the spill.
3. **No copy of what is only looked at**: `unity::shader_programs` returns `Programs` (the decompressed segments and ranges into them) instead of a `Vec` of copies; a Unity program is copied once into its `Shader` (twice only for the pre-2019.3 pass pairs).
4. **`scan_file_in`**: a Unity serialized file of any size goes through `scan_plain_big` (tables + the objects of the classes asked for), not only the files over 16 MiB (190 of Grizzy's 227 were read whole); `plain_start` already excludes everything the whole-file path
   would have decoded first, and both paths call the same `unity_shaders` / `finish`, so the output is the same (checked: 11 files `cmp`-identical, and the 3,312 differential vectors and 10 synthetic games, which exercise the small-file path).
5. **Search windows** (`contains_each`, `find_from`) 4 MiB -> 1 MiB.
6. **`run_pool`**: a worker does not start job `i` while the in-order merge is `threads + 2` jobs behind. A resumed split used to decode every memo file as fast as the threads could and let the results queue for the merge.
7. **The memo** (`memo.rs`) is written and read as a stream (`encode_to` / `decode_from`; same file format, so a memo folder of the earlier build still resumes, `SPLIT_REV` unchanged); a length field larger than what is left of the file is damage, never an allocation.

**Not reduced**: `main`'s decoded image (158 MiB) is held while `main` is scanned, before the pool starts; that is the 160 MiB floor (RSS 232 MiB with the 72 MB mapped NSO).
Per-thread working set above the floor, on Grizzy: nothing measurable at 6 or 8 threads.

Tests added: `spill::tests` (2). `tests/grizzy.rs` reads its inputs from `SHADERSPLIT_ROMFS` / `SHADERSPLIT_MAIN` when set (section 3). The profile tools: `examples/memprofile.rs` (the allocation profile of the split), `examples/mallocprobe.rs`.

## 10. Package SNOWDROP (round 4, 2026-11-19): the port follows the shipped engine

The engine the app ships (`out-stage/out-engine`) was rebased onto nxnative a3663659 with the NX2Apple patches after this port was written. `diff` of its `shader_split.py` against the copy of section 1 is 284 lines; every hunk, and what became of it:

| engine change | Rust | checked against the engine |
|---|---|---|
| **`paches/engine/0104`** (upstream PR 843): `snowdrop_zlib`, `snowdrop_objects`: older SDF slices (South Park: The Fractured But Whole) store each shader cache file as a **bare zlib stream** (a 64 KB block run, window 10: header `0x28 0x15`) with a decoded `0xabcd00d4` head; labels `#SDFZLIB@<offset>:<offset>` | `archive::snowdrop_zlib` / `snowdrop_objects`; `snowdrop_shaders` reads them first, then the c5 / c6 records, then the zstd chunks, as the Python | `tests/snowdrop.rs` (12), the `snowdrop` game (24 shaders, 1 / 4 / 8 threads, 8 defect files), 1,057 mutated slices |
| NVN2 ray tracing: `STAGE_NAME['rt'] = 'raytracing'`, `nvn2_stage` (+0x56 = 8), `same_shader`'s RT rule (the whole control compared, only +0x30 masked), `check(.., 'rt', ..)` with `dmsh_rt.parse_module` (the RTCore metadata: 425 lines of Python, its `_require` texts), the BNSH program's 7th pointer (+0x38) | `St::Rt`, `ctl.rs`, `rt.rs`, `nvn::bnsh_shaders`; packages `rt_<group>.bin`, map rows `raytracing`, memo code 8 | `rebase.rs`: 640 + 410 vectors, `rebase-misc` |
| CIVBSF: the revision at +8 (`0x1118` Civilization VI: 0x48-byte entries, four platform descriptors, NVBC; `0x1218`: as before; any other is `red`) | `nvn::civbsf_shaders` | 455 vectors, `rebase-misc` |
| bnd2: `BND2_LAYOUTS` (version 2, Burnout Paradise Remastered: 3 blocks and 0x40-byte entries; version 3: 4 and 0x50; the older copy read both as 4 / 0x50) | `archive::bnd2_resources` (zlib through `zlibx`) | 455 vectors, `rebase-misc` |
| Phyre: `glslc_stage` instead of the SPH, no `GFX_MAGIC` test, Unravel TWO's headerless compute (taken only in an RYHP effect and without a control magic inside its code), `scan_blob` reads RYHP / `GFX_MAGIC` at +8 itself and falls through when it finds nothing; `scan_file_in` sends such files to `scan_blob` | `nvn::phyre_shaders`, `scan::scan_blob`, `scan_file_in` | 549 vectors, `rebase-misc` |
| **KIT** archives (Unravel TWO): `kit.py` (index `kit!` + numbered parts, chained 64 KB LZ4 blocks with the previous 64 KB as dictionary, SHA-1 per member), `kit_jobs` (planned after the other archives, in ranges of members: 64 parts by stored size; an index that does not hold ends the whole split), the KIT job of `scan_file_in` | `kit.rs` (a SHA-1 of its own: no crate for it in the lock), `Work::Kit` in `split.rs` | 202 vectors, `rebase-kit`, `rebase-kit-bad`, 5 `err-kit-*` games |
| `main`: `results.append(('main', uniq_keys(found), ..))`: the **first** copy of a key stays and a different later copy is `red`; before, `{skey: ...}` let the last copy win | `split.rs` | `rebase-misc` (a pair of copies that differ past the key), `err-main-dup` |
| UE v9 paks: a mixed pak's `.ushaderbytecode` names are left out of the inline view | none: the UE pak path is `unsupported container` (section 5) | |
| docstrings; `game.py` `screen_aspect` (`wide` / `both`: not the split's) | none | |
| **`unzs` (no dictionary): `subprocess.run(['zstd', '-d', '-c', '-q'])`** instead of the in-process zstd of the older copy | **not mirrored, on purpose**: this port decodes in process | see the note below |

**A regression of the shipped engine.** The phone's wrapper (`shader_device._FakeSubprocess`) refuses every program but its own tools: on the app's Python a `.zs` file that names no dictionary ends `[RED] <file>: OSError: no subprocesses on this device: ['zstd', '-d', '-c', '-q'] (its shaders are left out)`.
The `misc` game's `Data/a.bnsh.zs` shows it: 10 shaders from the shipped engine under `shader_device.split` (the phone's wrapper, which runs on the Mac too); `shader_split.py` run by itself on a Mac, which has the `zstd` command, gives 12, as does this port and the older copy. Dictionary frames (Tears of the Kingdom) take `unzs_dict`, in process, and are not affected. The fix is an engine patch that restores the in-process decoder; this port keeps it,
so on such a game it finds **more** than the shipped Python does. `misc`'s `expected.json` was left as the older engine wrote it for that reason (the other nine games' expected files are byte-identical when regenerated with the shipped engine).

### What is exact, and what differs

* **zlib.** `zlibx.rs` is zlib's `inflate` (slow path) translated: the Snowdrop reader needs (1) `decompress(head, 20)` to stop at 20 bytes of output (a damaged symbol after them is no error there, while a decoder that works ahead, `miniz_oxide`, reports one: the stream would be skipped instead of read) and (2) the error text for the `[RED]` line.
  It is used for those and for bnd2 only; the older readers keep `inflate.rs`. The Mac's Python links Apple's libz 1.2.12, whose fast path words an invalid code as `invalid literal/length/distance code`; upstream zlib, and its slow path, say `invalid literal/length code` or `invalid distance code`: the tests compare those three as one.
* **Peeking a zstd chunk of a cut slice (found by the vectors, older code).** `snowdrop_wrapped` skipped a frame whose block headers ran past the data; the Python's streaming decoder still gives the first 20 bytes when the first block is raw or RLE and then fails `truncated frame`. Fixed (`zstd::peek_cut`); a compressed first block is decoded only when whole in both.
* **Differs, known.** libzstd and `ruzstd` answer differently for some damaged frames (a block larger than the frame's content size; a content size smaller than the decoded data: libzstd refuses it, `ruzstd` does not): 1 of the 1,057 slices, allowed in the test when one side ended in an error.
  The LZ4 error text of a KIT member (`Decompression failed: ... Error code: N`) is not reproduced (`lz4_flex` has no such code); the rest of that line is.
* **Not equal to the Python on purpose.** `#SDFZLIB@` is not in `SINGLE_STAGE` (the engine's list is not extended): its shaders are not "single-stage programs" in the `single-stage programs:` line and not in `Snowdrop shader objects` of the summary; reproduced as is.
* **Cost.** A 256 MiB slice of random bytes with 894 caches: 0.40 s for `snowdrop_shaders` (release, 1 thread); the scan is a byte test plus a 2-byte header check, then a 20-byte peek of the candidates (about 0.1% of the bytes).

### Not verified

No real file was read: South Park's 623 -> 4,614 shaders (and its `sdf-A-0200` slice of 1,994 caches) is the patch's own figure, Unravel TWO's KIT, Civilization VI's CIVBSF, Burnout Paradise's bnd2 v2 and a Switch 2 game's ray-tracing programs are exercised by synthetic files built with the engine's own functions and
the layouts its docstrings give. What a real RT module holds beyond what `parse_module` validates, and how `ibuilder-shader` / `ibuilder-shaderbuild` treat the new `rt_*.bin` packages and `raytracing` rows (a grep of those crates finds no `raytracing`), is for the packages that read the split's output.
A BERG data slice is still read whole into memory (`scan_file_in`: the c5 / c6 readers need it, and so does the bare-stream scan as written); the Python does the same. A slice of several hundred MB costs that much resident on the phone; a streaming scan (stream headers found in 1 MiB windows) was not written.


## 11. Reviews r3 / r4 and two small costs (2026-11-21)

* **r3 4.2 / r4 4.4, the Unity object decoder (`unity::read_node`).** r3 found that an array of a structure without fields pushes up to 2^31 empty values; the fix bounded each such array by the bytes left in the
  object. r4 showed that this bounds **one** array: an outer array of structures whose only field is such an array reads 4 bytes per element, and each inner array may then hold as many empty values as there are bytes left,
  so the work grows with the square of the object's size. Measured on the code before this round (the per-array guard only): a 3,804-byte object of that shape decoded to **1,805,002 values, `Ok`**
  (`tests::nested_arrays_of_elements_that_read_nothing_cannot_amplify_quadratically`). Now every decode has one **budget of values, 8 per byte of the object plus 4,096** (`node_budget`, `spend`: each
  `read_node` call and each element of an array of primitives spends one); past it the object is `[RED] MemoryError: an object of N bytes that decodes to more than M values ...`, where the Python runs out of memory. The bound is linear
  whatever the nesting (an attacker's object of N bytes holds at most 8 N + 4,096 values, about 48 bytes each), the per-array guard and its message stay. Honest objects are far below it: over the 16 shipped Shader / ComputeShader layouts
  the most values per byte any array element decodes to is **1.5** (`tests::honest_objects_stay_far_under_the_budget` computes it from the trees and asserts at most 2), and the objects the split decodes are mostly byte blobs (one value each).
  Not verified on a real Unity game (none is on this machine: Grizzy's RomFS image is gone, `tests/grizzy.rs` skips); the synthetic games and the 3,312 + 2,711 vectors still pass.
* **r3 5.13 / 5.21** (`ctl.rs`: a negative `shader_key_skip` is refused by `KeyRules::parse`, `o + l` is a checked sum in `key_ctl` and `same_shader`) and **5.22** (`unity.rs`: `doff + start` in `i128` with Python's slice rules,
  `tests::an_object_range_follows_python_slicing_and_cannot_wrap`): done in the round before, read again against the review, in place.
* **Costs (review r4 6.3, optimisations 4.6 / 4.7).** `scan::cover_greedy` compared each candidate by a fresh `String` copy of its key at every greedy step (now `&str`, the same order); `split`'s package sort called
  `prof.group()` (a scan of the profile's groups) twice per comparison (now once per shader, before the sort; the write loop reuses it). Both are outside Ocarina's path (NVN2, no Unity, 6 groups): on the full
  Ocarina of Time split (42,848 shaders, `--jobs 8`) the 18 output files are **sha256-identical** before / after and the wall time is the same, **61.35 s / 61.23 s** (1.2 GB max RSS both; load average 4-30; measured before the machine's reboot that day, whose scratch folder held the extracted game and the logs).
  `jobs.rs` `zip_member` (a backwards scan of the central directory per member, optimisations 4.8) was not changed.
