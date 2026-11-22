# `ibuilder-shaderbuild`: the stage 3 build driver in Rust (package B)

Plan: `docs/shaders/SHADER-BACKEND-PLAN.md`. Context and measurements of the Python: `docs/shaders/SHADER-STAGE-ANALYSIS.md`. What the chain crate does: `docs/shaders/SHADER-RUST.md`; the per-shader `finish()` (package F2):
`docs/shaders/SHADER-FINISH-RUST.md`. Code: `crates/ibuilder-shaderbuild/` (library, `src/bin/shaderbuild.rs` the Mac CLI, `src/bin/libdiff.rs` an image comparer), the adapter in
`crates/ibuilder-cli/src/adapters.rs` (module `shaders`), the features in `crates/ibuilder-ffi/Cargo.toml`.

**Python source ported** (2026-11-19; the parts the code ports were read, `dmsh_build.py` and `shader_device.py` in full): `engine/dm/gfx/tools/dmsh_build.py` 99,679 bytes / 1,157 lines (mtime 2026-11-16 01:27:59,
identical to the engine copy the harness ran: `cmp`), `dmsh_mtlcache.py` 10,654 B, `dmsh_chain.py` 13,767 B (`inputs`, `profile_rows`, `table_pairs`, `write_stamp`), `dm/tools/shader_lib.py`
12,335 B, `dm/games/game.py` 30,020 B, and the phone's driver `gameapp/shader_device.py` of the iOSBuilder worktree `$IOSBUILDER_WORKTREE`: **37,785 bytes / 585 lines, mtime 2026-11-19 00:48:32**
(`cmp` with the copy in `$OLD/shader/bin`: identical, re-checked at the end of this work; `gameapp_pipeline.py` there is 155,544 B, mtime 02:24, only its `build_shaders_on_device` / `learn_shaders`
call sites were read).

## 1. What it ports (Python -> Rust)

| Python | Rust (`crates/ibuilder-shaderbuild/src/`) |
|---|---|
| `dmsh_build.py` l.1-76 (profile, `GAME`, `MAP*`, `BF_WANT`, `DUAL_WANT`, `FACTS`), l.350-363 (`pc_of`, `loc`, `MAP_ROWS`), l.524-541 (`load_pairs`, `build_keys`), `dmsh_chain.py` `inputs` / `profile_rows` / `table_pairs` / `mesh_pairs` / `write_stamp`, `game.py` `Game` | `game.rs` |
| `record_index`, `rec_of` (l.77-85, 943-956) | `records.rs` (positioned reads, no map) |
| `job_for`, `vertex_ab`, `early_fragment_tests`, `ps_omap`, `tf_layout` (l.365-481) + `dmsh_ampere` (`is_nvn2`, `c1_segment`, `inst_len`, early flag) + `dmsh_tess.control_mode` + (round 3) the task / mesh branch (`dmsh_meshstage.info / memory / own_shared`, `mesh_stage` / `stage_memory` / `local` / `shared` of the job) | `jobs.rs` (the `dmsh_meshstage` functions are `ibuilder_shader::finish::meshstage`) |
| `partner_reads` (l.551-566: the SPH input maps of a mesh shader's paired pixel shaders) | `mesh.rs` `RecordPartners` (an `ibuilder_shader::finish::Partners`; `Finisher::finish_with`) |
| `mesh_finish` and its loop (l.1080-1117: `<vertex key>_<geometry key>_m3`, `dmsh_mesh.compose`) | `mesh.rs` `build_mesh_libs` over `ibuilder_shader::finish::mesh::compose`; called by `build.rs` between the summary and the image |
| `shader_key`, `pair_keyed`, `CODE_FP`, `TOOL_FP` (l.96-121, 519-522) | `key.rs` (+ `build.rs` at the crate root: the source fingerprint) |
| the SQLite store: `output`, `commit`, `cache_outputs`, `restore_cache`, `cache_lookup`, `publish_cache`, `STAGE` (l.123-243), `shader_cache.py` | `cache.rs` (a content-addressed folder; **not** compatible with the SQLite file, by design) |
| `publish_library`, `shader_lib.write`, the image epilogue of `shader_device.build` (l.562-583) | `library.rs` (streamed `DMROMFS1`), `build.rs` (names, stamp) |
| `c1_guard.py` / `cb_extent.py` run over the Mac image (`dmsh_build.py` l.1153-1156, `shader_device._script`) | `tablerows.rs` (per shader, at commit time) over `ibuilder_shader::tables::{unproven_in, cb_tokens}` |
| `NJ`, `BATCH`, `translate`, `xlate_errors`, `restored`, `fin`, `finished`, `progress`, the two phases, the summary (l.874-1135) | `build.rs` |
| `shader_device.build` (l.513-585), `_Progress` | `build.rs` (`build`), progress lines in the Python's format |
| `shader_device.live_package`, `_live_translate`, `live_translate` (l.378-474) + the parts of `shader_split.py` they call (`skey`, `glslc_stage`, `nvn2_ctl`, `STAGE_NAME`, `CTL_MAGIC`) | `live.rs` (`ids` is a **copy** of the 70 lines, see section 6) |
| `shader_device._FakeSubprocess._tool` (argv without the program name) | `tools.rs` (`FnTools`, `CountingTools`, `ProcessTools` behind `host-tools`) |
| `json.dump` spellings (`", "`, `": "`, `ensure_ascii`, `indent=1`) | `pyjson.rs` |
| the per-shader `finish()` | **not here**: `ibuilder_shader::finish` (F2) behind `finish::Finisher`; `finish_adapter.rs` (feature `f2-finish`) joins them |
| the split | **not here**: `ibuilder-shadersplit` (S); the cli adapter calls it |

The pipeline side: `crates/ibuilder-cli/src/adapters.rs` `shaders::RustShaders` implements `ibuilder_pipeline::adapters::ShaderBackend` (`split` -> `ibuilder_shadersplit::split`, `build` and `live_translate` ->
this crate, `split_version`); `shaders::fn_tools` turns the pipeline's `ShaderTools` closures (the app's C callbacks, already behind the memory/thermal gate, argv **with** the program name first) into
`ibuilder_shader::tools::ShaderTools` (argv without it); `shaders::process_tools` (feature `shader-host-tools`) is the Mac's native binaries as pipeline tools. cli features (default on:
`shaders`, `shader-finish`, `shadersplit`; off: `shader-host-tools`, which uses `Command` and must never be in the app). `ibuilder-ffi` needs nothing but features (`real-shaders`, `real-shader-finish`,
`real-shadersplit`): its `real-stages` already takes cli's `real_adapters`, and the app's `xlate` / `spirv_cross` callbacks already land in `Adapters::tools`.

