# Engine update: the host rebuilt with the 11 audited patches (2026-11-19)

The user's instruction after `docs/UPSTREAM-AUDIT.md` was "met à jour le moteur". This document says what that did, what it did not do, what it changes for the problems seen on the phone, what is still unproven, and how to go back.
**Nothing here ran on a device, and no app or IPA was built** (the rule of the session). What exists: two new host dylibs in `dist/host/`, the script that makes them, and one C# patch for the app that tells the host when it goes to the background.

## 0. Summary

| | |
|---|---|
| Engine tree | the patched tree the app ships (`out-stage/out-engine/engine`: nxnative `a3663659` + the 54 NX2Apple patches + `paches/engine/0103..0301`) with `engine-patches/0401..0411` applied in series: **11 of 11 apply, no fuzz, no offset** |
| Host-level patches | `paches/0001..0009, 0012, 0013` (and `paches/engine/*` from the raw clone) still apply on top, no fuzz (section 1.2) |
| Shader chain id | **`b24b25240cad`, unchanged** (`dmsh_chain.h` and `dmsh_chain.py --header` regenerated agree): no game's shader library has to be rebuilt |
| Host dylibs | `dist/host/host-prebuilt-loadable-ios.dylib` (device, arm64 iPhoneOS, platform 2, minos 26.0) and `-sim.dylib` (simulator, platform 7, minos 27.2), built clean from scratch in **43 s** and **19 s** on this 8-core Mac (a first run took 28 s and 13 s; the whole script is about a minute, 5 more with `HOST_TESTS=1`) with Xcode 27.2, iPhoneOS 27.2 SDK |
| Exports | nothing lost; one symbol added (`nvn_bc_decode_par`, from 0411); every symbol the app resolves is exported |
| Self-tests on this Mac | the engine's host unit tests, the CPU texture tests and nine of the ten graphics tests I ran pass (section 3); the tenth does not compile in the base tree, and 0409's Python test needs a module that is not installed (section 3.2) |
| App change | `app/patches/0030-suspend-graphics-in-background.patch`: the app tells the host when it leaves the foreground (section 5). Applies; the new C# members compile alone; **not compiled in the app, not run** |
| Needs your approval next | building the app with the new host (`GAMEAPP_HOST=...`), then a device run (section 7) |

## 1. What was applied

### 1.1 `engine-patches/` (sha256 in `dist/host/HOST-MANIFEST.txt`)

