# The iBuilder app with the Rust chain: no CPython on the phone (package APP, 2026-11-19)

What was asked: *an installable device .ipa (the SideStore flow, like the ones iOSBuilder delivers) in which the whole on-phone build chain (extract -> profile -> rewrite -> shaders -> assemble -> sign -> boot the game)
runs on our Rust libraries instead of embedded CPython.* This document says what was built, how to rebuild it, what was run and what was seen, and what was **not** verified (nothing here ran on a physical iPhone).

* **Deliverable**: `dist/GameApp-rust-<buildid>.ipa` (+ `.sha256`, `dist/MANIFEST.txt`) made by **one command**: `scripts/build-app.sh device`. Bundle id `dev.ibuilder.app` (renamed 2026-11-22 from dev.nxpocket.gameapp: a new app on the device), `CFBundleVersion 1` (as every earlier delivery), unsigned / ad hoc (SideStore re-signs).
* **Zero C# changes** in this first package. The C# of the iOSBuilder worktree (22 files, snapshot in `app/gameapp/`) is built as it is; `app/patches/` was empty then (it carries C# patches since: section 9). The only native piece that changed is `app/gameapp_shim_rust.c`, a drop-in for `gameapp/gameapp_shim.c`
  that exports the same 19 `gameapp_*` symbols the C# imports with `[DllImport("__Internal")]` and delegates the chain to `libibuilder_ffi.a`.
* Also done here, in the crate this package owns: `crates/ibuilder-ffi/src/launcher.rs` (`LauncherBase`: the game's settings rows, the engine's `settings.py` in Rust, identical to it on Grizzy) and the `BigBlocks` global allocator of package MEM installed in `ibuilder-ffi`.
* Files: `scripts/build-app.sh`, `scripts/build-ios-ffi.sh` (now `TARGETS=`-aware), `crates/ibuilder-ffi/{src/launcher.rs, tests/launcher_oracle.rs}`, `app/{gameapp_shim_rust.c, verify_ipa.sh, compare_ipa.sh, refresh-snapshot.sh, SNAPSHOT.tsv, inputs.tsv, gameapp/, c/, sim/}`.

## 1. Inventory: everything the app and the shim used from Python, and what replaces it

Sources read: the iOSBuilder worktree `$IOSBUILDER_WORKTREE` (`gameapp/GameApp*.cs`, `gameapp_shim.c` 1,662 lines, `gameapp_pipeline.py` 2,910 lines, `shader_device.py`, `mods.py`, `profile_derive.py`, `nup_zstd.py`, `lzma_py.py`, `build_gameapp.sh`)
and the newest Python device .ipa it produced (`out-game-eva/gameapp-out-device/GameApp.ipa`, unzipped: 824 `.py` files). **The C# never starts Python and never imports a `.py`**: it talks to the chain through the 19 P/Invoke symbols below and through
files in Documents (`gameapp.config`, `gameapp.stage`, the logs, the markers): everything that was Python sat behind `gameapp_run_chain`.

### 1.1 The 19 native imports of the C# (`[DllImport("__Internal")]`; all still exported, checked by `scripts/build-app.sh` against the C# source)

