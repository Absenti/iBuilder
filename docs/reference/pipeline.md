# Build pipeline: `ibuilder-pipeline`, `ibuilder-ffi`, `ibuilder-cli`, `ibuilder-alloc`

## What it does

These crates run the whole build chain for one game inside one process:

0. read the container (`.nup`) if the input is one,
1. derive the game profile,
2. rewrite the NSO modules (`main`, `sdk`, `subsdk0`, ...),
3. build the shader library (split the RomFS and `main`, translate, pack),
4. assemble the Mach-O framework and sign it,
5. boot the game (only in the app).

| crate | role |
|---|---|
| `ibuilder-pipeline` | The chain driver: stage model, config rows, run markers and resume state machine, timings, memory and thermal gates, log rotation, live-shader queue. `src/sys/` holds the OS calls. |
| `ibuilder-ffi` | The stable C ABI (`crates/ibuilder-ffi/include/ibuilder.h`) as `staticlib + cdylib + rlib`, plus C tests. |
| `ibuilder-cli` | The `ibuilder` command line for the Mac, and the real adapter set (the FFI links it with `--features real-stages`). |
| `ibuilder-alloc` | A global allocator that returns big blocks to the kernel (see Memory). |

Original it replaces: the CPython-based driver of the iOSBuilder app (`gameapp_pipeline.py`, its C bridge and the
app-side run-marker logic). The stage workers are other crates (`ibuilder-profile`, `-rewrite`, `-macho`, `-sign`,
`-shadersplit`, `-shaderbuild`, `-mods`, `-nup`) plugged in through adapters.

## Stage model and adapters

`Stage { fn id(); fn run(&self, &mut Ctx) -> Result<StageOutcome> }`. `Ctx` holds everything the original kept in
module globals (docs and bundle dirs, config, environment, logs, memory source, clock, thermal gate, timings, stats,
shader-pack root, cancel flag, adapters, live queue), so two chains can live in one process. The stages are `NupStage`,
`ProfileStage`, `RewriteStage`, `ShadersStage` and `AssembleStage` (`src/chain.rs`, `run_chain`, `ChainReport`).
The process environment is written once, by `Env::commit()`, right before the guest runs.

Workers plug in through the traits in `src/adapters.rs`:

| trait | wired to (in `ibuilder-cli::adapters`) |
|---|---|
| `ProfileDeriver` | `ibuilder-profile` |
| `Rewriter` | `ibuilder-rewrite::rewrite` on a rayon pool of `req.jobs` threads |
| `Assembler` | `ibuilder-macho::assemble_files` |
| `Signer` | `ibuilder-sign::sign_file` (ad hoc). In the app, a C callback (`ibuilder_sign_fn`) |
| `ShaderBackend`, `ShaderTools` | `ibuilder-shadersplit` + `ibuilder-shaderbuild`; tools are the app's C callbacks, or on the Mac the engine's binaries (feature `shader-host-tools`) |
| `NupReader` | `ibuilder-nup` (`RustNup`, feature `nup`) |
| `ModsHost` | `ibuilder-mods` (`RustMods`, feature `mods`) |
| `BootHost` | `sys::boot::DlopenBoot` (`dlopen` of the host, call through `dm_host_probe_boot`) |
| `LauncherBase` | not wired: the engine's launcher base document is not generated here |

With a feature off, the stage fails with a message naming the feature (`adapters::Unwired`).

`ibuilder-pipeline` contains no `fork`, `exec`, `posix_spawn` or `Command`, and `#![deny(unsafe_code)]` applies everywhere
except `sys/`, whose module docs justify each `unsafe` (Mach `task_info`, `vm_region_recurse_64`, `dlopen`/`dlsym`,
`dup2`, a signal handler, `setenv`, `mmap`). Scripts `scripts/check-no-spawn.sh` and `scripts/check-forbidden-files.sh`
guard the first rule and the repository content.

### Result codes

`ibuilder_run_chain` returns (header names `IBUILDER_RC_*`; the exit code of the CLI follows the same table):