| Patch | Upstream | What it changes in behaviour | Default | Mac test that exercises it |
|---|---|---|---|---|
| 0401 | nxnative `79f28b06` (PR #2983) | **audren burst pacing**: a device callback that frees several 5 ms blocks no longer starts back-to-back renders; fresh renders are spaced 4.5 ms apart after the prefill, the catch-up output is rechecked after the wait. Hand-merged with NX2Apple 0048's teardown | on | `t_audren_layout`: "48000 Hz/512 device frames, 936 callbacks (936 burst releases), 2000 PCM blocks in order, min exec spacing 4.50 ms, no underruns" |
| 0402 | `943a6111` (adapted to `rh_alloc_size`, NX2Apple 0055) | **memory**: render-target recycle slack capped at 4 MiB (a 296 MB shadow target had sat in a 333 MB heap) | on | `t_rt_recycle_policy` (57 checks) |
| 0403 | `1d1c5b4a` | **memory**: zero fills and scissored clears copied in bands that fit the 4 MiB shared buffer (it had grown to 64 MB) | on | `t_zero_colour` (30), `t_lazy_clear_order` (21) |
| 0404 | `93c76c1c` | trim completed idle upload staging | **off**; `DM_NVN_UPLOAD_IDLE_MS=<ms>` (the engine's own check uses 2000) | `t_upload_idle` (27 / 46 checks, 4 blocks trimmed when on) |
| 0405 | `31fdefeb` | bound the zero-copy upload sources per batch (keeps this tree's `!bc_cpu` guard) | **off**; `DM_NVN_UPLOAD_SRC_MB=<MB>` (the check uses 96) | `t_upload_src` (61) |
| 0406 | `5012821e` | pipeline look-ahead no longer resolves a future sparse attachment; only the Metal 4 path (`pso_mtl4`) reaches it, and this app runs the MTLDevice path: a guard | inert here | `t_pso_pending` |
| 0407 | `4b92e232` (PR #2878) | Npad style read under the bind lock (a bind could miss its first signal) | on | `t_hid` (in the host unit run) |
| 0408 | `74d17535` (PR #2938) | composed-mesh arguments checked live: no false "zero GPU addresses" (Switch 2 mesh shaders) | on | `t_cbs_fault` (21); `t_geometry_mesh` see 3.2 |
| 0409 | `0b5fd602` | rewriter: a conditional select on x18 no longer spills over live stack (Sunshine PCM corruption). **Changes `dm/tools/patch_nso.py` only: the host dylib does not contain it, and the Rust rewriter does not have it yet** | n/a | `test_x18_select.py` needs `capstone`: not run |
| 0410, 0411 | NX2Apple 0063, 0064 | BC buffer-to-texture copies decoded on the CPU, and BC decode split across cores. Run only on a GPU **without** BC (A16 and older); inert on the iPhone 15 Pro | inert on A17 Pro | `t_bc_cpu` (466), `t_bcdec`, `t_tex` |

`DM_AUDREN_TRACE=1` (0401) prints the first 200 audren updates with their times: the way to see the pacing on a device. The opt-in switches go in the game's `Documents/games/<id>/env.txt` like any `DM_*` variable.

### 1.2 Do the patches that were already there still apply on top?

Checked on scratch copies, `patch -p1 --forward`:

* `tools/patched_engine.sh` (the worktree's own script, run with `NXPOCKET_ENGINE_SOURCE` = a clone of `out-nx2apple/engine` in scratch): applies `paches/engine/0103, 0104, 0202, 0203, 0205, 0206, 0301`, chain `cf3be507e0eb -> b24b25240cad`, 15 files changed, `stamp.txt` and `changed.txt` equal to the shipped ones. Then 0401..0411 on top: the tree is **identical to the shipped tree plus 0401..0411** (`diff -r`, only `__pycache__` differs). So a `patched_engine.sh` that lists `engine-patches/04*` after `paches/engine/*` produces the same tree.
* On the updated tree, in the order the scripts use: `paches/0007 -> 0008 -> 0009` on `nvn_mtl.m` (`prebuild_loadable.sh` step 1b), `0012 -> 0013` on `DmGamepad.m`, `0006` on `hle_svc.c`, `0002 -> 0004 -> 0005` on `patch_nso.py`, `0003` on the four Metal files, `0001` on the two C# tools: **all apply, no fuzz**.

## 2. The build

`scripts/build-host.sh` (reads the iOSBuilder worktree, `$IOSBUILDER_WORKTREE` from the environment or from `scripts/local.env` (see `scripts/local.env.example`); writes only `$HOST_WORK` (default `target/host-work`) and `dist/host/`; no git; no app):

1. APFS-clones the shipped patched tree, applies `engine-patches/04*` (the first that does not apply stops the build and is named), then `paches/0006`. The worktree's `tools/build_host_ios.sh` applies 0006 to the engine tree itself with git for the length of its compile; a copy does not need that.
2. Stops if the shader chain id is not `$EXPECT_CHAIN` (default `b24b25240cad`).
3. Copies the worktree's `tools/`, `load/`, `paches/` (without 0006) and `out-stage/out-video`'s decoder archives into a mirror under `$HOST_WORK/repo` and runs **the worktree's own `load/prebuild_loadable.sh device|simulator`** from there, with `NXPOCKET_ENGINE` and `IOSBUILDER_OUT` in scratch. That is the recipe that produced the current prebuilt (the 2026-11-19 02:39 logs show `out-stage/load`, whose files equal the worktree's): all 77 host objects with the generic profile header, `paches/0007/0008/0009` on a copy of `nvn_mtl.m`, `0012/0013` on a copy of `DmGamepad.m`, the engine's marker and glue, `load/front_*.m`, libvpx and dav1d, `-segaddr` for `__DATA_CONST 0x7fff0000` and `__DATA 0x80000000`, the generated stand-ins, `fix_tlv.py`. Targets: device `arm64-apple-ios26.0`, simulator `arm64-apple-ios27.2-simulator`. A stub replaces `tools/build_video_ios.sh` so that nothing can write into the worktree.
4. Checks each dylib (platform and minos, exports, imports, chain id and patch markers as strings) before it writes `dist/host/`.
5. `HOST_TESTS=1` also runs the engine's self-tests (section 3) and records them in `dist/host/HOST-TESTS.txt`.

Compared with the current prebuilt (which `dist/host/previous/` keeps):

| | device | simulator |
|---|---|---|
| size | 6,747,984 -> 6,771,664 | 4,520,400 -> 4,526,848 |
| sha256 | `37f1b6b8f9f5a5326fe3c5e435abc38119b28846a3b7f076973f0fc589294a91` | `2a61e184329385bcc0e570bc6624022480e205b024a8a2e0d51eedbb7a1a79ac` |
| platform / minos | 2 / 26.0 (same) | 7 / 27.2 (same) |
| segments | `__DATA_CONST`, `__DATA` at the same addresses and sizes; `__TEXT` (+16 KB) and `__LINKEDIT` grew | same; only `__LINKEDIT` grew |
| exports (`nm -gU`) | 3,331 -> 3,332, none lost; added `nvn_bc_decode_par` | 2,932 -> 2,933, same addition |
| imports (`nm -u`) | identical (770) | one new, `_dispatch_apply` (libSystem, 0411) |
| generated stand-ins | 5, identical list | 134, identical list; the same 5 sources do not compile on the simulator SDK (MetalFX / MTL4) |
| markers in the strings | `DM_AS_AT` (0006), `DM_PSO_NOWAIT`, `DM_LOW_MEMORY`, `DM_RT_SHARE` (iOSBuilder), `DM_AUDREN_TRACE`, `DM_NVN_UPLOAD_IDLE_MS`, `DM_NVN_UPLOAD_SRC_MB` (new) | `DM_AS_AT`, `DM_AUDREN_TRACE` |
| chain id | `b24b25240cad` present once (the `b-%s` library folder) | same |

The symbols the app resolves from the loaded host: `dm_host_probe_boot`, `dm_front_set_layer`, `nvn_live_set`, `nvn_live_ready` (device only: the simulator host has no `nvn_mtl.m`, as before), `dm_front_pad_create`, `dm_front_pad_layout`, `dm_front_pad_layout_tv`, `dm_front_tv_attached`, `dm_game_menu`, `dm_gamepad_view`, `dmgame_image_marker`, `dm_glue_start_game / present_game / focus_event / version`, plus the lifecycle ones of section 5. All are exported by both new dylibs.

Not reproducible bit for bit: two links of the same device objects differed by about 1.3 KB (the stub tables, the UUID), the simulator ones did not; the object paths are in the symbol table (`-g` objects), so the size also depends on the work folder's name (the numbers above are those of the delivered files). Same inputs, same exports, same code.

The app build is unchanged by this: `NXPOCKET_ENGINE` can stay as it is. What `build-app.sh` reads from the engine tree is the profile tables, the extractor, the translator and the HLE hook names in `dm/host/*.c`; 0401..0411 add or remove no hook and touch no table (`hle_audren.c` and `hle_hid.c` differ in the old tree it bundles, harmlessly), and the chain id is the same.

## 3. Verification

### 3.1 What ran (on this Mac, an M1 Pro; the engine's CI commands, no game, no key)

`make -C dm/host/tests GAME=010015100B514000 -j8 run` (173 `PASS`, 0 `FAIL`, including `t_audren_layout` and `t_hid`, the tests 0401 and 0407 changed); `make -C dm/gfx/tex/tests run` (`t_tex` 5,386 checks, `t_bcdec`, including 0411's equality of the multi-core decode with the single-core one); `check_t_` of `rt_recycle_policy`, `gfx_suspend` (22 checks: the lifecycle gate the app will now drive), `lazy_clear_order`, `zero_colour`, `upload_idle`, `upload_src`, `pso_pending`, `cbs_fault`, `bc_cpu`. The list and the times are in `dist/host/HOST-TESTS.txt`.

### 3.2 What did not run

* `dm/gfx/tests/t_geometry_mesh` (0408's test): `t_geometry.m` reads `g_mdi_v`, which `nvn_mtl.m` **of the base tree, before any patch of this document**, does not define (it has `g_mdi_waits`): it does not compile there either. With the name changed in a scratch copy it passes (47 checks). A test-only inconsistency of the rebased engine, not of 0408.
* `dm/tools/tests/test_x18_select.py` (0409): needs the Python module `capstone`, not installed here (a download is not mine to make).
* The host dylibs were **not loaded**: the device one cannot be on a Mac, and the simulator one needs a simulator process (an app build). Static checks only: `otool`, `nm`, `strings`, imports, segment layout. Signature state as before (the device dylib is unsigned, the simulator one ad hoc; the pipeline signs the framework it builds).
* Nothing about sound, frame pacing, memory or the shader library was measured on a device.

## 4. What changes for the problems on the phone

| Problem | Effect of the engine update |
|---|---|
| **Mario Kart 8 Deluxe black screen / 0 fps** | **None, on purpose.** The cause is app-side: the shared fonts (`[RED] cannot open the standard shared font`, the game polls `GetSharedFontLoadState` for ever) and the missing `pso_manifest.tsv`. Both fixes live in this repository (`scripts/build-app.sh` builds `dm/font/`, `profile.rs` sets `DM_FONT_DIR`, `set_shader_pack` tops up the pack) and have not been seen on a device. The updated host does not touch that path: the way to find out is still the next device log with the new IPA (`shared font (standard.ttf)` line; then a frame). |
| **Audio pacing (every game on this phone)** | 0401. The MK8D logs show 467 / 963 / 486 catch-up renders at 480-frame callbacks (23-24 % of the renders). The new pacing removes the burst; the engine's own test shows 4.50 ms minimum spacing and no underrun at 44.1 and 48 kHz. Whether it is audible is a listening test: MK8D or Grizzy with sound, `DM_AUDREN_TRACE=1` and the `audren: ... catch-up renders so far` line. |
| **Memory (jetsam on big games)** | 0402 and 0403 are on and trim two measured growths (a 296 MB target in a 333 MB heap; a 4 MiB shared buffer that had reached 64 MB), measured upstream on Ocarina of Time. 0404 and 0405 are off until the two `DM_NVN_UPLOAD_*` switches are set. No number was measured on this phone. |
| **Background / resume (Grizzy's 84 lines of `BackgroundExecutionNotPermitted`)** | The host already had the entry points; nothing called them. The host update changes nothing there. `app/patches/0030` (section 5) makes the app call them. |
| **Sound lost after an interruption, a call, Siri** | Not addressed: the engine's front end observes `AVAudioSession` interruptions and GameApp links no front end. Follow-up, section 5.4. |

## 5. Suspend and resume

### 5.1 What the host exports

`dm_gfx_suspend(int on)` (both directions: **there is no `dm_gfx_resume`**), `dm_audio_suspend(int on)`, `dm_haptics_suspend(unsigned reason, int on)`, `guest_clock_freeze_for(int reason, int on)` (reason 1 = background, 2 = system keyboard), `dm_focus_set(int)` (1 in focus, 2 out of focus, 3 background), `dm_gamepad_reset`, `dm_gamepad_active`, `dm_movie_active`, `front_ios_focus_event` and `dm_glue_focus_event(int)` (the glue's wrapper of it). All were in the old prebuilt too (`nm -gU`). `dm_gfx_suspend(1)` blocks new submits and presents, waits for the calls in progress and drains the GPU, the Vulkan and GL queues, then starts the pipeline archive's write; `(0)` releases them (`hle_nvn.c`).

### 5.2 How the engine's own front end calls them

`dm/ios/Sources/main.m` (the scene delegate) maps `sceneWillResignActive / DidEnterBackground / WillEnterForeground / DidBecomeActive` to `front_ios_focus_event(0..3)` (the Route C shell reaches the same function through `dm_glue_focus_event`); `front_ios.m` does:

* 0, resign active: `dm_movie_active(0)`, `quit_close()`, `dm_gamepad_reset()`, `dm_focus_set(2)`;
* 1, background: `dm_focus_set(3)`, audio suspend, `guest_clock_freeze_for(1, 1)`, **`dm_gfx_suspend(1)`**, then the cloud save flush in a background task;
* 2, foreground: **`dm_gfx_suspend(0)`**, `guest_clock_freeze_for(1, 0)`, audio session re-activated, audio resume, `dm_focus_set(2)`;
* 3, active: idle timer off, audio session retry, `dm_gamepad_active()`, `dm_focus_set(1)`, `dm_movie_active(1)`.

`Launcher.swift` makes no lifecycle call. GameApp links no front end: `load/glue_front_standin.m` supplies `front_ios_focus_event` as a function that prints a line, so even `dm_glue_focus_event` would do nothing. The host only suspends on SIGTERM (the "graphics suspended" line at the end of Grizzy's log).

### 5.3 The change: `app/patches/0030-suspend-graphics-in-background.patch`

Two C# files, 48 added lines, no deletion. In `GameAppExternal.cs` the class `GameAppHost` (the one that already wraps the host's gamepad and TV entries) gets `ibuilder_host_symbol` (the Rust library's dlsym on the loaded host, what `gameapp_pad_*` use through the shim) and two methods that call the host's entries through `delegate* unmanaged` (as `RegisterShaderTranslator` already does):

* `Background(true)` (DidEnterBackground): `dm_focus_set(3)`, `dm_audio_suspend(1)`, `guest_clock_freeze_for(1, 1)`, `dm_gfx_suspend(1)`; `Background(false)` (WillEnterForeground): the reverse, graphics first. Guarded by a flag, and a no-op when the host is not loaded (the lookup returns null before the boot) or lacks the entry.
* `Active(true / false)` (WillResignActive / DidBecomeActive): `dm_focus_set(2 / 1)`, which the foreground step needs, or the guest would stay out of focus.

In `GameAppMain.cs`, `GameAppGameViewController.ViewDidLoad` registers the four `UIApplication` notifications with the view's other observers (they are removed by the same loop in `ViewDidDisappear`: they exist only while a game's window does). The order is the engine's.

Proof it applies: a fresh copy of `app/gameapp/GameApp*.cs`, `0010` then `0020` (as asked), then 0030: `patch --dry-run` and the real run succeed, no fuzz, no reject, the result equals the hand-edited files. In `build-app.sh`'s own order (`app/patches/*.patch`, file-name order: 0010, 0020, **0030-import-and-download-never-delete-first**, 0030-suspend, 0031, 0032) all six apply, no fuzz: 0030-suspend touches `GameAppExternal.cs` and one spot of `GameAppMain.cs` that no other patch touches. The three new `GameAppHost` members were also compiled alone in a scratch .NET 11 library (`AllowUnsafeBlocks`): 0 errors, 0 warnings. **The app was not compiled and the notifications' overloads were not checked by a compiler** (they follow `AddObserver(UIApplication.WillEnterForegroundNotification, ...)` at `GameAppMain.cs:850`). **Untested on a device.**

Two things to know: the file name shares its number with the other agent's `0030-import-and-download-never-delete-first.patch` (harmless, the order is alphabetical, but one of them may want a new number), and **`build-app.sh` applies every `app/patches/*.patch`, so the next app build picks 0030 up by itself**; move the file out of the folder to leave it out.

### 5.4 Not done

`dm_gamepad_reset` on resign-active (a button held while Control Center opens), `dm_movie_active`, and the audio-session re-activation and interruption observer (a native piece: `front_ios_audio_session` is a stub, so after a call or Siri the host's audio units may not restart). The clean place for all of them is a real `front_ios_focus_event` in `load/glue_front_standin.m` (a worktree file, read-only here), after which the app would only call `dm_glue_focus_event(ev)`: it would need another host rebuild.

## 6. What was not done

* **The full rebase onto nxnative `34ace8cc` (62 commits) or NX2Apple `3f431865` (90).** It moves the chain, conflicts in `DmGamepad.m` (11 hunks against 0012/0013/0103), `nvn_mtl.m` (2), `hle_thread.c` (2), `front_ios.m` (1), and has no game on this phone that needs it.
* **The seven chain-moving commits** (`760c7fe8`, `9f834e41`, `c311523c`, `e0c30043`, `b989137a`, `5df3dbe8`, `736a2ecb`: split formats, three libraries compiled together, raw-load pool checks) **ignored on purpose**: each moves `DMSH_CHAIN` (to `fded82111357`), every game's shaders would be translated again (Grizzy: 184 s), the Rust shader stages' byte-compare oracle would have to be re-recorded, and none of the four games needs a split format. This build refuses a moved chain (`EXPECT_CHAIN`).
* `985ea6cf` (lone Joy-Con through `GCControllerLiveInput`): our `paches/engine/0103` stays.
* 0409 is in the tree but **not ported to `ibuilder-rewrite`**: a game rewritten by the Rust chain still has the old x18 select. And a byte-compare of the Rust rewriter against this tree's `patch_nso.py` would now differ on modules with that pattern until the Rust port and a new oracle exist.
* The NX2Apple queue's refreshed iOS-shell patches and the Mac ones; the open upstream PRs.
* The Mac self-tests of the other 60 gfx tests (the ones the patches do not touch), the sanitizer jobs, coverage.

## 7. Risks, and what to look at on the first device run

1. **Nothing ran on a device.** The risks are in the patches with a hand merge or a changed lock: **0401** (the render thread now holds `cq_mu` across the SDK render, merged with 0048's teardown: watch "Quit game" and the graceful exit for a hang, `[RED] graceful exit not done`), **0407** (lock order in `hle_hid.c`), **0402 / 0403** (more, smaller allocations and copies for big targets: a slower frame on a game with huge render targets).
2. **0030 is untested C#**: a wrong symbol order could stall the main thread in `dm_gfx_suspend(1)` while a game compiles a pipeline (the engine does the same call on the main thread). Look for `[nvn] graphics suspended` / `graphics resumed` in `gameapp.guest.log` and the absence of `kIOGPUCommandBufferCallbackErrorBackgroundExecutionNotPermitted` after a home-button press.
3. The pin of the host in `app/inputs.tsv` will read `DIFFERS` (a warning, not an error unless `PIN_STRICT=1`); update the two `host-*` rows when the new host is accepted.
4. The simulator host keeps `minos 27.2` as before.

Next steps that need your approval (a build): `GAMEAPP_HOST=$PWD/dist/host/host-prebuilt-loadable-sim.dylib scripts/build-app.sh simulator` first (the simulator loads the host, which is the first real load test), then the device IPA with `-ios.dylib`.

## 8. Roll back

* Nothing in the worktree was replaced: its `out-load-ui/host-prebuilt-loadable-{ios,sim}.dylib` are as they were, and **copies are in `dist/host/previous/`** (sha256 in `HOST-MANIFEST.txt`). Leave `GAMEAPP_HOST` unset, or set it to the `previous/` file.
* One patch out: remove it from `engine-patches/` (or point `ENGINE_PATCH_DIR` at a folder without it) and run `scripts/build-host.sh` again (about a minute). 0411 needs 0410, and 0403, 0404, 0405, 0408, 0410 and 0411 all edit `nvn_mtl.m`: the later ones may need a re-diff against the shifted context, and the script stops at the first patch that does not apply and names it.
* The app side: delete `app/patches/0030-suspend-graphics-in-background.patch`.

## 9. Files

`scripts/build-host.sh`, `dist/host/` (`host-prebuilt-loadable-ios.dylib`, `host-prebuilt-loadable-sim.dylib`, `HOST-MANIFEST.txt`, `HOST-TESTS.txt`, `previous/`), `app/patches/0030-suspend-graphics-in-background.patch`, this file. Read-only inputs, never written (the shipped engine tree's metadata fingerprint is recorded before and after in the manifest): the iOSBuilder worktree, iPorterNX, `~/Documents/decomp`.


## 10. `engine-patches/0412`: the prepare step builds a head of the prewarm lists before the first frame, the tail in the background (2026-11-20)

The owner's decision: at the first Play of Ocarina of Time (Switch 2, `0400486024DEC000`) compile **half** of the prewarm lists before the first frame instead of all of them. Sections 0-9 above are unchanged; this one adds a twelfth patch, `engine-patches/0412-nvn-prepare-head-then-background.patch` (sha256 `ab572a62d364ea69669653d2399d4fa52177d18cd7c9c4c6c0509eb54c22a7ac`), and the host dylibs rebuilt with it. **Nothing here ran on an iPhone, and no app or IPA was built.**

### 10.1 What it adds

The prepare step (`nvn_mtl.m`: `mtl_prepare` -> `warm_start` -> `warm_prepare`, at the game's first submission, with a window) built every line of the prewarm lists (the profile's `pso_manifest.tsv`, then this device's `pso_user.tsv`) on `hw.activecpu` workers before the first frame. Two new variables, read when the step starts:

| Variable | Default | Meaning |
|---|---|---|
| `DM_PSO_PREPARE_PERCENT=<1..100>` | `100` | The step builds the first `ceil(n * p / 100)` of the lists' `n` **tasks** before the first frame (task = a shader load `S`, a compute pipeline `C`, a mesh pipeline `M`, or the render pipelines `R` of one shader pair; the lists' order). The progress screen and the `pipeline prepare:` lines count the head's lines. `100`, unset, empty, or anything that is not a whole number from 1 to 100 (one log line says so) leaves everything as it was. Ignored without a prepare step (`DM_PSO_PREPARE=0`, no window): there the whole prewarm already runs in the background. |
| `DM_PSO_PREPARE_TAIL=0` | on | Only with `p < 100`. On: the rest (the tail) is built **after** the prepare step (the pipelines' prebuild included) by **one** serial worker at `QOS_CLASS_UTILITY`, beside the game. `0`: the tail is not built at all; a draw that needs one of its pipelines builds it on demand, as for any pipeline the lists do not hold. |

A draw that needs a tail pipeline before the worker got to it takes today's on-demand path (the app sets `DM_PSO_NOWAIT=0`: that draw waits for its compile); the worker's later build of the same pipeline is dropped by `warm_claim` (the first one in the cache stays, as for the prefetch and the background prewarm). Both variables go in the game's `Documents/games/<id>/env.txt` like any `DM_*` switch (section 1.1); how the app itself sets environment was not looked at.

The log says exactly what happened (`gameapp.guest.log`):

```
pipeline prepare: DM_PSO_PREPARE_PERCENT=50: head H lines (the first k of n tasks) before the first frame, tail T lines (m tasks) afterwards in the background
                                                                                     | SKIPPED (DM_PSO_PREPARE_TAIL=0): not built, a draw that needs one builds it on demand
pipeline prepare: head D of H lines done in S s (DM_PSO_PREPARE_PERCENT=50); tail T lines: starts after the prebuild | skipped
pipeline prewarm head done: shaders ... / render pipelines ... / compute pipelines ... (the old "done" line, with "head")
pipeline prewarm tail: started in the background, m tasks (T lines), one worker at utility QoS, S s after the prewarm began
pipeline prewarm tail: N of M tasks done, X of T lines, after S s            (every 60 s)
pipeline prewarm tail: finished, m tasks (T lines) in S s                    then the usual "pipeline prewarm done: ..." (the whole-list totals)
pipeline prewarm tail: stopped after N of M tasks ... (the process or the graphics are ending)
DM_PSO_PREPARE_PERCENT=50: this start compacts the pipeline archive, ...: every line is built before the first frame
```

### 10.2 The five points the patch header analyses (full text in the patch)

* **Default = today.** With `p = 100` the diff reduces to the same statements: `warm_head_count(n, 100) == n`, `warm_prepare` gets `head == tasks.count` (`warm_claim_index` is the old `atomic_fetch_add` and `i >= n`), `warm_finish_ex(1, 0)` is the old `warm_finish` (kept as that call), `prepare_prebuild` follows, the tail is never reached; the background branch is untouched. `t_pso_head` runs that path (unset, empty, `100`, `99` of 40 tasks, five invalid values, `TAIL=0` alone) and checks lines, libraries, pipelines, dictionary, sweep and log.
* **(a) What runs after the head, and what only after the tail.** After the head: the log, `g_warm_running = 0`, `arch_flush()`, **no** archive sweep, and the shader dictionary (`g_warm_dict` / `g_warm_loads`) **kept** when a tail follows (released when none does); then `prepare_prebuild()` exactly as today. Then the tail starts (`g_warm_bg = 1`, `g_warm_running = 1` again: for it the prewarm is what it is in the app without a prepare step, so the archive's timer flushes follow `arch_warm_flush_due`: 60 s, 120 s, ..., not one file every 10 s). Only after the tail (`warm_finish`): the sweep, the `done` line, the dictionary released, `g_warm_running = 0`. The sweep waits because `arch_sweep` deletes the archive files none of whose pipelines a lookup took, on the premise that the lists asked for every pipeline: after a head alone that would delete files that hold only tail pipelines. It therefore never runs with `DM_PSO_PREPARE_TAIL=0` or after a stop. **No use after release, no race**: the head's workers (concurrent, as before) have all joined when `warm_prepare` returns; from then on one thread calls `warm_task`, the tail's worker, and `warm_shader` is reached only from `warm_task`, so that thread is the only user of the dictionary; it is released by that thread at its end, or by the head's end when no tail exists, or by `dm_gfx_teardown` after its `dispatch_group_wait(bg_group())` which joined the tail. Results reach the game only through `warm_enqueue` / `warm_claim` (`g_warm_lock`), as the background prewarm's do.
* **(b) Progress.** `warm_prepare` takes the head's lines (the sum over the head's tasks, a render task being one line per pipeline of its pair): the screen counts `g_warm_done` against them and closes at the head's end.
* **(c) Stop / teardown / background.** The tail tests `g_gfx_teardown` and `g_warm_stop` between two tasks and ends without `warm_finish`; it runs on `bg_group`, which `dm_gfx_teardown` waits for after setting its flag. Going to the background is not hooked (as for the background prewarm): the tail goes on until iOS suspends the process, and the archive writes the period under way each time.
* **(d) `prepare_prebuild` is left as it is, and is also large.** It builds the pulling base of every pair of `shader_pairs.tsv` and `pso_pairs.tsv` before the first frame, once per chain / depth mode / pair list (`prebuilt.tsv`). For this game `pso_pairs.tsv` has 6,472 rows; the engine tree's profile also has `shader_pairs.tsv` (28,179 lines) and the prebuild reads both: the engine's own note measured **34,649 pairs, 493 s on an M4 Pro** with 14 workers for the prebuild tool. The head setting does not change any of it: on a cold first start the prebuild may be most of what is left of the wait (`DM_PSO_PREBUILD=0`, an existing switch, skips it and leaves the bases to the draws). It may make the tail cheaper (the pulling bases of the tail's render pairs are already built or archived by then): not measured.

A start that compacts the pipeline archive (more than 8 files in a tier) ignores the percentage and builds every line before the first frame: compaction recaptures what the prewarm finds in the old files and closes at the prewarm's end, so it has to see the whole prewarm.

### 10.3 What was run (this Mac, an Apple M1 Pro, 8 cores; the engine's own harness, no game, no key)

| Check | Command (in the built tree, `GAME=010015100B514000`) | Result |
|---|---|---|
| Patch applies | `patch -p1 --forward --verbose` of `0401..0412` on a fresh clone of the shipped tree; and by `scripts/build-host.sh` | 12 of 12 apply, no fuzz, no offset (every 0412 hunk "succeeded at N" with no offset note); `paches/0007 0008 0009` still apply to the new `nvn_mtl.m` |
| Pure part, no GPU | `make -C dm/gfx/tests GAME=... check_t_warm_head` | **455,404 checks, 0 failed** (ceil against a 128-bit reference for 19 sizes up to `LONG_MAX` and every `p`; exhaustive for `n <= 1500`; the variable's text; 244 partition runs with 1-16 threads, the tail beside the head or after it: head = first k, tail = rest, nothing lost, nothing twice, ascending order). Same run under UBSan+integer and under TSan: 0 reports |
| Real prepare step on the GPU | `make -C dm/gfx/tests GAME=... check_t_pso_head` (runs it with `DM_PSO_MTL4=0` and `=1`) | **348 checks, 0 failed, both compilers.** Also under ThreadSanitizer (0 reports) and AddressSanitizer (0 errors), both modes, with the game's thread claiming and loading on demand beside the tail |
| Test sensitivity | the same test against a copy whose head end releases the dictionary and allows the sweep | 8 of its checks fail, as they should |
| Existing prewarm test | `make -C dm/gfx/tests GAME=... check_t_pso_warm` | 123 checks, 0 failed, both compilers (it calls `warm_finish()`) |
| Host build | `HOST_TESTS=1 scripts/build-host.sh` (a trial in scratch, then the real run into `dist/host/`; same results) | all **12 patches apply** (0 fuzz/offset lines in the script's log), chain id **`b24b25240cad` unchanged** (`dmsh_chain.h` = `dmsh_chain.py --header` = expected), 77 device / 72 simulator host objects compiled, 0 failed; the script's own verification passes: platform/minos as before (2 / 26.0 and 7 / 27.2), no export lost (3,331 -> 3,332 and 2,932 -> 2,933, the one added is 0411's `nvn_bc_decode_par`), device imports identical (770), every symbol the app resolves is exported (17 device, 15 simulator), the chain id once in the strings, the patch markers present; `DM_PSO_PREPARE_PERCENT` (6 strings) and `DM_PSO_PREPARE_TAIL` are in the device dylib too (checked by hand: the script's marker list does not know them). 259 s the whole script with the tests |
| Engine self-tests of section 3.1, again | the same 11 commands (`HOST-TESTS.txt`) | **11 of 11 PASS**, the same numbers as before 0412: host unit 173 `PASS` / 0 `FAIL` (100 s); CPU texture 5,386 of 5,386 checks; `rt_recycle_policy` 57; `gfx_suspend` 22; `lazy_clear_order` 21; `zero_colour` 30; `upload_idle` 27 and 46; `upload_src` 61; `pso_pending` 96; `cbs_fault` 21; `bc_cpu` 466 (all 0 failed). None failed before 0412 (the 2026-11-19 run: 11 of 11 PASS); the two tests that did not run then still do not (`t_geometry_mesh` does not compile in the base tree, `test_x18_select.py` needs `capstone`) |
| More tests of the code 0412 touches (not in 3.1's list; run once, on the built tree, no run on the 0411 tree to compare with) | `check_t_<name>` of `shmap_warm`, `pso_warm`, `pso_prewarm`, `pso_bg_look`, `pso_look_wait`, `pso_stall`, `pso_admission`, `pso_archive`, `pso_race`, `pso_depth_stencil`, `pso_bundle` | all pass: 12, 123 (x2 compilers), 64, 17, 9, rc 0, 16 + 16, 432, rc 0, 228 / 231, 83 checks, 0 failed (`pso_stall` and `pso_race` print no count) |

What `t_pso_head` shows (synthetic shader libraries compiled with the Mac's Metal toolchain, 40 tasks / 70 lines / 60 pipelines): with `PERCENT=50` the head is exactly tasks 0-19 (35 lines, 5 shader loads, 30 pipelines) when `warm_start` returns, the tail is held by a gate on the device's library loads and shows as not started, the dictionary is kept, the sweep is not allowed; released, the tail builds exactly tasks 20-39 in the manifest's order, nothing twice, on **one** worker (never two loads at once) at **utility** QoS (the head's workers ran at user-initiated), and only then the sweep is allowed and the dictionary released. With `DM_PSO_PREPARE_TAIL=0` nothing runs after `warm_start` returns, the dictionary is released, the sweep is not allowed. `g_warm_stop` and `dm_gfx_teardown` met mid-tail end it after the task under way (`dm_gfx_teardown` waited the 0.2 s the held load took) with no deadlock. A compacting start ignores the percentage. `PERCENT=1, 30, 51, 97` each build every task once; with one worker and slow loads the progress line reads "of 35 lines", never "of 70".

### 10.4 What changed in the host

`dist/host/host-prebuilt-loadable-ios.dylib` (device) and `-sim.dylib` were rebuilt by `scripts/build-host.sh` with the twelve patches:

| | device | simulator |
|---|---|---|
| size | 6,771,664 (0411) -> **6,773,592** | 4,526,848 (0411) -> **4,528,288** |
| sha256 | `1aaaaf7f27c9098585a1c0b64310ab4dff387e3aa4c016f1a0e96eb7193846e0` | `3139b043f196499d4e808cdb68337a7461c85aeeadee6aad3c554c2775471298` |
| platform / minos | 2 / 26.0 (same) | 7 / 27.2 (same) |
| exports / imports | 3,332 (same as 0411) / 770 (same) | 2,933 (same) / 635 (same) |

0412 adds no export and no import: its functions are `static`. The simulator host has no `nvn_mtl.m` (its object does not compile on the simulator SDK, as before): **0412 changes none of its code**, only the object paths in its symbol table, so its size and sha256 changed as any rebuild's do (section 2: not bit-reproducible). The device dylib is the only one that carries the two variables. The previous hosts are kept: `dist/host/previous/host-prebuilt-loadable-{ios,sim}-0411.dylib` (the hosts of section 2; sha256 `37f1b6b8f9f5a5326fe3c5e435abc38119b28846a3b7f076973f0fc589294a91` and `2a61e184329385bcc0e570bc6624022480e205b024a8a2e0d51eedbb7a1a79ac`) and, as before, `previous/host-prebuilt-loadable-{ios,sim}.dylib` (the worktree's original, `HOST-MANIFEST.txt`). The app is built with `GAMEAPP_HOST=<dist/host/...>` as in section 7; nothing was built here. `app/inputs.tsv`'s `host-*` rows will read `DIFFERS` until the new host is accepted.

### 10.5 What is NOT verified, and the risks

* **Nothing ran on a device.** Not measured: the head's or the tail's time on the phone, frame time while the tail builds, memory, the progress screen, a background / foreground during the tail, how the app sets the two variables, and the **pack's own manifest** (14,753 data rows per the owner; I did not have it).
* **Head by file order** is only meaningful if the manifest was recorded chronologically. The Ocarina file interleaves `S` runs and `R` / `C` rows (526 runs in the pack's, per the owner): consistent with a recording order, **not proven**. The engine tree's profile manifest for this game (a different file: 25,163 rows; a Python emulation of `warm_parse` without its shader-map test gives 23,191 tasks: 16,818 `S`, 5,166 `R` pairs with 7,138 pipelines, 1,043 `C`, 164 `M`) shows what "half" means there: its first fifth holds 4,613 library loads, 31 render pipelines and 11 compute ones; at 50 % the head holds 51.9 % of the lines but **47.6 %** of the render pipelines and **36.5 %** of the compute pipelines (25 %: 0.4 % of the pipelines; 75 %: 73.1 %). The split is by task count, as asked: `S` tasks are cheap, pipelines are not.
* **The tail still costs frame time.** The engine's own note: background builds raised an iPhone 17 Pro Max's GPU frame from about 13 ms to 15-18 ms in Breath of the Wild at user-initiated QoS. One utility worker should cost less: **not measured**.
* A draw that reaches a tail pipeline before the worker hitches once (it waits for its own compile).
* A killed process (jetsam) loses the tail's captures not yet written to the archive (the period under way: handed over at 60 s of age, then 120 s, ...; also at a suspend or a quit); the next start's head finds what was written and the tail goes on from the lists again.
* Each background / quit during the tail writes the period under way as a small archive file. More than 8 in one size tier make a later start a compaction, which builds **every** line before the first frame whatever the percentage.
* `prepare_prebuild` is untouched: on a cold first start it may remain the longest part of "Preparing" (10.2 d).

### 10.6 Use, and how to go back

* Use: `DM_PSO_PREPARE_PERCENT=50` in the game's `env.txt` (add `DM_PSO_PREPARE_TAIL=0` to leave the rest to on-demand builds). Look for `pipeline prepare: DM_PSO_PREPARE_PERCENT=50: head ...` and, in the background, `pipeline prewarm tail: ... finished` in `gameapp.guest.log`. Unset, nothing differs from section 1-9's host.
* Back: leave the variables unset (identical behaviour), or remove `engine-patches/0412-*.patch` and run `scripts/build-host.sh` again; or use the host of section 1-9: `dist/host/previous/host-prebuilt-loadable-{ios,sim}-0411.dylib` (the dylibs this document's section 2 describes: sha256 `37f1b6b8f9f5a5326fe3c5e435abc38119b28846a3b7f076973f0fc589294a91` and `2a61e184329385bcc0e570bc6624022480e205b024a8a2e0d51eedbb7a1a79ac`), `previous/host-prebuilt-loadable-{ios,sim}.dylib` being, as before, the worktree's original. `scripts/build-host.sh` was not changed: its marker list does not know `DM_PSO_PREPARE_PERCENT` (checked by hand: present in the device dylib's strings) and its `HOST_TESTS=1` list does not run `t_warm_head` / `t_pso_head` (run by hand, above); both are one line each to add there.

Files: `engine-patches/0412-nvn-prepare-head-then-background.patch` (touches `dm/gfx/nvn/nvn_mtl.m`, adds `dm/gfx/nvn/nvn_warm_head.h`, `dm/gfx/tests/t_warm_head.c`, `dm/gfx/tests/t_pso_head.m`, and two rules in `dm/gfx/tests/Makefile`), `dist/host/` (rebuilt: both dylibs, `HOST-MANIFEST.txt`, `HOST-TESTS.txt`, `previous/*-0411.dylib`), this section.


## 11. `engine-patches/0413..0415`: the learned list first, the MTLDevice pipeline archive in the engine, a tail that resumes and waits when hot (2026-11-21)

The device session of Ocarina of Time (Switch 2, `0400486024DEC000`) of 2026-11-20 (iPhone 15 Pro, `gameapp.guest.log`: two sessions, the second 44 min / 9,022 frames) and DeepSeek's reading of it (`../iBuilderRust-accelerations-2026-11-20.md`, `../iBuilderRust-leviers-detail-2026-11-20.md`) were checked against the log and the engine source. Sections 0-10 are unchanged; this one adds three patches. **Nothing here ran on an iPhone, and no app or IPA was built**; `dist/host/` was not touched (the hosts built for review are in `target/agent-work/eng/host-out/`, gitignored).

### 11.1 What the log says (re-measured with `target/agent-work/eng/tools/logstats.py`; no container path copied)

| Claim | Verdict | Evidence |
|---|---|---|
| 130 frames of 1 s or more, 108 caused by the host, 942 s blocked, 912 s of compiles | confirmed | `[longframe]` lines: 130 frames >= 1 s (1,381 s in all); 108 with submit+present >= half the frame: 942.1 s of submit+present, 912.4 s of compiles in them. 930 s of compiles over all long frames; the per-300-frame `pipelines up to frame N` lines sum to 580 s of pipelines and 350 s of shader libraries on the submit thread |
| frame 4342: 161 s for 262 libraries; frame 270: 118 s for 12 pipelines | confirmed, more precisely | 4342: `shader libraries 262, 161037.3 ms`, and 37 mesh pipelines built in it (`task / mesh pipeline ... required at frame 4342`, 50.0 s; 57 mesh pipelines, 92.1 s, in the session); 270: the window line says `compute pipelines ... built 11 (... 117511.4 ms building)`: eleven compute pipelines, about 10.7 s each (136 compute pipelines, 196.4 s, in the session) |
| head 2 % = 464 lines in 53.3 s on 6 workers | confirmed (session 1); 0.21 s in session 2 (cache hits) | `pipeline prepare: 464 lines (464 shaders, 0 render pipelines, 0 compute pipelines)`: the head held no pipeline at all |
| the tail (22,727 tasks, one utility worker) died with the process after 570 lines | session 1 only | session 1 ended by SIGTERM about 90 s in (`pipeline prewarm stopped ... (570 lines done)` = 464 + 106); session 2's tail restarted at the same task and reached 5,506 tasks in 2,237 s; the log then ends with no exit line |
| `archive lookups found 0`: arch_sweep deletes the archive | number confirmed, cause **refuted** | that counter is the Metal 4 archive's (`arch_lookup`), which iOS does not run (`render pipelines through the MTLDevice`: `pso4_on()` = 0, `pso4_init` returns first, `g_arch_use` stays empty, `arch_sweep` returns at its first test; `found none 0` in every window). 0412 already holds the sweep until the tail asked for every line. The archive iOS uses is iOSBuilder's paches/0009 MTLBinaryArchive: session 1 wrote none (SIGTERM's graceful exit ends with `_exit`, which skips 0009's atexit write, before its first write was due), session 2 started `(new)`, then `written, 3.3 MB: 4 pipelines taken from it, 472 built, 24 added, 451 dropped (queue full or too hot)` and `[RED] ... off for this run (Failed to generate machO ... missing 'vertex' stage in pipeline no. '32')` |
| `pso_user.tsv: 2 lines`: the learned list never grows | number confirmed, "not written" **refuted** | rows are appended (and flushed) the moment the game needs them, at the right path; the file skips every row the 25,163-line seed list holds, by design, and the prewarm read it after the seed. Changed by 0413 |
| thermal 2 all session | **refuted** | `on-screen ... thermal 2` up to frame 4200 (about 1,150 s), 0 for frames 4500-7800, 1, then 2 in the last two windows |
| clock 1.71 GHz against 3.6 | partly | 1.71-1.73 GHz in the last two `translation detail` windows, 1.14-1.57 before; it is the submit thread's cycles / CPU time, with 18-27 % of the process CPU on performance cores (`power` lines): mostly efficiency-core time, not a throttled performance core |
| 36,661 write-protection faults per window, up to 523 mprotect pages a frame, kernel 5.24 of 11 ms | partly; not a lever | the fault counter is cumulative (`(cumulative)`; about 4 a frame); `mprotect_pages` p50 8, p99 77 (13 frames above 523), `mprotect_ms` 0.73 s over the whole session; kernel 5.24 of 11.0 ms only in the window to frame 8700 (9.7 of 21.7 at 9000), 0.08-2.9 ms in the 27 others |
| 2,907 render targets / 2,782 MB released, 241 made again | confirmed (cumulative over the session) | 8.3 % made again; footprint 4,174 MiB at the end, where keeping more would raise the jetsam risk |
| residency commits 275-333 per 300 frames = 3.5-5.5 s | confirmed for the last two windows | 17.4 s over the session; 0-609 commits in the other windows, following the render-target churn and scene loads |
| `DM_QOS_MAP=1` moves the workers to the efficiency cores | **refuted** for this game | `CPU by guest priority <=44 4009 / 45-59 30 ms/s`: the workers are priority 44, which the default edges (44,59) keep user-interactive |
| compaction (> 8 files in a tier) builds every line before the first frame | true in the code, not on this phone | compaction is the Metal 4 archive's; iOS runs without it. 0414's archive is one file rewritten in place: no tiers, no compaction |

### 11.2 The patches

| Patch | What changes | Default | Switch back |
|---|---|---|---|
| 0413 `nvn-pso-user-list-first` | `pso_user.tsv` records every row the game asked for on this device (the seed's too), once, in first-need order, with one `write(2)` per row on an `O_APPEND` descriptor (a cut last row gets a line end first); the prewarm reads it before the seed list, so the split prepare step's head and its tail build first what this device drew | on | `DM_PSO_USER_FIRST=0` (the old list and order) |
| 0414 `nvn-device-pipeline-archive` | the MTLDevice path's MTLBinaryArchive in the engine (`dva_*`, and `#define dev_rps / dev_cps / dev_mps` to it, so 0009's code is compiled and never reached): render, compute **and mesh** pipelines; adds held back (never dropped) while hot, 8 a tick at thermal Serious; at most 512 waiting; written 60 s after the last write (or the size at 512 KB/s when longer) and at every end through `mtl_arch_save` (background, graceful exit, atexit), adding what waits first; a failed write goes on from the last written file (off after 3) | on where `pso4_on()` is 0 (iOS) | `DM_PSO_BINARY_ARCHIVE=0` (as for 0009); `DM_PSO_ARCHIVE_MB`, `DM_PSO_ARCHIVE_SECS` as before |
| 0415 `nvn-prewarm-tail-resume-and-heat` | the tail keeps how far it got in the seed list (`pso_tail.tsv`: the seed's size, time and chain; a `.tmp` renamed) and the next start's tail goes on from there (no sweep when it left rows out); its seed rows wait while the device is at thermal serious or critical (this device's rows go on); new `DM_PSO_PREPARE_TAIL=user`: the tail builds this device's rows only | resume and heat wait on; `user` only when set | `DM_PSO_TAIL_RESUME=0`, `DM_PSO_TAIL_HOT=1` |

Who owns the background work: the engine's tail runs only while the game runs (the app stops its own preparation at Play: `BeginRun -> StopWarm`, app patches 0070 / 0076) and loads what the game is about to need into the process (libraries, pipelines, and pipelines into the 0414 archive); the app's preparation compiles libraries into Metal's compile cache while no game runs. They never run at once; a library the app compiled is a 1-4 ms cache hit for the tail. Not changed in the app: GameWarmLogic.NeededKeys may read `pso_user.tsv` first to follow 0413, and 0076's run of every library fills a cache that kept about 5,500-5,800 of this game's libraries (DEVICE-FINDINGS): what it compiles first is what stays.

On-demand compiles on the submit thread (priority C): the engine already fans out a submission's libraries (`shader_prefetch_rest`, a parallel `dispatch_apply`) but only after a render-pipeline miss; frame 4342 had none (libraries and mesh pipelines), and the log has no `shader prefetch: ... loaded in parallel` line at all. Calling it at a submission's first program miss is a one-line hook in `shmap_load`, **not done**: it is the submit thread's hot path, and testing it needs synthetic NVN shader control sections. What 0413 / 0414 do instead: the rows (mesh ones too) drawn in a session come first in the next one's prewarm, and their pipelines are taken from the archive. The other lever is the app's `DM_PSO_NOWAIT=0` (`profile.rs`): the engine's iOS default (1) gives a draw at most 50 ms of a frame and builds the rest in the background (from HookLink / kid-with-sword-remake): an A/B on the phone, written in the game's profile file.

### 11.3 What was run (this Mac, Apple M1 Pro, macOS 27.3, Xcode 27.2; heavily shared with other jobs at the time: times indicative)

| Check | Command | Result |
|---|---|---|
| The series applies | `patch -p1 --forward --verbose` of `0401..0415` on a fresh clone of the shipped tree, then by `scripts/build-host.sh` | 15 of 15 apply, **no fuzz**; 0413 (5 hunks), 0414 (4), 0415 (10) with no offset either (0410 / 0411 keep their old offsets of 3 / 2 lines). Shader chain id **`b24b25240cad`, unchanged** |
| paches/0007, 0008, 0009 on top | `target/agent-work/eng/tools/apply789.sh` on the 0412 tree and on the 0415 tree, then the 0413..0415 hunks applied to the 0412 + 0007..0009 file | all three apply with no fuzz on both; `(0412 + 0007..0009) + 0413..0415` is **byte for byte** `(0412 + 0413..0415) + 0007..0009`: they commute, 0007-0009 land exactly where they did |
| Host build | `HOST_WORK=target/agent-work/eng/host-work HOST_OUT=target/agent-work/eng/host-out HOST_TESTS=1 scripts/build-host.sh` (310 s) | device 77 / 77 objects, simulator 72 (the same 5 MetalFX / MTL4 sources as always do not compile there); platform / minos 2 / 26.0 and 7 / 27.2; no export lost; device exports 3,331 -> 3,334: `nvn_bc_decode_par` (0411) and the two Objective-C ivar offsets of 0415's new `WarmTask` fields (`_OBJC_IVAR_$_WarmTask.src`, `.line`; the class's seven other ivars are exported the same way); simulator 2,932 -> 2,933; **imports unchanged (770)**; every symbol the app resolves exported (17 / 15). Strings: `DM_PSO_USER_FIRST`, `DM_PSO_TAIL_RESUME`, `DM_PSO_TAIL_HOT`, `pso_tail.tsv`, `iBuilderPSO2` present; 0009's `iBuilderPSO1` header string is **absent**: the compiler dropped 0009's unreachable code |
| The hosts built for review | `target/agent-work/eng/host-out/` (not `dist/host/`: the main agent installs a host after review) | device `host-prebuilt-loadable-ios.dylib` 6,764,296 bytes, sha256 `26b70a9a0e2cc5484aef6d6f4fe19fb85edfbc527ee34814c212f853f51a70d6`; simulator `-sim.dylib` 4,518,432 bytes, sha256 `8d23fc321641ffb67ce4d729953154c9094187dfad5cc5b90f27a31baeda3e47`; `HOST-MANIFEST.txt` and `HOST-TESTS.txt` beside them (patch sha256: 0413 `c312cbd9...`, 0414 `f968a1e2...`, 0415 `56db76ed...`). Not bit-reproducible (section 2): three builds of the same sources gave three device sha256 of the same size; the simulator host has no `nvn_mtl.m`, so 0413-0415 change none of its code |
| Engine self-tests of 3.1 | the script's `HOST_TESTS=1` list | **11 of 11 PASS**: host unit **173 `PASS` / 0 `FAIL`** (as before), CPU texture, `rt_recycle_policy`, `gfx_suspend`, `lazy_clear_order`, `zero_colour`, `upload_idle`, `upload_src`, `pso_pending`, `cbs_fault`, `bc_cpu` |
| The prewarm and archive tests (10.3's list) | `make -C dm/gfx/tests GAME=010015100B514000 check_t_<name>` on the 0415 tree | `warm_head` 455,404; `pso_head` 348 + 348; `shmap_warm` 12; `pso_warm` 123 + 123; `pso_prewarm` 64; `pso_bg_look` 17; `pso_look_wait` 9; `pso_stall` 110; `pso_admission` 16 (x7) + 10; `pso_archive` 432; `pso_race` rc 0; `pso_depth_stencil` 228 / 231; `pso_demand_record` 54 / 40 / 51 (+ reloads); all 0 failed, the same numbers as 10.3. `pso_bundle` 83 / 0 with Python 3.14 first on `PATH`, on the 0412 tree and on the 0415 tree alike (with the system's Python 3.9 its helper script `dmsh_side.py` does not parse, on both trees: not a patch's doing) |
| 0413 | `check_t_pso_user_first` (DM_PSO_MTL4=0 and =1) | **44 checks, 0 failed**, both: rows on / =0, a cut row, three writers killed with SIGKILL (about 155,000-180,000 rows, every one whole, none twice), eight threads at once (3,200 rows), `warm_paths`' three cases, `warm_parse`'s order, the real prepare step with DM_PSO_PREPARE_PERCENT=25 (head = the 10 tasks the list names, or the seed's first 10 with =0; the tail builds the rest once) |
| 0414 | `check_t_pso_binary_archive` | **11 sessions (processes), 0 failed, 115 checks**: quit, SIGTERM's graceful exit, and background + SIGKILL each leave a file the next process takes all five pipelines from (two render, a rasterization-off one, a compute, a mesh) and builds one it never saw; a header for another host build dropped, an unreadable file started again; heat / memory policy (Critical and low memory add none, Serious 8 a tick, nothing dropped for heat), the 512 cap, a failed write going on from the last file (written pipeline still found, the newer one built again) and off after 3; the Metal 4 path and DM_PSO_BINARY_ARCHIVE=0 do not use it |
| 0415 | `check_t_pso_tail_resume` (DM_PSO_MTL4=0 and =1) | **40 checks, 0 failed**, both: a tail stopped inside a pair keeps the line after the last whole task; the next start leaves tasks 10..20 out and builds 21..39, no sweep, keeps the end; the start after builds only the head; DM_PSO_TAIL_RESUME=0 and a changed seed walk all again; at thermal state 2 (a hook) this device's rows are built and the seed's wait until cool, a stop while waiting ends it, DM_PSO_TAIL_HOT=1 does not wait; DM_PSO_PREPARE_TAIL=user builds this device's rows only |
| Test sensitivity | `target/agent-work/eng/tools/mutate.py` (each mutation breaks one rule, builds and runs the test that guards it, restores) | all 7 mutations caught: M1 0413 the seed first again -> exit -14, 4 FAIL lines; M2 0413 no line end after a cut row -> exit 1, 3 FAIL lines; M3 0414 nothing added at Serious -> exit 1, 6 FAIL lines; M4 0414 an end does not add what waits -> exit 1, 25 FAIL lines; M5 0414 off at the first failed write -> exit 1, 6 FAIL lines; M6 0415 a task cut by a stop counted as walked -> exit 1, 3 FAIL lines; M7 0415 no wait while hot -> exit 1, 5 FAIL lines (M1 ends by the test's own 240 s alarm after its failures: in the wrong order the gate waits for a load that never comes) |
| Archive across processes, by hand | an MTLBinaryArchive probe (before the test was written) | render, rasterization-off, compute and mesh pipelines added, serialized, and all four found by a second process with FailOnBinaryArchiveMiss; a pipeline never added missed |

### 11.4 What is NOT verified, and the risks

* **Nothing ran on an iPhone.** Not measured: whether a second session on the phone takes its pipelines from the 0414 archive (on the Mac a second process takes all five kinds), the time of a lookup / an add / a write there, whether iOS's "missing 'vertex' stage" write failure (not reproduced on the Mac) comes back after a reopen, the head and tail's coverage of what a session draws, the heat effect of the tail's wait, the list's and the archive's size after a long session.
* The real game was not run on the Mac: the extracted Ocarina of Time folder of the earlier scratch area is gone (a reboot cleared it), the Mac runs the Metal 4 path and not paches/0007-0009, and a two-session run reaching 3D play is hours of work. `t_pso_binary_archive` stands in for "an archive written by one session is found by the next" with real render / compute / mesh pipelines across processes.
* 0414 replaces paches/0009 in behaviour through three `#define`s placed after 0009's code; a later revision of 0009 that hooks elsewhere than `dev_rps` / `dev_cps` would have to be checked against it. Every MTLDevice pipeline (the host's own too, as with 0009) is looked up first, and a write holds new compiles back for its length (the host's serialization gate, as with 0009).
* 0413 changes the head of a split prepare step for every game with a device list (by design). 0415 can leave out a seed row whose library Metal's cache has evicted since: it is then compiled when drawn, and recorded first for the next start.
* Roll back: the switches above, or remove the files from `engine-patches/` and run `scripts/build-host.sh` again (0415 needs 0413; 0414 stands alone).

Files: `engine-patches/0413-nvn-pso-user-list-first.patch`, `0414-nvn-device-pipeline-archive.patch`, `0415-nvn-prewarm-tail-resume-and-heat.patch` (they touch `dm/gfx/nvn/nvn_mtl.m`, `dm/gfx/tests/Makefile`, `t_pso_demand_record.m`, and add `t_pso_user_first.m`, `t_pso_binary_archive.m`, `t_pso_tail_resume.m`), `app/profiles/0400486024DEC000.env.txt` (comments only), this section.

## 12. The host of 2026-11-21: upstream nxnative fixes (0451..0462) and ideas from an MIT Ocarina port (0500..0505)

Built by `scripts/build-host.sh` (which now applies `engine-patches/0[45]*.patch`; `engine-patches/optional/` is never applied) on the shipped tree, with paches/0006..0009 on top, and installed in
`dist/host/` (the previous host is kept as `dist/host/previous/host-prebuilt-loadable-{ios,sim}-0412.dylib`; `app/inputs.tsv` is re-pinned). Shader chain id unchanged (`b24b25240cad`): no game's
shader library has to be rebuilt. **24 engine self-tests PASS** on this Mac (host unit 173, CPU texture, and the graphics tests listed in `HOST_TESTS`, now including 0412..0415's and the upstream
ones). Nothing ran on an iPhone.

* From nxnative after our base (`target/agent-work/up/REPORT.txt` has the table of the 186 upstream commits with a verdict each): 0451 residency staging retired off the feedback queue, 0453 a real-time
  deadline for the audio render thread (`DM_AUDREN_RT=0` off), 0454 short AudioOut tails (Bluetooth underruns), 0455 + 0456 empty-grid dispatches skipped and an interrupted compute compile retried
  (Ocarina's NDVE network), 0457 sparse descriptor lifetime (a crash), 0458 present start-up link rescue, 0460 allocation checked before pages are write-protected, 0461 stick diagonals within the
  Switch circle, 0462 the pipeline archive's timed write never blocks compiles (written on 0414's `dva_write`). 25 upstream commits that move the shader chain (including Ocarina shader fixes) are NOT
  taken: they would force every game's library to be rebuilt.
* Adapted from the MIT port kid-with-sword-remake (Ocarina of Time on iPhone, built on nxnative; `target/agent-work/kws/REPORT.txt`): 0500 `DM_NVN_POOL_SKIP_RETIRED` (off), 0501 the Texture Memory
  Saver `DM_NVN_TEX_MIP_SKIP` / `DM_NVN_TEX_MIP_LEVELS` (off: the look of the reduced textures is not checked), 0502 `DM_NVN_UP_BATCH_MB` (default unchanged), 0503 `DM_FPS_PAYBACK` (default unchanged),
  0504 a stall watchdog: 5 s without a frame while graphics run logs every guest thread's wait, up to 4 times (`DM_STALL_DUMP=0` off; a weak default of `svc_thread_dump_live` lets the graphics tests
  link without the host), 0505 a `[pools]` census line every 600 frames. The port's 50 ms per-frame pipeline budget and its render-target pacing were already in this tree (paches/engine/0203);
  the app had switched the budget off with `DM_PSO_NOWAIT=0`: for a seed list over 10,000 lines (Ocarina) the app now leaves it on (`crates/ibuilder-pipeline/src/profile.rs` `set_pso_nowait`).
* Held in `engine-patches/optional/` (not applied): **0450** (graphics suspend bounded to 3 s) moves the archive save of a suspend to a worker, and `t_pso_binary_archive` then shows the archive is
  NOT written before a SIGKILL that follows a backgrounding (iOS ends background apps that way): it breaks 0414's guarantee; **0452** (a nil pipeline function is an error, not a Metal abort) makes 9
  checks of `t_pso_pending` fail (its speculative compiles never reach the compiler); **0459** (Ocarina's 8 GiB alias, a 16.5 GiB reservation for the white screen at the Chamber of Sages) is an
  owner decision: not known to start on an iPhone 15 Pro without the extended-virtual-addressing entitlement.

## 13. Revision 2 of 0414 / 0415 and fixes in 0453, 0456, 0462, 0501, 0503, 0504 after the read-only review (2026-11-21)

The review `target/agent-work/review-eng/REVIEW.txt` found one HIGH and several MEDIUM problems in the series. Fixed in the patch files themselves (series still applies in order, no fuzz; paches/0007-0009 still apply on top):

| Finding | Patch | What changed | Test |
|---|---|---|---|
| H1: opening an MTLBinaryArchive costs a footprint that grows with the SQUARE of the file (Mac: 69 MB / 1,200 pipelines +367 MB, 103 MB / 1,800 +843 MB), never given back | 0414 | `DM_PSO_ARCHIVE_MB` 24 by default (was 256, bounded 1..4096) and new `DM_PSO_ARCHIVE_COUNT` 400 (count in the .hdr); never grown past either; a file past a cap (or of unknown count) is deleted unopened; the open is measured with `os_proc_available_memory()` (logged on iOS) and a drop past `DM_PSO_ARCHIVE_GUARD_MB` (150) turns the archive off and deletes the file | `t_pso_binary_archive`: count cap sessions, memory guard session (a test hook) |
| M1: a lookup missed (and recompiled) whenever an add held the lock | 0414 | a lookup waits up to 30 ms for an add, misses at once during a write; adds spaced by 1 ms | lookup through a 10 ms add hits, gives up after ~30 ms, misses at once during a write |
| M2: the background write was not protected, could run beside the tail's compiles, and added for 4 s first | 0414, 0415 | `dm_gfx_suspend(1)` pauses the tail (`mtl_warm_pause`); the background save writes what is added first, adds for 0.5 s, writes again, inside a UIApplication background task (found at run time) | background session: one task begun / ended, two writes; tail pause test |
| M3: a failed write lost every add since the last good one, forever if repeated | 0414 | the adds since the last good write are queued again; runs ending on a failed write are counted in `<archive>.fail`, after 2 the archive starts empty | re-queue and found again; 2 failing runs, then a fresh start |
| M4: an interrupted compute compile retried at every dispatch | 0456 | at most 3 interruptions per shader, then latched failed; logged 3 times | `t_dispatch_pipeline` 10 checks |
| M5-M7: 0501 (off by default) | 0501 | M7 fixed (no reduction for a texture with an alias pair); M6 logged; M5 not fixable in a contained way: enabling the saver prints a [RED] line naming the known limits | `t_mip_skip_limits` (new) 9 checks |
| L1, L3, L7 | 0414, 0415, 0453, 0503, 0504 | bounded archive variables; 250 ms wait steps; one on / off reader `dm/host/dm_envflag.h` for DM_PSO_PREPARE_TAIL, DM_PSO_TAIL_*, DM_AUDREN_RT ("on" used to turn it off), DM_FPS_PAYBACK, DM_STALL_DUMP | `t_env_flag` (new) 102 checks; `t_audren_layout` words |
| 0462 | 0462 | rebased onto 0414 revision 2 (clears 0414's write flag on its early return); no change of behaviour | |

Measured on this Mac: `HOST_TESTS=1 scripts/build-host.sh` into `target/agent-work/eng/host-out`: 24 of 24 self-tests PASS (host unit 173 `PASS` / 0 `FAIL`; `t_pso_binary_archive` 21 sessions, 198 checks; `t_pso_tail_resume` 55 + 55; `t_pso_user_first` 44 + 44, ...); `t_env_flag` 102, `t_mip_skip_limits` 9, `t_pso_bundle` 83 (Python 3.14), the prewarm / archive list unchanged. Nine mutations (one per fix) each make a test fail. Hosts: device sha256 `9fe94190943b2ed9cc760d1b4ca49984a98fb954ffb76b5429dfefb6279bf91d` (6,785,800 bytes), simulator `54908d08d757a96d648e22a564b0fc359b62df5119c514463cf927ca69c1da47` (4,519,536 bytes); against the host in `dist/host`: one export more (`mtl_warm_pause`), three Foundation imports more (`NSClassFromString`, `NSSelectorFromString`, `NSIndexSet`). `dist/host` was not touched. NOT verified on an iPhone: the real cost of an open there (the log line gives it), the background task's time, whether the "missing 'vertex' stage" write failure comes back. Not done: the review's L2, L4, L5, L6, L8.

## 14. The review's remaining LOW findings (2026-11-21)

Fixed in the patch files themselves (series still applies in order, no fuzz; paches/0007-0009 still apply on top; shader chain id `b24b25240cad` untouched). Patches edited: 0414, 0415, 0455, 0456 (Makefile hunk re-anchored), 0462 (rebased), 0501, 0504, 0505.

| Finding | Patch | What changed | Test |
|---|---|---|---|
| L2: `g_dva_state` read without a barrier at init | 0414 | `_Atomic int`, written with release and read with acquire (`DVA_ST` / `DVA_ST_SET`) | `t_pso_binary_archive` session `--race` (8 threads start the archive together; compile-time check of the type) |
| L4: 0501 details | 0501 | FIXED: a texture whose declared levels `tex_desc` cut is recognized as reduced (`tex_levels_made`); the lazy env reads are `dispatch_once`. NOT fixed (documented in the 0501 header and in its `[RED]` line): shifted levels for `textureLod` / `texelFetch` / `textureSize` (blurrier IBL cubes; a shader-side change that would move the chain), render-to and twin views of a reduced texture not checked | `t_mip_skip_limits` 18 checks (9 before) |
| L5: floats formatted from the frame hook | 0505 | `pool_census` prints whole MB and whole seconds | `t_pool_census` (new) |
| L6: stall watchdog | 0504 | decision in `nvn_stall_watch.h`: seconds with a compile on the submit path (counted in `ser_compile_enter` / `leave` when the thread is inside a graphics call or is the encode thread, `mtl_submit_compiling()`) or with the app not in front are not counted; the frame counter is reset by `nvn_hle_teardown`; a lock word that is one thread's handle and another's kernel ID is printed both ways (`dm_lockowner.h`) | `t_stall_watch` (new), `t_submit_compile` (new) |
| L8: second game in the same process | 0415, 0504 | trivial part only: the learned list's file, its seen set, the tail's seed index and the frame counter are reset at teardown. NOT done: the 0414 archive and the prewarm's lists (`g_warm_started`) | `t_session_reset` (new, runs `dm_gfx_teardown`) |
| `t_dispatch_zero` did not compile | 0455 | a Makefile rule with `-fobjc-arc` | `make check_t_dispatch_zero` |

`scripts/build-host.sh` runs the new tests in its self-test list (`dispatch_zero env_flag mip_skip_limits pool_census stall_watch submit_compile session_reset`).

Measured on this Mac: `HOST_TESTS=1 scripts/build-host.sh` into `target/agent-work/eng-lows/host-out`: 31 of 31 self-tests PASS (host unit 177 `PASS` / 0 `FAIL`; `t_pso_binary_archive` 22 sessions; `t_stall_watch` 26; `t_submit_compile` 11; `t_session_reset` 9; `t_pool_census` 12; `t_mip_skip_limits` 18; `t_dispatch_zero` passes). Hosts: device sha256 `fe9aaa100de057980a3ff5525d44949e9241512d501679fbeb8c69c7c91598ad` (6,804,264 bytes), simulator `6c5f1f4314ed7a241ffdb66e1ea1754b0ff6bebe2a442290150e52c92b30275c` (4,520,448 bytes); no export lost. `dist/host` not touched. Not run on an iPhone.

## 15. Host of 2026-11-22: 0506–0509 (South Park field report)

Field report (iPhone 15 Pro, South Park: The Fractured But Whole, provisional profile): the picture alternates image / black for ~2 s, then jumps ahead; audio and the game go on. The guest logs show presents steady at 30 Hz but stretches of frames with no draw (all attachments loaded, nothing written). Investigation in `target/agent-work/southpark/REPORT.txt` (not in the repo's docs: it quotes field logs).

| Patch | What | Default / switch |
|---|---|---|
| 0506 | a vertex stream whose bound size runs past its pool's end is bound up to the end instead of skipping the draw; every skip counted (`@vbuf_skips`) | on; `DM_NVN_VBUF_CLAMP=0` = old skip |
| 0507 | a texture the game has presented keeps `MTLStoreActionStore` when loaded and not written (Switch semantics: presenting without drawing shows the old picture); every present classified (drawn / not written / never written / replaced), summary every 300 presents, `@pres_unwritten` | on; `DM_NVN_PRESENT_KEEP=0` |
| 0508 | every early return of the draw path counted by reason (15 reasons), the game's own draw records counted, a `[RED] N presents in a row encoded no draw` line after 3 such presents (says whether the game issued none); `@draw_ops`, `@drops`, `@drop_why`; the Metal HUD on iOS through the game layer's `developerHUDProperties` (DM_METAL_HUD reached no code in GameApp) | counting only, no draw dropped differently |
| 0509 | the MTLDevice pipeline archive (0414) leaves out pipelines without a fragment stage on iOS (one such pipeline made every write fail: "expecting 'fragment' stage"); `DM_NVN_RENDER_COND=0` ignores render conditions (draws always run), still counted | archive filter iOS only (`DM_PSO_ARCHIVE_NOFRAG`); render conditions evaluated by default |

Self-tests added to `scripts/build-host.sh`: `vbuf_clamp present_keep drop_reasons render_cond_switch pso_archive_nofrag` (36 PASS on an M1 Pro). Shader chain unchanged (b24b25240cad). NOT run on a device: the next South Park log should say which drop reason (or none) empties those frames.

**Revision (2026-11-22, later):** 0506 replaced by `0506-nvn-vbuf-mapped-bytes-upstream-plus-pool-end-fallback.patch`: upstream nxnative c96c45e9 + 79555b67 (bind the bytes a draw reads, proven from the draw's vertex/instance/index ranges) with iBuilder's fallback when that proof cannot be made and the stream starts in a pool (bound up to the pool's end; a draw proven to read past the pool stays skipped). `DM_NVN_VBUF_CLAMP`: 0 = upstream exactly, 1 = fallback (default), 2 = the first 0506 (bind anyway). Field result of the first 0506 on South Park (iPhone 15 Pro): no frame without draws in ~3 min, flicker gone. 0507–0509 rebased (context only). Self-tests `vbuf` and `pull` (upstream's) added: 38 PASS. Not taken: upstream e74c3fa7 (South Park 1.05 game table; moves the shader chain).
