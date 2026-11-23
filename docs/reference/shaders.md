# Shaders (stage 3)

Stage 3 turns the game's Switch GPU programs into a library of Metal source that the iOS host loads. Three crates do it; the first step has its own page, [shader-split.md](shader-split.md).

```
game files --split--> pkg/*.bin + tables/*.tsv
   --job_for--> translator (dmsh_xlate) --> SPIR-V
   --spirv-cross--> MSL --post_rewrites chain--> finish (reflection, side libraries) --> library image
```

| crate | role | original it ports |
|---|---|---|
| `ibuilder-shadersplit` | read game files, cut shaders and tables | `shader_split.py` |
| `ibuilder-shader` | MSL rewrite chain, per-shader `finish()`, tessellation / mesh / geometry kernels, tables, library image writer | engine `dm/gfx/tools/dmsh_*.py`, `shader_post.py`, `c1_guard.py`, `cb_extent.py`, `shader_lib.py`, `romfs.py` |
| `ibuilder-shaderbuild` | build driver: jobs, phases, caches, library, live translation | `dmsh_build.py` (driver parts), `shader_device.py`, `dmsh_chain.py` |

The two native tools (the engine's translator `dmsh_xlate` and `spirv-cross`) stay external. They are passed in through the `ShaderTools` trait (argv without the program name); on the Mac they run as child processes (`host-tools`), in the app they are callbacks.

When a library built on the phone is split again: only when the split's global revision changed or a reader its game needs changed (the record each split leaves in `pack.txt`, `split-revs` / `split-uses` / `split-unknown`; `ibuilder-pipeline` `shaders::split_verdict`). Details: [shader-split.md, Targeted re-split](shader-split.md#targeted-re-split-recordrs).

All three libraries use `#![forbid(unsafe_code)]`, spawn no process, and build for `aarch64-apple-ios` and `-ios-sim`. The Mac CLIs are separate binaries.

## 1. The rewrite chain (`ibuilder-shader`, root modules)

`post_rewrites(src, &Job) -> Result<PostOut, PostError>` is the list of text rewrites of the Python's `finish()` for ps / vs / cs, plus the generic part for hs / ds and mesh jobs. `Job` is `Default`; `PostOut` carries the new text and what later steps need (`analysis` text for the reflection, `blend_zero`, `fs_uint`, `fs_dual`, `shared_body`, `vs_pad`, `entry_discard`, `layer_entry`, `first_store`, `tg_spill`).

| Rust module | Python function(s) |
|---|---|
| `post.rs` (driver), `steps.rs`, `guards.rs`, `utest.rs`, `border.rs`, `blend.rs` | `check_msl`, `precise`, `membar_fence`, `tg_fence`, `max_threads`, `uniform_tests`, `border_emu`, `blend_zero`, `select_guards` |
| `atomic.rs`, `wide.rs`, `pool.rs` | `wide_atomics`, `wide_loads`, `pool_arrays` |
| `pervertex.rs`, `bit0.rs`, `ampere.rs`, `hmma.rs` | `per_vertex`, `bit0_mask`, `mul_extended`, `float_add_msl`, `gather_offsets`, `shared_store_msl`, `hmma_msl` |
| `zsnap.rs`, `cs.rs` | `frag_z_snap`, `first_store`, `tg_spill` |
| `vs.rs`, `fsuint.rs`, `tf.rs` | vertex-stage entry rewrites (`vs_init`, `vs_depth`, `vs_discard`, `vs_layer`, `drop_point_size`, base instance), `fs_uint`, `fs_dual`, `vs_pad`, transform-feedback capture and `tf_layout` |
| `side.rs` | side-library entry points and shared bodies |
| `interval.rs`, `tables.rs` | `c1_guard.py` (interval proof that constant-buffer reads are bounded) and `cb_extent.py` |
| `romfs.rs` | `DMROMFS1` library image writer |
| `templates.rs` | fixed MSL text the rewrites insert, **generated** from the Python by `tools/gen_templates.py` |

Design points:
- The Python's regexes (look-behind, back-references, lazy quantifiers) are written as cursors and substring scanners (`text::Cur`, `memchr`). Scanners assume what SPIRV-Cross always writes: one declaration or statement per line, ASCII text.
- Steps fail like the Python: `PostError::Failed` carries the status string `finish()` would return; exceptions the Python does not catch end up as status `exception`.
- Still returned as `PostError::Unported`: a stage other than ps / vs / cs / hs / ds / mesh job, a vertex shader without the translator's answer JSON, and a transform-feedback job without its layout. Both of the last two are internal wiring errors that the build never triggers.
- Bins: `shader_post` (chain over a capture, compare with a library image), `step_diff` (one step over a case list, for differential tests), `shader_tables`, `shader_finish` (Mac only, drives the real tools).

## 2. `finish()` (`ibuilder-shader::finish`)

`finish` takes one translated shader and returns every file of its entry in the final library, plus a status. Source mode only: the library holds MSL text; the host compiles it on load.

```rust
let ctx = Ctx::load(profile_dir, game_id, chain, Options::ios_source_mode(pulling))?;  // once per build, immutable
let done: Finished = finish_partners(&ctx, key, work, &mut job, c1, tools, partners);
// done.status: "ok" | the Python's failure kind | "unported: <why>" | "fatal: <msg>"
// done.outputs: [(library file name, bytes)]; done.ps_locs: pixel shader locations for vs_pad
```

Output files per shader: `<key>_b.{json,metal}` (reflection + MSL), and by stage `_bx` (side library), `_bf` (readable-tile variant), `_bv` / `_bp` (vertex input / pulling variants), `_bg0..4`, `_bgv` (geometry), `_bl0/1/3` (passthrough geometry kernels), `_m3` (mesh libraries, see 3).

Contract:
- `Ctx` is immutable and `finish` is re-entrant for different keys (every file it writes in `work` is key-prefixed).
- Order: shaders without pixel-shader partners first, then the vertex shaders that have partners (`Ctx::is_paired`), because `vs_pad` reads the partners' finished locations.
- It calls the tools twice at most: the integer-input second translation pass, and `spirv-cross` with the Python's argv (`--msl --msl-version 20300 --msl-argument-buffers --msl-texture-buffer-native --msl-pad-fragment-output ...`; MSL 3.1 when the module uses image atomics).
- `finish` knows nothing about caching; the whole `outputs` list can be cached by the caller.

Rust modules (`src/finish/`): `mod.rs` (flow, `Ctx`, `Options`), `pyjson.rs` (Python JSON spelling, `indent=1`, insertion order), `reflect.rs` (reflection: bindings, driver reads, raster facts), `fetch.rs`, `unit.rs`, `vin.rs` (side libraries, vertex-input and pull variants), `entry.rs`, `spv.rs`, `geometry.rs`, `abi.rs` (generated from the engine's ABI header), `rx.rs` (run-time regex cache), `pycompat.rs` (Python `splitlines`, `strip`, `int()` semantics where they reach messages).

`rx.rs` replaces regexes built per call from shader names with exact equivalents (per-thread compiled cache, or substring search plus a fixed tail pattern). Its tests compare against the full regex on generated texts.

## 3. Tessellation, task / mesh shaders, passthrough geometry

| kind | Python | Rust |
|---|---|---|
| hull / domain (`hs`, `ds`) | `dmsh_tess.py` | `finish/tess.rs`, `tess_text.rs` (generated) |
| task / mesh (`ts`, `ms`, Switch 2) | `dmsh_meshstage.py`, `partner_reads` | `finish/meshstage.rs`, `finish/mod.rs`; `job_for` branch in `ibuilder-shaderbuild/src/jobs.rs` |
| mesh libraries `<vs>_<gs>_m3` | `dmsh_mesh.py`, `mesh_finish` | `finish/mesh.rs`; pair-level driver `ibuilder-shaderbuild/src/mesh.rs` (`build_mesh_libs`) |
| passthrough geometry (`_bl0/1/3`) | `dmsh_geometry.lean` | `finish/lean.rs`, `finish/geometry.rs` |
| geometry with transform feedback | `vs_tf(geometry=True)` | `finish/geometry.rs` |

Behaviour worth knowing:
- A hull or domain shape the Python refuses is not a failure: the shader is `ok`, keeps the translated source, and its reflection says `"tess": {"unsupported": "<message>"}`.
- A task / mesh shader the Python refuses is `mesh stage failed`; `ValueError` messages match the Python's text.
- A mesh shader that has pixel-shader partners needs the partners' SPH input maps; use `finish_partners` with a `Partners` implementation. `finish_with_locs` returns `unported` for that case.
- A `SPIR-V` file whose size is not a multiple of four stops the shader as `fatal` (the Python's `struct.error`).
- Whether the host's tessellation expansion kernel and the Metal compiler accept the rewritten MSL is not checked by these tests (source mode ships text).

Test tools: `crates/ibuilder-shader/tools/tessmesh/` (`gen_corpus.py`, `run.sh`, `make_game.py`, `e2e.sh`, mutation scripts). They need the engine's Python, `glslangValidator`, `spirv-opt` and `spirv-cross`.

## 4. The build driver (`ibuilder-shaderbuild`)

```
cargo run --release -p ibuilder-shaderbuild --features host-tools,f2-finish --bin shaderbuild -- build \
  --pkg <split pkg dir> --profile <profile dir> --game <title id> --out <dir> --chain <chain id> \
  --engine <engine dir with the translator and spirv-cross> --finisher rust \
  [--keys <file of keys>] [--jobs N] [--tables 0|1] [--pulling 0|1] [--cas <cache dir>] [--tool-memo 0|1]
shaderbuild live --entries <dir> --pack <dir> --derived <dir> --game <id> --work <dir> --chain <chain> ...
libdiff <a.img> <b.img> [--ignore-first-line c1_full.tsv --ignore-first-line cb_extent.tsv]
```

Library API: `build(&BuildOptions, &BuildEnv) -> Result<BuildReport, String>` (`BuildEnv { tools, finisher, log, cancel }`), `live::live_translate(...)`, `tools::{FnTools, CountingTools, ProcessTools}`, trait `finish::Finisher`. `--finisher replay:<image>` replays a Python library image shard by shard (tests the driver alone); `mock` writes a stub that is not a library.

| Python | Rust (`src/`) |
|---|---|
| profile / map / pair tables, `Game` | `game.rs`, `records.rs` |
| `job_for`, `vertex_ab`, `tf_layout`, NVN2 and mesh branches | `jobs.rs` |
| shader digests, source fingerprint | `key.rs`, `build.rs` (crate root) |
| SQLite output store and cache | `cache.rs` (content-addressed folder, own format) |
| `publish_library`, image epilogue of `shader_device.build` | `library.rs`, `build.rs` |
| per-shader `c1_guard` / `cb_extent` rows | `tablerows.rs` |
| translation batches, phases, summary lines | `build.rs` |
| `live_package`, `live_translate` | `live.rs` |
| NVN2 ray-tracing library plan (`dmsh_rt`, `dmsh_rt_build`) | `rt_build.rs` |
| JSON spelling | `pyjson.rs` |

Design points:
- **One pass per shader, no SQLite.** A shader's outputs are written once as a cache entry; the two tables' rows are computed from those bytes and stored in the entry; the image is then streamed entry by entry in name order (`DMROMFS1`, 16-byte aligned, 24-byte entries). The Python wrote the files three times.
- **Phases and batches as the Python's.** Unpaired shaders first, then paired vertex shaders, in batches of `4 * jobs`; a translator thread prepares the next batch while workers finish the current one. `BuildEnv::cancel` stops between batches; the pipeline's memory gate wraps the tools.
- **Cache key is not the Python's, the outputs are.** The digest covers the chain id, the finisher fingerprint, `spirv-cross --version`, `pulling`, translator options, the shader's record, its profile rows, its pairs (a paired vertex shader also hashes its partners' digests), and a hash of the source of `ibuilder-shaderbuild` and `ibuilder-shader` computed by `build.rs`. Any source edit therefore invalidates every entry and a stale entry cannot be served (a comment edit also costs a rebuild).
- **Cache entries** live at `<cas>/cas-<chain>.dmsb/<hh>/<digest>`: immutable, written to a temp name and renamed, hard-linked into the build's stage folder on lookup (so pruning during a build cannot take bytes away), safe for two builds sharing the folder, resumable shader by shader. A vertex shader built while a partner failed is not cached. It is not compatible with the Python's SQLite cache. Other chains' folders are removed at start.
- **Failure semantics.** A malformed shader fails alone (`translation failed`, message logged) and the build goes on; a `finish` panic becomes `finish panicked`. A mesh pair that cannot be composed is counted (`built / missing a library / not composed / failed to write`) and the build goes on.
- **Differences from the Python:** the tables' header time stamp is UTC; cache "reasons" are all `new key`; a missing conflicts file reads as empty.

### Tool memo (second cache level)

`--tool-memo 1` (`BuildOptions::tool_memo`, `toolmemo.rs`) caches the native tools' own answers, independent of the Rust code:
- translator entries per job: key is the memo format, chain id, translator source sha, the job JSON without its `code` path and the bytes of its code file; stored are all files the translator wrote;
- SPIRV-Cross entries per call: key is the chain id, `--version`, and the arguments with the input file replaced by its bytes; stored are the output file, stdout and stderr.

Only complete successes are stored (exit 0, no `[RED]` line, no unknown output file); a tool without an identity is not memoized; a damaged entry is a miss. Entries sit at `<cas>/cas-<chain>.dmsb/t/<hh>/<digest>`. Purpose: a change of the finishing code makes every first-level digest miss, but the tool answers are still valid, so the rebuild skips the translator and SPIRV-Cross (they are most of a build's time). It assumes the tools are deterministic and read only their job and code files.

It is off by default: it roughly doubles the cache folder, and the pipeline clears the whole shader cache when it exceeds `shader_cache_gib` (default 2, `ibuilder-pipeline/src/shaderpack.rs`), which a large game's cache exceeds (measured at 8.6 GiB for a 42,848-shader game). Enabling it is a decision for the pipeline's cache limit too.

## 5. Verification

Oracles are the Python's own outputs on the same inputs. Fixture-dependent tests skip with a message when the game files or capture are absent. Counts below are from runs on the dates stated; the full-game runs need a real game and were not repeated on every change.

```
cargo test --release -p ibuilder-shader
cargo test --release -p ibuilder-shaderbuild --features host-tools,f2-finish
cargo test --release -p ibuilder-shadersplit
cargo test --release -p ibuilder-shader --test oracle -- --ignored full_grizzy_chain   # full-game chain run (needs a capture)
```

| what | result |
|---|---|
| rewrite chain on every ps / vs / cs of one full game (21,990 shaders: 21,845 ps / vs / cs plus 145 gs) | all 21,845 byte-identical to the Python's library; the gs are handled by `finish` |
| per-step differential tests (`tests/steps.rs`, `tools/difftest/`): each Python function against its Rust port on real shaders, mutated real shaders and synthetic documents | 272,014 cases, 0 different (26,039 of them are cases where the Python raises and so does the Rust) |
| `finish()` against the Python's library for the same full game (`tests/finish_corpus.rs`, `-- --ignored full_grizzy_identical`) | 21,990 of 21,990 identical, 56,882 files; also with pulling on, with dual / readable-tile lists covering every pixel shader, and with a forced transform-feedback layout |
| `finish` parts the game never reaches (`tests/finish_cases.rs`): reflection of 29 binding kinds, driver reads, raster facts, fragment units, vertex libraries, SPIR-V rewrites, geometry helpers | every case equal, against engine functions run on synthetic MSL / SPIR-V |
| tessellation / mesh / lean kernels (`tests/tessmesh_cases.rs`, committed subset of 948 cases) | equal to the Python; larger corpora (133,843 cases) are generated by `tools/tessmesh/` and all equal |
| `tests/tessmesh_e2e.rs` (synthetic 181-shader game, real translator for 135 of them) | image byte-identical to the Python build's (409 files) |
| driver (`tests/driver.rs`, `jobs_oracle.rs`, `jobs_synthetic.rs`, `translate_oracle.rs`, `real_build.rs`, `live_oracle.rs`, `tables_streamed.rs`, `tool_memo.rs`, `atomic_writes.rs`, stop tests) | `job_for` equal on 2,860 real comparisons and 142 synthetic jobs; translator outputs byte-identical; library image byte-identical (the two table files differ only in the time-stamp line); second run is all cache hits; two builds sharing a cache both give the same image; memo hit / miss rules |
| full-game build with the Rust finisher and real tools | 21,990 shaders ok, 56,885 files, image identical to the Python's apart from the time-stamp line |

Both the chain and `finish` were also built for `aarch64-apple-ios` and `-ios-sim`; the simulator runs of the differential tests and of the full-game chain gave identical output. Host-tool tests (child processes) are Mac-only.

Measured on 2026-11-19 on an M1 Pro (shared machine, load average 20 - 390, indicative):

| | Python | Rust |
|---|---|---|
| MSL chain, one thread | 8.2 ms a shader | 0.19 ms a shader |
| `finish` Rust part, one thread | about 12 ms CPU a shader | 3.4 - 3.7 ms a shader |
| build, 2,459 shaders, 6 jobs | 64 s wall | 9.9 s |
| build, 21,990 shaders, 6 jobs, empty cache | 733 s, 340 MB RSS | 120 s, 160 MB peak footprint |
| same build over a filled cache | | 18.6 s (10 s of it writing the 1.7 GB image) |

The Rust build is bound by the two native tools (about 85 % of thread time); the numbers above include child-process start costs that the app's in-process callbacks do not have.

## 6. Known limits

- **One real game only.** Only that game's MSL exercises the chain; `per_vertex`, `mul_extended`, `float_add_msl`, `gather_offsets`, `hmma_msl`, `shared_store_msl`, `wide_atomics`, `first_store`, `tg_spill`, `vs_tf`, `bit0_mask`, `frag_z_snap` and `vs_base_instance` never fire on it. They are proven only by differential runs on documents built from the Python's own rules. A SPIRV-Cross output shape not considered there is the main risk; run `shader_post` / `step_diff` over a new game's capture first.
- Tessellation, task / mesh, passthrough geometry, geometry with transform feedback, the NVN2 branch of `job_for`, bindless handles, and the `gs_slice` success path were checked on synthetic or tool-made input and hand-assembled engine test programs, never on a real game's shader. There is no translator output for a real domain shader.
- Line structure is assumed (see 1). Python's Unicode-aware `\w` / `\s` and unbounded integers are not reproduced (ASCII, `i64` / `i128`).
- Python exceptions that `finish`, `job_for` or `mesh_finish` do not catch stop the Python build; here they become `fatal` / failed / `crashed` for that shader or pair with a different message.
- `drop_point_size` loops forever in the Python on a point size used outside a vertex function; Rust gives up after 4,096 rounds with a failure.
- `ShaderBackend::post_rewrite` (captured-manifest mode of the pipeline trait) is not implemented.
- `live::ids` copies a few helper functions that `ibuilder-shadersplit` also has; `tests/ids_agree_with_shadersplit.rs` checks they agree.
- Not measured: the phone's speed and memory, thermal gating with this backend, a second game, behaviour of the cache when another process prunes it mid-build under real concurrency (tested by deleting an entry after a hit).
- Mesh libraries are composed on every build and not cached.