| code | meaning |
|---|---|
| 0 | The chain ran and no stage failed. `shaders=none (...)` (a game with nothing to build) is not a failure. |
| 1 | A stage that must succeed failed (container read, profile, rewrite), or a boot-only launch whose boot did not run. |
| 2 | No `extract_out` in the config (also: no bundle, or create failed). |
| 3 | Boot-only had nothing to boot (no framework, no `flat/`, or a framework from another app build). |
| 4 | The game has no shader library: stage 3 had to build one and could not (`shaders=failed (no library: ...)`). The chain still assembles, but boots nothing and writes no framework stamp, so the app reads the build as not built. A boot-only launch whose stage 3 cannot build the missing library refuses the boot, consumes the request and keeps the framework. |
| 5 | No framework: stage 4 failed (assembly or signature). Nothing is stamped or booted. 5 wins over 4 when both failed. |
| 75 | The chain ended on purpose before stage 3 for memory: close the app, the next launch continues. |
| 130 | Cancelled (`ibuilder_cancel`, `ibuilder_request_stop(IBUILDER_STOP_CANCEL)`). |
| 131 | Paused by the user (`IBUILDER_STOP_PAUSE`); everything is kept (shader cache, rewrite cache, split jobs). Run the chain again to continue. |

`shaders=none` means nothing was due (no host chain in the config, `shader_build=0`, or a split that finds no shader);
`failed (no library: ...)` means a library was due and is missing (free space, no backend, missing inputs, split,
translation or pack install failed). A stop during stage 3 is 130 or 131. The decision is made here so callers need
not parse the status text.

## Run markers and resume

State lives in files in the Documents folder; names and formats are the contract with the app (`src/state.rs` has the
table: `gameapp.running`, `.args` and `.args.done`, `.boot-only` armed or attempted, `.stage`, `.killed`,
`.continue` and `.continue.run`, `.framework` stamp, `.load-blocked`, `.thermal`, `.timing.partial`,
`build_times.tsv`, ...). The logic that decides what the next launch does is a tested state machine:

- `begin_run` / `clear_run_marker` write and remove the interrupted-run marker (`gameapp.running`).
- `report_interrupted_run` classifies a previous run that never finished.
- `decide_launch` chooses: interrupted (start nothing), args request, continue, boot-only armed / attempted / withdrawn.
- `resume_plan`, `prepare_continued_build`, `restart_end_by_design` handle the restart before stage 3: the chain
  writes `gameapp.continue` and returns 75; the app closes; the next launch continues and merges the earlier run's
  timings (`take_continue`).
- Stage 3 has its own resumable work folder (`ShaderResume`: signature of the inputs, the split's job memo,
  `splitjobs/`). Every finished shader is one immutable cache file renamed into place, so a kill leaves whole entries
  only; a stray `.tmp-*` entry is never read and is swept later. After a killed stage 3 the next run uses one thread
  unless `shader_jobs` is set.
- Shader pack installs survive a kill: `install_pack` moves the old pack to `<id>.old` before renaming the new one in;
  `recover_pack` (at chain start and in `build_shaders`) restores `<id>.old` when `<id>` is missing and removes it when
  both exist; a leftover `<id>.part` is removed. Writers use per-writer temp names (`write_atomic`, `copy_atomic`).
- Shader cache: `shadercache/cas-<chain>.dmsb` (a folder); other `cas*` entries are removed with a logged reason.

Config rows settable with `--set KEY=VALUE` include `shader_cache_gib`, `shader_free_gib`, `shader_pull`, `shader_jobs`
and `keep_exefs`.

## C ABI (`crates/ibuilder-ffi/include/ibuilder.h`)

`IBUILDER_ABI_VERSION` is 1 (`ibuilder_abi_version()`); 29 exported `ibuilder_*` functions (plus `ibuilder_test_panic`
under `IBUILDER_TESTING`). Opaque handle `ibuilder_t*`; `ibuilder_options` starts with `struct_size` so it can grow.
It carries the Documents and bundle directories, log callback (stream 0 `pipeline.stages.log`, 1 `gameapp.chain.log`),
progress callback (stage k/n, text, bar fraction), a sign callback, the two tool callbacks (`xlate`, `spirv_cross`),
a live-shader notify callback, and `flags` (`IBUILDER_FLAG_BOOT_IN_PROCESS`: stage 5 runs in the process; without it
the chain only builds and the caller clears the run marker with `ibuilder_finish_run`).