## 2. How to run it

```
export CARGO_TARGET_DIR=$SCRATCH/target-B
cargo test  --release -p ibuilder-shaderbuild --features host-tools,f2-finish          # 25 tests (+2 ignored); the oracle tests skip with a message when a fixture is absent
cargo build --release -p ibuilder-shaderbuild --features host-tools,f2-finish          # bins: shaderbuild, libdiff
shaderbuild build --pkg $OLD/shader/split-out/pkg --profile $OLD/shader/profile --game 0100DFA027504000 --out <dir> --chain b24b25240cad \
    [--keys $OLD/shader/keys-s1800.txt] [--jobs 6] [--tables 0|1] [--pulling 0|1] [--cas <dir>] --engine <engine dir with dm/gfx/tools/{dmsh_xlate_bin,spirv_cross_bin}> \
    [--finisher rust | mock | replay:<library image>]
shaderbuild live  --entries <dir of <key>.ctl/<key>.code> --pack <dir> --derived <dir> --game <id> --work <dir> --chain <chain> --engine <dir> [--finisher rust|mock]
libdiff <a.img> <b.img> [--ignore-first-line c1_full.tsv --ignore-first-line cb_extent.tsv]       # file by file; exit 0 identical
```
`--finisher replay:<image>` hands back, for each key, the files of a Python library image: it proves the driver's own work (digests, batches, pairs, cache, library, tables, stamp) independently of
F2's code. `mock` runs spirv-cross and writes a stub; its output is **not a library** (the pipeline adapter never uses it: without the `shader-finish` feature `build` fails with a message that says so).

API: `build(&BuildOptions, &BuildEnv) -> Result<BuildReport, String>` (`BuildEnv { tools, finisher, log, cancel }`; `log` is called from any thread, the progress lines `cache check: i/N shaders` and
`compiling: n/N shaders` are the ones `ibuilder_pipeline::shaders::parse_progress` matches), `live::live_translate(entries, tables_dir, game_table_dir, game_id, work, chain, tools, make_finisher, log, cas)`,
`tools::{FnTools, CountingTools, ProcessTools}`, `finish::Finisher` (`fingerprint`, `finish(key, work, job, c1, tools, paired_locs) -> Finished`).