| symbol (C# file:line of the snapshot) | was | now |
|---|---|---|
| `gameapp_run_chain()` (GameAppMain.cs:42) | `Py_BytesMain` on `gameapp_pipeline.py` | shim: `ibuilder_create` + `ibuilder_run_chain(<Documents>/gameapp.config)` + `ibuilder_chain_status` + `ibuilder_destroy`, flag `IBUILDER_FLAG_BOOT_IN_PROCESS` (= `PIPELINE_BOOT=1`), callbacks sign / xlate / spirv_cross / live_notify. Same result codes (0, 1, 2, 3, 75, 130) |
| `gameapp_memline(byte[], int)` (:43) | `mem_now` + `statfs` in C | `ibuilder_mem_line(Documents)` (same text) |
| `gameapp_available()` (:44) | `os_proc_available_memory` | `ibuilder_available_bytes()` (-1: no ceiling; on the simulator the symbol exists and answers 0, which now reads as -1: the "unknown" row, and no aggressive GC per translation) |
| `gameapp_extended_va()`, `gameapp_increased_memory()` (:45-46) | parse of the app's own CodeDirectory | `ibuilder_address_space_info` (`extended`, `increased_memory`) |
| `gameapp_get_task_allow()` (:47) | same parse; -1 on the simulator slice | `ibuilder_address_space_info.get_task_allow`; -1 on the simulator slice kept in the shim |
| `gameapp_address_space(out long, out long)` (:48) | `vm_region_recurse_64` census | `ibuilder_address_space_info` (`free_total`, `largest`) |
| `gameapp_identity_status/_store_bytes/_where` (:51-53) | Keychain item `dev.nxpocket.certificate` / protected file | **kept in C** (Security.framework), copied unchanged into the new shim |
| `gameapp_set_game_layer(IntPtr)` (:57) | `CFRetain` + remembered for the boot | shim: `CFRetain` + `ibuilder_set_game_layer` |
| `gameapp_quit_game()` (:58) | log + consume `gameapp.boot-only` + `SIGTERM` | `ibuilder_quit_game(Documents)` |
| `gameapp_nsz(src, dst, err, len)` (:60) | `nsz_decompress.c` (CommonCrypto + the app's zstd) | **kept in C**; linked against a *standard* zstd decoder object (`app/c/stdzstd.c` + zstd sources through `ld -r`) instead of the ZBIC-variant zstd of `libpyext_static.a` |
| `gameapp_pad_create`, `gameapp_pad_layout` (:63-64), `gameapp_pad_layout_tv`, `gameapp_tv_attached`, `gameapp_game_menu` (GameAppExternal.cs:236-238) | `dlsym(host, "dm_front_pad_*")` | shim: `ibuilder_host_symbol("dm_front_pad_create")` etc. (`why` = 1 "not loaded yet" is `dm_host_probe_boot` not found) |
| `gameapp_set_xlate(fn)` (:78) | stored the managed translator's entry | shim: stores it; a 6-line trampoline is the `ibuilder_tool_fn` handed to the chain: it drops `argv[0]` (the managed `XlateEntry` takes `<job.json> <outdir>`; the Rust library passes the full command line, program name first) |

Not imported by the C#, still exported for a later patch: `gameapp_cancel_chain()` (`ibuilder_cancel` on the running handle), and from the zstd object `gameapp_stdzstd_new/feed/free` (the streaming decoder `nup_zstd.py` used).

### 1.2 The 16 functions of the `_dmgameapp` builtin module (what the Python called into C)

| `_dmgameapp.` | used by | replaced by |
|---|---|---|
| `boot(fw, dir, modules, profile, romfs, seconds)` | `boot_only`, `_chain` | `ibuilder-pipeline::sys::boot::DlopenBoot` (dlopen, layer, pad, time limit, redirect of fd 1/2, `dm_host_probe_boot`) |
| `sign(framework, exe)` | `do_assemble` | `ibuilder_sign_fn` = `sign_cb` in the shim: zsign with the Keychain `.p12` + `embedded.mobileprovision` (device) or zsign ad hoc (simulator), unchanged code |
| `xlate(argv)`, `spirv_cross(argv)` | `shader_device` (stage 3, live shaders) | `ibuilder_options.xlate` (trampoline to the managed translator) and `.spirv_cross` (`app/c/spvc_cli.c` over the SPIRV-Cross C API, unchanged) |
| `live_next`, `live_done`, `live_stop` | `LiveShaders` thread | `ibuilder-pipeline::live::LiveQueue` + `sys::boot::live_handler` + `ibuilder_live_notify_fn` (the `GameAppLiveShader` notification stays in the shim) |
| `mem()`, `address_space()`, `trim()`, `self_identifier()` | memory gates, `check_address_space`, `release_memory` | `ibuilder-pipeline::sys::{mem, vm, entitlements}`; `ibuilder_self_identifier` |
| `release_mimalloc()` | `release_memory`, `restart_for_shaders` | gone: Rust's allocator has nothing to hand back (an explicit no-op seam, `docs/chain/PIPELINE-RUST.md` section 8) |
| `set_chain_result(rc)` | `__main__` (a `SystemExit` would have ended the app) | gone with the interpreter |
| `zstd_decompress(bytes)` | the shader split's `zstd=` argument | `ibuilder-shadersplit` `Ruzstd` (pure Rust, standard zstd; the ZBIC variant is the SW2 package's, in progress) |
| `zstd_stream_new`, `zstd_stream_feed` | `nup_zstd.py` (stage 1 for a `.nup`) | **`ibuilder-nup` (in progress, SW2)** -- see 1.4 |

### 1.3 The Python files and native modules the bundle carried

Measured on the newest Python device .ipa (824 `.py`, 1,151 files and 28.76 MB uncompressed (9.32 MB stored) of "Python pieces", `app/compare_ipa.sh`):

| carried by the Python bundle | size | replaced by |
|---|---|---|
| `gameapp_pipeline.py` + `Frameworks/Python.framework` + `py/` (stdlib) + 144 stdlib module frameworks | 11.76 + 5.05 + 6.07 MB | `ibuilder-pipeline` + `ibuilder-ffi` (+ the adapters of `ibuilder-cli`) |
| `profile_derive.py`, `dm/games/game.py`, `profile_guess.py`, `dm/tools/{nso_syms,nacp,...}.py` | | `ibuilder-profile` |
| `pyrewrite_driver.py`, `dm/tools/{patch_nso,nso,nso_asm,nso_code,jump_solver,...}.py` (52 files), `dm/games/*/patches.py`, `pyext/{capstone,lz4,zbic}` (+ the `_capstone`, `lz4`, `zbic` inittab modules) | | `ibuilder-nso`, `-a64`, `-rewrite` (capstone as a Rust binding) |
| `assemble.py` (+ `macho_write.py`) | | `ibuilder-macho` |
| `shader_post.py`, `shader_device.py`, `shaderengine/` and `engine/` (`dmsh_build.py`, `shader_split.py`, `shader_lib.py`, ... 280 files, 4.31 MB), `lzma_py.py` | | `ibuilder-shadersplit` (incl. LZMA through `lzma-rs`), `ibuilder-shaderbuild`, `ibuilder-shader` |
| `mods.py` | | `ibuilder-mods` |
| `extract_nup` (in `gameapp_pipeline.py`), `scripts/pack_nup.py`, `nup_zstd.py` | | **`ibuilder-nup` (in progress, SW2); adapter `NupReader` of `ibuilder-cli`, feature `nup`** |
| `dm/games/settings.py` (+ `settings.tsv`) | | **`LauncherBase` of `ibuilder-ffi` (`src/launcher.rs`), written in this package** (no crate had it): same JSON as the real `settings.py` on Grizzy (5.5). `settings.tsv` still ships (it is the input) |
| `dm/games/*/game.tsv` (187 tables), `dm/host/*.c`, `dm/gfx/*.c`, `dm/gfx/nvn/*.c`, `dm/gfx/vk/*.m` | | **still shipped, read by Rust**: `ibuilder-profile` reads `game.tsv` of a registered game and the host sources for the HLE hook set |
| `host-prebuilt.dylib` | | still shipped (third-party engine host) |
| stdlib `.fwork` loaders, `Python.framework`, `libpyext_static.a` (capstone, lz4, zbic, zstd ZBIC) | | gone |

Libraries that stay on purpose: zsign + OpenSSL.framework + zlib/minizip (signing), SPIRV-Cross (static, C API), a standard zstd decoder (NSZ; a future `.nup` reader may use `gameapp_stdzstd_*`), the managed translator (`dmsh_xlate` over Ryujinx, C#), LibHac (extraction, C#), SSH.NET (NAS).

### 1.4 The `.nup` path (stage 1 for a Switch 2 game)

`extract_nup` + `scripts/pack_nup.py` + `nup_zstd.py` are **replaced by `ibuilder-nup` (in progress, package SW2)**; it is not in the build this document describes. The design that keeps the shim ready for it:

* The C# is unchanged: its Import UI already accepts `.nup` (`GameAppMain.Dumps()` lists `.nsp .xci .nsz .nup`) and `ConfigText` writes `nup=<path>` into `gameapp.config` for such a game. The chain's stage 1 (`ibuilder-pipeline::chain`, `NupStage`) calls the `NupReader` adapter when that key is present. **No shim code is needed**: the reader reaches the chain through the same adapter set as the other stages (`ibuilder-cli::adapters::real_adapters`, which `ibuilder-ffi --features real-stages` links).
* `scripts/build-app.sh` enables it by itself: if `crates/ibuilder-cli/Cargo.toml` defines a feature `nup` that is not a default one, the Rust build gets `--features real-stages,ibuilder-cli/nup` (no Cargo.toml of this package needs to change when it lands); if it is a default feature it comes with `real-stages`. The script, `verify_ipa.sh` and `MANIFEST.txt` say which case they built ("Switch 2 package reader (.nup): ..."; `verify_ipa.sh` reports `ibuilder-nup` linked or not, as information, never as a failure).
* Progress: the stage file and log lines the reader writes are tailed by the C# as before. Free-space check, SHA-256 per file and the streaming zstd of tens of GB are the reader's. **Cancel**: `gameapp_cancel_chain()` (shim) -> `ibuilder_cancel`; the reader must poll `Ctx::cancel` for it to take effect inside stage 1 (the chain itself only stops between stages). Memory: the reader must stay out of the way of the 3.3 GiB jetsam ceiling (stream, never map whole files).
* **State of this build**: see `dist/MANIFEST.txt` ("NUP reader"). Without the reader a `.nup` game stops at stage 1 with `no NUP reader in this build`; NSP, NSZ and XCI games are unaffected.

## 2. Design

```
 GameApp (C#, unchanged)  --P/Invoke "__Internal"-->  libgameappshim-rust-<kind>.a
   GameAppMain.cs ...                                   gameapp_shim_rust.c      keychain identity, nsz, host dlsym pad entries, translator trampoline, live notification
   gameapp.config, .stage, logs (files)  <---------->   zsign + OpenSSL           sign_cb (device: .p12 + profile, simulator: ad hoc)
                                                        spvc_cli.c + SPIRV-Cross  ibuilder_tool_fn
                                                        nsz + standard zstd
                                                             |  ibuilder.h (29 functions, ABI 1)
                                                        libibuilder_ffi.a  (Rust, --features real-stages)
                                                          ibuilder-ffi -> ibuilder-pipeline (chain, markers, state, memory, boot)
                                                          ibuilder-cli::adapters -> profile, rewrite(+nso,a64), macho, sign, mods, shadersplit, shaderbuild, shader
```

* `gameapp_run_chain()` takes the bundle folder from the executable's path and Documents from `$HOME`, as before. The options struct carries the sign callback (`sign_cb`: embedded.mobileprovision present -> `device(zsign, in-memory .p12)`, else
  `adhoc(zsign, no certificate)`; same return codes 10 / 11 / 12 / 13 / 20, same mode strings), the two tool callbacks, the live-shader notification and `IBUILDER_FLAG_BOOT_IN_PROCESS`. The Rust library writes the logs and markers (`pipeline.stages.log`,
  `gameapp.chain.log`, `gameapp.stage`, `gameapp.framework`, `build_times.tsv`, ...) in the formats the C# reads.
* The launch-time address-space reservation (the `__attribute__((constructor))` of the old shim) is now the Mach-O initializer inside the Rust archive (`ibuilder-ffi`, `early::ctor`). The iOS 26 link turns `__mod_init_func` into `__TEXT,__init_offsets`; the executable has
  5 initializers and the first one is the Rust one (it reads `IBUILDER_EARLY_INIT_FORCE`, 25 characters: found by disassembly of the stripped device executable); in the simulator build its lines appear in `gameapp.chain.log` (`as-census at launch ...`, `as-reserve: deferred ...`).
* **DM_PROFILE before the host exists** (found by the player test below): the shim sets `DM_PROFILE` (the `profile=` path of the config) before `ibuilder_run_chain`, because the Rust boot sets it only after the dlopen and the address-space reservation, and the game view's touch pad can ask the host
  for its profile in between. See section 5.3.
* `ibuilder-ffi` installs the `BigBlocks` global allocator (package MEM: blocks of 128 KiB and more are mmap/munmap'd) and the `LauncherBase` adapter (settings rows) on top of `ibuilder-cli`'s adapter set; both are in the final archive.
* The Rust archive is linked with `ForceLoad` as a static `NativeReference`; the SDK's `-u` list (the `gameapp_*` P/Invoke names) pulls the shim, the shim pulls what it calls. No Python.framework, no `libpyext_static.a`, no `.py` and no `pyext/` in the project or the bundle (`build-app.sh` fails if any is found).
* The bundle carries only the data the Rust chain reads: `dm/games/*/game.tsv` + `settings.tsv`, `dm/host/*.c` + `dm/gfx/**` (hook set), `host-prebuilt.dylib`.

## 3. How to rebuild

The build reads a few READ-ONLY inputs of the original iOSBuilder project (prebuilt engine host, SPIRV-Cross archives, zsign / OpenSSL / zstd sources, the patched engine). No path is built into the
scripts: put `IOSBUILDER` (a checkout of that project) and, when its newest work lives in a separate worktree, `IOSBUILDER_WORKTREE` in the environment or in the gitignored file `scripts/local.env`
(copy `scripts/local.env.example`; `app/refresh-snapshot.sh` needs `IOSBUILDER_WORKTREE`). The scripts source that file first and stop with a message naming the variable when one is missing.
`IBUILDER_TEAM_SUFFIX` (optional) is the team suffix your sideloading service gives the bundle id: it adds that exact background-task id to the Info.plist (patch 0056).

```
scripts/build-app.sh device                  # dist/GameApp-rust-<buildid>.ipa + .sha256 + MANIFEST.txt  (~2 min with the Rust build cached, ~5 min cold; measured numbers in 4)
scripts/build-app.sh simulator               # <APP_WORK>/simulator/app/GameApp.app, ad hoc signed, bundle id dev.ibuilder.app.rusttest unless SIM_APP_ID is set (NOT installed by the script)
APP_WORK=<dir> CARGO_TARGET_DIR=<dir> APP_OUT=<dir> SKIP_RUST=1 APP_BUILD_ID=<id> PIN_STRICT=1   # optional
app/refresh-snapshot.sh [--check]            # take the C# / C sources again from the iOSBuilder worktree (the build never reads them from there)
app/verify_ipa.sh <ipa>   app/compare_ipa.sh <old> <new>   app/sim/{place-grizzy,verify-chain,verify-export}.sh
```

Prerequisites: rustup with `aarch64-apple-ios[-sim]` (Homebrew keg, put on PATH by the script only), Xcode with the iPhoneOS SDK, .NET SDK 11 with the ios workload, `LibHac` and `SSH.NET` in `~/.nuget/packages`.
The script reads (never writes) these inputs; each is pinned in `app/inputs.tsv` (size + sha256 when this build was made) and a mismatch is a warning in the log and in `MANIFEST.txt` (`PIN_STRICT=1`: fatal):

| input (read only) | what | why trustworthy |
|---|---|---|
| `<worktree>/out-load-ui/host-prebuilt-loadable-ios.dylib` (6,747,984 B, 2026-11-19 02:39) and `-sim.dylib` (4,520,400 B, 02:38) | the engine host the assembler grafts the game onto (third-party code, not part of the rewrite) | built by the worktree's `load/prebuild_loadable.sh` from the patched engine (`prebuild-loadable-ios.log` in the same folder); the script checks it carries every symbol the app dlsyms and the shader chain `b24b25240cad` of the patched engine's header. The older host in the main checkout's `out-load/` (2026-11-17, 4.7 MB) has none of `dm_front_pad_*`, `nvn_live_set`, ... and is not used |
| `<worktree>/out-stage/out-engine/engine` (patched engine, stamp `out-engine/stamp.txt`) | extractor / translator C# sources, registered games' tables, host sources | the stamp holds the source commit and the sha256 of each `paches/engine/*.patch`; the seven current patches' sha256 equal the stamp (compared by hand: `0103 6b0fee9e... 0301 b6b1530b...`) |
| `<main checkout>/out-sign/signing/zsign/src` (commit 9299685, patched by iPorterNX: in-memory asset, dylib entitlements) and `.../OpenSSL.xcframework` | signing | the same inputs of every earlier delivery; `inapp_shim.cpp` compiles against the patched API or the build stops |
| `<worktree>/out-spvc/libspvc-{ios,sim}.a` + `include/` (2026-11-17) | SPIRV-Cross f11ba9f as static libraries | built by `tools/build_spvc_ios.sh` (pinned commit); the C wrapper links them or the build stops. Missing -> rebuilt under `$APP_WORK/shared/spvc` by that script pointed there |
| `<main checkout>/out/src/zstd/lib` | zstd sources | standard zstd, the decoder object is built from them |

`tools/patched_engine.sh` (the worktree's engine cloner) would write `out-engine/` **inside the worktree** when called without an argument: the script never calls it unless the patched engine is missing, and then with an out folder under `$APP_WORK` and `GIT_OPTIONAL_LOCKS=0`.
The worktree's `build_gameapp.sh` was not run (it writes `out-game/`, installs into a simulator and uninstalls the app there); `build-app.sh` is its replacement for the Rust shape and shares none of its output folders.
A host dylib that is missing is not rebuilt by this script (`load/prebuild_loadable.sh` needs the video libraries and ~a dozen engine objects): it stops and says so.

## 4. Device .ipa: what was built and checked

(numbers of the build made while writing this document; `dist/MANIFEST.txt` is the record of the final one)

**The file**: `dist/GameApp-rust-20261119-121022-rust92ef2af6.ipa`, **26,618,897 bytes**, sha256 `29bf55ebb56961a5d22eafce7beaeefad08dd72546cabe350a074d175a971ed0` (`dist/*.ipa.sha256`; `dist/MANIFEST.txt` has the crate-source hashes, the pinned inputs, the toolchain, the size table).
Built by `scripts/build-app.sh device` in 173 s (the Rust archive was incremental, `dotnet publish` 81 s; a cold cargo build of the device archive is a few minutes more). The orchestrator re-runs the same command at the end: the file name carries the date and the hash of the Rust sources (`rust92ef2af6`).
Features: `ibuilder-ffi --features real-stages` (profile, rewrite, macho, sign, mods, shadersplit, shaderbuild, shader finish; the `LauncherBase` of `ibuilder-ffi`; the `BigBlocks` global allocator of `ibuilder-alloc`). **The Switch 2 reader `ibuilder-nup` is not in it** (the crate exists, `ibuilder-cli` has no `nup` feature yet; 1.4).

**`app/verify_ipa.sh` on the archive read back: 182 PASS, 0 FAIL** (+ one INFO line: the NUP reader is not linked). What it checks, by group:
* *layout*: `Payload/GameApp.app` only, no `Symbols/`, no `__MACOSX` / `.DS_Store`, 328 entries; the packer is deterministic (sorted names, fixed times, `zip -X -D`).
* *identity*: `CFBundleIdentifier dev.ibuilder.app`, `CFBundleVersion 1`, executable `GameApp`, arm64, `LC_BUILD_VERSION platform 2 minos 26.0 sdk 27.2`; display name iBuilder + icon; universal, scenes (incl. the external display), `UIFileSharingEnabled`, `LSSupportsOpeningDocumentsInPlace`, photo-library usage string, ProMotion key.
* *signature*: CodeDirectory `flags=0x2(adhoc)`, no authority, no team id, no `embedded.mobileprovision`; entitlements in the executable: **`com.apple.developer.kernel.increased-memory-limit = true`, no `extended-virtual-addressing`** (a free Apple ID cannot carry it).
* **no Python**: no `.py` / `.pyc` / `.fwork` / `py/` / `pyext/` / `lib-dynload/` / `shaderengine/` / `Python.framework` among the 328 entries; `otool -L` of the executable has no Python; no `Py_*` symbol and none of `PYTHONHOME`, `gameapp_pipeline.py`, `_dmgameapp`, the inittab line in its strings.
* **the Rust chain is linked**: its own texts are in the executable (`[ibuilder] chain: start`, `ibuilder_run_chain: a run is already in progress`, `[ibuilder] PANIC: `, ...), the crates by name (`ibuilder-pipeline`, `-rewrite`, `-profile`, `-macho`, `-shadersplit`, `-shaderbuild`, `-shader`), 17 of the chain's own log lines the app or the next launch reads (`rewrite taken from the cache`, `ended by iOS`, `build times: `, `DM_NVN_SHMAP`, `DM_MSL_DIR`, `DM_ROMFS_OVERLAY`, `launcher settings:`, ...), the launcher code (`settings.py` in Rust).
* **the shim**: all 19 `gameapp_*` imports are global `T` symbols (+ `gameapp_set_xlate`'s trampoline text, the NSZ converter `NCZSECTN`, SPIRV-Cross's usage text, zsign's `Can't find TeamId`, the Keychain service name, both signer mode strings).
* *initializer*: the executable has 5 entries in `__TEXT,__init_offsets`; the first is the Rust early initializer (`IBUILDER_EARLY_INIT_FORCE` read there; found by disassembly of the stripped executable).
* *the host* (`host-prebuilt.dylib`, platform 2): the 16 entries the app dlsyms, `DM_AS_AT`, the Metal-source path, the touch pad / editor / players / capture / cheats / pipeline archive / MetalFX / frame generation / live shaders strings, movie decoders; **the shader chain `b24b25240cad` is the host's, the patched engine's header's and the app's (`GameAppBuild.ShaderChain` in `GameApp.dll`)**.
* *the managed half*: 44 UTF-16 / metadata markers of `GameApp.dll` (the screens' texts, the P/Invoke names): the C# is the snapshot's, so they hold.

**Sizes (the structure of the Python .ipa against this one)** `app/compare_ipa.sh`:

| | Python, newest (`out-game-eva/.../GameApp.ipa`, 2026-11-19 10:32, the same C# generation, the same host) | **Rust (this build)** | difference |
|---|---|---|---|
| .ipa | 34,103,288 B (32.52 MiB) | **26,618,897 B (25.39 MiB)** | **-7,484,391 B (-21.9 %)** |
| uncompressed | 89.17 MiB, 1,479 files | **65.21 MiB, 328 files** | -23.95 MiB (-26.9 %), -1,151 files |
| `GameApp` (executable) | 3.71 MiB | **8.81 MiB** | +5.10 MiB (the whole Rust chain, zsign, SPIRV-Cross, the standard zstd) |
| Python pieces | **28.76 MiB uncompressed (9.32 MiB stored), 1,151 files**: `py/` 11.76, `Python.framework` 5.05, 144 stdlib module frameworks 6.07, `pyext/` 0.08, `dm/tools` + `dm/gfx/tools` + `dm/ios` scripts 1.18, `shaderengine/` + `engine/` 4.31, top-level `.py` 0.27, `scripts/pack_nup.py` 0.03 | none | -28.76 MiB |
| `Frameworks/` | 48.83 MiB, 164 files | 37.70 MiB, 18 files | the Python framework and its 144 module frameworks are gone; `GameApp.r2r`, `OpenSSL`, `libcoreclr`, `libSystem.*` stay |
| `dm/` | 4.01 MiB, 334 files | 2.83 MiB, 249 files | the engine's `.py` are gone; `game.tsv` (187), `settings.tsv`, the host sources stay |
| `_CodeSignature` | 0.37 MiB | 0.08 MiB | |

Against `GameApp-28-with-ram-entitlement.ipa` (24,350,365 B, 2026-11-18 13:39, the delivery the task named; **the file was deleted from the worktree during this session**, only the folder I had unzipped is left, so only uncompressed sizes are comparable): 1,274 files / 67.59 MiB -> 328 files / 65.21 MiB, gone: `py/` 17.69 MiB, `Python.framework` 4.71, `shaderengine/` 2.89, `engine/` 1.02, `pyext/` 0.08, the `.py` files; new: `BouncyCastle.Cryptography.dll` 3.11 + `Renci.SshNet.dll` 0.17 (the NAS, added to the app after delivery 28), the app icons, and `GameApp.r2r` growing 12.47 -> 26.96 MiB with that newer C#.
So the Rust .ipa (26.6 MB) is larger than delivery 28 (24.4 MB) only because the app grew in between; like for like it is 7.5 MB smaller (`compare_ipa.sh` shows both).

The device bundle's `Frameworks/`: `GameApp.r2r.framework`, `OpenSSL.framework`, `libcoreclr.framework`, `libSystem.{Globalization,IO.Compression,Net.Security,Native,Security.Cryptography.Native.Apple}.framework` (8; the Python build had 9 + 144 module frameworks).

## 5. The simulator run

Everything here ran on this Mac (M1 Pro, shared with other agents: load average 90-370 during these runs, so every duration is **indicative**), in the **iPhone 17 simulator** `<SIMULATOR-UDID>` (iOS 27.2), driven with the Argent MCP tools
(`reinstall-app`, `launch-app`, `restart-app`, `screenshot`, `describe`, `gesture-*`, `await-ui-element`); `xcrun simctl` only for what Argent cannot do (the data-container path, a data-preserving `install`, `terminate`). That simulator held the user's own iBuilder
(`dev.nxpocket.gameapp`, running) and, at 12:41, an exported game (`dev.nxpocket.gameapp.g0100dfa027504000`) and `dev.nxpocketapple.app`: **this package never installed, uninstalled or launched any of them** (its uninstalls named only its own two test ids). When it finished, the simulator no longer listed the last two (`simctl listapps`):
someone else removed them between 12:41 and 13:21 (another package's cleanup, presumably); the user's iBuilder is still there. The test app is the simulator build of this package with its own bundle id `dev.nxpocket.gameapp.rusttest` (`scripts/build-app.sh simulator`; `SIM_APP_ID=` overrides), so it has its own data container.
No key, certificate or provisioning profile was read, written or named; the extraction stage (stage 1, needs `prod.keys`) was **not run**: `app/sim/place-grizzy.sh` leaves Documents the way the app's stage 1 leaves it (read from `GameAppMain.ConfigText` and
`gameapp/run_gameapp.sh --chain-only`): `extract/ExeFS/{main,sdk,subsdk0,main.npdm,rtld}`, `extract/RomFS.img`, `extract/control.nacp`, `extract/icon.jpg` (APFS clones of the read-only Grizzy inputs) and the one-shot request `gameapp.args` = `chain-only`.
`out/ref-img-grizzy/RomFS.img` and the oracle folders `out-py/` were **deleted from the iOSBuilder checkout while this package ran** (2026-11-19 ~12:09); the 2,498,033,693-byte image (same size and mtime as the original) came from a clone another package of this round had made.

### 5.1 What was observed, in order

1. **Install and launch** (`reinstall-app`, `launch-app`): the library screen comes up ("No games yet", the prod.keys and signing-certificate banners, the storage bar). `gameapp.ui.log`: `memory at launch: resident 457 MiB ... | free 86.28 GiB` (that line is `gameapp_memline` -> `ibuilder_mem_line`),
   `memory ceiling at launch: available unknown (os_proc_available_memory unavailable here)` (`gameapp_available` -> -1). The Mach-O initializer ran: `gameapp.chain.log` starts with `[gameapp] as-census at launch, before the .NET runtime: ...` and `as-reserve: deferred ...` (Rust `early_init`) once a framework stamp is on disk.
2. **The chain, started by the app itself** (`gameapp.args` = `chain-only` -> `AutoRunFromArgs` -> `BeginRun` -> `gameapp_run_chain` -> `ibuilder_run_chain`): the library screen shows the build card driven by `gameapp.stage`
   (`49% · Translate the shaders · 315 / 21,990 shaders · 15s`, then `Watch the build`). `pipeline.stages.log` of that run (the lines that matter):
   ```
   stage 1/5: deriving the profile from the game in Documents          stage 1/5 profile: derived in 0.5s
   stage 2/5: rewriting the modules (1/3: main) ... (2/3: subsdk0) ... (3/3: sdk)
   stage 3 shaders: parallel shader preparation on; the build finishes shaders on 6 threads (Rust threads: no interpreter lock)
   stage 3 shaders: 21990 shaders found in 2 s       (compute 767, geometry 145, pixel 10541, vertex 10537)
   stage 3 shaders: Documents/shaders/0100DFA027504000 built on this device in 141 s (21990 shaders, Metal source: the host compiles each library at its first use)
   stage 4a assemble ok: 192435104 bytes, 45 load commands (0.3s)
   stage 4b sign rc=0 mode=adhoc(zsign, no certificate) identifier=dev.nxpocket.gameapp.rusttest       stage 4b signed adhoc(zsign, no certificate) in 1.5s (185.9 MB) -> 194904144 bytes
   stage status: {"assemble": "ok", "profile": "ok", "rewrite": "ok", "shaders": "ok"}        process spawns: none (ibuilder-pipeline has no process API: no fork, exec, posix_spawn or system anywhere in the chain)
   build times: profile 0.5s | rewrite 8.0s | shaders 141.0s (reading 1.6s, translation 139.3s; 21990 shaders; 6 reading / 6 translating threads; peak 980 MiB) | assemble 1.8s -- total 151.2s, ok on iPhone18,3 (simulator)
   ```
   The translation ran through the **C# translator in the app process** (`XlateEntry`, via the shim's trampoline) and **SPIRV-Cross through the C wrapper** (`spvc_cli.c`): no child process exists on iOS. Memory (`[mem:...]` lines; the simulator's `footprint` and `available` are not the phone's):
   resident 740 MiB when the translation starts, process peak 981 MiB (reached in the rewrite of `main`), 343 MiB at the end. Total 151 s on a loaded Mac, against ~205-240 s measured for the Python app in the same simulator (`docs/STATE-GAMEAPP.md` of the iOSBuilder worktree) and 809 s of Python on a phone.
3. **The app recognises the result**: the library shows *Grizzy and the Lemmings - Shaders ready* with its icon and the **Play** button (`GameStatusOf` = Ready: the framework stamp is this build's). Tapping **Play** (`boot-only`, no stage re-run) boots the guest inside the app process (5.3).
4. **Export as .ipa** through the UI, Light and Full (section 7).
5. **The exported game's player** (installed in the same simulator under an id of its own) boots from its bundle (5.4).

### 5.2 Checks of the chain's outputs (`app/sim/verify-chain.sh <Documents> <reference run>`)

| check | result |
|---|---|
| (2) the 21 rewrite files (`main`, `sdk`, `subsdk0` x `patched.bin fixups sites gates cuts obs changed`) against a Mac CLI run of the same inputs (`scripts/e2e-grizzy.sh run`, the orchestrator's cold run `orch-e2e2/cold1`, deleted afterwards, whose 21 files the orchestrator compared with the Python oracle `out-py/pyrewrite/out`: 21 of 21, `docs/chain/E2E-RUST.md`) | **21 of 21 identical** (`cmp`). The Python oracle folder itself no longer exists, so the chain of evidence is: in-app = CLI (checked here) = Python (checked by the orchestrator earlier) |
| the profile tables of the pack (`shader_facts shader_map shader_pairs pso_pairs bf_want pso_manifest`) | all six `cmp`-identical to that CLI run |
| (3) the shader library `b-b24b25240cad.img` (56,885 files, 1,902,667,444 B) against that same CLI run (`libdiff`) | **27,230 identical, 29,653 different** (all of them `_b.metal`, `_bx.metal`, `_b.json`: shader text) -- **explained below, not a defect of the Rust chain** |
| (3b) the same library against a CLI run of the **same Rust sources** with the translator built from the **same C# sources the app compiles** (the patched engine's `dmsh_xlate`, as a child process; `libdiff` = `cargo build --release -p ibuilder-shaderbuild --features host-tools,f2-finish`) | **identical 56,883 + 2 identical but for the time-stamp line, different 0, only-in-one 0**. So the in-app path (callbacks into the managed translator and into SPIRV-Cross, 6 threads, in-process) writes exactly what a process-based run writes |
| (4) the framework | 194,904,144 B, LC_BUILD_VERSION platform 7 (simulator host), CodeDirectory `flags=0x2(adhoc)`, `Identifier=dev.nxpocket.gameapp.rusttest` (the app's own identifier, read from its signature by `ibuilder_self_identifier`), `Info.plist entries=3`, `Sealed Resources version=2`; `codesign --verify --strict --verbose=2 Documents/DmGameProbe.framework`: **valid on disk, satisfies its Designated Requirement** (signed by zsign in the app process, not by `ibuilder-sign`) |

**Why (3) differs, exactly.** The earlier CLI run (and the Python oracle before it) used the engine's **native `dmsh_xlate` binary of 2026-11-16** (`$IOS/engine/dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate`, `--source-sha c75f3cb403920cde`, engine commit `e9f2af6`).
The app compiles the translator from the **patched engine** (commit `75c7336` + 7 patches: `Program.cs` sha `4a79f0f1...` against `44917a2b...`; `Ryujinx.Graphics.Shader` differs in `CodeGen/Spirv/{CodeGenContext,Declarations,Instructions}.cs`, a new `RayQuery.cs`, `Decoders/DecoderAmpere.cs`, ...; `--source-sha e2d3359d2a182f50`).
The newer translator emits different SPIR-V (more ids: `_751` becomes `_775`; the NaN-safe `fmin`/`fmax` lowering) hence different MSL; that is the translator the host of this app was built with (`DMSH_CHAIN b24b25240cad` from the patched engine's header). Everything that does not pass through the translator is identical (names, tables, 27,230 files).
Experiment (3b) proves it: a Mac CLI run with `dmsh_xlate` built from the app's own translator sources (`app/sim/xlate-native.csproj`: replace `@XLATE_SRC@` by the `xlate/` folder `build-app.sh` generates under `<APP_WORK>/<kind>/proj/`, `dotnet build -c Release`, put the result as `dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate` of an `--engine` folder next to the pinned `spirv-cross`; its `--source-sha` is `e2d3359d2a182f50` where the old native binary says `c75f3cb403920cde`) plus the pinned `spirv-cross` binary gives the in-app library byte for byte.

### 5.3 The boot, Grizzy built in the app (what a simulator can show)

Play -> `gameapp.chain.log` (Rust boot): `as-reserve: ...`, `dlopen OK`, `the game layer 0x... was handed to the host`, `touch gamepad created over the game view`, `dm_host_probe_boot(patched=".../extract/flat", modules="main subsdk0 sdk", profile=".../profile.tsv")`.
`gameapp.guest.log`: `game profile .../profile.tsv: 37 rows`, `game profile: 0100DFA027504000 (Grizzy and the Lemmings - Crazy Party)`, `>>> nnosInitialize`, `>>> nninitInitializeSdkModule`, `>>> nninitStartup`, **`>>> nnMain`**, `fs RomFS image .../extract/RomFS.img: 237 entries`,
`fs mount rom`, `[nvn] WindowInitialize swap chain 3 textures (3 active), interval 1, first 1920x1080`, `[nvn] present tick 60 Hz`. The game view comes up in landscape ("-- fps", the menu gear). **No picture**: the simulator's guest uses the engine's null Metal device (`[nvn] null device: 743 names, 550 implemented`),
as for the Python app (`docs/STATE-GAMEAPP.md`); whether the engine draws into the layer is a device question. This is what the Python app showed in the same simulator (`>>> nnMain`, RomFS mounted, a 1920x1080 swap chain): same landmarks. The game ran until it was ended by hand (`simctl terminate` of the test app).

### 5.4 Found while testing: the exported player crashed on its first Play, and why (fixed in the shim)

The exported game's player (section 7) was installed in the simulator and Play was tapped. The guest crashed right after `>>> nnosInitialize` (`[dm] CRASH sig=11 addr=0 pc=main+0xaafa40`). The host log said `game profile: 0000000000000000 (Generic) from compiled game_profile.h` (`reserve 0x100000000 alias 0 stack 0x1000000`)
instead of `game profile <path>: 37 rows`: **the host had loaded its compiled generic profile**, because `hle_profile.c` reads `DM_PROFILE` lazily at the first `dm_profile()` call, and that call came before the variable was set. The Rust boot (`ibuilder-pipeline::sys::boot`) sets `PROBE_MODE`, `DM_PROFILE`, `DM_ROMFS` **after** the dlopen, the address-space
reservation (a 51.75 GB block and two census walks of ~2,600 regions in the simulator) and the layer hand-over; in that window the game view's touch pad (`gameapp_pad_create` -> `front_pad.m`, which reads the profile) is created by the main thread as soon as the host is dlopen'd (`guest.log` shows the pad lines *before* the probe lines;
in the run that worked they come after). The old shim had the same order (dlopen, reservation, `setenv`), so the race is older than the Rust port; what decides the winner is how long the reservation takes against the C# poll that creates the pad, so the outcome varies from run to run (here: the plain boot of the built game won the race, the exported player lost it). **Fix applied** (`app/gameapp_shim_rust.c`, `preset_profile_env`): `DM_PROFILE` is set from the config's
`profile=` path before `ibuilder_run_chain` (the profile file is final before any host is loaded: stage 5 is the only stage that loads one). After the fix the same exported player boots exactly like the built game: `game profile ...: 37 rows`, modules 3, `>>> nnMain`, the RomFS from the `.romfs` in Documents, the 3-texture swap chain
**The proper fix belongs in `ibuilder-pipeline/src/sys/boot.rs` (not this package's crate): set the three variables before `dlopen`.** On a device the reservation runs the same code (the census walks the process's regions there too; its duration is unmeasured), so the window may be narrower; it is not closed.

### 5.5 What the settings sheet showed

Without a `LauncherBase` implementation the chain logged `launcher settings: no settings generator (dm/games/settings.py port) in this build` and the game's sheet drew only its four legacy fields (Mode, Frame rate, Picture, Show FPS, plus Game memory). This package therefore implements
`LauncherBase` in `crates/ibuilder-ffi/src/launcher.rs` (the engine's `settings.py` in Rust, 22 rows for Grizzy) and wires it into the adapter set of the FFI; checked against the real `settings.py` run by python 3.14 on the same Grizzy inputs: `tests/launcher_oracle.rs`, **identical JSON text, key order included** (`identical: 22 rows`), plus 6 unit tests.
Second chain run, with the final sources (`LauncherBase` and the `BigBlocks` allocator in the archive; caches cleared first: shaders, shader cache, rewrite, framework, profile; ExeFS placed again): **the chain ran again inside the simulator app, 133.8 s** (profile 0.4 s, rewrite 9.3 s, shaders 122.3 s of which translation 121.0 s on 6 threads, assemble 1.8 s;
five stages ok, `process spawns: none`), the 21 rewrite files are **identical to a Mac CLI run of the same Rust sources** (21 of 21), the framework is again valid / sealed / ad hoc, and the log now says
`launcher settings: .../games/0100DFA027504000/launcher.json (24 rows: DM_FRAMEGEN DM_OPMODE ...)` (the 22 rows of the base document, plus GameApp's own rows after the pipeline's post-processing). **The game's settings sheet is now drawn from them**: Console mode (Docked / Handheld) with the engine's note, Game language, Frame generation, ... and its scroll bar says 9 pages
(3 before: only the four legacy fields). (A reference run of the library for this second run no longer exists: the first run's reference and library were deleted for disk space, so the library of run 2 was not re-compared; run 1's comparison is 5.2.)

**The allocator (`BigBlocks`, from package MEM) in the app**: installed in `ibuilder-ffi` (`#[global_allocator]`, feature `real-stages`, `crates/ibuilder-ffi/src/lib.rs`). A `#[global_allocator]` of a staticlib covers the Rust code of this archive only: capstone's C code, zsign, SPIRV-Cross and the .NET runtime keep their own `malloc`, and no pointer crosses the C boundary to be freed on the other side
(the tool callbacks' strings are `malloc`ed by the C# and `free`d by `libc::free`; everything the Rust gives the callbacks is borrowed). It linked (`ibuilder_alloc::map::{alloc,free}` and `__rust_alloc` are in the simulator executable), the chain ran (above), and the 12 tests of `ibuilder-ffi` pass with it.
**Footprint, as the app measured itself** (`[mem:...]` lines of `pipeline.stages.log`; the simulator's `phys_footprint` is its own accounting and reads far below resident, so read the shape, not the level):

| point | run 1, no allocator (resident / peak / footprint MiB) | run 2, `BigBlocks` |
|---|---|---|
| chain start | 470 / 470 / 64 | 475 / 475 / 67 |
| after rewrite of `main` | 679 / **981** / 85 | 673 / **979** / 88 |
| before shaders | 680 / 981 / 85 | 676 / 979 / 88 |
| during the translation (max footprint seen) | 283 | 307 |
| end of chain | 343 / 981 / 179 | 653 / 979 / 177 |

**No gain is visible in the simulator's numbers** (peak 981 -> 979 MiB; the peak is the rewrite's working set, which the allocator does not change: MEM's section 5), and the end-of-chain resident is *higher* in run 2 (653 against 343 MiB) -- resident here includes file pages the process touched (the 2.5 GB RomFS image, the 1.7 GB library written), which the host keeps or drops as it likes, so I do not read it as a regression, but I did not prove that either.
MEM's 852 -> 345 MiB is a Mac CLI **footprint** measurement and is not reproduced by the simulator's counters; whether the app's footprint on a phone follows it is a **device question** (jetsam counts footprint). What I can say: the app works with the allocator, and nothing here shows a cost.

## 6. What is NOT verified, limits and findings

**Nothing here ran on a physical iPhone.** The device .ipa was built and read back; its executable was never run. Everything that is device-only is unverified: jetsam and the 3.3 GiB ceiling, `os_proc_available_memory` and the increased-memory entitlement taking effect, the real `dlopen` of the device host
from Documents (and the sandbox's refusal on a distribution certificate), the launch-time 5 GB address-space block at a real ceiling, zsign with the Keychain `.p12` and `embedded.mobileprovision` (`sign_cb`'s device branch: only its ad hoc branch ran, in the simulator), Metal rendering, thermal behaviour, SideStore / FlareStore actually installing this .ipa, speed.
The measurements above are a loaded Mac's CPU in a simulator, not a phone's.

What is known not to be done or not to work:

| item | state |
|---|---|
| **`.nup` (Switch 2) games** | **`ibuilder-nup` is in progress (package SW2) and is not in the build this document describes**: a `.nup` stops at stage 1 (`no NUP reader in this build`). `build-app.sh` enables it by itself when `ibuilder-cli` gets its `nup` feature (1.4); a rebuild after it lands needs no change here. NSP / NSZ / XCI games are unaffected. `dist/MANIFEST.txt` says which case a given build is |
| ZBIC-compressed NSO modules | rewriter, profile and mods refuse them; SW2 is adding it |
| **stage 1 (extraction with `prod.keys`, LibHac, C#)** | **not run**: it needs the user's keys, which this work never touches. It is the C# of the worktree, unchanged, so it is as verified as it was; the hand-off (`extract/`, `gameapp.config`) is what `place-grizzy.sh` reproduces |
| `LauncherBase` (settings rows) | implemented here (`ibuilder-ffi`), identical to `settings.py` on Grizzy; the sheet drawn from it was checked in the simulator (5.5). Only one game (Grizzy) was compared; the `mac`-only branches are intentionally absent |
| second run in one process | unchanged: the C# keeps its `pythonRanThisProcess` rule and asks the player to reopen the app after a run (the Rust chain probably could run twice; nobody tried) |
| the C# decisions that `ibuilder.h` can also take (`ibuilder_decide_launch`, `classify_*`, `parse_stage`, `begin_run`) | present in the archive, **not switched**: the C# keeps its own (zero C# changes) |
| `ibuilder_cancel` | exported through `gameapp_cancel_chain()`, not called by the C# (the interpreter had no way to stop either) |
| the pipeline's `PROBE_MODE`/`DM_PROFILE`/`DM_ROMFS` order (5.4) | worked around in the shim; the cause is in `ibuilder-pipeline` |
| in-process translator speed | the in-app translation is the C# translator running 6 threads inside the app (`GameAppXlate` serialises the console swap under one lock); on the Mac CLI the same work is child processes (84-89 s in a quiet-machine run, 1,165 s under load 300 here) |
| oracle data | `out-py/`, `out/ref-img-grizzy/`, `out-game/*.ipa` (incl. `GameApp-28`) and most `out-*` folders of the iOSBuilder checkout were deleted by someone during this session (~12:09-12:24). Consequences: no Python oracle re-run; the library/rewrite comparisons are against earlier verified CLI outputs (5.2); the size comparison with `GameApp-28` uses the folder I had unzipped before it disappeared (uncompressed sizes only), and the like-for-like comparison uses the newest Python .ipa that is still there |
| test-harness shortcuts | the export through the UI needed a game card: `Grizzy and the Lemmings [0100DFA027504000][v0].nsp` is a 90-byte placeholder file (the real dump needs keys) and the `game=` line of `Documents/gameapp.framework` was set to its name (the chain-only run left it empty). Both only in the test container, which was deleted afterwards |

Findings worth acting on:

1. **DM_PROFILE race** (5.4): move the three `setenv` calls of `DlopenBoot::boot_framework` before `dlopen` (and before the reservation). Until then the shim's early `DM_PROFILE` is what protects the player.
2. The in-app library differs from the process-based CLI library **only because the app's translator is newer than the CLI's native `dmsh_xlate`** (5.2). A CLI run for comparisons should be given a `dmsh_xlate` built from the patched engine (`app/` documents how: a csproj over the sources the app compiles), or it will "differ" on 52 % of the shader files.
3. `tools/patched_engine.sh` of the worktree, called without an argument, writes inside the worktree (`out-engine/`) and runs `git status` in the read-only engine clone; `build-app.sh` avoids both. The same goes for the worktree's `build_gameapp.sh` simulator branch (it uninstalls the user's app from the chosen simulator first).
4. The Python .ipa kept `host-prebuilt.dylib` of 2026-11-18 (6.07 MB); the one in the newest worktree build is the 2026-11-19 02:39 one (6.75 MB); this package uses the newest. The 2026-11-17 host of the main checkout lacks the pad / live-shader entries.
5. The device executable grew by 4.8 MB (3.71 -> 8.51 MB) for the whole Rust chain; the Python pieces it replaces were 28.8 MB (9.3 MB stored).

## 7. Exported game .ipa layout

This is the **input contract** for a later step that takes a game .ipa to a Mac, optimises it and reinstalls it. It is written from `GameAppExport.cs` (read in full: `Plan`, `Export`, `PreparePlayerDocuments`, the `ExportedGame/manifest.tsv` reader)
and from two real exports of Grizzy and the Lemmings made through the app's own UI in the iPhone 17 simulator (Settings sheet of the game -> *Export as .ipa...* -> Light IPA / Full IPA), read back with `app/sim/verify-export.sh`. The simulator build was the
exporter, so the plist platform keys of those two say `iphonesimulator` and the nested frameworks of the simulator bundle carry `_CodeSignature`; a device-built app exports the same layout with `iphoneos` keys and no `_CodeSignature` anywhere (its bundle signs only the executable).

**Which app exports what.** `Export` copies THIS app's bundle (`NSBundle.MainBundle.BundlePath`) plus the built game from Documents. Nothing it needs comes from the interpreter: the Rust-backed bundle has none (no `Python.framework`, `py/`, `pyext/`, `.py`, `shaderengine/`: checked
in both exports, and by `build-app.sh` on the bundle). The Python app's export carried all of them (about 28.8 MB of Python pieces in every exported game).

### What `Export` reads, and where it puts it

| source | goes to (`Payload/<Name>.app/...`) | skipped |
|---|---|---|
| the app bundle, every file | the same relative path | `_CodeSignature/` (root), `Info.plist` (replaced, below), `ExportedGame/` (never nested) |
| `Documents/DmGameProbe.framework/` (what the chain assembled and signed) | `Frameworks/DmGameProbe.framework/` | its `_CodeSignature/` and `Info.plist` (replaced, below) |
| `Documents/extract/flat/` (21 files) | `ExportedGame/extract/flat/` | |
| `Documents/extract/RomFS.img` | `ExportedGame/extract/RomFS.img` -- **Full export only**; Light: a clone `Exports/<Name>/<Name> [<title id>].romfs` beside the .ipa | |
| `Documents/shaders/<id>/` | `ExportedGame/shaders/<id>/` | |
| `Documents/profile.tsv` | `ExportedGame/documents/profile.tsv` | |
| `Documents/games/<id>/` | `ExportedGame/documents/games/<id>/` | `rewrite-cache/`, `temp/`, `env.txt.part` |
| `Documents/profile-derived/<id>/` | `ExportedGame/documents/profile-derived/<id>/` | |
| generated | `ExportedGame/manifest.tsv`, `Info.plist`, `Frameworks/DmGameProbe.framework/Info.plist`, `GameIcon60x60@2x.png`, `GameIcon60x60@3x.png` (from `games/<id>/icon.jpg`, when there is one) | |

### The two real exports (Grizzy and the Lemmings, `0100DFA027504000`, built by the Rust chain in the app, simulator build)

| | Light | Full |
|---|---|---|
| `.ipa` | 2,699,590,815 B, 505 entries (2,624.8 MB uncompressed), written in 7.5 s | 5,197,625,288 B, 506 entries (5,007.1 MB), 10.0 s |
| beside it | `Exports/Grizzy and the Lemmings/Grizzy and the Lemmings [0100DFA027504000].romfs`, 2,498,033,693 B, an APFS clone (no extra space) | nothing |
| `app/sim/verify-export.sh` | ALL EXPORT CHECKS PASSED | ALL EXPORT CHECKS PASSED |

Sizes by folder in the Light one (uncompressed MB, files): `ExportedGame/shaders/0100DFA027504000` 1,818.0 (12), `Frameworks/GameApp.r2r.framework` 360.6 (3, the simulator's non-trimmed runtime: a device bundle is 26.96 MB here), `Frameworks/DmGameProbe.framework` 185.9 (2),
`ExportedGame/extract/flat` 180.5 (21), `Microsoft.iOS.dll` 22.2, `GameApp` 14.8, `host-prebuilt.dylib` 4.3, `dm/host` 1.8 (53), `dm/games` 0.4 (188). (A device export is smaller: its bundle is ~65 MB, `docs` section 4.)

### The exact layout (file by file)

```
Payload/<Name>.app/                         <Name> = GameAppUi.ExportFileName(display name): "Grizzy and the Lemmings"
  Info.plist                                GENERATED from the app's own: CFBundleIdentifier = <app id>.g<title id lower case> (device: dev.ibuilder.app.g0100dfa027504000),
                                            CFBundleDisplayName = the game's name, CFBundleName = its first 15 characters, CFBundleIcons -> GameIcon60x60 (when the game has an icon;
                                            CFBundleIcons~ipad removed), everything else as the app's (CFBundleExecutable GameApp, CFBundleVersion 1, UIFileSharingEnabled, scene manifest, ...)
  GameApp                                   the app's executable (Mach-O arm64, platform 2 on a device build): UNCHANGED, and it is iBuilder's: it becomes the game's player because ExportedGame/manifest.tsv exists
  GameIcon60x60@2x.png  GameIcon60x60@3x.png    120x120 and 180x180 PNG of games/<id>/icon.jpg (absent without an icon)
  Assets.car  AppIcon*.png  PkgInfo  *.dll  libcoreclr / libSystem.* / Microsoft.iOS.dll ...   the .NET runtime and the app's managed code (GameApp.dll, LibHac.dll, Renci.SshNet.dll, ...), as in the app
  host-prebuilt.dylib                       the engine host the chain grafted the game onto (not used at the boot of an export; the framework is already assembled)
  dm/games/settings.tsv, dm/games/<id>/game.tsv (187), dm/host/*.c, dm/gfx/**        the data the Rust chain reads from the bundle (profile tables, hook-set sources)
  Frameworks/
    GameApp.r2r.framework/  OpenSSL.framework/  libSystem.*.framework/  libcoreclr.framework/     the app's own frameworks (device build: 8 in all, no Python)
    DmGameProbe.framework/
      DmGameProbe                           THE GAME: host + the rewritten NSO modules, assembled by ibuilder-macho and signed by zsign (adhoc / device identity) in the app; ~195 MB
      Info.plist                            GENERATED: CFBundleExecutable DmGameProbe, CFBundleIdentifier <export app id>.DmGameProbe, CFBundlePackageType FMWK, MinimumOSVersion/CFBundleSupportedPlatforms/UIDeviceFamily from the app
  ExportedGame/
    manifest.tsv                            KEY<TAB>value: title_id, dump (the dump's file name: the game's identity inside the app), name, app_build (the exporting app's build id), exported (UTC);
                                            Light adds romfs_file, romfs_size, romfs_check (a checksum of the .romfs, read by the player to accept a file)
    extract/
      flat/                                 21 files: for each of main, subsdk0, sdk: <m>.patched.bin, .fixups.tsv, .sites.tsv, .gates.tsv, .cuts.tsv, .obs.tsv, .changed.tsv
      RomFS.img                             Full export only (DMROMFS1 image of the game's files; for Grizzy 2,498,033,693 B)
    shaders/<title id>/
      b-<chain>.img                         the shader library (DMROMFS1; 1,902,667,444 B for Grizzy: 21,990 shaders, 56,885 files), the chain being the host's (b24b25240cad)
      shader_map.tsv  shader_pairs.tsv  shader_facts.tsv  c1_full.tsv  cb_extent.tsv  pso_manifest.tsv  pso_pairs.tsv  bf_want.tsv  shader_map.tsv.conflicts.tsv  game.tsv  pack.txt     the tables the host reads beside the library
    documents/                              first-launch seed for the player's Documents: a file is copied only when it is not there yet (the player's own settings and saves are never overwritten)
      profile.tsv                           the runtime profile (what DM_PROFILE names): 37 rows for Grizzy
      games/<title id>/icon.jpg  build_ids.txt  mods.applied  [launcher.json  env.txt  used_shaders.tsv  mods/ ... when the game has them]
      profile-derived/<title id>/game.tsv shader_map.tsv ...     the derived game table and the maps beside it
```

### What the app does with it at launch (player mode)

`GameExport.IsPlayer` = `ExportedGame/manifest.tsv` has `dump`. Then: `PreparePlayerDocuments` seeds Documents from `ExportedGame/documents/` (missing files only) and **rewrites `Documents/gameapp.framework`** with `app_build=<this app's build id>` and `game=<dump>` on every launch
(so the framework in the bundle always counts as this build's); the library lists only this game; `ConfigText` (player branch) points the chain at the bundle: `extract_out=<bundle>/ExportedGame` ... `framework_out=<bundle>/Frameworks/DmGameProbe.framework/DmGameProbe`,
`shader_packs=<bundle>/ExportedGame/shaders`, `shader_build=0`, `romfs=` the bundle's `extract/RomFS.img` (Full) or the `.romfs` found in Documents / remembered through a security-scoped bookmark (Light), `profile=Documents/profile.tsv`. Play = a boot-only run of the Rust chain:
it reads `extract/flat`, `shaders/<id>/`, `Documents/profile.tsv`, `Documents/games/<id>/` and calls `dm_host_probe_boot` on the bundle's framework. No stage of the build runs.

### What a Mac-side optimiser may replace (and what must stay consistent)

Read from the code and the two exports; **none of the replacements below was tried** (no optimiser exists yet). The installer re-signs every Mach-O and the bundle: nothing here is covered by a signature the optimiser must keep.

| file | may be replaced by | must stay true |
|---|---|---|
| `Frameworks/DmGameProbe.framework/DmGameProbe` | a better build of the same game (re-assembled host, recompiled guest) | same exported entries the app and host dlsym (`dm_host_probe_boot`, `dm_front_set_layer`, `dm_front_pad_*`, `dm_front_tv_attached`, `dm_game_menu`, `nvn_live_set/ready`, `_dmgame_image_marker`); the shader chain it carries (`b24b25240cad` string) = the library's = the app's `GameAppBuild.ShaderChain`; platform 2 (device) |
| `ExportedGame/extract/flat/<m>.patched.bin` + tables | re-rewritten modules | the host loads `<dir>/<m>.patched.bin`, `.fixups.tsv`, `.gates.tsv` (`loader.c`) and `main.changed.tsv` (cheats) from this folder at every boot, so these must pair with the framework's assembled modules; `modules=main subsdk0 sdk` order |
| `ExportedGame/shaders/<id>/b-<chain>.img` + the tables | a library built with better shaders/compilation (it is a DMROMFS1 image the host reads through `DM_MSL_DIR`) | the file name carries the host's chain; `shader_map.tsv`, `shader_pairs.tsv`, `pso_*`, `c1_full.tsv`, `cb_extent.tsv` must belong to the same library; `pack.txt` says which game/chain/count |
| `ExportedGame/extract/RomFS.img` (Full) or the `.romfs` (Light) | a repacked RomFS (e.g. recompressed assets) | the manifest's `romfs_size` / `romfs_check` (Light) are what the player accepts a `.romfs` by: update them with the file (`GameAppUi.RomfsCheck`) |
| `ExportedGame/documents/**` | the seed files | only for a **fresh install**: files already in the player's Documents are never overwritten |
| `host-prebuilt.dylib`, `dm/**` | not needed to play an export | harmless to keep |
| `GameApp`, `*.dll`, `Frameworks/*` other than DmGameProbe | the app's own build (newer iBuilder) | keep `ExportedGame/` out of the copy; the export's identity is `Info.plist` + `ExportedGame/manifest.tsv` |
| `Info.plist` | | keep the generated bundle id (`<app id>.g<title id>`: the installer spends one App ID per distinct id), `CFBundleExecutable GameApp`, `CFBundleVersion` |

Must NOT be added: `_CodeSignature/`, `embedded.mobileprovision` (SideStore / AltStore / FlareStore add their own), a `Python.framework` or any `.py` (nothing reads them).

## 8. Installing

1. Build or take the file: `dist/GameApp-rust-<buildid>.ipa` (`.sha256` beside it; `dist/MANIFEST.txt` says what is inside and which Rust sources it was made from). Check `shasum -a 256 -c`.
2. Send it to the phone the way the earlier .ipa files went (Files / AirDrop) and install it with **SideStore** (or FlareStore) with the **same Apple ID** as the installed iBuilder: My Apps -> + -> the .ipa. Its bundle id is `dev.ibuilder.app` (renamed from `dev.nxpocket.gameapp` on 2026-11-22), so it installs as a **separate, new app**; the old app stays and keeps its data. See "Moving from the old app" below.
   The .ipa is unsigned / ad hoc: SideStore signs the app and every framework in it, and asks the signing account for the increased-memory-limit capability because the executable declares it
   (`com.apple.developer.kernel.increased-memory-limit`; extended-virtual-addressing is left out on purpose: a free Apple ID cannot carry it. `GAMEAPP_EVA_ENTITLEMENT=1` builds with it).
   **Moving from the old app (`dev.nxpocket.gameapp`):** the new app has its own data container and its own Keychain, so nothing moves by itself: games, builds, saves, the `.p12` and its password (Keychain) and the NAS password stay in the old app. The old Info.plist sets `UIFileSharingEnabled` and `LSSupportsOpeningDocumentsInPlace`, so the old app's Documents are visible in the Files app (On My iPhone > iBuilder): copy the games and saves you want from there into the new app's folder (also under On My iPhone > iBuilder) or point the new app at them with its folder picker. The `.p12`/certificate and the NAS password are re-entered in the new app. **Do not delete the old app until you have copied what you need**: that deletes its Documents and Keychain items. With a free Apple ID through SideStore the new id spends one more App ID of the weekly quota (the old app's App ID still counts until it expires or the old app is removed).
3. First launch: iBuilder now says its framework was built by an older version of the app (the stamp is the build id): the game needs one build (the Rust chain, expect it to be far quicker than before; the shader library of the previous build is reused only if its stamps match). Some work is redone once by design: `launcher.json` is rewritten (new catalogue digest),
   an interrupted stage 3 left by the Python app does not resume once, the log marks restart (`docs/chain/PIPELINE-RUST.md` section 7, "Formats that changed on purpose").
4. What to look for, and to send back: **Settings -> Keys**: "RAM unlocked" and the ceiling (the only proof the entitlement applied); a build's **Logs -> Share** (`pipeline.stages.log` says `ibuilder-pipeline` in its banner and has no Python lines; `build_times.tsv`); the `[mem:...]` lines against the Python app's 2.9 GB; whether Play boots (`gameapp.guest.log`).
5. **Not available in this build**: Switch 2 `.nup` games until `ibuilder-nup` is in it (section 1.4): import `.nsp` / `.xci` / `.nsz` as before.
6. To go back: install an earlier Python .ipa over it (same bundle id); the Documents are shared by both.

## 9. The C# patches (`app/patches/`), and what the chain's result tells the app (2026-11-21)

**How they are applied.** `scripts/build-app.sh` copies `app/gameapp/GameApp*.cs` (the snapshot, never edited) and applies every `app/patches/*.patch` to the copy in glob order with
`patch -p1 --forward`; a patch that does not apply stops the build. `scripts/check-patches-apply.sh` does exactly that on a scratch copy (with a `--dry-run` of each first) and says which
hunk needed fuzz or an offset: it proves the patches APPLY, not that the result compiles.

**How a patch is made** (0077 was regenerated this way, 0078 made this way): apply the patches before it to a scratch copy of the snapshot (`a/`), copy that to `b/`, edit `b/`, then
`/usr/bin/diff -ruN a b` (not a shell alias that adds `--color`) is the body and a `#` comment header goes on top (WHY, WHAT IT DOES, VERIFIED with the commands' results, NOT VERIFIED). To
change a patch in place, `a/` is the tree with the patches BEFORE it and `b/` that tree with the patch's new state; every later patch must still apply (`check-patches-apply.sh`), and the
tree the repository's patches give must equal the tree that was compiled (`diff -r`).

**How it is checked.** (1) `scripts/check-patches-apply.sh`. (2) A compile, with no .ipa: the app project that step 3 of `scripts/build-app.sh` generates (the patched `GameApp*.cs`, the
engine's extractor `Program.cs` with its two edits, the translator sources, the same csproj items), made in a scratch folder from the read-only engine, then `dotnet build gameapp.csproj -c Release -r ios-arm64`
(a full build needs the native libraries of an earlier `build-app.sh` run for its `NativeReference` items) or the same with `-t:Compile` and those items left out (the C# compile alone,
no link: the trimmer's IL warnings are not there); errors must be 0 and the warnings are compared per file and code with the tree before the patch. (3) A host harness for the pure logic: a net console program that compiles the REAL source text of the files that have no UIKit
(`GameAppStop.cs`, `GameAppNas.cs` with `NAS_LOGIC_ONLY`, `GameAppCloud.cs`, `GameAppUi.cs`, ...), reads the UIKit files' text for the wiring it cannot run, and is itself checked by
mutations of the sources (each must make it fail). Nothing of this runs the app: the screen is looked at only in the simulator or on a phone.

**The chain's result, as the app shows it** (`docs/chain/PIPELINE-RUST.md`, "Result codes"). The C# decides on `gameapp_run_chain()`'s code (`StopMachine.ChainReturned`: 0 finished,
130 / 131 a stop, anything else failed) and on the framework stamp (`GameAppUi.ClassifyFramework` / `GameStatusOf`: a stamped framework of this app build is "Ready"). Before review r4 a
build whose stage 3 could not make the shader library returned 0 and was stamped (and booted with `boot_after_build=1`, or at the next Play): the app said "Ready" / "Built: ready to play"
for a game that draws nothing; and a build whose assembly or signature failed returned 0 too ("Built: ready to play." for a game that is not there). Now the library returns **4**
(`IBUILDER_RC_NO_SHADER_LIBRARY`: does not boot, does not stamp) or **5** (`IBUILDER_RC_NO_FRAMEWORK`: stage 4 failed, nothing to boot or stamp), and the shim passes them through
(`_Static_assert` on both values); an app without 0078 already shows 4 and 5 as failed runs, and **patch 0078** (`app/patches/0078-no-shader-library-is-a-failed-build.patch`) gives them
their words:

| where | text (0078, `ChainResult` in `GameAppStop.cs`) |
|---|---|
| the log, as an error row (the build screen's live rows, "See what stopped it") | `[RED] The shader library could not be built: the game would draw nothing. Rebuild, or send the log.` |
| the run line under the game (instead of "Stopped at <step>") | `Shader library not built · <time>` |
| the "Notify me" notification | `The shader library could not be built: the game would draw nothing. Rebuild, or send the log.` |
| the build queue's note | `the shader library could not be built: see the log` |
| code 5: the log (error row) and the notification | `[RED] The game could not be assembled or signed: see the log.` (the notification without `[RED] `) |
| code 5: the run line | `Not assembled or signed · <time>` |
| code 5: the build queue's note | `the game could not be assembled or signed: see the log` |

The library shows the game as "Needs setup" after such a build (no stamp); after a refused Play (boot-only) it stays "Ready" (the framework is this build's) and the next Play tries the
library again. Strings worth a marker in `app/verify_ipa.sh` (owned by the scripts package): the six texts above, as they land in `GameApp.dll`.

**Patch 0077, regenerated in place** (review r4 §4.5): the `Exports` folder is matched ignoring case like the NAS's own folders (the exact name preferred when two differ by case); the
summary's subfolder count is the subfolders READ (the attempts are `Looked`, the cap is `SubfoldersFound > Looked`); the texts say the cap of 200 is on subfolders, not on files.

Verified (M1 Pro, 2026-11-21): all 34 patches apply in order (`check-patches-apply.sh`, no fuzz, no offset) and the repository's patches give exactly the compiled tree; the full build:
0 errors and 41 warnings for the tree up to 0077 (as it was, and as regenerated) and with 0078, the same warnings per file and code; after a reboot wiped that scratch, the compile alone
(`-t:Compile`, recipe above) again: 0 errors and 40 warnings for the trees up to 0076, 0077 and 0078, the same per file and code, and the three texts are in the compiled GameApp.dll;
host harness 56 of 56, 15 of 15 mutations killed (run twice, before and after the reboot). With code 5 added to 0078: the compile alone again 0 errors and 40 warnings with and without
0078, the same per file and code, the six texts in GameApp.dll; harness 61 of 61, 19 of 19 mutations killed. Not verified: no
.ipa was built, nothing ran in the simulator or on a phone; the screen with these texts was not looked at.