Function groups:

| group | functions |
|---|---|
| lifecycle | `ibuilder_abi_version`, `ibuilder_version`, `ibuilder_last_error`, `ibuilder_create`, `ibuilder_destroy` |
| run | `ibuilder_run_chain(h, config_path_or_NULL)`, `ibuilder_chain_status`, `ibuilder_cancel`, `ibuilder_request_stop(kind)`, `ibuilder_stop_requested` |
| memory / platform | `ibuilder_mem_info`, `ibuilder_mem_line`, `ibuilder_available_bytes`, `ibuilder_address_space_info`, `ibuilder_self_identifier`, `ibuilder_early_init` |
| game host | `ibuilder_set_game_layer`, `ibuilder_host_symbol`, `ibuilder_quit_game`, `ibuilder_live_handler` |
| launch state | `ibuilder_classify_boot_request`, `ibuilder_classify_framework`, `ibuilder_classify_framework_id`, `ibuilder_classify_previous_run`, `ibuilder_parse_stage`, `ibuilder_is_boot_stage`, `ibuilder_decide_launch` (JSON), `ibuilder_begin_run`, `ibuilder_finish_run`, `ibuilder_logs_mark`, `ibuilder_log_trim` |
| container | `ibuilder_nup_icon`, `ibuilder_nup_icon_read` (title icon of a `.nup`, error `IBUILDER_E_NUP`) |

Error codes: `IBUILDER_E_BAD_ARG` -1, `E_BUSY` -2, `E_NUP` -3, `E_IO` -4, `E_PANIC` -99.

Panics: every entry point runs under `catch_unwind` and returns `IBUILDER_E_PANIC` on unwinding builds. The workspace
release profile is `panic = "abort"`, where nothing can be caught: a hook installed by `ibuilder_create` writes
`[ibuilder] PANIC: ...` to `gameapp.chain.log` and the process aborts.

Linking into an app: `scripts/build-ios-ffi.sh <out>` builds `libibuilder_ffi` for `aarch64-apple-ios` and
`aarch64-apple-ios-sim` (with `--features real-stages`), checks the exported symbols against the header and makes an
xcframework; link with force-load (the exports are reached through `__Internal`). The app keeps what depends on Apple
frameworks: the Keychain identity and zsign behind `ibuilder_sign_fn`, and the shader translator and SPIRV-Cross behind
the tool callbacks. `ibuilder_early_init(documents)` (or, on iOS, a Mach-O initializer) reserves address space for the
guest early. The config text is not exported; keys and order are in `crates/ibuilder-pipeline/src/configtext.rs`.

## CLI

```
cargo build --release -p ibuilder-cli
target/release/ibuilder (--exefs <ExeFS> | --nup <game.nup>) --out <dir> \
    [--bundle <engine dir: dm/games, dm/host>] [--host <host-prebuilt.dylib>] [--romfs <RomFS.img|manifest|dir>] \
    [--control <nacp>] [--modules "main subsdk0 sdk"] [--threads N] [--app-build ID] [--identifier ID] \
    [--shaders --shader-chain ID --engine <engine dir> [--shader-jobs N]] [--set KEY=VALUE]... \
    [--report f.tsv] [-v] [--mock]
```

`--out` is laid out like the app's Documents folder. Inputs are linked (symlinks) into `<out>/extract/`, never copied or
written. The command prints, per step, wall time, peak resident size and footprint, and what was left behind;
`--report` writes the same as TSV.

- `--nup` reads a Switch 2 container into `<out>/extract` first (see profile-and-nup.md).
- `--shaders` runs stage 3 between rewrite and assembly. It needs `--shader-chain` (the library is
  `b-<ID>.img`) and `--engine` (`dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate` and `spirv_cross_bin/spirv-cross` are run, never
  written), and a binary built with `--features shader-host-tools`, which runs those tools as child processes. That
  feature is off by default because it uses `std::process::Command`, which the app never has. Bad option
  combinations exit 2 before anything is written.
