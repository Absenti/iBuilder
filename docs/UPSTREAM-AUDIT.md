# Upstream audit: seedds/nxnative and LNPANDY/NX2Apple against the engine that runs on the phone (2026-11-19)

Scope: what upstream `seedds/nxnative` and its fork `LNPANDY/NX2Apple` carry that the phone host lacks and that matters for the iPhone host, with a verdict per item,
the iOSBuilder patches classified against upstream, the Mario Kart 8 Deluxe findings, and a port order. Read-only audit: no engine, host, app or IPA was built, nothing was
applied to a real engine tree; the upstream clones were made in a scratch folder (partial clones, deleted afterwards) and only GitHub GET calls were used.
All dates are 2026-11 and in the committer's own offset (upstream: +0800 unless noted).

## 0. Read this first

1. **The host on the phone is not the 2026-11-16 snapshot.** The brief (and `docs/DECISIONS.md` trap 41, written before the rebase of trap 50) say the prebuilt host comes from the engine pinned in
   `iPorterNX/third_party/NX2Apple` (nxnative `94a0944`). It does not. `out-stage/out-engine` is a clone of `out-nx2apple/engine`, which delivery #33 rebased by hand onto nxnative
   **`a3663659` (2026-11-18 18:25)** with 54 NX2Apple patches (section 1). The "163 commits behind" of `docs/PERF-RESEARCH.md` was true of the old snapshot; the host is now
   **62 commits behind nxnative main** (237 behind for the old snapshot, which the main checkout's `engine` symlink still points at).
2. **The Mario Kart 8 Deluxe black screen / 0 fps is not a missing upstream fix.** In all four MK8D launches the host logged `[RED] cannot open the standard shared font`: the app
   ships no shared-font stand-ins, `nn::pl::GetSharedFontLoadState` then answers "not loaded" for ever, and the game waits. The `[RED] pipeline prewarm: seed list does not exist` is a second
   app-side gap (the bundle's `pso_manifest.tsv` never reached `Documents/shaders/<id>/`). Both are data/configuration, both already have a fix in this repository's Rust app code
   (not yet seen on a device). The upstream fix for the black screen that issue #281 reported (`0963959f`, the notification deadlock) is in the host (section 4).
3. Eleven patch files (nine from nxnative, two NX2Apple patches) were written to `engine-patches/`, all checked with `git apply --check` and applied in series on a copy of the real patched
   tree (section 5). Nothing was compiled.

## 1. Provenance of the host that runs on the phone

| What | Identity | How it was established |
|---|---|---|
| Prebuilt host | `$IOSBUILDER_WORKTREE/out-load-ui/host-prebuilt-loadable-ios.dylib`, 2026-11-19 02:39 | its strings contain `DM_PSO_NOWAIT` (paches/engine/0203) and `iOSBuilder paches/0008`; the log's `shader chain b24b25240cad` equals `DMSH_CHAIN` of `out-stage/out-engine/engine/dm/gfx/nvn/dmsh_chain.h` (the unpatched clone says `cf3be507e0eb`, upstream main `fded82111357`) |
| Patched tree | `out-stage/out-engine/engine` = `tools/patched_engine.sh` output, stamp 2026-11-19 02:28: source HEAD `75c7336` + `paches/engine/{0103,0104,0202,0203,0205,0206,0301}` | `out-stage/out-engine/stamp.txt` (first line `75c7336105c9d5b3674aa84e030ff69da3e35381`) |
| Engine source | `out-nx2apple/engine`, git HEAD `75c7336` ("NX2Apple: generated files", 2026-11-18 16:26 +0100), clean; `HEAD~55` is nxnative **`a3663659342ef43afe1f29909c7c36d0cf7555be`** (2026-11-18 18:25, seedds, "chain/ampere: translate VABSDIFF (PR #2053)"), under 54 NX2Apple patch commits (`dm/fork_patches.txt`) | git log of that tree; `out-nx2apple/.upstream-ref` in the working tree reads `a3663659` (committed value still `94a0944`, the edit is uncommitted) |
| NX2Apple wrapper | `out-nx2apple` HEAD `7541e9e` (PR #1405, 2026-11-17 23:29) | `git log` there |
| Independent content check | of the 1,799 files under `dm/` (not `dm/games`) at `a3663659`, **1,655 are byte-identical** in the patched tree (the other 144 differ, as the 54 + 7 patches would make them); only 1,310 match `94a0944` and 1,600 match upstream main | blob hashes (`git hash-object` against `git ls-tree`) |
| Not what runs | `iOSBuilder/engine` -> `iPorterNX/third_party/NX2Apple/engine`: nxnative `94a0944` (2026-11-16 00:07, PR #2115) + 56 patch commits; wrapper `3739300` (PR #1365, 2026-11-16 06:13) | `engine/.git`; `diff -rq` against the patched tree: 959 files differ; `a3663659` has 175 commits that `94a0944` lacks |

Upstream compared against (GitHub, fetched 2026-11-19 evening):

| Repo | HEAD | Commits ahead of ours |
|---|---|---|
| `seedds/nxnative` main (MIT, Copyright 2026 seedds) | `34ace8cc` 2026-11-19 23:29 "game: default Persona 5 Tactica to 60 fps" | **62** since `a3663659` (237 since `94a0944`) |
| `LNPANDY/NX2Apple` main (MIT; also Copyright jhaizhou and NX2Apple contributors) | `3f431865` 2026-11-19 22:41 (PR #1455); pin `.upstream-ref` = `0b5fd602`; latest release `v1.5.0-rc9` (2026-11-19) | **90** since `7541e9e` |

NX2Apple stopped taking engine patches on 2026-11-18 (`1b4b9cea`, PR #1433: "we take no engine patches, send them to seedds/nxnative"). Its queue is 57 patches against the new pin; it carries
no engine work that is not either fork-only (BC decode, shell, launcher) or on its way upstream.

Upstream has **no registered profile** for Grizzy and the Lemmings (0100DFA027504000) or South Park: The Stick of Truth (no directory in `dm/games`, no issue or PR mentioning either in either repo);
both run on provisional (derived) profiles, so nothing upstream can fix them directly. Upstream's profile for Mario Kart 8 Deluxe is unchanged since our base; the one for Ocarina of Time changed by 5 README lines.

## 2. Relevant upstream and NX2Apple changes since our base

"Present in our host?" was checked in the patched tree: for a code change it means the upstream hunks are absent (and `git apply --check` of the upstream diff succeeds or conflicts only where
shown); for an earlier fix it means the commit is an ancestor of `a3663659` and its marker is in the tree.
Actions: **port** = engine patch + host rebuild (needs the user's approval); **data** = configuration or bundle content only; **later** = take at the next engine bump; **ignore**.

### 2a. Landed in nxnative main since `a3663659` (62 commits; 27 are listed below, the rest are game profiles / maps / READMEs, CI, OpenGL, Mac HID)

| Id | Summary | Area | Games | In our host? | iOSBuilder / NX2Apple patch overlapping | Action | Risk |
|---|---|---|---|---|---|---|---|
| `79f28b06` (PR #2983, fixes #2981) | audren: a device callback that frees several 5 ms blocks no longer triggers back-to-back renders; fresh renders spaced 4.5 ms apart after prefill, catch-up output rechecked after the wait | audio (audren) | Animal Crossing NH, Binding of Isaac (1,227 -> 1 catch-ups per 6,000 renders); **every game on this phone**: the MK8D logs show 467 catch-up renders per 2,000 renders at 480-frame callbacks, 963 at the next line | **no** | NX2Apple 0048 (teardown in `hle_audren.c`): 3 hunks conflict, resolved by hand | **port** `0401` | low-medium (the render thread now holds `cq_mu` across the SDK render) |
| `943a6111` | render-target heap headroom capped at 4 MiB (a 296 MB shadow target sat in a 333 MB heap) | memory | Ocarina of Time (Switch 2) | no | NX2Apple 0055 (`rh_alloc_size`), iOSBuilder 0202 | **port** `0402` (adapted to 0055) | low |
| `1d1c5b4a` | zero fills and scissored clears copied in bands that fit the 4 MiB shared buffer (it had grown to 64 MB) | memory | Ocarina of Time | no | none; applies on top of 0202/0203 | **port** `0403` | low-medium |
| `93c76c1c` (PR #1491) | upload staging: trim completed idle blocks, opt-in (default 0 on every platform) | memory | OoT on iPhone 17 | no | 0202 (staging block sizes) | port `0404`, optional | low |
| `31fdefeb` (PR #2270 finding) | bound the zero-copy upload sources per batch, opt-in (732 MB in 300 frames on an iPhone 17) | memory | OoT | no | 0202; keeps our `!bc_cpu` guard | port `0405`, optional | low-medium |
| `5012821e` | pipeline look-ahead no longer resolves a future sparse attachment (a CPU upload ended the pass; Metal encoder crash, MKW start) | Metal compile | all games with `pso_mtl4` | no, and inert here: the phone runs the MTLDevice path (`pso4 0`, "render pipelines through the MTLDevice") | 0203 | port `0406` as a guard for when `pso_mtl4` is turned on | low |
| `74d17535` (PR #2938) | composed-mesh arguments checked live: no false "zero GPU addresses" reports | mesh shaders | Donkey Kong Bananza (Switch 2) | no | none | port `0408` | low |
| `4b92e232` (PR #2878) | Npad style read under the bind lock (a bind could miss its first signal) | controllers | all | no | NX2Apple 0027 touches the same file | port `0407` | low |
| `985ea6cf` | lone Joy-Con through `GCControllerLiveInput`, same player / press-to-join / applet routing as gamepads (refs #2971) | iOS input | Kirby, any single Joy-Con | no (we carry the #2971 patch instead) | **supersedes `paches/engine/0103`**; 4 conflicts against NX2Apple's `DmGamepad.m` (11 against the patched file with 0103 on; 0012/0013, applied later, add more) | later (keep 0103 until the bump); upstream says phone validation is still open | medium |
| `0b5fd602` | rewriter: a conditional select on x18 no longer spills two registers over live stack (Sunshine PCM corruption) | **rewriter** | Super Mario 3D All-Stars (Sunshine), Binding of Isaac; any module with the pattern | no | `patch_nso.py` also carries 0002, 0004, 0005, 0301: they apply on top | **port** `0409` **and** port to `ibuilder-rewrite`, then re-record the byte-compare oracle | medium |
| `c84297cf` (PR #2955) | movie SDKs that number NV12 colour spaces differently: report the right matrix/range | movies | Layton, Kirby Star Allies | no | none (also edits `patch_nso.py`) | ignore unless MK8D's menu previews show raised blacks | low |
| `b83913bc` (PR #2974) | C Opus calls forwarded on C++ `HardwareOpusDecoder` objects | audio | games using the class | no | none | ignore (none of the four games) | - |
| `e7dc2d9a` | offline network accounts answer "cancelled" (a loading screen called `EnsureNetworkServiceAccountAvailable` once a frame) | host account | Taiko Rhythm Festival | no | none | later | low |
| `79ac8c89` (PR #1508) | Polish and Thai as game languages (Switch 2 codes) in the NACP tools and the host | pack / settings | Switch 2 titles | no | none | data: check `ibuilder-pack`'s NACP language table accepts `pl` / `th` | low |
| `e22744db` (PR #2845) | font tools accept `.ttc` sources | build tool | - | no | - | data: only if `STD_FONT` is a `.ttc` | none |
| `538da5d9` (PR #2027) | replay clock kept out of relative waits | `DM_DETCLOCK` harness | MKW | no | NX2Apple 0048 conflicts in `hle_thread.c` | ignore | - |
| `3d8fc824`, `4815cbbf`, `398fc046`, `ac3fc4d7` | Metal HUD before UIKit; Xcode resource staging; AppleDouble NRO sidecars; Mac shader image names | engine's own iOS shell / Mac builder | - | no | none | ignore (not linked in GameApp / Mac-only) | - |
| 7 chain commits: `760c7fe8`, `9f834e41`, `c311523c`, `e0c30043`, `b989137a`, `5df3dbe8`, `736a2ecb` | shader split formats (PACx, TT Games DAT, UbiArt IPK, BAHS validation), faster guard scan, **three libraries compiled together**, raw-load pool checks | shader chain | none of the four games (MK8D's map is unchanged by `5df3dbe8`) | no | 0104 (`shader_split.py`) | **ignore until a game needs a split format**: each moves `DMSH_CHAIN` (now `fded82111357`), every game's shaders are translated again (Grizzy: 184 s), and the Rust shader stages' byte-compare oracle must follow | high |

Everything else in the 62 (about 19 game profile / map / README commits, 7 CI, lint and test commits, 5 OpenGL commits, 2 Mac HID commits, and the MT Framework split `c931e584`) is **ignore**.

Verified present already (all ancestors of `a3663659`, all relevant to the games in the brief): `0963959f` startup deadlock in blocking notification waits (2026-10-26; `native_wait` is in `hle_thread.c`),
`6ec201c7` + `c443ba1d` MK8D profile and 4.0.1 registration, `15316fd9` / `28126750` / `51492a90` Mii database and stand-in, `422ec240` (PR #2674) prewarm pipelines bundled in IPAs, `60284e79`
(`DM_PRESENT_LATCH`, PR #2757), `960d203e` look-ahead admits every later draw's pipeline, `04ba572d` far-plane depth bias (MK8D Sky-High Sundae), `fae1db03` stop on a 32-bit ARM version (`check_aarch64`),
`8ef752a2` merged NSP/XCI with base + update (`nx_extract` takes one package as both), PR #1315 `DM_GUEST_FILE` (the log prints `DM_GUEST_FILE unset: anonymous`), the audren SDK-4 DeviceSink (`g_sink4`, the log prints it).

### 2b. NX2Apple since `7541e9e` (90 commits, 57-patch queue)

| Id | Summary | Area | In our host? | Action | Risk |
|---|---|---|---|---|---|
| patch 0063 (issue #446) + 0064 (issue #777) | BC buffer-to-texture copies decoded on the CPU, BC decode split across cores (BC7 2048x2048 19.8 -> 2.7 ms; A15: largest load frame 132.6 -> 76.6 ms) | texture upload | no; **apply as is** (`engine-patches/0410`, `0411`) | optional: only a GPU **without** BC runs it (A16 and older; the engine refuses a device that is not A17 Pro or later unless the profile says `tex_bc_cpu`). Inert on an iPhone 15 Pro. Fork-only, upstream declined (#488) | low |
| patch 0062, 0065, 0066, 0067 | Mac launcher save, two test stubs, iOS framework copies as clones | Mac / build | no | ignore | - |
| refreshed 0039, 0041, 0046, 0050, 0053 | rebased onto the new pin | iOS shell | our hand rebase carries older copies | ignore (the shell is not used by GameApp) | - |
| dropped from the queue: 0014, 0023, 0044 (and 0024, 0025, 0057, 0059 that our rebase already dropped) | NX2Apple removed them at the pin bump `69277522` because upstream has the behaviour | Joy-Con pair (Mac), nsd services, Joy-Con haptics | our tree still carries 0014, 0023, 0044 on top of a base that has the upstream versions (`25cb950e` nsd, PR #1199) | check once for double hooks before the next bump (it builds, so no duplicate symbols) | low |
| PR #1438 (open) | iOS `DM_AUDIO_MIX=1` crackles with 1024-frame RemoteIO callbacks: lock-free host slot prebuffering in `hle_audio.c` | audio | no, not merged | track: only matters when the player turns on "mix with other apps" (the row exists in the launcher settings) | low |
| PR #1412, #1400, #1453, #1403 (open) | pipeline archive built for the phone's GPU; raw-HID fallback; G8 MFi rumble; touch layout | builder / input | no | ignore (our app has its own pipeline cache and touch layout) | - |

### 2c. Not landed upstream (open PRs / issues), worth watching for this phone

| Id | Subject | Why it matters here |
|---|---|---|
| nxnative #2884 (issue) | iOS: frames standing in the display link's queue (NX18 `present_trim` measurements) | input latency; our `DM_PRESENT_DRAIN/LATCH` (0206) is the same area |
| nxnative #766 (issue) | iOS: only BotW sets `pso_mtl4`, other games build pipelines on the submit thread | the cause of the "hybrid pipelines off" line and of first-session stalls; our 0203/0009 work around it |
| nxnative #1971, #2270, #2566 (PRs, open); #2966 (issue) | Ocarina of Time on iPhone 17 / 18 Pro: file-backed guest memory without freezes, smaller textures, residency census; guest memory grows until a crash on 8 GB devices | the Ocarina memory ceiling; the four landed commits above are the first slice |
| nxnative #2772, #2965, #2444 (issues) | iOS `DM_GUEST_FILE` stalls (Star Fox); TotK iPhone: prewarm of a large `pso_user.tsv` exhausts virtual address; BotW iPhone abort loading a save | iOS memory / prewarm hazards of our 0009 archive and prewarm design |
| nxnative #2705, #2677 (PRs, open) | "OoT-30 B/C": snapshot presents on their own queue, predicted-key base lookups in the background | OoT pacing on the phone |
| nxnative #2967, #2582, #2279, #2841 | frame generation (kid-with-sword-remake L6.6 on main; ring slot fix; scene-depth version; 2x / 1.5x scope) | our 0205 is the older cheemu21 / HookLink version: diverging, will conflict if these land |
| nxnative #357, #1103 | thermal cap as a launcher setting; kernel memory entitlement triplet (`DM_ENTITLEMENTS=distribution`) | thermal (PERF-RESEARCH 8) and the memory ceiling |
| nxnative #2890, #2909, #1326 | OoT GPU hangs (A20 Pro iPhone Duo, M2) | Switch 2 / mesh shader risk |
| nxnative #843 (open) | South Park: Fractured But Whole 1.05 profile + bare-zlib Snowdrop shader caches | our `paches/engine/0104` is its generic part; still unmerged |
| nxnative #1423 (open) | proposal: compute-based tessellation for every game | tessellation (no landed change since our base) |

There is **no landed upstream change since the base** for: thermal handling, MetalFX / frame generation, tessellation, Switch 2 Ampere shader semantics (all the Ampere PRs #2042-#2053 are in the base),
present pacing, AV1 / movie decoding beyond `c84297cf`, and 32-bit programs (issue #583 closed "not planned": Apple silicon has no AArch32 and the host assumes 64-bit pointers).

## 3. iOSBuilder's own patches against upstream

Method: each patch's files and header were read; "forecast" is `git merge-file` of upstream's whole base -> main change into the patched file (conflict count); the 9 new patches and
`paches/0002 0004 0005 0006 0007 0008 0009 0012 0013` were applied in series on a copy of the patched tree: **all apply**.

| Patch | Change | Class | Upstream / NX2Apple overlap |
|---|---|---|---|
| `0001-tools-main-public` | `Main` public in the two C# tools | (iii) unique | `Program.cs` of both tools untouched upstream in the 62 commits |
| `0002-python-no-fork-no-execve` | `patch_nso.py` without fork / execve on iOS | (iii) unique | forecast 0 conflicts with upstream's `patch_nso.py`; applies after 0409 |
| `0003-metal-layer-simulator-guard` | simulator-only guards in 4 Metal files | (iii) unique | files untouched upstream |
| `0004`, `0005` rewrite memory | spin-search memory, no image copies | (iii) unique | applies after 0409 |
| `0006-guest-as-at` | `DM_AS_AT`: the guest block at an address the app chose | (iii) unique | `hle_svc.c` untouched upstream |
| `0007-msl-source-at-runtime` | Metal source libraries compiled on the device | (iii) unique | upstream ships offline libraries; `b989137a` rewrites nearby library code (`nvn_mtl.m` +26 -10): expect a moved hunk |
| `0008-live-shaders` | shaders missing from the library translated while the game runs | (iii) unique | none |
| `0009-pso-archive` | per-game `MTLBinaryArchive` on the MTLDevice path | (ii) diverging, complementary | upstream's archive exists only for `pso_mtl4` (`DM_PSO_ARCHIVE`) |
| `0012-touch-layout-hook`, `0013-controllers-players-remap` | player-arranged touch layout, per-controller button map | (iii) unique | `DmGamepad.m`: forecast **11 conflicts** (upstream `985ea6cf` against the file with 0103 on; 0012 / 0013 are applied later and add to it) at the next bump |
| engine `0103-issue-2971-lone-joycon` | lone Joy-Con via `GCController.input` | **(i) superseded** by `985ea6cf` (2026-11-19, same approach, same issue) | keep until the bump; upstream's version is not validated on a phone either |
| engine `0104-upstream-pr843-snowdrop-bare-zlib-shaders` | `shader_split.py` generic part of PR #843 | (iii) not upstream (the PR is open) | upstream's `shader_split.py` +376 -64: forecast 0 conflicts with 0104 on |
| engine `0202-gameapp-low-memory` | `DM_LOW_MEMORY` | (iii) unique | same area as `943a6111`, `1d1c5b4a`, `93c76c1c`, `31fdefeb`: they apply on top of it |
| engine `0203-perf-nowait-rt-share-queues` | `DM_PSO_NOWAIT`, `DM_RT_SHARE`, queue sizing (HookLink `6255076f2c`) | (iii) not upstream (`DM_PSO_NOWAIT` / `DM_RT_SHARE`: 0 hits in upstream's `env_switches.tsv`) | `5012821e` applies on top |
| engine `0205-framegen` | MetalFX frame interpolation with estimated motion (`fgmv.metal`) | (ii) diverging | upstream's open PRs #2967 / #2582 / #2279 are another implementation |
| engine `0206-live-low-latency` | in-game Low latency switch | (iii) unique | `DM_PRESENT_LATCH` itself is upstream (`60284e79`) |
| engine `0301-rewrite-threads` | `DM_REWRITE_JOBS` threads | (iii) unique | forecast 0 conflicts with `0b5fd602` |

Nothing in upstream or NX2Apple conflicts with these patches **except** `DmGamepad.m` (0012, 0013, 0103 against `985ea6cf`), which is the one file to plan around in a bump.
Forecast for a whole bump to `34ace8cc` (conflicts per file, patched tree vs upstream delta): `nvn_mtl.m` 2, `DmGamepad.m` 11, `hle_audren.c` 3 (solved in 0401), `hle_thread.c` 2, `front_ios.m` 1, the rest 0.

## 4. Mario Kart 8 Deluxe on the iPhone (device logs, 2026-11-19)

Evidence: a device `gameapp.guest.log` (not kept in this repository), lines 2425-3134: four launches (pids 2477, 2480, 742, 745), iPhone 15 Pro (`iPhone16,1`, "Apple A17 Pro GPU"), profile `0100152000022000` from
`Documents/profile.tsv`, SDK `nnSdk-4_4_0`, three modules (`main subsdk0 sdk`: the 64-bit 4.0.x build, not the AArch32 base), shader chain `b24b25240cad`, 29,650-entry map.

What the logs show, in every launch:

| Observation | Meaning |
|---|---|
| `>>> nnMain`, RomFS mounted, swap chain, Mii database, audren renderer open, shader libraries compiled from source (3 in the short launches, 500 in the two longer ones: 5.1 s and 0.7 s), always "0 times a draw found one still compiling" | the guest boots and creates its programs; **no draw ever reached the host** |
| `[RED] cannot open the standard shared font` (4 of 4), and no `shared font: ...` line (that line is printed by the first `GetSharedFontAddress`) | `hle_sdk16.c`: `font_get(0)` finds neither `$DM_FONT_DIR/standard.ttf` nor macOS's Arial Unicode (absent on a phone), so `GetSharedFontLoadState` returns 0 and `GetSharedFontAddress` NULL: a game that polls the load state (this one, SDK 4) never leaves the loop |
| no `segmented submission`, no `timing up to frame 300` / `longframe` line, no `oe focus state` / `oe notification popped` line (Grizzy's session in the same file prints `timing up to frame 300` lines) | no command buffer was ever submitted: frame 0 never ends |
| `[RED] pipeline prewarm: seed list does not exist: Documents/shaders/0100152000022000/pso_manifest.tsv`, then "this device's user list does not exist yet" | the registered profile has `pso_manifest.tsv` (1,518 lines: 722 shaders, 596 render pipelines); the app's pack did not get it. The host copes (it logs and skips), but there is no "pipeline prepare ... before the first frame" step, so once the game draws, every pipeline compiles on the submit thread |
| `hybrid pipelines off: a draw whose pipeline is not built yet waits for it` | not an error: `pso_mtl4` is off, so the phone takes the MTLDevice path (nxnative #766) |
| `audren: ... catch-up renders so far 467 / 963 / 486`, `device callbacks ... 480-480 frames` | the burst-render signature fixed by `79f28b06` (section 2a); silent here, it matters once the game plays sound |

Which upstream commits explain or fix a black screen / 0 fps in this situation, and does our host have them:

| Upstream item | What it is | In our host? |
|---|---|---|
| nxnative issue #281 (iPhone Duo comment: "black screen at 0 fps, no frame drawn", log ends after `oe notification popped by the game: 15`) | MK8D 4.0.1 stops at boot | its cause was **`0963959f`** (2026-10-26, startup deadlock: `PopNotificationMessage` parked a thread the scheduler still counted as runnable; the reporter confirmed an iPhone Duo then boots, shows menus and runs a 3-lap race at 60 fps) - **in our host** (`native_wait`, `hle_thread.c:166`). Our logs stop earlier than #281's, before any `oe` line, so this is not that deadlock |
| engine doc "Shared fonts on iOS" (`docs/reference/services.md`): "on a phone `GetSharedFontLoadState` would report not loaded ... the iOS app ships the stand-ins in `dm/font/` and sets `DM_FONT_DIR`" | the cause seen in our logs | the host code is there; **the app does not supply the data**. `scripts/build-app.sh` in this repository now builds `dm/font/` with the engine's `make_shared_fonts.py` and `ibuilder-pipeline/src/profile.rs` sets `DM_FONT_DIR`; the current IPA has not been run on the phone |
| engine doc "The device's prewarm list unread without a seed list" and `422ec240` (bundle prewarm pipelines in IPAs) | a missing seed list is logged and skipped, the device list is still warmed | in the host; the seed list is the app's to copy. `pipeline_spec.rs` ("the bug: Mario Kart's pack was built when the bundle held only game.tsv") already tops up an existing pack |
| nxnative #578 / `fae1db03`, #583 | a 32-bit ARM program cannot run: #583 closed "not planned" (Apple silicon has no AArch32, iOS no run-time code generation) | the 1.0.0 base's `main` (AArch32) is refused by the Rust `preflight` (`ibuilder-pipeline/src/preflight.rs`); the registered builds are 4.0.0 / 4.0.1 |
| nxnative #842 / `8ef752a2` | a merged package (base + update + DLC, merged XCI) is read by giving the extractor the same file as base **and** update | in the host's `nx_extract`; the Rust `app/patches/nx_extract_merged_update.py` reaches the same result inside the extractor. Passing the file twice (no patch) is what upstream's script does |

Also seen in the MK8D logs and still true upstream: Mii heads draw nothing (nxnative PR #2512, "a head with a drawn face", is not in upstream main); once fonts are served, button icons need `nintendo_ext.ttf`.

**Verdict.** The pinned host already has every upstream fix for MK8D's known boot hangs. What is missing is data (fonts, seed list) that the app must supply: with `GetSharedFontLoadState` answering "loaded" the game
should proceed. To test before any rebuild: copy the six TTFs that `make_shared_fonts.py` writes into a folder under the app's `Documents/` and put `DM_FONT_DIR=<that path>` in `Documents/games/0100152000022000/env.txt`
(the host reads the variable when the game first asks for a font), and copy the profile's `pso_manifest.tsv` into `Documents/shaders/0100152000022000/`.
This is a diagnosis from the host source and the logs; no log shows the game getting past the wait, and the guest's own wait loop was not observed (no `DM_HITCH_DUMP` / `sample`).

One more finding from the same files, **outside upstream**: Grizzy's session ends with 84 lines of `kIOGPUCommandBufferCallbackErrorBackgroundExecutionNotPermitted` (GPU work submitted while the app was in the
background). The engine's own iOS front end calls `dm_gfx_suspend(1)`, `dm_audio_suspend`, `guest_clock_freeze_for` on `sceneDidEnterBackground` (`front_ios.m`, case 1), but GameApp does not link that front end
(`load/glue_front_standin.m` stubs `front_ios_focus_event`) and no GameApp source calls `dm_gfx_suspend`; the host only suspends on SIGTERM ("graphics suspended" appears once, at exit).

## 5. What to do, in order

| # | Item | Fixes (seen on the phone) | Effort | Needs |
|---|---|---|---|---|
| 1 | **Shared-font stand-ins in the bundle** (`dm/font/*.ttf`, `DM_FONT_DIR`) | MK8D black screen / 0 fps; any game that waits on `GetSharedFontLoadState` | S: already in `scripts/build-app.sh` / `profile.rs`; verify the IPA holds `dm/font/standard.ttf` and the next log prints `shared font (standard.ttf)`. No-rebuild test: env.txt as in section 4 | a device run |
| 2 | **Copy `pso_manifest.tsv`** (and the other profile tables) into `Documents/shaders/<id>/` | the `[RED] seed list does not exist`, first-frame pipeline stalls | S: already in `set_shader_pack` (top-up); verify on MK8D's existing pack | a device run |
| 3 | **Suspend graphics / audio / clock when the app goes to the background** (the engine's `front_ios_focus_event` mapping) | the GPU-submission error flood in Grizzy's log, risk of a game that resumes confused | S-M, iOSBuilder side, not upstream | app change (no engine change) |
| 4 | **`0401` audren burst pacing** | 23-24 % catch-up renders at 480-frame callbacks on this phone (crackle once a game plays sound) | S (patch ready, 3 hunks hand-merged) | host rebuild, listen test on MK8D / Grizzy |
| 5 | **`0402` + `0403`** (and optionally `0404`, `0405`) memory trims | the jetsam ceiling for big games (Ocarina: about 10 GB on a Mac against about 6 GB on the phone); upstream measured them on OoT / iPhone 17 only | S each (patches ready) | the same host rebuild; an OoT run |
| 6 | **`0409` x18 select** in `patch_nso.py` **and** `ibuilder-rewrite` | PCM / stack corruption in guest code with the pattern | M: Rust port + new oracle | decision to move the oracle |
| 7 | `0406`, `0407`, `0408` | guards (Metal 4 look-ahead crash, HID bind race, mesh diagnostics) | S, same rebuild | - |
| 8 | Engine bump to nxnative main | `985ea6cf` replaces 0103, `79f28b06` and the memory set come with it | L: conflicts above; **moves the shader chain to `fded82111357`** (every game's shaders translated again, Rust shader oracle re-recorded) | only when a game needs a split format or a fix not listed here |
| 9 | Optional: `0410` + `0411` | BC decode on A15 / A16 phones (MK8D on an iPad mini 6 was ended by jetsam, NX2Apple #344) | S | irrelevant to an iPhone 15 Pro |

Ignore list: GL commits, Mac HID, Persona / Resident Evil / Witcher / SNES / MKW profile and map commits, Xcode staging, CI, `538da5d9`, `b83913bc` (none of the four games).

### Patch files (`engine-patches/`)

Numbered 04NN so they do not collide with `paches/engine/` (0103-0301); apply with `patch -p1` in number order after `paches/engine/*`, before the host-level `paches/0006-0013`.
Each header carries the upstream URL, author, co-authors and licence (MIT, Copyright 2026 seedds; NX2Apple's two keep their own author lines). Docs, game notes and conflicting test files were left out of each upstream diff.
Result of `git apply --check` then applying each in order on a copy of the patched tree's files (the 21 files they touch, 14 of them tests), and of `patch -p1 --forward` (same result, no fuzz): **all 11 apply**.
Three needed hand work: `0401` (3 conflicting hunks with NX2Apple 0048, plus a test-stub merge), `0402` (adapted to `rh_alloc_size` of NX2Apple 0055), `0405` (one call site keeps `!bc_cpu`).
The shader chain id is untouched (none of the files is an input of `dmsh_chain.py`). **Not compiled, not run**: each needs the host rebuild you must approve, and a device run.

| File | Upstream |
|---|---|
| `0401-audren-pace-burst-refills` | nxnative `79f28b06` |
| `0402-nvn-rt-recycle-slack-cap-4mib` | `943a6111` (adapted) |
| `0403-nvn-zero-fills-in-shared-buffer` | `1d1c5b4a` |
| `0404-nvn-upload-trim-idle-staging-opt-in` | `93c76c1c` |
| `0405-nvn-upload-bound-zero-copy-sources` | `31fdefeb` |
| `0406-nvn-pso-lookahead-no-texture-resolution` | `5012821e` |
| `0407-hid-npad-style-under-bind-lock` | `4b92e232` |
| `0408-nvn-geometry-check-live-mesh-arguments` | `74d17535` |
| `0409-rewriter-x18-conditional-select-no-stack-spill` | `0b5fd602` |
| `0410-nx2apple-0063-bc-cpu-buffer-to-texture-copies` | NX2Apple `patches/0063` |
| `0411-nx2apple-0064-bc-cpu-decode-across-cores` | NX2Apple `patches/0064` |

Not extracted: `985ea6cf` (4 conflicts with NX2Apple's `DmGamepad.m`; our 0103 covers it), the chain commits (they move the chain id), the opt-in memory PRs still open.

## 6. What could not be determined

* Whether MK8D leaves the font wait and then draws: no log exists after a fix, and the guest's wait loop was not observed.
* Any behaviour of the patches: nothing was compiled or run (user's rule). The audren merge in particular keeps NX2Apple 0048's teardown and upstream's pacing by reading, not by test.
* That an A17 Pro uses native BC textures: taken from the engine's own `mtl_init` message and the absence of any BC line in the logs; not queried on the device.
* Whether our copies of NX2Apple 0014 / 0023 / 0044 duplicate upstream behaviour (they build, so no duplicate symbols).
* Open upstream PRs were read by title and summary, not line by line; merge state of a PR is not shown by GitHub for seedds (it lands commits itself), so "landed" was decided from the commit list.
* Nothing upstream exists for Grizzy and South Park: The Stick of Truth, so there is nothing to compare for them; thermal and present pacing have no landed upstream change to port.
