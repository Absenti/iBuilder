# Memory of the whole chain on Grizzy (package MEM, round 3, 2026-11-19)

The iPhone's jetsam ceiling is about 3.3 GiB and the game needs most of it, so the build chain (profile -> rewrite -> shaders -> assemble -> sign, all in one process) must leave as much of it as it can.
This round measured where the chain's memory goes, and cut it **without changing one output byte** (every check of `docs/chain/E2E-RUST.md` was run again, section 8).

Everything below was run on this Mac (M1 Pro, 8 cores, 16 GB) **while two other agents were working on it**: the load average was 20 to 400 during the measurements of this round (stated with each
number; wall times are therefore not benchmarks, and RSS is under-reported when the machine swaps, which is why the footprint is the number to read). Nothing was run on a phone.

Two numbers per row, and they are not the same thing:

* **RSS** (`resident_size`, `ru_maxrss`): every page of the process that is in RAM, **including clean pages of mapped files** (the NSO the rewrite and the split map: 72 MB of `main`).
* **footprint** (`phys_footprint`, the `time -l` line "peak memory footprint"): dirty anonymous memory plus compressed memory. It is the number iOS's jetsam is expressed in, and what the game's budget is made of.

## 1. Result

Cold `scripts/e2e-grizzy.sh run cold`, same inputs, same machine. The translation (the engine's native tools, 80-230 s depending on the load) is not something this work can change.

Per step, MiB. "before" = the unchanged binary (`base1`, load 11 -> 116, 144.6 s), "after" = the final binary (`cold`, load 96 -> 248, 257.5 s: the machine was swamped by the other agents' work, so **the wall column is not a comparison**:
the native translation took 232 s instead of 135 s and the rewrite's 5 s became 15 s; the memory columns are not affected the same way). Before, the table had RSS only; the `[mem:]` lines of the pipeline give the footprint at the stage boundaries.

| step | peak RSS before -> after | peak footprint after | footprint left behind: before -> after | wall after (s) |
|---|---|---|---|---|
| 1 deriving the profile | 261 -> 174 | 173 | 259 -> **9** | 1.4 |
| 2 rewrite main | 483 -> 466 | **345** (the chain's high-water mark) | 360 -> 54 | 15.2 |
| 2 rewrite subsdk0 / sdk | 482, 489 -> 174, 188 | 65, 76 | 368, 375 -> 55, 57 | 1.8, 3.7 |
| 3 shaders split | **864 -> 393** | 221 (starts at 57 MiB of residue; alone it is 160) | 158 -> 163 | 2.4 |
| 3 shaders build | 448 -> 233 | 205 | 270 -> 122 | 232 |
| 4 assemble / sign | 90, 94 -> 78, 87 | 123, 132 | 270, 272 -> 122, 123 | 0.14, 0.15 |
| **whole run, `time -l`** | **906 MB (864 MiB) -> 489 MB (466 MiB)** | **894 MB (852 MiB) -> 362 MB (345 MiB)** | | |

(The RSS of the rewrite barely moves because its 72 MB mapped input and its image count in RSS; its footprint is what fell, 481 -> 374-390 MiB without stage 3, section 4.3. The split's "left behind" did not fall: after the split the
process holds 163 MiB, as before; what changed is that it holds 57 MiB less before the split and the split needs 160 instead of 600.)

**Peak of the whole chain**: footprint 852 MiB -> 345 MiB, RSS 864 MiB -> 466 MiB. The split alone (`shadersplit`, Grizzy, 6 threads): footprint 603-631 MiB -> 160 MiB, RSS 604-631 MiB -> 232 MiB.
Between the profile and the split the process now holds 9-57 MiB of footprint after each stage instead of 260-375 MiB (after the whole chain 123 MiB instead of 272): the next stage no longer starts on top of the last one's garbage.

What each change took off (section 4), in the order they were made, standalone split and no-shader chain (profile + rewrite + assemble + sign), footprint at the process's peak:

| step | split alone (MiB) | chain without stage 3 (MiB) |
|---|---|---|
| before (first port, system allocator) | 603-631 (memo on: 720; A/B runs 619-622) | 480 (503 MB), 377 left behind after the signature |
| all the code changes below, **system allocator** (`--features system-alloc` builds) | 358-398 (memo fresh 395, resumed 267) | 409 (429 MB), 306 left behind |
| + `ibuilder-alloc` (big blocks back to the kernel), all of it | **160** in every configuration (fresh, memo, resumed, 6 and 8 threads) | **374-391** (392-410 MB), **51-69 left behind** |

The same, in the order the changes were made (the standalone split with `ibuilder-alloc` in its binary; peak footprint):

| step | split alone (MiB) |
|---|---|
| `ibuilder-alloc` alone (`memprofile --bigmmap 131072` on the first-port code) | 273-297 |
| + the pixel cover reads 104 bytes per shader, the merged shaders live in a spill file | 234-252 |
| + Unity programs borrowed, 1 MiB search windows, serialized Unity files read through their tables at any size | 189-197 (resumed memo: 235) |
| + at most `threads + 2` results waiting for the merge, the memo streamed in and out | 160 |

And for the no-shader chain the rewrite adapter's mapped input is worth 75 MB of footprint (system-allocator build 429 MB against the first port's 503 MB; with the allocator 391 MB): RSS does not move, the mapped pages are clean.

## 2. How it was measured

| tool | what it gives |
|---|---|
| `/usr/bin/time -l` | wall, `maximum resident set size`, `peak memory footprint` (the kernel's lifetime maximum of `phys_footprint`) of a whole run |
| the CLI's step table (`crates/ibuilder-cli/src/meter.rs`, new columns) | per step: wall, largest RSS and largest footprint sampled every 5 ms **during** the step, and RSS / footprint **when it ended** (what the stage left behind); `--report f.tsv` has the same in bytes |
| the pipeline's `[mem:<tag>]` lines (`pipeline.stages.log`) | resident / peak / footprint at the stage boundaries; each `release:` line is the allocator's relief call (section 3) |
| `ibuilder_alloc::prof` (`crates/ibuilder-alloc`, feature `profile`) | a counting `GlobalAlloc`: live heap and its peak, a sample of the live allocations with their call chains at the moment of the peak (tcmalloc's scheme: every allocation of 64 KiB or more, a smaller one with probability size / 64 KiB), live bytes by size class, a 2 ms timeline of (live heap, RSS, footprint) |
| `crates/ibuilder-shadersplit/examples/memprofile.rs`, `crates/ibuilder-cli/examples/rewriteprofile.rs`, `ibuilder --features memprofile` | the profiler around the split, around one module's rewrite, around the whole chain |
| `crates/ibuilder-shadersplit/examples/mallocprobe.rs` | what libmalloc does with freed memory (section 3) |

Reproduce (own target dir; `CARGO_PROFILE_RELEASE_DEBUG=line-tables-only` puts source lines in the call chains):

```
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -p ibuilder-shadersplit --example memprofile
target/release/examples/memprofile --romfs RomFS.img --main main --pkg <dir>/pkg --tables <dir>/tables --profile <profile> --game 0100DFA027504000 --jobs 6 --top 25 --depth 6 --timeline [--bigmmap 131072] [--memo <dir>]
cargo run --release -p ibuilder-cli --example rewriteprofile -- --src ExeFS/main --out <dir> --module main --game 0100DFA027504000 --games <derived profiles> --games <engine>/dm/games --threads 4 --bigmmap 131072 [--mmap]
target/release/examples/mallocprobe [zones|delay]
```

## 3. Where the memory was, and why freed memory stayed

**The split's live heap was never the problem.** Allocation profile of the first-port split (system allocator, 6 threads, `memprofile`):

```
heap: peak live 195 MiB, total ever allocated 2798 MiB in 795,056 allocations
kernel: sampled peak resident 473-582 MiB, sampled peak phys_footprint 581-628 MiB; at the end resident 314 MiB, footprint 57 MiB
the call chains that hold the live bytes at the peak (sampled; 135 of the 192 MiB live were sampled):
  54.4 MiB  to_vec at split.rs:659   the pixel cover: the code area of every pixel shader cloned into a map, to read 104 bytes of each
  26.1 MiB  to_vec at scan.rs:303    each decoded Unity program copied into its Shader (ctl 17.2 + 8.9 MiB, code 8.4 + 5.4 MiB: these are what the merge keeps)
  12.6 MiB  to_vec at unity.rs:807   every subprogram copied out of the decompressed segments (shader_programs)
   8.5 MiB  the LZ4 output buffers of a Shader object
live bytes by allocation size: 4-8 KB blocks 61 MiB, 8-16 KB 46 MiB, 16-32 KB 54 MiB (the merged shaders), 3 MiB in blocks of 2-4 MiB
```

The program asked malloc for at most 195 MiB at once, and the process's footprint was 580-630 MiB: **400 MiB of freed memory that malloc kept**. The timeline shows it: the footprint climbs from 163 to 332 MiB
while the live heap is 25-47 MiB, and only falls when a whole region of the heap is empty (end of the split).

`crates/ibuilder-shadersplit/examples/mallocprobe.rs` (allocate 250 MB of blocks of one size, touch them, free them all, read `task_info` before and after, and again after `malloc_zone_pressure_relief` on every zone):

| block size | footprint allocated -> all freed (MiB) | returned by the free | `malloc_zone_pressure_relief(NULL, 0)` and `(zone, 1 GiB)` on both zones |
|---|---|---|---|
| 100 B x 2.5 M | 327 -> 88 | yes (nano zone) | 0 bytes |
| 6 KB x 40 k | 241 -> 114 | half | 0 bytes |
| 200 KB x 1.2 k | 617 -> 617 | **nothing** | 0 bytes |
| 8 MB x 31 | 363 -> 363 | **nothing** | 0 bytes |

Waiting 6 s after the free changes nothing. So on this macOS the system allocator keeps freed blocks of 128 KiB and more (and half of the 4-32 KB ones) in the footprint, and the call the task named, `malloc_zone_pressure_relief`
(`ibuilder-pipeline`'s `sys::mem::trim`, called by `release_memory` at the stage boundaries since the first port, behind `cfg(target_vendor = "apple")` with a `0` elsewhere), **reports 0 bytes in every case I tried**, in the
chain too (`[mem:after-rewrite-main] release: the allocator handed back 0 MiB`). It stays in the chain as the hook for the small blocks (it may do more on a device; not measured there), but it cannot be what returns the big buffers.

**The fix that works is not to give big blocks to malloc.** `memprofile --bigmmap N` serves every allocation of at least N bytes with `mmap` / `munmap` (which return the pages the moment the block is freed):

| N | split peak footprint (MiB), 2 runs each | footprint at the end of the split |
|---|---|---|
| malloc for everything | 595 / 598 | 45-48 |
| 1 MiB | 282 / 284 | 42-44 |
| 128 KiB | 231 / 243 (273 / 297 on the first measurement) | 113-116 |
| 64 KiB, 32 KiB, 16 KiB, 8 KiB | 228-242 | 114 / 112 / 102 / 74 |

(wall time of the split unchanged, 0.83 s in each.) 128 KiB is about where libmalloc's large-block path starts and takes almost all of it; the smaller thresholds only lower the residue at the end and cost a mapping per block.

## 4. What was changed, and what each change saved

### 4.1 `ibuilder-alloc`: a global allocator that returns big blocks (new crate)

`crates/ibuilder-alloc`: `BigBlocks`, a `#[global_allocator]` that serves every allocation of 128 KiB or more with an anonymous `mmap` and frees it with `munmap`, and everything smaller with the system allocator.
Only on `target_vendor = "apple"` (macOS, iOS, the simulators): elsewhere it is the system allocator (glibc and Android already `munmap` big blocks), so Linux CI is unchanged. The size class is a function of the `Layout`
alone, so `dealloc` and `realloc` always know which allocator made a block; alignments above a page fall back to the system allocator; a mapping is zero-filled, so `alloc_zeroed` is free. `realloc` of a block that is or becomes
big copies it (there is no `mremap` on Darwin; `Vec` growth is geometric, so about twice the final size is copied once).
`unsafe` is confined to one module (`mmap`/`munmap` and the `GlobalAlloc` impl, each with its safety argument); `#![deny(unsafe_code)]` elsewhere in the crate.

Installed in the binaries: `ibuilder` (`crates/ibuilder-cli/src/bin/ibuilder.rs`) and `shadersplit` (`crates/ibuilder-shadersplit/src/bin/shadersplit.rs`). **The app's FFI archive is not mine: section 6.**
Cost: one mapping and the page faults of fresh memory per big block, where malloc's cache would sometimes have saved them. Measured wall times with and without it (5.0-5.4 s vs 5.15-5.6 s for the rewrite of `main`,
0.7-1.0 s vs 0.67-1.2 s for the split, under load) show no slowdown; the translation's 80-130 s is the engine's native tools.

Tests: `crates/ibuilder-alloc` unit tests (layout classes, every way to allocate / grow / shrink / free a block across the boundary keeps contents and zeros, the counters) and `tests/footprint.rs`
(`BigBlocks` as the real global allocator: a 256 MiB buffer touched and dropped is +256 MiB of footprint and then -256 MiB; 40 MiB grown by `push` across the boundary is back to the starting footprint).

Effect (cold chain, section 1): the footprint left behind after the profile 259 -> 9 MiB, after the rewrite 360-375 -> 54-57 MiB, after the whole chain 272 -> 123 MiB; the split and the build no longer start on top of those.

### 4.2 The split (`crates/ibuilder-shadersplit`)

Allocation profile of the split before the second round of changes (live heap peak 187 MiB at the start of the second pass, `memprofile --bigmmap 131072`):

```
45.3 MiB  read_all of 1-16 MiB files (scan_file_in:711)           40.0 MiB  4 MiB search windows (contains_each, 10 live at that instant)
27.3 MiB  LZ4 buffers of Shader objects (shader_programs)          23.0 MiB  to_vec of every subprogram (shader_programs:807)
10.6 MiB  object bytes (for_each_object:714)                       15.6 MiB  ctl / code copies (unity_program:68, 3 sites)
```

| change | where | saved (peak footprint, standalone split) |
|---|---|---|
| The pixel cover (single-stage programs pair every vertex shader with a cover of the pixel shaders) cloned every pixel shader's code area into a map; it only reads the input map in the first 0x68 bytes | `split.rs`, `scan.rs` (`pixel_cover_of_inputs`, `PS_INPUTS_PREFIX`) | 54 MiB at the peak |
| The merged shaders are kept in a **spill file** (`spill.rs`, a temporary `.split-spill-<pid>` in the package folder, removed on drop, swept after a kill): the map holds `(offset, lengths)`; the bytes are read back for `same_shader` of a second copy of a key, for the pixel cover's 104-byte prefixes and when the records are cut and written | `split.rs`, `spill.rs` | 127 MB of live heap (the file is in the page cache, which is not footprint) |
| A Unity program was copied twice (`unity_program`, then again into the `Shader`); the second copy is only needed for the pre-2019.3 pass pairs | `scan.rs` | 13 MiB in flight |
| `shader_programs` copied every subprogram out of the decompressed segments; it now returns the segments and the ranges (`Programs`), the readers borrow | `unity.rs`, `scan.rs` | 23 MiB in flight |
| The magic searches read 4 MiB windows per thread, now 1 MiB | `source.rs` | 30 MiB |
| A Unity serialized file of any size is read through its tables and the objects asked for (it was whole-file for files under 16 MiB: 190 of Grizzy's 227 files); the same readers run on the same bytes, so the output is the same | `scan.rs` (`is_plain_serialized`) | 45 MiB in flight |
| `run_pool` lets a worker start job `i` only when the merge is less than `threads + 2` jobs behind (results are merged in job order, so a result that is ready waits in memory for the ones before it; a resumed split decodes memo files much faster than it merges) | `split.rs` | with the next row: resumed split 235 -> 160 MiB, fresh 189-197 -> 160 MiB |
| The memo is encoded straight into its file and decoded straight from it (it was a whole `Vec` copy each way); a damaged length cannot make the reader allocate | `memo.rs` | the job's size, once per thread |

Outputs: `diff -r` of `pkg/` and `tables/` against the Python oracle is empty for 1, 2, 6 and 8 threads, with a fresh memo, a complete memo and a half-deleted memo (section 8). The memo's file format is unchanged
(`SPLIT_REV` is not bumped: an interrupted split of the earlier build resumes with the new one).

Final allocation profile of the split (`memprofile --bigmmap 131072`, 6 threads, load ~25): **peak live heap 158.1 MiB** (`main`'s decoded image, allocated in the first 2 ms and held until the scan of `main` ends at 0.6 s; the footprint
follows it to 163 MiB and falls to 6 MiB the moment it is freed), total allocated 1,682 MiB in 731,235 allocations (before: 2,798 MiB in 795,056), live heap in the thread-pool phase at most 48 MiB in the first pass and 107 MiB in the second
(the Unity files whose type trees come from the first pass), footprint at the end of `split()` 54 MiB.

### 4.3 The rewrite's adapter (`crates/ibuilder-cli/src/adapters.rs`, `RustRewriter`)

It read `main` with `fs::read`: 72 MB of heap for the whole rewrite. It now maps the file read-only (`memmap2`, the way `ibuilder_nso::read` does; a file that cannot be mapped is read as before). The mapped pages are clean and file-backed:
they count in RSS, not in the footprint. Footprint peak of the no-shader chain 448 -> 374-390 MiB (RSS unchanged). The 21 rewrite files of the three modules are `cmp`-identical to the oracle.

### 4.4 Stage boundaries and the meter

* `release_memory` (the relief call, `cfg(target_vendor = "apple")` / no-op elsewhere) now also runs after the profile (as `before-rewrite`), after the split and after the build, in addition to after each module, before the shaders and after the signature:
  `tests/chain_spec.rs` checks that the six `release:` lines of a full chain are all there and in stage order.
* The CLI step table and `--report` carry the footprint: per step the largest footprint sampled, and what each of RSS and footprint was when the step ended.

## 5. The rewrite: the next biggest cost (`ibuilder-rewrite` is read-only here: numbers and the exact changes)

With the allocator and the mapped input, the chain's high-water mark is the rewrite of `main`: footprint 374-390 MiB (RSS 467-483 MiB, 72 MiB of it the mapped file). Allocation profile of `rewriteprofile` (4 threads, mapped input), live heap peak 333 MiB:

| live at the peak | where | note |
|---|---|---|
| 158.0 MiB | `ibuilder_nso::load`, the decompressed image (`rewriter.rs:442`) | inherent to the design: the rewriter patches an image; 158 MiB is `main`'s text + rodata + data |
| 45.3 MiB | `par_iter().collect()` in `spin.rs:927` (`run`, called from `scan.rs::emit_all`) | a `Vec` of per-region results materialised before use: folding them into the output as they come would never hold them |
| 18.8 + 13.0 MiB | `Arc::new(ins)` in `spin.rs:185` (`ins`, from `spin_lock` / `spin_region`) | one `Arc` allocation per decoded instruction: 31.8 MiB of 100-byte allocations that live as long as the region |
| 18.4 MiB | `Vec::push` in `decode_no_regs` (`cs.rs:251`) | growth of a per-call vector: `with_capacity` or reuse |
| 13.9 MiB | `collect` in `spin.rs:923`; 13.1 MiB `push` in `scan.rs:426` | |
| ~70 MiB | the rest: sets, maps, strings | |

The rewrite allocates 77 million times (3.7 GB in all) for a 333 MiB peak: most of the cost is churn, which is also why its footprint after the stage is small once the big blocks are returned. If the 550 MiB target is to go lower,
the three entries above (`spin.rs:927`, `spin.rs:185`, `cs.rs:251`) are the 100 MiB that can go without touching the algorithm; the 158 MiB image is the floor.

## 6. In the app (`ibuilder-ffi`, owned by package APP)

The footprint gain only exists in a process whose global allocator is `BigBlocks`. The FFI archive is the app's Rust; one line makes it so, in `crates/ibuilder-ffi/src/lib.rs` (with the `real-stages` feature, which already depends on
`ibuilder-cli`; `ibuilder-cli` re-exports the crate, so no `Cargo.toml` change):

```rust
#[cfg(feature = "real-stages")]
#[global_allocator]
static GLOBAL: ibuilder_cli::alloc::BigBlocks = ibuilder_cli::alloc::BigBlocks;
```

Not done by me and **not verified in the app**. It builds for the iOS simulator and device (`aarch64-apple-ios-sim`: `ibuilder-alloc`, `ibuilder-shadersplit`, `ibuilder-pipeline`, `ibuilder-cli`, section 8); the simulator run of
`shadersplit` (which has the allocator) gives the oracle's bytes. Footprint accounting of anonymous `mmap` memory on a device is the same as for malloc's own large blocks, but nothing here ran on a phone.

## 7. What is not reduced, and why

* **The split's floor is `main`'s decoded image, 158 MiB**, held while `main` is scanned (before the thread pool starts): the readers search the whole image (`raw_shaders`, the GLSLC / BNSH magics, the `tfxo` streams), and the Python does the same.
  Scanning it in pieces would change what a match across a piece boundary does; not attempted. RSS shows 232 MiB there because the mapped 72 MB NSO is in it.
* **The rewrite** (section 5): 374-390 MiB of footprint, set by the image and the analysis's working set, in a crate that is not mine.
* **The shader build's own heap grows from 52 to ~205 MiB of footprint** over the 21,990 shaders (about 2.7 KB per shader: the cache entry table, the result rows, the `locn` sets, the digests; all of it is needed again when the tables and the image index are written);
  the 1.7 GB library is streamed, never in memory. The translator and SPIRV-Cross are child processes here (not in these numbers; `time -l` reports the largest one, not the sum) and in-process C# / C on the phone (not measured).
* **Small blocks**: the process still holds 50-60 MiB of footprint after the rewrite and ~93 MiB after the split (malloc's magazines of 1-100 KB blocks with survivors in them; `malloc_zone_pressure_relief` does not return them). An arena per stage
  would, but only if nothing allocated in the stage outlives it, which the pipeline's strings, logs and maps do; not attempted.
* `SHADER_PER_JOB` (450 MiB per translation thread) and the 900 MiB per reading thread of `shader_jobs_for_memory` are still the Python-era figures. On Grizzy the Rust split's footprint is the same at 6 and 8 threads (160 MiB: the
  floor above), so on a phone with 2.8 GiB left the gate would cut the split to 2 threads for nothing (the split is 1 s). I left the policy as it is: a game with UnityFS bundles reads whole bundles, and the measurements here are Grizzy's only.
* Nothing was measured on a phone: jetsam, the gate under pressure, the app's allocator.

## 8. Verification (what I ran; load average with each)

```
cargo test --release -p ibuilder-shadersplit                    # 24 tests: 15 unit (2 new: spill), differential 2, games 1, grizzy 2 (1/2/8 threads, memo fresh / complete / half-deleted + damaged + .tmp), synthetic 4
cargo test --release -p ibuilder-pipeline -p ibuilder-cli -p ibuilder-alloc
cargo test --release --workspace --no-fail-fast
E2E_ROOT=... IBUILDER=... LIBDIFF=... PYLIB=... IOSBUILDER=... ENGINE=... scripts/e2e-grizzy.sh run cold && scripts/e2e-grizzy.sh verify cold
PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-shadersplit -p ibuilder-pipeline -p ibuilder-alloc -p ibuilder-cli
xcrun simctl spawn <SIMULATOR-UDID> <ios-sim shadersplit> --romfs ... --jobs 6
```

| check | result |
|---|---|
| `cargo test --release -p ibuilder-shadersplit` (`SHADERSPLIT_ROMFS` / `_MAIN` = the private copies), load ~60 | 24 passed (15 unit, differential 2, games 1, grizzy 2 in 4.8 s: 1 / 2 / 8 threads and the memo cases against the Python oracle, synthetic 4) |
| `cargo test --release -p ibuilder-pipeline -p ibuilder-cli -p ibuilder-alloc` | all suites ok (pipeline 57 + boot 1 + chain 9 + delta 10 + pipeline_spec 53 + resume 16; cli suites incl. `cli.rs` 8, `mods_chain` 4; alloc: 3 unit + `footprint.rs` 1); `chain_spec` re-run after the relief-order assertion: 9 passed |
| `cargo test --release --workspace --no-fail-fast` (load 160-330, 10 min 40 s) | **379 passed, 1 failed, 5 ignored** in 90 suites. The failure is `ibuilder-shader/tests/tessmesh_cases.rs::tessellation_task_mesh_and_passthrough_geometry_match_the_python` (8 `tess_hull` cases, `in_bytes` 30 vs 32): package SHADERS's crate, whose files were edited at 12:41-12:55 during my run; nothing I changed is in its path. Not a regression of this package, but the workspace is **not** green at this moment |
| cold e2e, new binary: exit 0, `stages: assemble=ok, profile=ok, rewrite=ok, shaders=ok` | 257.5 s, load 96 -> 248 |
| `scripts/e2e-grizzy.sh verify cold` (2) rewrite files vs the oracle | **21 of 21 identical** (against the orchestrator's earlier cold-run files: see below) |
| (3) `libdiff` of the library vs the Python's `b-b24b25240cad.img` | **56,882 identical of 56,885**; the 3 that differ are `c1_full.tsv`, `cb_extent.tsv`, `inputs.tsv`, the documented digest / time-stamp lines (`docs/chain/E2E-RUST.md` section 3) |
| (4) `codesign --verify --strict` on the signed Mach-O | `valid on disk`, `satisfies its Designated Requirement` |
| split alone, `diff -r pkg/ tables/` against `split-out` (Python oracle) | empty for: first-port binary, `--jobs 1`, `6`, `8`, a fresh memo, a complete memo, the system-allocator build (fresh memo and resumed), the iOS-simulator binary |
| `PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-shadersplit -p ibuilder-pipeline -p ibuilder-alloc -p ibuilder-cli` | Finished (2 min 39 s at load 177) |
| `xcrun simctl spawn <iPhone 17> <ios-sim shadersplit> ... --jobs 6` | `shaders 21990 wall 2.9 s` at load 177, output identical to the oracle. The simulator has the Mac's memory accounting: no phone number |
| speed (+10 % budget) | rewrite of `main` 4.99-5.44 s with the allocator and the mapped input against 5.15-5.63 s before (loads 19-41); split 0.6-1.0 s at load 20-60 against 0.67-1.2 s. The cold chain's 257 s vs 145 s is the machine's load, not the code (the translation tools alone took 232 s vs 135 s) |

Not run: the `aarch64-apple-ios` (device) build of the new code (only the simulator target), the FFI archive with the allocator (not mine), any phone.

**The inputs moved during the round.** At about 12:09 the checkout `iOSBuilder/out/ref-img-grizzy/RomFS.img` and `iOSBuilder/out-py/` (the Python rewrite's oracle outputs) disappeared (not by me: iOSBuilder is read-only for me and I
ran nothing that writes there; the disk went from 409 to 350 GB used at that time). I worked from private copies in the scratch folder (`mem/ios/`, deleted at the end of the round to free the disk: `cp -c` of the two files below recreates them): the RomFS image from `out-load/0100DFA027504000.nxgame/romfs.img` (the package's copy,
`DMROMFS1`, 2,498,033,693 bytes: split with the baseline binary it gives the stored Python split byte for byte), and, as the rewrite oracle, the 21 files of the orchestrator's earlier cold run
(`orch-e2e/cold/extract/rewrite/`, which the orchestrator had compared with the Python oracle, 21 of 21, before it vanished; the unchanged baseline binary's own output is `cmp`-identical to them). The shader library is compared with the Python's
`runs/full-gil-j6/b-b24b25240cad.img`, which was not touched. `crates/ibuilder-testkit`'s `grizzy_oracle` points at the deleted folder: the rewrite crate's tests that need it skip.

## 9. Files

`crates/ibuilder-alloc/{src/lib.rs, src/prof.rs, tests/footprint.rs}` (new), `crates/ibuilder-shadersplit/{src/{spill.rs (new), split.rs, scan.rs, unity.rs, source.rs, memo.rs}, src/bin/shadersplit.rs, examples/{memprofile,mallocprobe}.rs, tests/grizzy.rs}`,
`crates/ibuilder-cli/{src/{adapters.rs, meter.rs, lib.rs, bin/ibuilder.rs}, examples/rewriteprofile.rs, Cargo.toml}`, `crates/ibuilder-pipeline/{src/{rewrite.rs, shaders.rs}, tests/chain_spec.rs}`, `docs/chain/MEMORY-RUST.md`, `docs/shaders/SHADERSPLIT-RUST.md`.
Run data kept (not in the repo, 328 KB): `$SCRATCH/mem/{e2e/*.{stdout,time,report.tsv,uptime.*} (base1 = before, bb1 = allocator only, cold = final), prof-*.txt (allocation profiles), workspace-tests.log, split.sh, chain-noshader.sh}`; every output tree, binary copy and target directory was deleted after its numbers were recorded here.