## 3. Design

* **Streaming, one pass per shader, no SQLite.** The Python staged every output in a SQLite file, read them all back for the Mac image, wrote the iOS image, then rewrote it (`build()`'s epilogue) and
  ran two tables tools over the first: three image writes, 3 x 1.7 GB for Grizzy, and 37 % of its wall time in the tables (`docs/shaders/SHADER-STAGE-ANALYSIS.md` 0). Here a shader's `finish` outputs are written once, as one cache entry
  (below); the tables' rows are computed from those bytes at that moment (`c1_guard.unproven` and `cb_extent.row` answer from one shader's own files) and stored in the entry; at the end the image is
  copied entry by entry in name order into the output (`library::write_image`: 16-byte aligned, 24-byte entries, the Python's layout, 8 MiB buffers; peak footprint of the whole build 160 MB for 21,990
  shaders, the first port's writer needed the 1.9 GB of files in memory).
* **Phases and batches as the Python's.** Everything without pairs first, then the vertex shaders with pixel-shader partners (`vs_pad` reads the partners' MSL), in batches of `4 * jobs`: a translator
  thread makes the jobs of the next batch (`job_for`, the `<key>.bin` files) and runs `tools.xlate([jobs.json, dir])` in `jobs` threads over `miss[i::jobs]` while `jobs` worker threads (8 MiB stacks)
  finish the current batch; at most two batches' working folders exist; the first paired batch waits for all earlier shaders. A paired vertex shader is given each partner's `user(locnN)` set
  (`Finished::ps_locs`, kept in memory and in the entry: no 20-45 KB of MSL is held per pixel shader). Threads are `std` threads, the digests/lookups use rayon. The memory/thermal gate is applied
  where the Python applied it: the **tools** the caller passes are already `gated` (every translator / spirv-cross call waits), nothing else needs a hook; `BuildEnv::cancel` stops between batches.
* **The cache key is not the Python's, its outputs are** (rule 1 of the plan). `key::shader_digest` = sha256 of tagged parts: `CODE_FP`, the chain id, `Finisher::fingerprint`, `spirv-cross --version`,
  `pulling`, the translator options, the package name, the record, the map row, the conflict row, the answer-table row (compact sorted JSON), the profile's `nvn_*`/`shader_*` rows, the pairs, `dual_want`,
  `bf_want`; a vertex shader with pairs then hashes in its pixel shaders' digests (`pair_keyed`). `CODE_FP` includes a hash of this crate's `src/*.rs` and of `ibuilder-shader`'s `src/**` computed by
  `build.rs`: an edit to the finish port or the driver changes every digest, so **nothing has to be bumped by hand and a stale entry cannot be served** (the Python hashed its own source for the same
  reason; a vendored crate without `ibuilder-shader` beside it falls back to the crate version, `IBSB_SRC_FP=none`). Cost: an edit of a comment is a cache miss.