- `--mock` needs `--features mock-stages` (stand-in workers, no game data).
- Exit codes: 0 all stages ok, 1 a stage failed (stage 3 included), 2 usage, 75, 130, 131 as above.

## Memory

The phone's memory ceiling is about 3.3 GiB and the game needs most of it, so the chain keeps its footprint low.
Two numbers are different: RSS counts every resident page including clean file-backed pages (the mapped NSO);
`phys_footprint` counts dirty and compressed memory and is what iOS jetsam limits. The chain reports both: the `[mem:<tag>]`
lines in `pipeline.stages.log` and the CLI step table.

Measured on macOS (M1 Pro, 2026-11): libmalloc keeps freed blocks of 128 KiB and more in the footprint, and
`malloc_zone_pressure_relief` returned 0 bytes in every case tried (the split's live heap peaked near 195 MiB while its
footprint was 580-630 MiB).

- `ibuilder-alloc::BigBlocks` is a `#[global_allocator]` that serves allocations of 128 KiB or more with an anonymous
  `mmap` and frees them with `munmap`; everything smaller goes to the system allocator. Active only on Apple targets
  (elsewhere it is the system allocator). The size class depends only on the `Layout`, so `dealloc` and `realloc` always
  know which allocator made a block. `realloc` of a big block copies (no `mremap` on Darwin). Installed in `ibuilder`,
  `shadersplit` and, with `real-stages`, in the FFI archive (it covers the archive's Rust code only; C code keeps its
  own `malloc`, and no pointer crosses the boundary to be freed on the other side).
- The shader split was also reduced (no per-shader code clones, a spill file for merged shaders, 1 MiB search
  windows, bounded results waiting for the merge, streamed job memo). Output is byte-identical to before.
- The rewrite adapter maps `main` read-only (`memmap2`; falls back to reading) so the input counts in RSS but not in
  the footprint.
- `release_memory` runs after each stage and module; `tests/chain_spec.rs` checks the `release:` lines and their order.
  The memory and thermal gates (`gate.rs`, `thermal.rs`, testable with a fake `Clock`) cut shader thread counts when
  memory or heat ask; their per-thread budgets are the original's figures and not re-measured for the Rust split.

Measured on 2026-11 on an M1 Pro with a loaded machine (indicative, not benchmarks), Grizzy and the Lemmings, cold
chain: peak footprint 852 MiB before, 345 MiB after; peak RSS 864 MiB before, 466 MiB after; the standalone split 603-631
MiB before, 160 MiB after, for 1 to 8 threads. The remaining floors: the rewrite's decoded `main` image (about 158
MiB) and the shader build's per-shader tables (about 2.7 KB per shader). Nothing here was measured on a phone.

## Verification

```
cargo test --release -p ibuilder-pipeline -p ibuilder-ffi -p ibuilder-cli -p ibuilder-alloc
```

| suite | what it covers |
|---|---|
| `ibuilder-pipeline` unit tests | config, markers, memory line, log window, stage parsing, etc. |
| `tests/pipeline_spec.rs`, `tests/resume_spec.rs` | behaviour ported check by check from the original Python test suites (settings, shader pack and cache, resume, run markers) |
| `tests/chain_spec.rs` | the whole chain on stand-in workers: stage order, outputs, cache on the second run, restart 75, failure semantics (codes 4 and 5), boot decision, cancel, continued timings |
| `tests/boot_spec.rs` | a real `dlopen` boot of a host built with clang: guest output in `gameapp.guest.log`, environment seen by the host, live shader queued, translated and answered |
| `delta_spec`, `preset_spec`, `pso_prepare_spec`, `atomic_markers`, `stop_spec`, `thermal_callbacks`, `sign_rc`, `config_arith` | mods hooks, presets, prewarm share, atomic writes, stop requests, thermal, signer codes |
| `ibuilder-ffi` `tests/c_abi.rs` | builds the static library and compiles C programs with clang against `ibuilder.h`: the whole chain through the ABI with C callbacks, `panic=abort` behaviour, a host that runs past `boot_seconds` |
| `ibuilder-cli` `tests/cli.rs`, `mods_chain.rs`, `shaders_glue.rs`, `stale_extract.rs` | layout, mock run, exit codes, mods through the real rewriter, cache folder naming |
| `ibuilder-alloc` unit tests and `tests/footprint.rs` | size classes, grow/shrink across the boundary; a 256 MiB buffer is +256 MiB of footprint and then back |

