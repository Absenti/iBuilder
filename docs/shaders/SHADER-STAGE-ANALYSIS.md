# Stage 3 (the on-device shader translation): call tree, measurements, design, estimate

Crate: `crates/ibuilder-shader` (what was ported: `docs/shaders/SHADER-RUST.md`). Everything here is a measurement with its command, or says it is an estimate.
**[M]** = measured by me, **[D]** = from the iOSBuilder documents/logs (not re-measured), **[E]** = my estimate.

## 0. Bottom line

* The 809 s of Grizzy's stage 3 on the iPhone **[D]** is Python under one GIL, as `PERF-RESEARCH.md` says, but the *finishing* chain is only part of it. On the Mac, with the
  same driver (`gameapp/shader_device.py`, GIL interpreter, 6 build threads, same engine Python), the full Grizzy build took **733 s** **[M]**; of the Python process's 802 s of CPU:
  **269 s (34 %) is the two post-build tables** (`c1_guard.py` 90 s + `cb_extent.py` 179 s, serial under the GIL), 153 s (19 %) the MSL post-rewrite chain
  (7.0 ms a shader), 95 s (12 %) the other finishing steps (reflection, pixel-shader side libraries, ...), 131 s (16 %) glue (JSON, SQLite cache, file copies), the rest (19 %)
  start-up, job building and harness overhead. The native translator (C#) and spirv-cross are **~213 s of CPU in all (9.7 ms a shader) and run outside the GIL** (the 856 -> 809 s of the parallel translator [D] implies it).
* So Python is ~80 % of the CPU and ~100 % of the wall (it serialises). The tables alone are ~37 % of the wall when the build runs them (they are the default,
  `shader_tables`; `shader_device.build`'s docstring says the host works without them) and are the one block that is **self-contained, serial, and trivially verifiable**.
* Ported and verified byte-for-byte (details in `docs/shaders/SHADER-RUST.md`): the post-rewrite chain (96.5 % of a 2,459-shader random sample byte-identical, the rest gated, 0 differences),
  both tables (identical to the Python's, and to an independent earlier full Mac build), the library image writer (identical, 1.7 GB).
  Rust timings: chain **0.23 ms a shader on one thread vs 8.2 ms in Python (36x)**; tables **16.7 s on one thread / 4.3 s on 6 threads vs 269 s** in Python under the GIL.
* What those ports remove: ~422 s of the 802 s Python CPU (53 %), i.e. **[E] ~733 s -> ~320 s on this Mac, ~809 s -> ~350 s on the phone** under the same assumptions.
  Porting the remaining Python (design in section 6) leaves the native translators plus I/O: **[E] ~50-80 s on this Mac, ~80-150 s on the phone**.
* Whether the free-threaded interpreter (now the default on the phone) helps is not settled by my runs: on the (heavily loaded) Mac it was 9 % faster in wall time and used
  **2.0x the CPU** (section 4.4). I did not measure the phone.

## 1. What runs on the phone (stage 3), end to end

Entry: `gameapp_pipeline.build_shaders_on_device` (worktree `gameapp/gameapp_pipeline.py` ~l.1990-2160) -> `gameapp/shader_device.py` (the engine's own scripts run in-process with
`subprocess`, `multiprocessing`, `ctypes`, `fcntl`, `zstd` replaced by stand-ins, `shader_device.py` l.32-283), once for the **split** and once for the **build**.

| # | step | code | reads -> writes | native tools |
|---|---|---|---|---|
| 1 | **split** (reading the game's shaders), 12 s on the phone [D], 13.9 s / 1.8 GB RSS on the Mac [M] | `shader_device.split` -> engine `shader_split.py` (3,821 lines) with a thread pool and a zstd callable | `RomFS.img` + `main` NSO -> `pkg/{ps,vs,cs,gs}_{data,main}.bin` (records `BTCDBTCD`+key+control section+code, 127 MB for Grizzy) and `tables/{shader_map,shader_facts,shader_pairs,pso_pairs}.tsv` (21,990 shaders: 10,541 ps, 10,537 vs, 767 cs, 145 gs) | none (zstd is the app's C callable) |
| 2 | **build start-up**: load profile/facts/pairs, key every shader, look every key up in the cache | `dmsh_build.py` module level: `rec_of` (pread of the record), `shader_key` (sha256 of record + map row + facts + tool fingerprints + code fingerprint), `cache_lookup` (SQLite `cas-<chain>.db`) | pkg + tables -> `digests`, `cache_hits` | none |
| 3 | **translate** a batch (4*NJ shaders, next batch translated while this one is finished) | `translate()` -> `job_for` (parse the record: control section, VertexA/B merge `vertex_ab`, early-fragment flag, transform feedback, facts -> job dict, writes `<key>.bin`) -> one `XLATE` call per chunk | job json + `.bin` -> `<key>.spv` (SPIR-V) + `<key>.res.json` (answer json: storage-buffer slots, cbufs, textures, notes) | **dmsh_xlate** (C#/Ryujinx): on the phone AOT-compiled C# inside the app (`GameAppMain.XlateEntry`, `[UnmanagedCallersOnly]`, `GameAppXlate.Run(job.json, outdir)`, registered with `gameapp_set_xlate`) called as `dm.xlate([...])`, no process, same files; on the Mac the engine's NativeAOT binary as a child process |
| 4 | **finish** one shader (`fin` -> `finish`), `NJ` threads | see 1.1 | spv + res.json -> MSL, reflection json, side-library sources | **spirv-cross** (C++): on the phone `gameapp_spirv_cross` (`spvc_cli.c`, the C API behind the CLI's flags) through `dm.spirv_cross([...])`, in-process; on the Mac a child process |
| 5 | publish the shader: `write_refl` (json.dump indent=1), `clone` (`_b.metal`), `publish_cache` (reload the json, SQLite `put` x outputs), `commit` (`put_file` x outputs into the cache DB, unlink files) | `dmsh_build.py`, `shader_cache.py`, `dmsh_mtlcache.py` (source mode: `Cache.compile/compile_split` write the MSL text under the library names) | | none |
| 6 | **publish_library** x2 (Mac side and iOS side): read every staged blob back out of SQLite and `shader_lib.write` a `DMROMFS1` image | `dmsh_build.publish_library`, `shader_lib.write` | -> `cache/b-<chain>.img`, `ios/b-<chain>.img` | none |
| 7 | **tables**: `c1_guard.py` (shaders whose reads of c1 are not proven bounded) and `cb_extent.py` (how far into each uniform slot a shader reads; unwritten storage slots) over the Mac-side image; `multiprocessing.Pool` is a thread pool on the phone | run with `runpy` in-process (`shader_device._FakeSubprocess._script`) | image -> `c1_full.tsv`, `cb_extent.tsv` | none |
| 8 | `build()` epilogue: read the iOS image, rename `.metallib` -> `.metal`, swap `_b.metal` of a cut unit for the plain MSL, add the tables and the stamp, **`shader_lib.write` again** (a third image) | `shader_device.build` l.562-583 | -> `b-<chain>.img` (1.7 GB for Grizzy) | none |

### 1.1 What `finish()` does to one shader (the per-shader Python)

`dmsh_build.py` l.737-889. In order, per shader (ps / vs / cs; gs has its own `geometry_finish`, 5 re-translations):

1. gs: `gs_slice` (spirv-cross `--version 450` to GLSL, regex over it); vs/ds: `spv_single_lane` (SPIR-V words rewritten in Python, 0.4 ms) .
2. `spirv-cross <spv> --msl --msl-version 20300 --msl-argument-buffers --msl-texture-buffer-native --msl-pad-fragment-output --output <key>.metal` (3.1 for image atomics, `dmsh_atomic.cross_args` reads the SPIR-V to decide).
3. vs only: `dmsh_entry.int_inputs`; if an input is only used as an integer, patch the job, **translate again** and run spirv-cross again.
4. **The post-rewrite chain** (what `tools/shader_post.py` calls `post_rewrites`; the file is rewritten to disk after most steps): `check_msl`, `per_vertex` (ps), `precise`,
   `membar_fence`, `bit0_mask`, `select_guards`, `mul_extended`, `float_add_msl`, `gather_offsets`, `hmma_msl`, `shared_store_msl`, `pool_arrays`, `wide_loads`, `tg_fence`,
   `border_emu`, `frag_z_snap` (ps), `uniform_tests`, `blend_zero` (ps); then per stage: cs `first_store`, `tg_spill`, `max_threads`; vs `vs_init`, `vs_layer`,
   `vs_base_instance`, `vs_pad` (reads the paired pixel shaders' **finished** MSL), `dmsh_geometry.capture`, `vs_depth`, `vs_writes`/`vs_discard`, `vs_tf`, `vs_layer_entry`;
   ps `fs_uint` (+`uint_entry`), `fs_dual`, `shared_body`. 18 + ~10 functions, ~2,000 lines, regular expressions over 20-45 KB of text (92 MB for 2,459 shaders).
5. `dmsh_reflect.reflect` (the argument buffer -> binding list + `decl`), `drv_reads`, `unfillable`, `attributes`, `raster_facts` (+ `dmsh_fetch.screen_varyings`).
6. ps: `ps_libraries`: `dmsh_side.unit` (alpha-test and logic-op entry points: `dmsh_alpha`, `dmsh_logic`), `dmsh_fetch.fold` (readable-tile version), source-mode `compile_split`
   writes the unit under `_b`, `_bx`, `_bf`. vs: `vin_finish` (`dmsh_vin`), `pull_finish` (`dmsh_pull.variant` still runs with pulling off; only the compile is skipped).
7. Assemble the reflection dict, `write_refl`, `clone`, cache, commit (rows 5 of the table above).

Data formats: the translator's job file is JSON (`key, stage, code, tex, img, attr, fout, c1 (hex), local, tf, ...`), its answers `<key>.spv` and `<key>.res.json`; spirv-cross
reads `.spv` and writes `.metal` text; the library is `DMROMFS1` (header, files 16-byte aligned in name order, 24-byte sorted index, name pool; `dm/tools/romfs_image.py`, `shader_lib.py`).

## 2. Can the stage run on the Mac, and what does it need?

Yes, **without keys**: its inputs are `RomFS.img` and the `main` NSO that `nx_extract` already wrote (`iOSBuilder/out/ref-img-grizzy/RomFS.img`, `out/ref-out-grizzy/ExeFS/main`),
the game's provisional profile (`game.tsv`, copied from the worktree's `out-shaders/0100DFA027504000/profile/`, a derived text file) and the engine's Python
(`iOSBuilder/engine/dm/{gfx/tools,tools,ios/tools,games/*.py}`, identical to `iPorterNX/third_party/NX2Apple/engine/dm/gfx/tools`: `diff -rq` empty), copied to the scratch folder. The
native tools are the engine's Mac binaries (`dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate`, `spirv_cross_bin/spirv-cross`) called through the same `tools` callables the phone
passes to `shader_device.build` (the harness wraps them with `os.wait4` timers).

```
S=$SCRATCH_OLD; E=iOSBuilder/engine/dm; D=$S/shader/engine/dm
rsync -a --exclude __pycache__ $E/gfx/tools/ $D/gfx/tools/; rsync -a --exclude __pycache__ $E/tools/ $D/tools/; rsync -a --exclude __pycache__ $E/ios/tools/ $D/ios/tools/
cp $E/gfx/nvn/dmsh_abi.h $E/gfx/nvn/dmsh_chain.h $D/gfx/nvn/; cp $E/games/{game.py,profile_guess.py,settings.py,settings.tsv} $D/games/
cp <worktree>/gameapp/{shader_device.py,lzma_py.py} $S/shader/bin/            # the phone's driver, unchanged
PYTHONDONTWRITEBYTECODE=1 S=$S $S/ref/venv/bin/python -I crates/ibuilder-shader/tools/measure/run_split.py          # 13.9 s -> split-out/{pkg,tables}
S=$S $S/ref/venv/bin/python -I crates/ibuilder-shader/tools/measure/make_prof_engine.py    # an instrumented COPY of dmsh_build.py (timers, cProfile, capture)
S=$S /usr/bin/time -l $S/ref/venv/bin/python -I run_build.py --engine $S/shader/engine-prof --keys all --jobs 6 --tables 1 --out runs/full-gil-j6   # the full build
```

The split's shader map equals the stored one apart from one comment line, `shader_facts/pairs/pso_pairs` are byte-identical (`cmp`). The harness differs from the phone in
exactly two ways: the tools are child processes (their start-up, ~5.4 ms CPU for `dmsh_xlate`, 2.4 ms for spirv-cross, measured with an empty job list and `--version`, is subtracted below)
and the CPU is an M1 Pro, not an A17 Pro. **The Mac was shared with other agents while I measured (load average 7 to 114 on 8 cores): wall times are indicative, thread-CPU times
(`time.thread_time`, `wait4` rusage) are the robust ones, and every comparison below says which it uses.**

## 3. Measurements

### 3.1 Whole build (Grizzy, 21,990 shaders, `--jobs 6`, pulling libraries off, tables on, fresh cache) **[M]**

| | GIL CPython 3.14.1 (`runs/full-gil-j6`) | free-threaded 3.14.1 (`runs/full-ft-j6`, `PYTHON_GIL=0`, the worktree's `out-pydev-ft/mac-ft`) |
|---|---|---|
| wall | **733.4 s** | 671.2 s |
| Python process CPU | 802.0 s | 1,568.0 s |
| `fin` (finish + publish) thread CPU, summed | 488.9 s (22.2 ms a shader) | 1,060.4 s (48.2 ms) |
| tables (`c1_guard` + `cb_extent`), wall | **90.1 + 179.3 = 269.4 s** | 26.5 + 46.1 = 72.6 s |
| `publish_library` x2 (wall) | 13.4 s | not recorded |
| peak RSS / peak footprint (Python process) | 340 MB / 364 MB | 656 MB / 866 MB |
| children (translator, spirv-cross): CPU / largest RSS | 299 s (xlate 139.7 + spirv-cross 159.3) / 46 MB | 345 s / 46 MB |
| the phone, for reference | 809 s (GIL runtime, 6 threads, parallel translator) [D] | not measured |

Output of the run: 21,990 shaders, 0 failed, library image 1,706,277,556 bytes, 56,885 files.

Sample (401 shaders, `--jobs 1`, serial, `runs/s300-j1`): 12.7 s wall = 31.7 ms a shader, process CPU 8.9 s, children CPU 6.4 s.

### 3.2 Where the Python CPU of the GIL build goes (802 s) **[M]**, `it.json` timers (inclusive per function, `thread_time`)

| block | CPU | share | per shader |
|---|---|---|---|
| tables (`c1_guard.py`, `cb_extent.py`) | 269.4 s (wall; GIL-serial) | 33.6 % | 12.3 ms |
| **post-rewrite chain** (the 32 steps ported or gated here) | 153.3 s | 19.1 % | 6.97 ms |
| other finishing: `reflect` family, `ps_libraries`, `vin/pull_finish`, `int_inputs`, `spv_single_lane`, `geometry_finish` | 95.4 s | 11.9 % | 4.3 ms |
| glue: `write_refl` 24.1, `publish_cache` 50.9, `commit` 36.8, `clone` 14.0, `job_for` 4.9 | 130.8 s | 16.3 % | 5.9 ms |
| rest: process start-up and digests, harness child-process management (Mac only, `fork_exec`/`wait4`), JSON of answers, final image writes | ~153 s | 19.1 % | |

Top single functions (CPU, 21,990 shaders): `ps_libraries` 40.7 s, `select_guards` 39.7 s (8,349 shaders changed), `uniform_tests` 24.8 s (713 changed), `fs_uint` 14.8 s,
`geometry_finish` 12.6 s (145 shaders), `blend_zero` 11.9 s, `raster_facts` 11.4 s, `gather_offsets` 9.6 s and `frag_z_snap` 8.5 s (changed 0 shaders), `spv_single_lane` 8.3 s,
`pull_finish` 7.7 s, `int_inputs` 7.0 s, `precise` 6.7 s, `bit0_mask` 6.5 s (changed 0), `tg_fence` 5.5 s. **There is no single hot spot** (the largest, `ps_libraries`, is 40.7 s of 802); roughly 40 % of the
chain's time (~60 s of 153 s) is spent in steps that change a shader rarely or never (`uniform_tests`: 713 of 22,715 calls changed; `gather_offsets`, `frag_z_snap`, `bit0_mask`: none; `tg_fence` 42; `wide_loads` 160): a regular expression that scans the whole text to say "no". Which steps change Grizzy
shaders (`DMSH_CAPTURE_STEPS`, 401 shaders): `precise` 88 %, `blend_zero` 100 % of ps, `border_emu` 63 %, `select_guards` 45 %, `fs_uint` 58 % of ps, `vs_init/vs_depth/vs_pad/capture`
every vs, `vs_layer_entry` 12 % of vs, `uniform_tests` 3-5 %, `wide_loads`/`pool_arrays`/`tg_fence` <1 %.

### 3.3 Native work **[M]**

From `wait4` rusage of every child (22,870 translator jobs in 5,802 calls, 23,015 spirv-cross calls): xlate 139.7 s CPU, spirv-cross 159.3 s. Subtracting the measured process start-up
(5,802 x 5.4 ms and 23,015 x 2.4 ms = 86 s) leaves **~108 s of translator compute (4.7 ms a job) and ~104 s of spirv-cross (4.5 ms a call), 213 s in all (9.7 ms a shader)**.
Both run outside the GIL (on the phone in-process: `dm.xlate` / `dm.spirv_cross` release it), so with 6 threads they cost ~35 s of wall at best.

### 3.4 Is the Python finishing "the bottleneck"? Evidence

* Wall 733 s against Python process CPU 802 s and `fin` wall summed over 6 threads of 2,626 s = 6 x 438 s: each thread waits ~5/6 of the time for the GIL. The loop's throughput (~20 ms a shader of wall, derived: (733 - 269 tables - 13 publish - ~18 start-up and final write) / 21,990) equals the
  Python CPU of one shader (22.2 ms), not the 31.7 ms serial total nor the 9.7 ms native part divided by 6.
* The 856 -> 809 s of "parallel translator" [D] is what this predicts: the natives are ~28 % of one shader's CPU, and moving them off the GIL saves only that share.
* But the finishing chain is **not the largest block**: tables (269 s, run after the loop, serial) are bigger than the chain (153 s). Whether the 809 s run had them on is not in the logs I have
  (`shader_tables` defaults to on; the stage logs in `~/Downloads` end at 938 of 21,990 shaders, the run that was killed by jetsam).

## 4. Memory and the free-threaded interpreter

### 4.1 Memory **[M]**

Python (GIL, 6 threads): 364 MB footprint, children <= 46 MB each; free-threaded: 866 MB (mimalloc keeps what threads freed, as `DECISIONS.md` trap 71 says). The split peaks at 1.8 GB RSS.
Rust: the tables use **17-25 MB footprint** over the 1.7 GB image (the image is mapped; clean file pages are not footprint); the chain needs about five copies of one shader's MSL
(~200 KB) per thread and holds nothing between shaders (the benchmark's 530 MB footprint is the 92 MB of inputs and the outputs it keeps to compare them).

### 4.2 Free-threaded, as measured

`fin` thread CPU 48.2 ms a shader against 22.2 ms (2.2x) and process CPU 1,568 s against 802 s: the free-threaded interpreter burns twice the CPU for the same work (single-thread penalty + contention);
with 6 threads that bought 9 % of wall on this shared Mac. The tables are where it clearly helps (269 -> 73 s), which matches the 50-60 s the doc quotes for the phone [D]. I could not reproduce
"2.1x on stage 3" [D] under this load; I do not claim the opposite for an idle machine.

## 5. What is attackable of the 809 s (estimates, **[E]**)

Method: take the Mac GIL profile (the same configuration as the 809 s run: GIL, 6 threads, tables on) and assume the phone is 809/733 = 1.10x slower, uniformly.

| step | Mac | removed by | remaining |
|---|---|---|---|
| tables | 269 s | `ibuilder-shader::tables` (16.7 s on 1 core, 4.3 s on 6 [M]) | ~5-15 s |
| MSL chain | 153 s CPU | `ibuilder-shader::post` (0.23 ms a shader [M]) | ~5 s CPU |
| image writers (3 images) | 13.4 s for two [M], the third not timed | `romfs::write_image_file` (5.0 s for the 1.7 GB image, from memory [M]) | ~5-10 s |
| other finishing + glue + start-up | ~380 s CPU | not ported: needs the design of section 6 | ~380 s |
| natives | 213 s CPU | stay; already parallel | ~35-55 s wall |

* With only what is ported: **[E] ~733 s -> ~320 s on this Mac, ~809 s -> ~350 s on the phone** (-56 %). The estimate assumes the remaining Python (~380 s) is still serial under the GIL.
* With the rest ported (section 6): **[E] ~50-80 s on this Mac** (natives 213 s CPU on 4-6 effective cores + I/O), **~80-150 s on the phone**, where the A17 Pro's performance cores are fewer than the Mac's and
  the translator's speed in-process is unmeasured.
* The part of the 809 s that is **not** attackable by a Rust port: the native translators (~28 % of the CPU, ~5-7 % of the wall once parallel), the 2 GB of flash writes, and thermal throttling
  (the doc measured thermal 2 after a build, 1.3-1.5 GHz against 3.6 [D]); less CPU time helps all three.
* Not attackable without a policy change: nothing needs the tables to build (`shader_tables=0` skips them; the host "works without them" according to `build()`'s docstring) but they carry
  the c1 bank exclusion list and the uniform-extent hints the host uses, so dropping them is a runtime trade the owners must judge. The Rust port makes the question moot (< 5 s).

## 6. Design of the Rust replacement of the Python finishing

* **One pass over a shader, no GIL, no files.** A shader is a job `{key, stage, record bytes (mmap slice of pkg), facts row, pair set}`. Per thread: `job_for` (portable: bytes -> job struct; JSON only at the translator boundary)
  -> translator FFI (C ABI over `GameAppXlate.Run`, returning the SPIR-V and the answers as buffers, not files) -> `spv_single_lane` (words in place) -> spirv-cross FFI -> chain over one `String` -> reflection struct -> side sources.
  rayon `par_iter` over shaders; the thread count is the caller's (memory gate as `ShaderMemoryGate`).
* **Two phases, as the Python**: everything but vertex shaders, then vertex shaders, because `vs_pad` needs the paired pixel shaders' `user(locnN)` set. Keep only that `BTreeSet<u32>` per ps, not its MSL (the Python re-reads `_b.metal`).
* **No per-shader heap churn**: the steps return edit lists (start, end, replacement) against the original text, merged and applied once, or work in a reused buffer; the ported steps allocate a new `String` only when they change the text
  (`Cow`), which is the case for 1-3 steps a shader.
* **Streaming outputs, one store**: replace SQLite + two image rewrites with an append-only pack (`name, bytes` as they finish) and one final sequential write in name order (the `DMROMFS1` image only needs the index sorted; data order is free,
  but byte-identity with the Python image fixes it). The cache becomes a content-addressed directory keyed by the same sha256 digest. Not needed for correctness of anything ported so far.
* **Reflection JSON**: `json.dump(indent=1, ensure_ascii=False)` with insertion-ordered keys; a struct with ordered fields and a small writer that matches Python's separators, not `serde_json::to_string_pretty`.
* **Portable now** (pure functions of text / bytes with a Python oracle): the chain (done), tables (done), image writer (done), `job_for`, `spv_single_lane`, `dmsh_reflect` (name decoders), `dmsh_side.unit` + `dmsh_alpha` / `dmsh_logic`
  / `dmsh_fetch.fold`, `dmsh_vin` / `dmsh_pull`, `geometry_capture`/`output_words` (done), `raster_facts`. **Depends on the translator's formats**: the job and `res.json` schemas (`dmsh_xlate/Program.cs`), the SPIR-V it writes,
  and the geometry shaders' five re-translations (`geometry_finish` drives the translator several times per shader: needs a multi-job FFI call); a change there is in `gameapp/xlate/*.cs`, not in Rust.
* **Cannot be removed by Rust**: the translator's and spirv-cross's own time, and the host's Metal compile (a separate stage, `GameAppWarm.cs`).

## 7. Reproduction notes and what is NOT measured

* Scripts: `crates/ibuilder-shader/tools/measure/{run_split,run_build,make_prof_engine,bench_post}.py` (they need `S=$SCRATCH_OLD`, the engine copy, and the worktree's `shader_device.py`).
* Per-step Python vs Rust: `bench_post.py` times `shader_post.post_rewrites` alone (8.18 ms a shader, 2,439 shaders, best of 3, `process_time`); `shader_post --profile-steps` the Rust steps.
* **Not measured**: the phone (every phone number is the 809 s [D] or my scaling of the Mac); the free-threaded runtime on the phone; the cache-hit path (a second build of the same game);
  the translator's in-process speed (the Mac's child-process numbers minus start-up); an idle machine (all numbers are from a loaded Mac); games other than Grizzy (South Park's Snowdrop split differs: `paches/engine/0104`).
* The harness runs `dmsh_build.py` exactly as `shader_device.build` does except through my instrumented copy (timers around ~45 functions, a `finish` wrapper); the instrumentation adds a few microseconds a call
  (the two uninstrumented-vs-instrumented sample runs, 12.7 s and 14.6 s with capture on, differ by the capture's file writes).
