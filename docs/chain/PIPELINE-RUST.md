# The chain driver in Rust: `ibuilder-pipeline`, `ibuilder-ffi`, `ibuilder-cli`

What runs on the iPhone today is CPython (Python.framework + the stdlib, extensions registered statically) running `gameapp/gameapp_pipeline.py`, bridged to C by
`gameapp/gameapp_shim.c`. These three crates replace **the Python and the CPython bridging**; the stage workers are other crates (`ibuilder-rewrite`, `ibuilder-macho`,
`ibuilder-sign`, `ibuilder-profile`, ...) plugged in through adapters. Sources ported: the iOSBuilder worktree `$IOSBUILDER_WORKTREE` (newer than the main checkout) —
`gameapp/{gameapp_pipeline.py, gameapp_shim.c, test_pipeline.py, test_resume.py, run_tests.sh}`, the logic parts of `GameAppMain.cs`, `GameAppResume.cs`, `GameAppUi.cs`,
`GameAppLogs.cs`, and `docs/DECISIONS.md` traps 17–28, 32, 39, 44–47, 49–51, 58, 71, 74–76, 80.

| crate | what | size |
|---|---|---|
| `crates/ibuilder-pipeline` | the chain driver: stage model, settings table, run markers + resume state machine, timings, memory/thermal gates, log rotation, live-shader queue; `sys/` = the OS calls | 8.6 k lines lib, 2.2 k lines tests |
| `crates/ibuilder-ffi` | the stable C ABI (`include/ibuilder.h`, 29 functions), `staticlib + cdylib + rlib`; the C tests | 1.7 k lines incl. header and C tests |
| `crates/ibuilder-cli` | `ibuilder`, the Mac CLI, and the **real adapter set** (also what the FFI links with `--features real-stages`) | 0.7 k lines |

Hard rules kept: nothing outside `iBuilderRust` and the scratch folder was written; no `git`; no secrets or game data in the repo (the Grizzy end-to-end test is `#[ignore]`d and
skips when its read-only fixtures are absent); `ibuilder-pipeline` contains no `fork` / `exec` / `posix_spawn` / `Command` (checked with `grep`: the only hit is the log line saying so)
and `#![deny(unsafe_code)]` everywhere except `sys/`, whose module doc justifies each `unsafe` (Mach `task_info`, `vm_region_recurse_64`, `dlopen`/`dlsym` + a call through the host's
`dm_host_probe_boot`, `dup2`, signal handler, `setenv`, `mmap`).

## 1. Python → Rust

### `gameapp_pipeline.py` (2,815 lines)