* **The cache** (`cache.rs`): `<cas>/cas-<chain>.dmsb/<hh>/<digest>`, one container per shader (`"DMSBCAS1"`, meta JSON, a file table, the files) holding the shader's library files under their final names and
  the table rows. Immutable; written to `<name>.tmp-<pid>-<thread>` and renamed (atomic: a reader sees a whole entry or none; a kill leaves a stray temp file, swept after an hour). **Prune-safe**: a hit is
  *hard-linked into the build's own stage folder* when it is looked up and the library is composed from there, so an entry deleted by another build or by the app's cache limit cannot take bytes from a
  running build (a filesystem without hard links falls back to the entry in place: a vanished entry is then the one error, rerun resumes). **Concurrency-safe**: nothing is updated in place, two builds
  (stage 3, a live translation during play) can share the folder. **Resumable**: one entry per shader, written as it finishes (what the pipeline's `ShaderResume` relies on). Other chains' folders can never
  hit and are removed at the start (the pipeline's `shader_cache_folder` deletes `cas-<other>.db` *files*; a folder is this crate's to remove). A truncated or damaged entry is a miss. A vertex shader
  built while a partner failed is **not cached** (the Python's `publish_cache` rule: its padding is incomplete); a shader that fails is not cached and leaves no files.
* **Failure semantics.** A malformed shader (`sys.exit('[RED] ...')` in `job_for`/`vertex_ab`, which stops the Python build) fails that shader (`translation failed`, the message in the log) and the build goes on;
  a `finish` that panics (unwinding builds only: the release profile aborts) is `finish panicked`; the translator's `[RED] key: message` lines are the Python's `xlate_errors` (160 characters). The summary
  lines (`shaders N: ok X, <status> n`, `shader cache: hit H, rebuilt R`, the top 10 reasons per status, the stamp, the tables' counts) are the Python's.
* **Live** (`live.rs`): `live_package` (key check with the profile's `shader_key_skip`, stage, record format, the cs facts) and `_live_translate` (merge of the live rows into the map, pair filtering, facts
  appended, a partial build of `sorted(ok)` with `jobs = clamp(len, 1, 4)`, no tables, no pulling) with the Python's messages. The finisher is built **after** the merged profile exists (`FinisherFactory`).
* **Portability.** `#![forbid(unsafe_code)]`; no `fork` / `exec` / `Command` in the library (the process tools are behind `host-tools`); positioned reads (`std::os::unix::fs::FileExt`, which iOS has), hard
  links with a copy fallback; no memory maps. Dependencies beyond the allowed list: `memchr` (already in the lock), `serde_json` with `preserve_order` (the job's key order is the Python dict's),
  `sha2` (also a build dependency, for `build.rs`), `memmap2` only for the two Mac bins and one test.

## 4. Verification (everything below was run; command, then result)

All on this Mac (M1 Pro, 8 cores), **shared with other agents** (load average 20-390 while I worked: timings are indicative; the orchestrator re-runs the benchmark). Oracles are the Python's own outputs;
the Python was rerun by me (an instrumented *copy* of the engine, `tools/make_jobdump_engine.sh`) and is deterministic: its rerun images are `cmp`-identical to the stored `runs/s300-j1` and `runs/s1800-cap`.

| check | command / fixture | result |
|---|---|---|
| `job_for`, real shaders | `cargo test -p ibuilder-shaderbuild --test jobs_oracle`: the Python's `json.dumps(job)`, `<key>.bin` bytes and `c1` right after its `job_for`, for the 401 (`keys-s300`) and the 2,459 (`keys-s1800`) shaders, two samples that may overlap (stages 260 + 1,539 ps, 110 + 700 vs, 25 + 200 cs, 6 + 20 gs; the 25 + 200 cs have an answer-table row) | **2,860 / 2,860 comparisons identical** (job text byte for byte, code bytes, c1 bytes) |
| `job_for`, what Grizzy does not contain | `tests/jobs_synthetic.rs` against the Python functions *extracted* from `dmsh_build.py` by `ast` and run on 95 seeded synthetic records (`tools/gen_synth_jobs.py`): VertexA+B merge (EXIT -> BRA rewrite, both failure exits), transform feedback (3 valid layouts and 7 invalid ones, for vs and gs: stride / count 0 and 129 / size / location errors, with the `[RED]` lines), early fragment tests, depth-writing SPH, domain tessellation modes, headered / dirty / short compute, c1 ranges, answer-table merge (floats, unicode), Switch 2 records (vs/ps/gs/hs/ds/cs, c1 inverted/past, `nvn2_cs`, `stage_bank`) | 91 identical jobs + **3 identical `sys.exit` messages**; the NVN2 mesh record is refused as unported (checked) |
| the translator's answers | `--test translate_oracle` (`host-tools`): our jobs through the real `dmsh_xlate` | 401 `.spv` and 401 `.res.json` **byte-identical** to the Python's (`tr.spv`/`tr.res.json` saved by the instrumented copy) |
| the driver alone (library layout, stamp, phases, pairs) | `shaderbuild build ... --finisher replay:<Python image>` over the real translator: `keys-s300` (401 shaders) and `keys-s1800` (2,459) | images **byte-identical** to `runs/s300-j1` (38,962,236 B, 1,142 files) and `runs/s1800-cap` (222,345,770 B, 6,782 files), `cmp` |
| the driver alone, full Grizzy with tables | same, no `--keys`, `--tables 1`, `replay:runs/full-gil-j6/b-b24b25240cad.img`, `--jobs 6`: 21,990 shaders | `libdiff` against the Python's 1,706,277,556-byte image: 56,885 files, **56,883 identical, the 2 tables identical but for their time-stamp line**, 0 different; stamp `inputs 8ea0535d1d1c`; `c1` 6,679 keys, `cb_extent` 21,522 shaders / 49,309 slots / 1,578 read-only storage slots (the first port's counts) |
| **the stage with F2's finish** | `shaderbuild build ... --finisher rust` (`ibuilder_shader::finish`, real tools): `keys-s300`, `keys-s1800`, the full library | 401 / 2,459 / **21,990 shaders ok, 0 failed**; `keys-s300` and `keys-s1800` images **byte-identical** (`cmp`; also `tests/real_build.rs`); full library 56,885 files: **56,883 identical + 2 tables identical but for the time-stamp line** against `runs/full-gil-j6` (`libdiff`) |
| **round 3: task / mesh `job_for`, `mesh_finish`, partner reads** | `tests/jobs_synthetic.rs` (64 NVN2 task / mesh records added to `tools/gen_synth_jobs.py`); `tests/tessmesh_e2e.rs` over a synthetic game (`docs/shaders/SHADER-TESS-MESH-RUST.md` 3.3) | **142 identical jobs** (48 of them with `mesh_stage`), 16 records where the Python crashes are errors here; the game: 181 shaders (135 through the real translator), image **byte-identical** to the Python build's (409 files), 201 translator jobs identical, mesh libraries built 4 / missing 2 / not composed 1 as in the Python |
| tables streamed | `tests/tables_streamed.rs`: per-shader rows assembled vs `ibuilder_shader::tables::{c1_guard, cb_extent}` over the Python's `s1800-cap` image | 990 c1 rows, 2,401 cb rows: equal |
| cache and driver behaviour | `tests/driver.rs` (6): second run = all hits, 0 translator calls, same image; another chain / `pulling` never hits; paired vertex shaders start only after every unpaired shader is finished and get each partner's locations (291 + 110 shaders); failed shaders leave no files, the build goes on, a vertex shader built without its failed partner is not cached; a cancelled build keeps what it finished (resumed: 132 of 401 came from the stopped build's entries); unknown keys / missing profile are errors; **two builds at once over one cache folder** both give the Python's image, 401 whole entries, no stray temporary file | pass |
| live | `tools/run_live.py` = the phone's `shader_device.live_translate` on 16 real shaders (ps/vs/cs/gs) + 3 bad entries -> `tests/live_oracle.rs` | ok / bad / file names and bytes identical (84 files), the Python's refusal messages, and the live **library image identical** (stamp included); also with the real finish and tools |
| pipeline request | `crates/ibuilder-cli/tests/shaders_pipeline.rs`: `RustShaders.build(&BuildRequest{..})` over a mini game (the profile cut to the 401 shaders), real finish + real tools | the 401 shaders' files equal the Python's; 9 progress lines parsed by `ibuilder_pipeline::shaders::parse_progress`; `c1_full.tsv` / `cb_extent.tsv` at `<out>/_work/profile/<id>/` where the pipeline looks |
| glue | `tests/shaders_glue.rs` | argv with / without the program name; without `shader-finish` / `shadersplit` the backend refuses in words (no stand-in library); bad live entries refused |
| suites | `cargo test --release -p ibuilder-shaderbuild --features host-tools,f2-finish` | 11 unit + 6 + 2 + 1 + 2 + 1 + 1 + 1 + 1 (`tessmesh_e2e`, round 3) = **26 pass, 2 ignored** (the two big real builds above, run by hand); `cargo test --release -p ibuilder-cli --features shader-host-tools`: all pass (3 + 4 + 2 + 1) |
| iOS | `PATH=/opt/homebrew/opt/rustup/bin:$PATH IPHONEOS_DEPLOYMENT_TARGET=14.0 cargo build --release --target aarch64-apple-ios{,-sim} -p ibuilder-shaderbuild`; the test binaries (`cargo test --target aarch64-apple-ios-sim --no-run --features f2-finish`) run in the booted iPhone 17 (`xcrun simctl spawn <SIMULATOR-UDID>`): unit, driver (incl. the Python-identical image with hard links), jobs, synthetic, live, tables | both targets build; **22 tests pass in the simulator** (11 unit, 6 driver, 2 + 1 + 1 + 1 oracle tests; the host-tools ones cannot run there). `ibuilder-ffi --features real-stages` (now including this backend, the split and the finish): `libibuilder_ffi.a` builds for device and simulator (65.6 MB each), 29 exported `ibuilder_*` symbols as before |

## 5. Numbers (indicative: a shared Mac; the machine state is stated)

`keys-s1800` (2,459 shaders), `--jobs 6`, tables off, the engine's native tools as child processes in both, back to back (load average 19-30):

| | wall | CPU (user + sys, children included) | max RSS / peak footprint |
|---|---|---|---|
| Python (`run_build.py` -> `shader_device.build`, GIL CPython 3.14) | **64.2 s** | 64.7 + 37.8 s | 155 MB / 107 MB |
| Rust (`shaderbuild build --finisher rust`) | **9.9 s** (6.5x) | 36.3 + 15.8 s | 115 MB / 112 MB |

Full Grizzy (21,990 shaders, `--jobs 6`, tables on, fresh cache, children as processes, load 47 -> 20): **120.1 s wall**, 312 s user + 143 s sys, max RSS 164 MB, **peak footprint 160 MB**; inside the tool calls (summed over
the 6 threads): translator 5,802 calls 402 s, spirv-cross 23,016 calls 357 s = the whole thread time: the build is bound by the native tools (+ 28,800 child-process starts, ~100 s of CPU that the app's in-process
callbacks do not have: ~86 s of CPU by the start-up costs measured in `docs/shaders/SHADER-STAGE-ANALYSIS.md` 3.3), not by Rust. Same build again over the filled cache: **18.6 s** (21,990 hits, 0 tool calls, 10.3 s of that is writing the 1.7 GB image), max RSS 95 MB. Reference Python, same build,
same driver (`docs/shaders/SHADER-STAGE-ANALYSIS.md` 3.1, a different load): **733 s wall, 340 MB RSS / 364 MB footprint** -> about 6x faster and half the memory; on the phone the translator and spirv-cross run in
process (no spawn) so more of the remaining time is the translator itself. Per-job memory is ~12-20 MB (footprint 87 MB at 4 jobs, 112 MB at 6): `SHADER_PER_JOB` and `shader_free_gib` in the pipeline were
sized for the Python (4.4 GB peak) and can be relaxed; the cache holds 1.6 GB for Grizzy and the library is 1.7 GB: about 3.3 GB of disk at the peak (the stage folder is hard links, no extra bytes).
Driver-only costs (replay finisher, real translator, full Grizzy, load 390): digests 0.13 s, library write 19.5 s (slow disk under load).

## 6. What is NOT done, gated, or not verified

* **Mesh libraries** (`<vertex>_<geometry>_m3`, `dmsh_mesh.py`, `mesh_finish`): built since round 3 (`mesh.rs`; the log line is the Python's, `mesh libraries (...): built N, missing a library M, not composed K`; `BuildReport::{mesh_built,
  mesh_missing, mesh_not_composed}`, `mesh_pairs_unported` is always 0 and kept for API compatibility). The libraries are written under `<out>/_work/mesh/` and streamed into the image from there (no cache entry: composing is a text
  rewrite of two cached files). Verified differentially on a **synthetic game** only (`docs/shaders/SHADER-TESS-MESH-RUST.md` section 3.3); Grizzy has no passthrough pairs. A pair whose Python run would crash (`TypeError` on a
  `[[buffer]]` without an index, `KeyError` on a missing `capture_reads`) is counted as `crashed` and gets no library, where the Python's build stops.
* **Switch 2 task / mesh shaders** (`dmsh_meshstage`): `job_for` builds their job (round 3; compared with the Python's `job_for` on 64 synthetic task / mesh records plus the end-to-end game, section 4); `finish` finishes them
  (`ibuilder_shader::finish`). The rest of the NVN2 branch of `job_for` is ported and checked **only on synthetic records** against the Python (section 4), never on a real Switch 2 shader (no fixture here).
* **`ShaderBackend::post_rewrite`** (the captured-manifest mode): the trait default (error); not ported.
* The tables' header time stamp is **UTC** (`+00:00`), Python wrote the local offset (no time-zone database on the phone); the files are otherwise identical. The Python's cache "reasons" (`rule changed`, `code
  fingerprint changed`) are all `new key` here. A missing `shader_map.tsv.conflicts.tsv` is read as empty (the Python crashes).
* `live::ids` is a **copy** of `skey` / `glslc_stage` / `nvn2_ctl` / `STAGE_NAME` / `CTL_MAGIC` from `shader_split.py` (package S's `ibuilder_shadersplit::live` has the same functions; the cli could assert
  equality; switch to a re-export if the dependency weight is acceptable: S's crate pulls `ruzstd`, `flate2`, `lzma-rs`).
* **Not run when this was written** (done since: the CLI has `--shaders --shader-chain --engine`, and `docs/chain/E2E-RUST.md` runs the whole pipeline on Grizzy through it; the FFI path is still not run); anything on a physical iPhone; the app's real in-process translator callbacks (the tests use child processes and a stand-in); a second game (only Grizzy exists locally);
  the cache under another process pruning it *during* a build (tested by deleting an entry after the hit, `cache::tests`, not by a racing build); the Python-compatible reasons; thermal / memory gating
  behaviour of the pipeline's gate with this backend (the gate only wraps the tools, tested in the pipeline).
* **Hand-offs to other packages (not changed by me)**: the pipeline's `shader_cache_folder` named the engine's `cas-<chain>.db` (fixed since, `docs/chain/PIPELINE-RUST.md` section 9: it keeps the folder
  `cas-<chain>.dmsb` and removes the rest, folders included; other chains' folders are also removed by this crate; `folder_bytes` already counts folders); `SHADER_PER_JOB` / `shader_free_gib` could use the numbers above; F2 could expose a
  `FINGERPRINT` const (the cache digest gets the same from `build.rs`, so it is optional).
* **Workspace note**: the shared workspace did not load for a good part of the work (F1/F2 half-written); everything was built and tested in private workspaces (`$SCRATCH/B/ws*.sh`), and at the end in
  the shared one (`cargo test -p ibuilder-shaderbuild --features host-tools,f2-finish` and `-p ibuilder-cli --features shader-host-tools`: pass).

## 8. Round of 2026-11-21: the tool memo (second cache level), review items, measurements

### 8.1 Why a second level, and why it cannot serve a stale answer

A shader's digest (`key.rs`) carries `CODE_FP`, a hash of every source of this crate and of `ibuilder-shader` (`build.rs`): any edit of the Rust finishing, even one that changes no byte (a comment, the perf
edits of this round), makes every shader a miss, and a miss runs the translator and SPIRV-Cross again. That is deliberate (no stale entry can be served; a hand-kept "behaviour version" was rejected for the
same reason: forgetting to bump it serves stale libraries). The measured Ocarina build below shows what a miss costs: **86-89 % of the build's thread time is inside the two native tools**
(translator 12,399 calls, SPIRV-Cross 44,532 calls for 42,848 shaders), so a finishing change costs a whole cold build.

`toolmemo.rs` (`MemoTools`, `BuildOptions::tool_memo`, CLI `shaderbuild build --tool-memo 1`) keeps the tools' own answers, keyed by nothing of the Rust code: translator entries per **job** (the memo format, the chain
id, the translator's `--source-sha`, the job's JSON without its `code` path, the bytes of its `code` file; stored: every file the translator wrote for the job, captured in an empty private folder, so a later
translator's extra outputs are kept too), SPIRV-Cross entries per **call** (the chain id, the `--version` commit line, the arguments with the input file replaced by its bytes and `--output` by a placeholder; stored: the
output file, stdout, stderr). Only whole successes are recorded (exit 0, no `[RED]` line for the job, no unknown output file); a tool without an identity is not memoized; a damaged entry is a miss and is rewritten.
With the memo on, the translator's `--source-sha` also enters every shader's digest (the chain id covers the translator's sources, but a chain given by hand on the Mac may not). Layout:
`<cas>/cas-<chain>.dmsb/t/<hh>/<digest>`, swept with its chain. Assumptions (the same as the first level's and the Python's cache): the tools are deterministic, the translator reads only its job file and the
jobs' `code` files and writes only under its output folder, the chain id or the source sha changes with the translator's code.

Tests (`tests/tool_memo.rs`, 8, stand-in tools whose answers carry their version, a synthetic game): a repeated job is a hit with the same bytes wherever its code lies; only missed jobs reach the translator;
a new translator / job field / stage / code / chain is a miss; failures and failed calls are never kept (also a `[RED]` job that has files in a call that exits 0); a damaged entry is a miss and is rewritten;
no identity = no memo; an unknown output file records nothing; SPIRV-Cross keyed by input bytes, arguments and version, stdout answers included; and the whole build: **a finishing change ("f1" -> "f2") re-finishes all 40
shaders with 0 translator calls and 0 SPIRV-Cross runs, a new translator translates all 40 again (both levels miss), a new SPIRV-Cross crosses all 40 again, memo on and off give the same library files**. Every key part was
removed once on purpose (identity, chain, code bytes, SPIRV-Cross input bytes, arguments, the `[RED]` check): each of those 6 mutants fails at least one test (run by hand with a throw-away script).

**Off by default**, for two reasons: (1) it roughly doubles the cache folder (the SPIR-V and the raw MSL beside the finished files; not measured on a real game, see 8.3), and (2) the pipeline wipes the whole cache
folder at the start of a build when it is over `shader_cache_gib`, **2 GiB by default** (`ibuilder-pipeline/src/shaderpack.rs` `shader_cache_folder`; nothing in `app/` or the C# sets another value): the
measured Ocarina cache alone is **8.6 GiB**, so on the phone with the default every Ocarina build starts from an empty cache whatever the keys (read in the code, not run on a device). Enabling the memo is the pipeline
owner's decision, together with that limit (`BuildOptions::tool_memo = true` in `ibuilder-cli/src/adapters.rs`).

### 8.2 Review r3 items of this crate

5.23 (`mesh.rs`: a library composed but not written has its own bucket `failed to write`, unit test `a_library_not_written_has_its_own_count`): in place. Test harness: `tests/common/mod.rs` `keys_file` read the
fixture list before the fixture check, so `real_build::the_300_shader_sample_is_the_pythons_library` **panicked** (`No such file or directory`) on a machine without the Grizzy fixtures instead of skipping; it now skips
through `ibuilder_testkit::skip` (`driver.rs` and `real_build.rs` call it after their own checks).

### 8.3 Measurements on Ocarina of Time (Switch 2, 42,848 shaders) and what the reboot took

Run before the machine rebooted (the scratch folder with the extracted game, the binaries and the logs was lost; the numbers were read from the runs' outputs at the time): the split and the build of stage 3 with the
binaries **before** and **after** this round's edits, `shaderbuild build --jobs 6 --tables 1 --finisher rust --engine $IOSBUILDER/engine`, fresh cache each, the engine's tools as child processes, a heavily loaded Mac
(load average 27-125 during the builds, other agents compiling):

| | before | after |
|---|---|---|
| split (`shadersplit --jobs 8`) | 61.35 s, 1.22 GB max RSS | 61.23 s, 1.25 GB; the 18 output files **sha256-identical** |
| build wall | 473.36 s | 473.38 s |
| inside the tools (summed over 6 threads) | translator 1,350.6 s, SPIRV-Cross 1,186.7 s | 1,265.2 s, 1,174.8 s |
| process CPU (children included) | 1,535 s user + 336 s sys | 1,444 s user + 342 s sys |
| library | 9,109,648,504 B, 119,517 files | same size and count |
| library sha256 | 7a2153e9... | c0c43da1... |
| cache folder | 8,983,116 KiB | 8,983,116 KiB |
| peak footprint | 349 MB | 368 MB |

The wall time does not move: the build is bound by the tools' child processes. The regex edits remove ~4 ms of CPU per vertex shader (`docs/shaders/SHADER-FINISH-RUST.md` section 6), which is in the
91 s smaller user CPU only within this load's noise. **The library identity is NOT proven**: the whole-image sha256 must differ (`c1_full.tsv` / `cb_extent.tsv` start with the build's time stamp), and the
per-file comparison of the second "before" build was cut by the reboot. What stands instead: the before / after harness over every fixture input of `ibuilder-shader` (`docs/shaders/SHADER-FINISH-RUST.md` section 6),
the Python oracle tests, and the identical split. The tool-memo measurements on the game (cache size with the memo, the time of a re-finish from the memo) never ran; the estimate from the table: a finishing change with
the memo skips the ~2,500 thread-seconds inside the tools, leaving ~300 thread-seconds of Rust and I/O plus the 35-39 s library write, **[E] ~60-90 s instead of ~473 s** on this Mac.

## 7. Files

`crates/ibuilder-shaderbuild/{Cargo.toml, build.rs, src/*.rs (2026-11-21: `toolmemo.rs`), src/bin/{shaderbuild,libdiff}.rs, tests/*.rs, tools/{gen_synth_jobs.py, make_jobdump_engine.sh, run_live.py}}`,
`crates/ibuilder-cli/{Cargo.toml (features), src/adapters.rs (module shaders), tests/{shaders_glue,shaders_pipeline}.rs}`, `crates/ibuilder-ffi/Cargo.toml` (features), this file.
Oracle runs (not in the repo): `$SCRATCH/B/oracle/{jobs-s300, jobs-s1800, synth, live, run-s300, run-s1800}`, logs `$SCRATCH/B/run/*.log`.