Tests that need game files are `#[ignore]` or skip when the files are absent; they need the original iOSBuilder
checkout (read only). Example: `cargo test --release -p ibuilder-cli --test grizzy_e2e -- --ignored --nocapture`
(add `--features shader-host-tools` and the test name `grizzy_end_to_end_with_shaders` for stage 3). The FFI builds for
`aarch64-apple-ios` and `aarch64-apple-ios-sim`; the symbol check of `scripts/build-ios-ffi.sh` uses `nm -gU --no-llvm-bc`.

## End-to-end check: `scripts/e2e-grizzy.sh`

Runs the whole chain on one reference game (Grizzy and the Lemmings, title id `0100DFA027504000`) through the CLI and
compares the result with the original implementation's outputs.

```
cargo build --release -p ibuilder-cli --features shader-host-tools
cargo build --release -p ibuilder-shaderbuild --features host-tools,f2-finish     # libdiff, optional
scripts/e2e-grizzy.sh run    NAME [ibuilder options...]   # run into $E2E_ROOT/NAME under /usr/bin/time -l
scripts/e2e-grizzy.sh kill   NAME SECONDS [options...]    # SIGTERM after SECONDS (an interrupted stage 3), report what is left
scripts/e2e-grizzy.sh verify NAME                         # the checks below
```

Environment: `IOSBUILDER` (required; the original checkout holding the game inputs, host dylib and the oracle outputs;
or set it in `scripts/local.env`, see `scripts/local.env.example`), `IBUILDER`, `LIBDIFF`, `E2E_ROOT`, `ENGINE`,
`CHAIN` (the host's shader chain id), `PYLIB` (the original's full library image; check 3 is skipped without it).
Nothing is written outside `E2E_ROOT`. Typical sequence: `run cold`, `run warm` (same name again: everything cached),
`kill resume 30` then `run resume --shader-jobs 6`, `verify cold`.

What `verify` checks:

1. Exit 0 and `stages: assemble=ok, profile=ok, rewrite=ok, shaders=ok` (from `run`).
2. The 21 rewrite files (`patched.bin`, `fixups`, `sites`, `gates`, `cuts`, `obs`, `changed` for `main`, `sdk`,
   `subsdk0`) are byte-identical to the original's.
3. The shader library against the original's with `libdiff`: all 56,885 files have the same names; 56,882 are
   byte-identical. The three that differ (`c1_full.tsv`, `cb_extent.tsv`, `inputs.tsv`) differ only in the stamped
   digest and time-stamp lines. The digest covers eight profile tables, and one comment line in `shader_map.tsv` differs
   between the original's stored profile and the one derived in-process; no shader file depends on it.
4. `codesign --verify --strict` passes on the signed Mach-O alone. On the framework bundle it reports missing
   resources, because `ibuilder-sign` signs the Mach-O only and writes no `_CodeSignature/CodeResources` (the app's
   zsign seals the bundle).

Measured on 2026-11 on an M1 Pro (loaded, indicative): cold run 92-97 s (mostly the engine's native translation tools),
resumed after two kills 49.7 s, rebuild from a full cache 14.8 s, fully cached repeat 0.35 s.

## Known limits

- Verified on macOS and in the iOS simulator, not on a physical iPhone (jetsam, `os_proc_available_memory`, `dlopen` of
  the real host, zsign). The simulator reports a tiny `phys_footprint`: read no footprint conclusions from it.
- Cancel and stop requests act between modules and stages, not inside the rewriter or a translation.
- Device signing (Keychain, provisioning profile, zsign) stays in the app; `ibuilder-sign` is ad hoc only.
- `heap_signals` and `depth_signal` are re-implemented from the engine's code; equality across all games is not verified.