| Python | Rust |
|---|---|
| `read_config`, `cfg.get(...)` | `config.rs` (`Config`) |
| `mem`, `mem_line`, `release_memory`, `shader_jobs_for_memory`, `rewrite_jobs` | `memory.rs` (`MemSource`, `mem_line` — the `[mem:tag]` text is the contract with the app) |
| `STAGE_FILE`, `stage(k, n, text)`; the UI's `ParseStage`/`IsBootStage` | `stage.rs` (`StageId`, `write_stage`, `parse_stage`, `is_boot_stage`, the `Stage` trait, `set_stage_hook`) |
| `Log` | `log.rs` (`Log`, `FileLog` append + banner per run, `TeeLog`, `MemLog`) |
| `_profile_title_id`, `profile_hex`, `_map_rows`, `_last_guest_run`, `game_table_dir`, the engine's `Game` rows | `tsv.rs` |
| `_derived_shader_maps`, `set_shader_map`, `install_shader_packs`, `set_shader_pack`, `shader_cache_folder`, `aoc_images`, `dlc_signature`, `stale_split`, `stale_dlc`, `use_config_paths` | `shaderpack.rs` |
| `GAME_SETTINGS`, `LAUNCHER_NOTES(_REPLACE)`, `GAMEAPP_ROWS`, `LINKED_SWITCHES`, `write_launcher_settings`, `apply_game_settings`, `apply_env_files`, `launcher_envs/_catalogue/_stale`, `game_gfxapi`, `shader_pulling_wanted` | `settings.rs` (+ `to_python_json`: `json.dump(indent=1)` spelling, ensure_ascii) |
| `profile_modules`, `presize_heap`, `apply_heap_check` (+ the engine's `heap_signals`, `depth_signal`), `set_alias_size`, `check_address_space`, `set_game_env`, `invalidate_previous_build`, `save_game_icon`, `drop_exefs` | `profile.rs` |
| `FRAMEWORK_STAMP`, `boot_only_mark_attempted/_consume`, `note_boot_result`, `note_killed_run`, `take_continue`, `restart_for_shaders` | `markers.rs` |
| `rewrite_key`, `_link_tree`, `do_rewrite`, `rewrite_is_cached` | `rewrite.rs` (`Rewriter` adapter) |
| `file_fingerprint`, `shader_work_signature`, `ShaderResume`, `shaders_will_build`, `build_shaders_on_device`, `do_shaders`, `learn_shaders`, `relearn_shaders`, `LiveShaders`, `BUILD_STATS` | `shaders.rs` (`ShaderBackend` adapter) |
| `ThermalGate`, `read_thermal` / `ShaderMemoryGate` | `thermal.rs` / `gate.rs` (`Clock` trait so the tests run on a fake clock) |
| `Timings`, `device_model`, `BUILD_TIMES_*` | `timings.rs` |
| `do_assemble`, `INFO_PLIST` | `assemble.rs` (`Assembler` + `Signer` adapters) |
| `boot_only`, the boot tail of `_chain` | `boot.rs` |
| `main`, `_chain`, `extract_nup`, `do_profile` | `chain.rs` (`NupStage`, `ProfileStage`, `RewriteStage`, `ShadersStage`, `AssembleStage`, `run_chain`, `ChainReport`) |
| `os.environ[...]` | `env.rs` (`Env`, `MapEnv`, `ProcessEnv`: the process environment is written once, by `commit()`, right before the guest runs) |
| `_dmgameapp` | `adapters.rs` (traits) + `sys/` |
| `stdlib_extensions_check`, the `_SPAWN_EVENTS` audit hook, `PYTHONHOME`, `choose_gil`, `Py_BytesMain`, `g_chain_result` | gone with the interpreter (the "no process is spawned" proof is now that the crate has no process API) |

### `gameapp_shim.c` (1,618 lines): what was logic stays, what was CPython bridging disappears

| shim | Rust |
|---|---|
| `mem_now`, `gameapp_memline`, `gameapp_available`, `gameapp_free`, `py_mem`, `py_trim` | `sys/mem.rs` (`task_info` resident/peak/`phys_footprint`, `getrusage`, `os_proc_available_memory` by `dlsym`, `malloc_zone_pressure_relief`) |
| `as_census`, `as_reserve_block`, the `gameapp_as_reserve` constructor, `gameapp_address_space` | `sys/vm.rs` (+ `sys/boot.rs::early_init`, + the `__mod_init_func` initializer in `ibuilder-ffi`) |
| `read_code_directory`, `self_identifier`, `extended_va_init`, `gameapp_get_task_allow` | `sys/entitlements.rs` (safe byte parser of the app's own Mach-O signature) |
| `redirect_guest_output`, `restore_output` | `sys/redirect.rs` |
| `boot_framework`, `term_after`, `py_boot`, `gameapp_quit_game`, `consume_boot_request` | `sys/boot.rs` (`DlopenBoot`) |
| `install_signal_logger` | `sys/signals.rs` (now with its own duplicate of the log fd: the shim's was closed at the end of a run) |
| the `live_*` queue (`live_handler`, `live_next`, `live_done`, `live_stop`) | `live.rs` (`LiveQueue`) + the C trampoline in `sys/boot.rs` |
| `gameapp_run_chain` | `ibuilder_run_chain` |
| **not ported, stays on the app's side** (Apple frameworks, C#, third-party code): the Keychain identity (`gameapp_identity_*`, `device_identity_sign`, `py_sign` = zsign), `gameapp_nsz`, the touch pad (`gameapp_pad_*` are just `dlsym`s: use `ibuilder_host_symbol`), `CFRetain` of the game layer, the `GameAppLiveShader` notification (hook: `ibuilder_live_notify_fn`), the managed translator entry (`gameapp_set_xlate` → `ibuilder_tool_fn`), SPIRV-Cross, the zstd bindings (belong to the shader crate) | callbacks in `ibuilder_options` |

### What the C# app decided, now a tested state machine (`state.rs`, `configtext.rs`, `logwindow.rs`)

`ConfigText` → `config_text`; `BeginRun`/`Finish` markers → `begin_run`/`clear_run_marker`; `AutoRunFromArgs`/`ConsumeArgs`/`ResolveUnderDocuments` → `read_args_request`;
`ReportInterruptedRunIfAny` + `RecordKilledBuild` → `report_interrupted_run`; the branches of `ViewDidLoad` (interrupted → start nothing; args; continue; boot-only armed /
attempted / withdrawn) → `decide_launch`; `ResumeKilledBuild`/`StartContinuedBuild`/`RestartForStage3` → `resume_plan`/`prepare_continued_build`/`restart_end_by_design`;
`GameAppUi.Classify{BootRequest,Framework,PreviousRun}`, `LastRunLine`, `ParseFrameworkStamp` → `classify_*`, `last_run_line`; `LogWindow` (marks, window, trim; not the zip) → `logwindow.rs`.

The files and their meaning are tabulated at the top of `crates/ibuilder-pipeline/src/state.rs` (`gameapp.running`, `.args` → `.args.done`, `.boot-only` armed/attempted, `.stage`, `.killed`,
`.continue(.run)`, `.framework`, `.load-blocked`, `.thermal`, `.timing.partial`, `build_times.tsv`, ...). Names and formats are unchanged: the C# keeps reading them.

## 2. The stage model and the adapters

`Stage { fn id(); fn run(&self, &mut Ctx) -> Result<StageOutcome> }`; `Ctx` holds what the Python kept in module globals (docs/bundle/config/env/logs/memory source/clock/thermal gate/timings/
stats/shader-pack root/cancel flag/adapters/live queue), so two chains can live in one process. Workers plug in through `adapters.rs`:

| trait | replaces | wired to (in `ibuilder-cli::adapters`) |
|---|---|---|
| `ProfileDeriver` (`derive`, `render`) | `profile_derive.py` | `ibuilder-profile` (`derive::derive`, `game::write_profile`) |
| `Rewriter` | `pyrewrite_driver.run_once(patch_nso.py)` | `ibuilder-rewrite::rewrite` on a rayon pool of `req.jobs` threads, writing `patch_nso.py`'s files |
| `Assembler` | `assemble.build` | `ibuilder-macho::assemble_files` |
| `Signer` | `_dmgameapp.sign` | `ibuilder-sign::sign_file` (ad hoc); on the phone a C callback (zsign + Keychain) |
| `ShaderBackend` (`split_version`, `split`, `build`, `live_translate`, `post_rewrite`), `ShaderTools` (`xlate`, `spirv_cross`) | `shader_device.py`, `dmsh_build.py`, `shader_split.py`; the managed translator and SPIRV-Cross | `ibuilder-shadersplit` + `ibuilder-shaderbuild` (`shaders::RustShaders`, section 9); tools: the app's C callbacks, or on the Mac the engine's binaries (`shader-host-tools`) |
| `NupReader` | `extract_nup` / `pack_nup.py` | **nothing yet** |
| `LauncherBase` | the engine's `dm/games/settings.py` | **nothing yet** (`ibuilder-profile::game::header_macros` is most of what it needs) |
| `BootHost` | `_dmgameapp.boot` | `sys::boot::DlopenBoot` |
| `ModsHost` (`plan`, `apply_module`, `remove_patched`, `mark_applied`, `apply_boot`) | `gameapp/mods.py` | `ibuilder-mods` (`RustMods`, feature `mods`) -- section 8 |

Result codes (`ibuilder_run_chain`, same as `gameapp_run_chain`; the shim passes them through unchanged; `include/ibuilder.h` has them as `IBUILDER_RC_*`):

| code | meaning |
|---|---|
| 0 | the chain ran and no stage failed (`result=ok`; `shaders=none (...)` is not a failure) |
| 1 | a stage that must succeed failed (NUP read, profile, rewrite), or a boot-only launch whose boot did not run (the dlopen failed, no entry point) |
| 2 | no `extract_out` in the config (the shim also returns 2 when it has no bundle or `ibuilder_create` failed) |
| 3 | boot-only had nothing to boot (no framework, no `flat/`, a framework another app build made) |
| **4** | **the game has no shader library** (`IBUILDER_RC_NO_SHADER_LIBRARY`, review r4 §4.1): stage 3 had to build one and could not (`shaders=failed (no library: ...)`, `result=failed (shaders)`). A build still goes on to the assembly (a failed stage 3 does not stop the chain, as in Python) but it **boots nothing** (in-process, or by writing `gameapp.boot-only`), **writes no framework stamp** (the app reads the build as not built: "Needs setup", never "Ready") and logs `stage 5 NOT STARTED: [RED] the game has no shader library (stage 3: ...)`. A boot-only launch whose stage 3 cannot build the missing library **refuses the boot** (`stage 5 boot-only REFUSED: [RED] ...`), consumes the request (a later launch does not start it by itself; a tap tries again) and keeps the framework and its stamp |
| **5** | **no framework** (`IBUILDER_RC_NO_FRAMEWORK`, review r4 follow-up): stage 4 failed, the assembly or the signature (`assemble=failed`; the signer's rc 10-14 for a missing or unreadable certificate, a module the assembler refused, ...). Nothing is stamped or booted (as before) and the chain logs `stage 5 NOT STARTED: [RED] the game could not be assembled or signed (stage 4: ...)`. It returned 0, as Python did, and the app said "Built: ready to play.". When stage 3 failed too, the code is 5 (the game has neither; the status names both) |
| 75 | the chain ended on purpose before stage 3 (memory): close the app, the next launch continues |
| 130 | cancelled (`ibuilder_cancel`, `ibuilder_request_stop(2)`) |
| 131 | paused by the user (`ibuilder_request_stop(1)`): everything kept |

**`none` vs `failed` for stage 3.** `shaders=none (...)` is a game with nothing to build: no host chain in the config, `shader_build=0`, a game whose split finds no shader (the
`<id>.none` marker). It is not a failure: the result stays `ok`, the code 0, the framework is stamped and booted as before (such a game may draw nothing; that is what the configuration
asked for). `shaders=failed (no library: <why>)` is a game that was to get a library and has none: too little free space (`shader_free_gib`), no backend or translator in the build, inputs
missing (RomFS, `ExeFS/main`, the profile table), the split, the translation or the pack install failed. That is code 4. A stop (pause, cancel) during stage 3 is neither: it is 131 / 130.

Why the decision is the library's and not the shim's or the C#'s: one place decides (the Rust chain, unit-tested with the CLI and the C ABI test on the same code), and the C# needs no
string parsing of `ibuilder_chain_status()` (a 1 KiB buffer the reason may overflow). An app without patch 0078 already treats 4 and 5 as failed runs (anything but 0 / 130 / 131 is a
failure there): it fails closed, with a generic text; 0078 gives them their words. The shim pins the values (`_Static_assert(IBUILDER_RC_NO_SHADER_LIBRARY == 4)`, `... NO_FRAMEWORK == 5`)
because the C# reads them.

The restart before stage 3 (trap 75) is `restart_for_shaders` → `gameapp.continue` → result 75 → the app closes → the next launch's `decide_launch` returns `ContinueBuild` →
`prepare_continued_build` writes `gameapp.continue.run` → the chain's `take_continue` merges the earlier timings (see "a latent Python bug" below). The decision is explicit and tested; whether
Rust's allocator needs it as much as CPython's did is a question for the phone (nothing in Rust "keeps ~3 GB the rewrite's threads freed"; the conditions are unchanged so behaviour is identical).

## 3. The C ABI (`include/ibuilder.h`) and how the C# app switches

Handle `ibuilder_t*` (opaque); `ibuilder_options` (struct_size-versioned) carries documents/bundle dirs, a log callback (stream 0 = `pipeline.stages.log`, 1 = `gameapp.chain.log`), a progress
callback (stage k/n, text, bar fraction), a sign callback, the two tool callbacks (same C signature as the shim's `gameapp_tool_fn`, so the existing `XlateEntry` plugs in unchanged),
a live-shader notify callback and flags. `ibuilder_cancel` is the cancellation flag. `ibuilder_last_error` + no panic across the boundary: every entry point runs under `catch_unwind`
(unwinding builds: the call returns `IBUILDER_E_PANIC`, tested); the workspace release profile is `panic = "abort"`, where nothing can be caught, so a panic hook (installed by
`ibuilder_create`) writes `[ibuilder] PANIC: ...` to `gameapp.chain.log` and then the process aborts (tested in release: `tests/c/panic_abort.c`).

Switching the app, step by step (nothing here is a build I ran; the C# was not touched):

1. Link: `scripts/build-ios-ffi.sh` makes `libibuilder_ffi-ios.a`, `-ios-sim.a` and `ibuilder.xcframework`. In the .csproj replace the CPython `NativeReference`s with
   `<NativeReference Include="libibuilder_ffi.a" Kind="Static" ForceLoad="true" SmartLink="false" />` (ForceLoad: the exports are only referenced through `__Internal`).
   Set `IPHONEOS_DEPLOYMENT_TARGET` to the app's minimum when building the archive (else one linker warning per object).
2. P/Invoke (`[DllImport("__Internal")]`), replacing the `gameapp_*` imports of `GameAppMain.cs`:
   ```csharp
   [StructLayout(LayoutKind.Sequential)] struct IBuilderOptions { public uint StructSize; public IntPtr DocumentsDir, BundleDir;
       public IntPtr Log, LogUser, Progress, ProgressUser, Sign, SignUser, Xlate, SpirvCross, LiveNotify, LiveNotifyUser; public uint Flags; }
   [DllImport("__Internal")] static extern IntPtr ibuilder_create(ref IBuilderOptions o, out int err);
   [DllImport("__Internal")] static extern int   ibuilder_run_chain(IntPtr h, IntPtr configPathOrNull);   // = gameapp_run_chain()
   [DllImport("__Internal")] static extern void  ibuilder_cancel(IntPtr h);
   [DllImport("__Internal")] static extern int   ibuilder_chain_status(IntPtr h, byte[] buf, int cap);
   [DllImport("__Internal")] static extern int   ibuilder_mem_line(string documents, byte[] buf, int cap);   // = gameapp_memline
   [DllImport("__Internal")] static extern long  ibuilder_available_bytes();                                // = gameapp_available (-1: none)
   [DllImport("__Internal")] static extern int   ibuilder_address_space_info(out IBuilderAddressSpace a);    // = gameapp_address_space + extended_va + increased_memory + get_task_allow
   [DllImport("__Internal")] static extern void  ibuilder_set_game_layer(IntPtr layer);                     // after CFRetain
   [DllImport("__Internal")] static extern void  ibuilder_quit_game(string documents);                      // = gameapp_quit_game
   [DllImport("__Internal")] static extern IntPtr ibuilder_host_symbol(string name);                        // dm_front_pad_create / dm_front_pad_layout (gameapp_pad_*)
   ```
   Callbacks are `[UnmanagedCallersOnly]` function pointers (`delegate* unmanaged<...>`), exactly as `XlateEntry` is today. `ibuilder_decide_launch` / `ibuilder_begin_run` /
   `ibuilder_finish_run` / `ibuilder_classify_*` / `ibuilder_parse_stage` replace the C# decisions one at a time (`ibuilder_decide_launch` returns JSON).
3. Keep in the C shim only: Keychain identity + `py_sign`'s zsign call (now the `ibuilder_sign_fn` callback; its mode text and rc codes are unchanged), `gameapp_nsz`, the `CFNotification` for live shaders.
4. Call `ibuilder_early_init(documents)` or rely on the Mach-O initializer that is in the archive for `target_os = "ios"` (`__DATA_CONST,__mod_init_func`, one 8-byte entry, checked with `otool -l` on the
   linked simulator client): it replaces the shim's `__attribute__((constructor))`.
5. The config: `ibuilder_config_text` is not exported (the C# keeps its `ConfigText`; the same keys are in `configtext.rs`, tested against the app's key order).
6. `IBUILDER_FLAG_BOOT_IN_PROCESS` = `PIPELINE_BOOT=1`. Without it the chain builds and returns; as in Python **the interrupted-run marker is then the caller's to clear** (`ibuilder_finish_run`),
   which is what `Finish()` does today.

## 4. The CLI

```
cargo build --release -p ibuilder-cli
target/release/ibuilder --exefs <ExeFS> --out <dir> --bundle <engine dir: dm/games, dm/host> --host <host-prebuilt.dylib> [--romfs ..] [--threads N] [--report f.tsv] [-v] [--mock]
```
`--out` is laid out like the app's Documents; the inputs are linked (symlinks) into `<out>/extract/`, never copied or written (`keep_exefs=1`). Prints per step: wall, peak resident (sampled every
5 ms from `task_info`) and the process high-water mark. Exit 0 all stages ok, 1 a stage failed, 2 usage, 130 cancelled. `--features mock-stages` adds `--mock` (stand-in workers).

## 5. What is verified, with the exact commands

All on this Mac (M1 Pro, 8 cores), `CARGO_TARGET_DIR=$SCRATCH/target-pipeline`. **The machine was shared and heavily loaded (load average 22–37): timings are indicative.**

```
cargo test --release -p ibuilder-pipeline -p ibuilder-ffi -p ibuilder-cli
```

| suite | tests | result |
|---|---|---|
| `ibuilder-pipeline` unit tests (`src/`) | 57 | pass |
| `tests/pipeline_spec.rs` — `gameapp/test_pipeline.py` | 52 | pass |
| `tests/resume_spec.rs` — `gameapp/test_resume.py` | 16 | pass |
| `tests/chain_spec.rs` — the whole chain on stand-in workers (order, outputs, cache on 2nd run, restart 75, failure semantics, boot decision, cancel, continued timings) | 9 | pass |
| `tests/boot_spec.rs` — **real `dlopen` boot** of a host built by clang: guest output → `gameapp.guest.log`, env seen by the host, layer handed over, live shader queued by the host callback / translated / answered / learned, request consumed, marker cleared, time limit stood down | 1 | pass |
| `ibuilder-ffi` unit tests (callbacks, panic guard, argument checks, launch JSON) | 6 | pass |
| `tests/c_abi.rs` — builds the staticlib, **compiles three C programs with clang against `ibuilder.h`** and runs them: `test_abi.c` (42 checks: the whole chain through the ABI with C log/progress/sign/tool callbacks, memory, address space, launch state, cancel, early initializer), `panic_abort.c` (release `panic=abort`: child aborts, cause in the log), `boot_limit.c` (a host that runs 5 s, `boot_seconds=1`: the process ends by its own SIGTERM after ~1.1 s, request consumed, cause logged) | 1 | pass |
| `ibuilder-cli` (layout, mock run, step table, bad input) + `meter` | 4 | pass |
| `tests/grizzy_e2e.rs` (`#[ignore]`: `cargo test --release -p ibuilder-cli --test grizzy_e2e -- --ignored --nocapture`) | 1 | pass (7.9 s) |

**Python tests → Rust.** Every `check(...)` of the two Python files is a Rust assertion in the specs above; each Rust test names the Python checks it carries (`// py: "..."`).
Deliberately **not** carried over (about 10 of `test_pipeline.py`'s 157 checks and 20 of `test_resume.py`'s 92; counts by reading, not by tool):

* `test_resume.py`'s split block (11 checks: `JobMemo`, "an interrupted split resumed is byte-identical", damaged job file, ...) and `test_pipeline.py`'s `pyrewrite_driver` `sys.exit` block (2): they test
  `shader_device.py` and `pyrewrite_driver.py`, not the pipeline — they belong to the `ibuilder-shader` crate (the memo) and vanish with CPython (`sys.exit` in-process). The pipeline half of the same
  behaviour (`ShaderResume`, the signature, the kept `splitjobs/` dir handed to the backend) *is* tested.
* source-text greps (2 in `test_pipeline.py`: "the rewrite passes DM_REWRITE_JOBS" and "the build drops ExeFS only once..."; the 9 "the pipeline carries 'ended by iOS'..." marker checks in `test_resume.py`): replaced by
  behaviour tests (the hard-link grep became an inode check; `chain_spec` tests "ExeFS dropped only when framework and shaders are there"; the log lines are asserted where they are produced).
* the checks that need the engine's own `settings.py` / `profile_guess.py` from an engine clone (4 of the launcher block: "rows its rewritten modules allow", "NVN-only rows", "no Mac-only row", "patched
  engine's rows 0203/0205"): the base-document generator is the `LauncherBase` adapter; the pipeline's post-processing (Metal 4 rows dropped, notes, DM_SHOW_FPS default, GameApp's rows, catalogue digest,
  stale check, player settings → switches) is tested on a stand-in generator. `heap_signals`/`depth_signal` are re-implemented in `profile.rs` from reading `profile_guess.py` (it is engine code); they are
  tested on the cases the Python test covers (and the Python test ran the engine's real function) — **equality with the engine's function across all games is not verified**.
* the GIL combinations (2): Rust has no GIL; "parallel shader preparation" is on unless the player turned it off.

### End to end on Grizzy and the Lemmings (real stage crates)

```
target/release/ibuilder --exefs iOSBuilder/out/ref-out-grizzy/ExeFS --out <scratch>/cli-grizzy --bundle <scratch>/ref/engine \
   --host iOSBuilder/out-load/host-prebuilt-loadable-ios.dylib --romfs iOSBuilder/out/ref-out-grizzy/romfs_manifest.tsv
/usr/bin/time -l ...
```
Grizzy is not a registered game in the engine copy, so stage 0/1 derives the provisional profile (`ibuilder-profile`); stage 3 has no backend (`shaders=none`, see gaps).

| step | wall | peak resident (sampled) |
|---|---|---|
| 1 derive the profile | 0.38 s | 261 MiB |
| 2 rewrite `main` (75 MB, 4 threads) | 4.9 s | 491 MiB |
| 2 rewrite `subsdk0` / `sdk` | 0.6 s / 1.3 s | 483 / 424 MiB |
| 4a assemble (192.7 MB, 42 load commands) | 0.07 s | 424 MiB |
| 4b sign (ad hoc, 185 MB) | 0.07 s | 402 MiB |
| **total** | **7.3–7.8 s** (3 runs: 7.51, 7.75, 7.27) | `/usr/bin/time -l`: max RSS 515–524 MB, peak footprint 504 MB |
| same with `--threads 1` | 11.9 s (`main` 9.3 s) | max RSS 534 MB |

Versus the Python reference on this Mac (`docs/CONVENTIONS.md`): `main` rewrite 200.5 s / 4.13 GB RSS serial, 69.6 s / 1.94 GB with the fork pool (the iPhone cannot fork: 113–155 s, ~2.2 GiB footprint on an
A17 Pro). The Rust numbers are the whole chain on a loaded Mac; they are **not** a phone measurement.

* **Byte-identical**: all 21 files the Rust rewrite writes for `main`/`subsdk0`/`sdk` (`patched.bin`, `fixups`, `sites`, `gates`, `cuts`, `obs`, `changed`) equal the Python oracle
  (`out-py/pyrewrite/out/<module>/`) — `cmp` on each and `tests/grizzy_e2e.rs` with `ibuilder_testkit::compare_files`. (`match.tsv` is not written: the oracle used the registered table with `build` rows;
  the derived table has none. The chain does not read it.)
* The framework is signed: `ibuilder_sign::verify_file` → all 47,047 pages ok; `codesign --verify --strict` on the Mach-O alone passes. On the *bundle* `codesign` says "code has no resources but signature
  indicates they must be present": `ibuilder-sign` signs the Mach-O only and writes no `_CodeSignature/CodeResources` (zsign, on the phone, seals `Info.plist`).

## 6. iOS portability

The Homebrew toolchain has no `aarch64-apple-ios` std; the coordinator installed rustup, used **only** by prefixing the command (`PATH=/opt/homebrew/opt/rustup/bin:$PATH`, never globally).

```
export CARGO_TARGET_DIR=<scratch>/target-pipeline IPHONEOS_DEPLOYMENT_TARGET=14.0
PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios     -p ibuilder-ffi --features real-stages    # Finished, 29 s
PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-ffi --features real-stages    # Finished, 32 s
scripts/build-ios-ffi.sh <out>        # both targets + exported-symbol check + ibuilder.xcframework (xcodebuild -create-xcframework): ran, ok
```
1. **Both targets build, with the real stage crates linked** (profile, rewrite incl. the capstone C code, macho, sign): `libibuilder_ffi.a` 44.9 MB each (without `real-stages`: 24.9 MB).
2. **Exported symbols = the header**: `nm -gU --no-llvm-bc libibuilder_ffi.a | grep ' T _ibuilder_'` → **29 symbols on `aarch64-apple-ios` and on `-ios-sim`, identical to each other and to the 29 functions in
   `ibuilder.h`** (a `mock-stages` build adds the test-only `ibuilder_test_panic`, declared under `IBUILDER_TESTING`) (`comm` of the two lists: empty both ways). Note: Xcode's `nm` cannot parse the objects without `--no-llvm-bc` ("Unknown attribute kind"): the workspace release profile has `lto = "thin"`, so
   Rust's LLVM 23 embeds bitcode (`__LLVM,__bitcode`) that Apple's LLVM 21 reader rejects. Apple's linker links them fine (below). If the app's Xcode ever fails on those objects, build the archive with
   `CARGO_PROFILE_RELEASE_LTO=off` and `-C embed-bitcode=no`.
3. **Linked and run in the booted iPhone 17 simulator** (iOS 27.2): `xcrun --sdk iphonesimulator clang -arch arm64 -target arm64-apple-ios14.0-simulator ... test_abi.c libibuilder_ffi.a -framework CoreFoundation`
   (LC_BUILD_VERSION platform 7, minos 14.0), then `xcrun simctl spawn <SIMULATOR-UDID> ./test_abi <docs> <docs2>`: **0 failures of 42 checks** (the mock-stage build: whole chain with
   C callbacks, memory, address space, launch state, ...).
   And `tests/c/run_chain.c` against the **real-stage** archive: `xcrun simctl spawn ... ./run_chain_sim <Documents> <engine bundle>` ran the **whole Grizzy chain inside the simulator process**:
   `ibuilder_run_chain -> 0 in 12.13 s`, `peak 502 MiB`, rewrite cache cleared first, and the 21 rewritten files are **byte-identical to the Python oracle**; the framework passes `codesign --verify --strict`.
4. **Memory/footprint paths on iOS** (`tests/c/mem_probe.c` in the simulator): `task_info` works (`resident 23.9 MiB → 280.1 MiB (+256.2) after touching 256 MiB; footprint 3.6 → 259.7 MiB; peak follows`);
   `dlsym(os_proc_available_memory)` is **present** on the simulator and returns 0 → `available` 0 / `ibuilder_available_bytes() = -1` ("no ceiling", as the shim reported for the simulator);
   `vm_region_recurse_64` census: `free 130625 GB, largest hole 130623 GB`; the CodeDirectory parser read the simulator binary's own signature (`self_identifier: mem_probe_sim`, entitlements 0). Same program on
   macOS: same behaviour (and the census there shows the same `384.00 GB tag 31` guard region the notes describe).
5. The Mach-O initializer is in the iOS client binary (`otool -l`: `__DATA_CONST,__mod_init_func`, size 8). Its logic (`early_init`) runs on macOS with `IBUILDER_EARLY_INIT_FORCE=1` in `tests/c_abi.rs` and writes the census and the "deferred" decision to the chain log.

**Not verified on iOS:** the archive linked into the real .NET app; anything on a physical iPhone (jetsam numbers, a real `os_proc_available_memory`, the 5 GB reservation at a real 3364 MiB ceiling, `dlopen` of the
real host from Documents, zsign through the callback); `dlopen`/`dm_host_probe_boot` was tested on macOS with a clang-built stand-in host, not on iOS; the early initializer's reservation itself on iOS.
`phys_footprint` reads 5 MiB while resident is 208 MiB in the simulator (the simulator's accounting): do not read footprint conclusions from sim runs.

## 7. Gaps, decisions and findings

* **Stage 3 had no real backend when this section was written** (superseded: `ibuilder-shadersplit` and `ibuilder-shaderbuild` now are the backend and the CLI runs the whole chain on Grizzy, section 9). The orchestration around
  it (resumable work folder, gates, cache, stale libraries, live queue, learned shaders) was ported and tested against stand-in backends. Without `--shaders` the CLI still reports `shaders=none` (not a failure, as in Python).
* **No `NupReader`** (`pack_nup.py`; `ibuilder-pack` is `pack_nxgame.py`) and **no `LauncherBase`** (`settings.py`).
* **Device signing**: `ibuilder-sign` is ad hoc; the Keychain `.p12` + provisioning-profile path (zsign) stays in C behind `ibuilder_sign_fn` unless someone ports it.
* **A latent Python bug, fixed in the port**: `Timings.merge_partial(game, ...)` is called with `cfg['game']` (the *dump's file name*) but the partial stores `timing.game` (the *title id*, set after stage 0), so a
  continued build never merged the earlier run's timings (the unit test used the same string for both). The Rust partial also writes `dump=` and matches on either. Found by reading, reproduced by `chain_spec::a_continued_build...`; not seen on a device.
* **Formats that changed on purpose** (first run after switching redoes some work; nothing breaks): `launcher_catalogue()` digest (sha256 of a canonical form, was sha1 of Python reprs → every `launcher.json` is rewritten once),
  `file_fingerprint` (sha256 prefix, was sha1 → an interrupted stage 3 left by the Python app does not resume once), `logmarks` use epoch seconds (was .NET ticks), the `pipeline.stages.log` banner names `ibuilder-pipeline`
  instead of Python, `gameapp.timing.partial` gains `dump=`.
* **Kept as they were, on purpose**: a failed stage 3/4 still returns 0; `gameapp.running` is cleared only by the boot path or by the caller; the in-process boot refusal below `boot_floor_mib`; every log line text the app greps.
* `rewrite_jobs`: no GIL, so `min(4, cores)` reduced by thermal/low-performance/memory as before. The DM_REEXEC re-run of `patch_nso.py` is inside `ibuilder_rewrite::rewrite` (its own loop of 3).
* Boot adapter details tested only on macOS: SIGTERM-at-limit and the designed-end log lines (also in a C process), `dlopen` failure rc 3 / missing entry rc 4 are covered by logic but only rc 0 and the limit path run in tests.
* Hazards worth knowing: `ProcessEnv::commit` uses `setenv` (unsafe in edition 2024) once, before the guest; `ibuilder_cancel` stops between modules/stages, not inside the rewriter (the rewrite crate has no cancel hook) or a translation;
  the C# worker thread's stack size was never an issue for Rust here but is unverified on a device (the shim raised `RLIMIT_STACK` to 16 MiB for CPython).

## 8. Delta since first port (2026-11-19)

The worktree's `gameapp_pipeline.py` went from 2,815 to 2,910 lines after the port above was written (and gained `gameapp/mods.py`, `tools/pyrewrite_driver.py`'s `SystemExit` handling, `paches/engine/0301-rewrite-threads.patch`,
newer `test_pipeline.py` (883 lines) and `test_resume.py` (614 lines)). Versions read (size, mtime): `gameapp_pipeline.py` 155,544 B 2026-11-19T02:24:57, `mods.py` 26,470 B 02:24:27, `test_mods.py` 22,118 B 02:24:27,
`test_pipeline.py` 57,809 B 01:43:31, `test_resume.py` 35,499 B 02:17:13, `pyrewrite_driver.py` 4,907 B 2026-11-18T20:54:36, `0301-rewrite-threads.patch` 16,092 B 01:36:03 (re-stat'ed at the end of the work: unchanged).
No copy of the previous `gameapp_pipeline.py` exists, so the audit is by reading: every function the task named, every `def`/`class`/constant of the current file against the Rust, and every string literal of the Python
(`ast`, literals of 14+ characters) against the Rust sources: 145 of 493 were not found verbatim, most of them docstrings; the others were read one by one and are the rows below or CPython-only lines.

| Python function / behaviour | Rust place | Status |
|---|---|---|
| `gameapp/mods.py` (whole) | crate `ibuilder-mods` (`docs/crates/MODS-RUST.md`) | **new**: ported, differential-tested |
| `mods_module()` (loads `mods.py`) | `adapters::ModsHost` (+ `ModPlanEntry`, `ModsFailure`), `Adapters.mods`; `ibuilder-cli::adapters::RustMods` (feature `mods`, default) | **new** |
| `rewrite_key(cfg, src, mods_digest='')` | `rewrite::rewrite_key(ctx, src, mods_digest)` | **changed**: third argument; `b"mods=" + digest + b"\0"` after `app_build`, so a game without mods keeps its key byte for byte (test: the key equals sha256 of the Python's input) |
| `do_rewrite`: `plan` before the loop, `apply_module` on a cache miss (the patched copy replaces the module as input), `remove_patched` + `mark_applied` after | `rewrite::do_rewrite` + `mods::{mods_plan, finish_rewrite}` | **new**. A plan / apply / mark failure the Python lets through (an uncaught exception in `mods.py`) fails stage 2, as in Python |
| `rewrite_is_cached`: quiet `plan`, digest into the key | `rewrite::rewrite_is_cached` | **changed**. A plan that fails counts as "not cached" (the Python lets the exception escape `restart_for_shaders`; stage 2 would have failed on it first) |
| `apply_mods(cfg, log)` in `boot_only` and in `_chain`'s boot, between `apply_env_files` and `set_shader_pack`; never raises | `mods::apply_mods`, called by `boot::prepare_boot` (both boots) | **new**. No host in the build + a game with a `mods/` folder: one line "mods: this build has no mods support" (no silent skip) |
| `rewrite_cache_dir`, cfg `rewrite_cache`, `_link_tree`, `REWRITE_RAN` | `rewrite.rs`, `ctx.rewrite_ran` | already ported; unchanged semantics (re-read) |
| `shader_parallelism`, `app_setting`, `parallel_shaders` | `shaders::{app_setting, shader_parallelism}` | already ported (the GIL texts are gone with the GIL) |
| `learned_dir`, `learned_keys`, `learned_raw`, `learn_shaders`, `relearn_shaders`, `LiveShaders` | `shaders.rs` | already ported (re-read) |
| `nup_extracted` | `chain::nup_extracted` | already ported |
| `release_mimalloc(tag, log)`, called by `release_memory` and by `restart_for_shaders` (`before-restart-check`, then the memory is re-read) | `memory::release_mimalloc` over `MemSource::release_mimalloc() -> Option<i32>`; used by `memory::release_memory` and `markers::restart_for_shaders` | **new, explicit no-op by design**: `_PyMem_MimallocRelease` is CPython-allocator-specific (a patched interpreter); Rust's allocator has nothing to hand back, so the default is `None` ("this runtime has no such function": nothing logged, the restart decides). The seam stays, so a host whose allocator can do it plugs in; five of the six cases of the Python test run against a fake that does (the sixth, a call that *raises* and is logged "mimalloc release failed", has no Rust form: a host reports failure as a non-zero rc, logged "not in this runtime (rc N)") |
| `GAMEAPP_ROWS`: `DM_PSO_BINARY_ARCHIVE` (Pipeline cache), `DM_METALFX`, `DM_EXTERNAL_DISPLAY` | `settings::gameapp_rows` | **new** (the launcher catalogue digest changes with them: every `launcher.json` is written again once, as designed) |
| `METALFX_LEVELS`, `apply_game_settings`: `DM_METALFX` -> `DM_PRESENT_UPSCALE=metalfx` + `DM_RENDER_SCALE` (0.75 / 0.5), an environment switch wins | `settings::{METALFX_LEVELS, apply_game_settings}` | **new** |
| `write_launcher_settings`: the engine's `DM_PRESENT_UPSCALE` row is dropped for an NVN game (one MetalFX row) | `settings::write_launcher_settings` | **new** |
| in-process boot's own result line ("the guest ran in this process and came back ... never called") | `boot::BOOT_RETURN_NOTE_IN_PROCESS` | **fixed** (the first port used `boot_only`'s wording for both) |
| `json.dump(doc, f, indent=1)` of `launcher.json` (no final newline) | `settings::to_python_json` | **fixed** (pre-existing: the port appended a `\n`) |
| `pyrewrite_driver.run_once`: a refused module's `sys.exit(msg)` -> `RuntimeError('patch_nso.py stopped: msg')`; exit 0/None is success | `Rewriter::rewrite` returns `Err` -> `do_rewrite` -> "stage 2 FAILED: ..." -> result 1 | **verified, tested** (`delta_spec::a_refused_module...`; `ibuilder-cli/tests/mods_chain.rs` with the REAL rewriter on a non-module: `[RED] ...: not an NSO or NRO (magic [0, 0, 0, 0])`, no panic, no exit) |
| `0301` `DM_REWRITE_JOBS=N`: N threads for the three image-only scans, output identical for any N; unset / 1 = old behaviour; `N < 1` or not a number -> `sys.exit('[RED] DM_REWRITE_JOBS must be a positive integer')` | `RewriteRequest.jobs` -> rayon pool of `jobs` + `Options.workers`; only `main` (or a one-module chain) gets threads, as `do_rewrite` always did; `RustRewriter` refuses `jobs == 0` with the same message | **verified**: `cargo test --release -p ibuilder-rewrite` (oracle: sdk, subsdk0, main identical on 1, 2 and 8 threads; 21 synthetic modules) pass; refusal tested |
| `_audit` hook, `_SPAWN_EVENTS`, the "spawn events" log line | none: the crate has no process API (checked by grep, as before); the chain logs "process spawns: none (...)" | gone by design |
| `release_memory`'s "N object(s) collected" (`gc.collect`), `stdlib_extensions_check`, `set_chain_result`, the `SystemExit` handler of `__main__` | -- | gone with the interpreter (unchanged from the first port) |
| `release_memory` -> `release_mimalloc` ordering (trim, collect, mimalloc, `mem_line`) | `memory::release_memory` | **new** (same order) |

Still open from the first port (not part of this delta): `NupReader` (`pack_nup.py`), `LauncherBase` (`settings.py`), the stage-3 backend (package B), device signing through zsign.

**Tests of the delta** (all Rust, all pass): `tests/delta_spec.rs` (10: the allocator block of `test_resume.py` against a fake patched runtime and the no-op default; the mods hooks against a recording `ModsHost`: order of calls, key by digest,
cache hit makes no copy, failing plan fails stage 2, no host says so, boot hook never raises and runs after `env.txt`; the three new rows and their key order; one MetalFX row and the two switches `DM_METALFX` sets; the in-process
boot line; a refused module through the chain), `crates/ibuilder-cli/tests/mods_chain.rs` (4: `test_mods.py`'s `do_rewrite` / cache / `rewrite_is_cached` / `apply_mods` blocks through the real mods; a refused module through the real
rewriter; zero threads refused), and `ibuilder-mods`' own (`docs/crates/MODS-RUST.md`). The new blocks of `test_pipeline.py` / `test_resume.py` since the port are the allocator block (5 + 2 of its 8 checks ported, see above) and the mods
hook in `test_mods.py`; every other `# ----` block of the two files has a Rust test of the same subject (compared block by block through the headings, not check by check).

**Results** (`cargo test --release -p ibuilder-pipeline -p ibuilder-ffi -p ibuilder-cli -p ibuilder-mods`, 2026-11-19, loaded shared Mac): pipeline unit 57, `boot_spec` 1, `chain_spec` 9, `delta_spec` 10, `pipeline_spec` 52,
`resume_spec` 16; ffi unit 6, `c_abi` 1 (three C programs compiled with clang against the unchanged `ibuilder.h`); cli unit 1, `cli.rs` 3, `mods_chain` 4 (`grizzy_e2e` stays `#[ignore]`); mods 5 unit + 7 spec. iOS:
`aarch64-apple-ios-sim` and `aarch64-apple-ios` builds of `ibuilder-mods` and `ibuilder-pipeline` finish; `ibuilder-ffi --features real-stages` builds for `-ios-sim` with mods wired in, 29 exported `_ibuilder_*` symbols (unchanged).
C ABI: unchanged (no struct, no function added): mods need no entry point, the real adapter set (`ibuilder-cli::adapters::real_adapters`) wires `RustMods` for the FFI's `real-stages` as it does for the CLI.

**Hand-off notes**: `Adapters` gained a field (`mods: Option<Arc<dyn ModsHost>>`); `Adapters::unwired()` and the mock set set it to `None`, so only code that builds `Adapters { .. }` literally needs `mods: None` (none outside this crate).
`ShaderBackend` / `ShaderTools` were not touched. `rewrite::rewrite_key` has a third parameter.

## 9. Stage 3 wired (2026-11-19): the CLI runs the whole chain on Grizzy

Results, commands and checks: `docs/chain/E2E-RUST.md`. What changed here:

* **CLI** (`crates/ibuilder-cli`): `--shaders` runs stage 3 between the rewrite and the assembly; it needs `--shader-chain ID` (the host's chain: the library is `b-<ID>.img`; it becomes the config row `shader_chain`, which is what
  `shaders_will_build` / `build_shaders_on_device` read, beside `romfs` = the linked `RomFS.img`, `exe_dir/main`, and the derived profile folder `Documents/profile-derived/<id>`) and `--engine DIR`
  (`dm/gfx/tools/{dmsh_xlate_bin/dmsh_xlate, spirv_cross_bin/spirv-cross}`, only run: `adapters::shaders::process_tools` = child processes, wired into `Adapters::tools`). `--shader-jobs N` (config `shader_jobs`) and `--set KEY=VALUE`
  (any config row, applied last: `shader_cache_gib`, `shader_free_gib`, `shader_pull`, ...) complete it. The options are refused with exit 2 and a message, before anything is written, when `--shaders` lacks the chain or the engine's tools,
  when a stage-3 option is given without `--shaders`, or when the build has no `shader-host-tools`. **Feature gate**: `shader-host-tools` (off by default: it uses `std::process::Command`, which the app never has);
  `cargo build --release -p ibuilder-cli --features shader-host-tools`. The default build, the FFI archive (`ibuilder-ffi --features real-stages`, simulator and device) and the pipeline crate still build for iOS (checked, section 6 commands).
  The per-step table now has two stage-3 rows, `3 shaders split` and `3 shaders build` (`3 shaders already built` when the pack is there); `--version` names `shader tools as child processes` when the feature is on.
* **`shader_cache_folder`** (`shaderpack.rs`): the Rust build's cache is the FOLDER `Documents/shadercache/cas-<chain>.dmsb` (`ibuilder_shaderbuild::cache`), not the engine's SQLite file `cas-<chain>.db`. The function keeps
  `cas-<chain>.dmsb` (`shader_cache_name`, `SHADER_CACHE_SUFFIX`), removes everything else that starts with `cas` **files and folders** (another chain's `.dmsb` folder, any `.db` / `-wal` / `-shm` of the engine's SQLite store, the same chain's included:
  this build cannot read it; each removal is logged with its reason; a symlink is removed, never followed; a removal that fails is logged and the build goes on), and counts the folder's total size for the `shader_cache_gib` limit
  (`folder_bytes` recurses: the entries sit in `<hh>/` sub-folders). The doc comments of `shaders.rs` / `shaderpack.rs` say `.dmsb` now. Tests: `pipeline_spec::the_shared_shader_cache` (rewritten for the folder: files and folders of other
  chains, the same chain's SQLite file, an unrelated file, the limit over sub-folders, resuming over the limit, `shader_cache=0`), `a_symlink_named_like_a_stale_cache_is_removed_and_never_followed`, and in the cli crate
  `shaders_glue::the_pipelines_cache_folder_is_the_one_the_build_makes` (the pipeline's name is the folder `ibuilder_shaderbuild::cache::Store` creates, and the build's own sweep removes the same ones).
* **Found, not changed**: `shader_pulling_wanted(work/profile, cfg)` (Python line 2216, ported as is) looks for `work/profile/game.tsv`, but the game table is copied to `work/profile/<id>/game.tsv`, so the profile's `pso_mtl4` row is
  never read and the pulling libraries are built only with `shader_pull=1` (`--set shader_pull=1`). Read from the Python and the Rust; not exercised (Grizzy's profile has no `pso_mtl4` row).
* **Kept as designed, visible in a run**: after a killed stage 3 the next one runs on 1 thread (`[ORANGE] the previous shader build ... never finished`) unless `shader_jobs` is set; `--shader-jobs 6` overrides it.

## Update (SW2): `NupReader` is wired

`NupReader` (`extract_nup` / `pack_nup.py`) is implemented by the new crate `ibuilder-nup` and wired in `ibuilder-cli` (`RustNup`, feature `nup`, default on; config key `nup`;
`ibuilder --nup FILE --out DIR`). The adapter passes the pipeline's cancel flag and progress callback into the extraction and `Docs::game_dir(tid)/aoc` as the DLC folder. The
lines above that say "nothing yet" / "No `NupReader`" describe the state before this change. Measured on the real Ocarina of Time NUP (25.6 GB): docs/crates/SWITCH2-RUST.md.

## Update (conformity audit, 2026-11-19)

`docs/CONFORMITY-AUDIT.md` is the row-by-row matrix of this port against the latest iOSBuilder. What it changed in these crates: `shaderpack::top_up_pack` (a shader pack that already exists gets the registered game's small input tables it
lacks, never overwriting anything: the prewarm seed list `pso_manifest.tsv` for Mario Kart 8 Deluxe; called from `set_shader_pack` and from stage 3), and `ibuilder-profile`'s `screen_aspect` now takes `wide` / `both` (it stopped stage 0
for Ocarina of Time, Pokopia and Animal Crossing). The warm-up of the C# app became manual through `app/patches/0020-warm-up-manual-only.patch`.

## Update (first-Play prewarm share, 2026-11-20)

`profile::set_game_env` now also sets `DM_PSO_PREPARE_PERCENT=50` (`set_pso_prepare_percent`) for a game whose seed list is large, so the host's "Preparing shaders" step builds half of the prewarm lists before the first frame and the rest in a background pass
(Ocarina of Time's first Play compiled about 25,000 shaders before its first picture). "Large" = more than 10,000 data lines (not empty, not `#`: the count the host logs as `pipeline prewarm list ...: N lines`) in the `pso_manifest.tsv` the host reads, i.e. the one
beside its shader map (`DM_NVN_SHMAP`: the pack folder `Documents/shaders/<id>/`; `DM_PSO_MANIFEST=<file>` names another, `off` none), streamed in 32 KiB blocks with no allocation that grows with the file; `pso_user.tsv` is never counted (it grows by play). A value already in the environment
(env.txt, the settings, a harness) wins, `pso_prepare_percent=<1..100>` in the config sets that share (`100` = leave the host's default; a bad value is logged and ignored), an absent or unreadable list sets nothing and says why, and `DM_PSO_PREPARE_TAIL` is never set. Only Ocarina of Time changes by default (owner's decision: Nintendo Switch Sports Resort, 5,405 lines, and Mario Kart 8 Deluxe, 1,516, keep today's behaviour). Measured on the engine's own lists, read in place: Ocarina of Time 14,748 data lines (set to 50), Nintendo Switch Sports Resort 5,405 and Mario Kart 8 Deluxe 1,516 (untouched); of the 177 lists, Ocarina is the only one above 10,000 (the next is 5,405); tests: `crates/ibuilder-pipeline/tests/pso_prepare_spec.rs` (19). **Not verified on a device, and no effect yet:** the host in `dist/host` does not read
`DM_PSO_PREPARE_PERCENT` (`strings` finds no such name in `host-prebuilt-loadable-ios.dylib` / `-sim.dylib`); the switch exists only once engine patch 0412 is built into the app. The game's `app/profiles/0400486024DEC000.env.txt` documents `DM_PSO_PREPARE_PERCENT` / `_TAIL` / `_THREADS` as commented examples.

## Update (about 500 lines before the first frame, 2026-11-20, afternoon)

Replaces the rule above ("50 for a seed list of more than 10,000 lines"). Field log (iPhone 15 Pro, iOS 27.2.1, Ocarina of Time, pack built on the phone: `pso_manifest.tsv` 25,163 lines): at 50% every Play built 13,066 lines (9,124 libraries, 3,397 render and 381 compute pipelines) before the first frame, in 1,708 s, 1,061 s and 1,118 s in three sessions; the system's Metal compile cache kept only the first ~5,500-5,800 libraries between sessions (1-4 ms each), every library after that was compiled from source again (200-1,800 ms each), so that wait came back at every Play.
`set_pso_prepare_percent` now sets `pso_head_percent(lines)` = `ceil(500 * 100 / lines)` within 1..=99 for any seed list of more than `PSO_PREPARE_HEAD_LINES` (500) data lines: Ocarina of Time 25,163 lines -> 2 (the engine's own list, 14,748 -> 4), Nintendo Switch Sports Resort 5,405 -> 10, Mario Kart 8 Deluxe 1,516 -> 33. The host's knob is a whole percent of its tasks, so the head is about 500 lines (within one percent step). The order of precedence (environment / env.txt, then `pso_prepare_percent` in the config, then the list) and `DM_PSO_PREPARE_TAIL` (never set: the background pass builds the rest while the game runs) are unchanged.
Consequence to watch on the phone: every game, Mario Kart 8 Deluxe included, now compiles most of its seed list during play (the tail: one worker at utility QoS) or on demand (a draw waits for its pipeline, `DM_PSO_NOWAIT=0`): hitches where a long first wait used to be. `DM_PSO_PREPARE_PERCENT=100` in the game's env.txt restores the old behaviour for one game. Tests: `tests/pso_prepare_spec.rs` (19 tests); not run on a phone.

## Update (the settings' "Profile" row, 2026-11-20, evening)

`settings::add_preset_row` puts a "Profile" row (`DM_PRESET`: Standard / Memory saver) first in every NVN game's launcher.json, with a `presets` map: for each option, the value of each row it covers (Standard: the row's own resolved default; Memory saver: `PRESET_MEMORY_VALUES`). Memory saver is the Apple TV port of Ocarina of Time's memory set: `DM_LOW_MEMORY=1`, `DM_OPMODE=0` (handheld), `DM_DYNAMIC_RESOLUTION=1`, `DM_FPS_TARGET=30`, `DM_RENDER_SCALE=0.5` (the host's lowest; 640x360 for a 1280x720 handheld picture) and `DM_PSO_PREBUILD=0` (no row: the boot sets it).
Ocarina of Time (`PRESET_MEMORY_GAMES`) starts at Memory saver and its covered rows' defaults become the memory values, so the sheet shows what the game runs with; every other game starts at Standard. `settings::apply_preset`, called by `prepare_boot` after the settings and env.txt, sets the chosen (or default) profile's values for the switches nothing set yet (a settings row, env.txt, the environment each win) and logs `game profile: <name> (<why>): ...`. The catalogue digest includes the profile table, so an existing launcher.json is written again at the next boot. App side: patch `app/patches/0072-settings-profile-row.patch` (picking a profile writes its values into the covered rows; each row stays editable). Tests: `tests/preset_spec.rs` (8). Not run on a phone.

Change (same evening, after the device log of Ocarina in Memory saver): `DM_LOW_MEMORY` is out of the profile (its two command allocators kept the submit thread waiting 40-325 ms a frame for 11 ms of work; GPU event waits of 5 s appeared, absent from the session without it; GPU memory still rose from 2.8 to 5.2 GB in play) and `DM_PSO_PREPARE_TAIL=0` is in (no background pass compiling the 24,699 remaining prewarm lines during play: 5,500 in 37 min, heat and resident pipelines; iBuilder's own preparation fills the Metal cache instead). Memory saver = `DM_OPMODE=0`, `DM_DYNAMIC_RESOLUTION=1`, `DM_FPS_TARGET=30`, `DM_RENDER_SCALE=0.5`, `DM_PSO_PREBUILD=0`, `DM_PSO_PREPARE_TAIL=0`.

## Update (review r4, 2026-11-21: no shader library is a failed run; pack installs that survive a kill; I/O on the hot paths)

**A game with no shader library is neither booted nor "Ready"** (r4 §4.1, §2.2; the codes above). `chain.rs`: `RC_NO_SHADER_LIBRARY` (4) and `shader_library_failure(status)`; after the
assembly a `failed` stage 3 skips `boot_after_build` (so no in-process boot and no `gameapp.boot-only`), clears the run's marker when the boot is enabled, and returns 4. `assemble.rs`: the
framework stamp is not written when stage 3 failed (`framework stamp: NOT written: stage 3 could not build the shader library ...`). `boot.rs`: `boot_only` calls `build_shaders` (the
`Result`) and refuses the boot on an `Err`, with the status `shaders=failed (no library: ...)`, `result=failed (shaders)`, the request consumed and the marker cleared. App side: patch
`app/patches/0078-no-shader-library-is-a-failed-build.patch` (the message "The shader library could not be built: the game would draw nothing. Rebuild, or send the log.").
Fixture changed: `tests/boot_spec.rs` booted a pack with a shader map and no library image (exactly the case refused now); it now has its library.

**A failed assembly or signature is code 5** (the owner's decision after this package's finding): `assemble=failed` returned 0 as in Python; without a stamp the game read "Needs
setup", but the app's run card said "Built" and the notification "Built: ready to play.". `chain.rs`: `RC_NO_FRAMEWORK` (5), the `stage 5 NOT STARTED: [RED] the game could not be
assembled or signed (stage 4: ...)` line, the run's marker cleared when the boot is enabled; 5 wins over 4. App side: 0078's `ChainResult.NoFramework` ("The game could not be assembled
or signed: see the log."). The C ABI test had the pattern without seeing it: its later runs assembled nothing (the first run dropped `extract/ExeFS`) and returned 0; its config now has
`keep_exefs=1`, every run assembles, and a run whose C signer returns 12 checks 5. Tests: `chain_spec::a_failed_rewrite_stops_the_chain_and_a_failed_assembly_does_not` (5, no boot, no
request), `chain_spec::a_build_that_fails_at_stage_4_and_at_stage_3_returns_no_framework`, `cli::the_exit_code_says_how_the_run_ended` (5 -> 1), `tests/c/test_abi.c`.

**Pack install after a kill** (r4 §4.1). `install_pack` moves the old pack aside (`<id>.old`) before renaming the new one in: a kill between the two left `<id>.old` and no `<id>`, and the
next install's `rmtree(<id>.old)` deleted the only copy. Now `recover_pack` (at the chain's start for every `<id>.old`, `recover_interrupted_installs`, and in `build_shaders` for its game)
renames `<id>.old` back when there is no `<id>`, and removes it when there is one (an install that finished but for its last step); a leftover `<id>.part` is removed at the chain's start;
`install_pack` never removes an `<id>.old` that has no `<id>` beside it before the new pack is in place, and puts it back when the new one cannot be. The import of a `.dmshaders` pack
(`shaderpack::install_shader_packs`) uses `install_pack` too (it was `rmtree(dest)` then `rename`). Tests: `shaders::tests::the_only_copy_of_a_library_survives_...`,
`chain_spec::a_pack_install_cut_short_by_a_kill_gets_its_library_back_at_the_next_start`.

**No fixed temporary name left**: `timings.rs` (`build_times.tsv`) uses `write_atomic`, `profile.rs` (the game icon) `util::copy_atomic` (the same per-writer `<path>.<pid>.<n>.tmp`).

**I/O on the hot paths** (r4 §6.3, output-neutral; a release program over synthetic inputs, outside the repository (`target/agent-work/pl/bench`), on an M1 Pro whose load average was 23 to 180 during the four runs: the times are indicative, the syscall counts are not). The syscall counts are the kernel's
(`task_info(TASK_EVENTS_INFO).syscalls_unix`), the times medians:

| item | before | after |
|---|---|---|
| shader cache after a build (40,000 files): the log line's walk + the walk before removing it (`shaders.rs`) | 2 walks, 80,960 syscalls, ~270-455 ms | 1 walk for both numbers (`cache_sizes`), 40,482 syscalls, ~100-160 ms |
| `folder_bytes` (also the walk before the build, kept: it decides the cache limit) | `stat` of the full path per file, ~130-220 ms | `DirEntry::metadata` (an `fstatat` on Apple), the same count, ~104-174 ms; a link is still followed |
| `FileLog::line`, 50,000 lines | 100,000 `write`s (`writeln!` on an unbuffered `File` is two), ~200-320 ms | 50,000 (one buffer per line, kept with the file; every line still reaches the kernel before `line` returns: a SIGKILL loses nothing), ~100-165 ms; byte-identical files |
| "has the shader map a row?" (`map_has_rows`, 43,000 rows), stage 3 and the boot | whole file read and parsed, ~1.4 ms | reads to the first row, ~16 µs (the log lines that print the count still count) |
| last `[mem:` line of a 64 MB `pipeline.stages.log` (`state::last_matching`, also `last_line`, the chain log's last run line) | whole file, ~31 ms | `util::rfind_line` (64 KiB blocks from the end), ~22-43 µs; no match: one read of the file either way (~44 vs ~49 ms) |
| last boot of a game in a 32 MB guest log (`tsv::last_guest_run`) | whole file, ~87-90 ms | from the end, then only from that line on, ~1.3-2.6 ms |
| memory gate, per check of a waiting worker (`gate.rs`) | `mem()`: 2 `task_info`, `getrusage`, `statvfs` (+ `os_proc_available_memory` on iOS) | `MemSource::available()`: `os_proc_available_memory` only (the peak needs no sample: `resident_size_max` and `ru_maxrss` are kernel maxima) |
| CLI meter (`meter.rs`) | a sample every 5 ms: 28,200 samples over a 141 s build (1 `getrusage` + 2 `task_info` each) | every 100 ms (`SAMPLE_EVERY`): 1,410; `finish` unparks the sampler; a step that raised the process high-water mark has it as its resident peak (exact whatever the sampling) |
| `gameapp.stage` during a translation (`stage::StageThrottle`) | 282 atomic rewrites in 141 s (one per progress line, 4 syscalls each) | at most one a second: 142; a held line is written by `flush` after the build, and the last count (`done == total`) at once |
| a log line handed to a C callback (`ibuilder-ffi`, `CallbackLog`, the progress hook) | 2 allocations (`replace`, `CString::new`) | none once the thread's buffer has grown (`with_c_line`; a nested call gets its own `CString`); the app's shim passes no log callback, so the app's log path is `FileLog` only |
| every chain log line (`Ctx::log` → `diag::log_line`) | took the process-wide `DIAG` lock to see whether a diagnostics file was open, then looked for its words | looks for the words first; only a line that has one (`[RED]`, `FAILED`, ...) takes the lock (`event`); the same lines are written |

Not measured on a phone; the walks' cost on the iPhone's APFS and the Documents folder the app also walks is not known. Tests (296 in these three crates after code 5, 277 before): the new ones cover the
behaviour changes (boot refused, rc 4 in the chain, boot-only and the C ABI, the stamp, `.old` recovery, the fixed temporary names, the throttle, the readers against whole reads, the cache
sizes against two walks, `folder_bytes` against the full-path `stat`, the gate's question, the C line buffer, the meter's peaks, the diagnostics lock); each one fails when the change is undone in a scratch
copy (15 of 15, and 4 of 4 for code 5: 0 again, 4 winning over 5, no log line, the C ABI's 5).

