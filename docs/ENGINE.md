# The engine host and its patches

The game runs on a C host: the nxnative engine with the NX2Apple patches (third party, pinned), which provides the
Switch's system services and a Metal back end for its GPU API. The app ships it as a prebuilt dylib that the assembler
grafts the game onto. This repository does not contain the engine; it contains patches against it in
`engine-patches/` and a script that rebuilds the host with them.

## Rebuilding the host

```bash
scripts/build-host.sh                    # both dylibs   -> dist/host/
scripts/build-host.sh device             # or only one (device | simulator)
HOST_TESTS=1 scripts/build-host.sh       # also run the engine's own self-tests that need no game (a few minutes)
```

The script needs `IOSBUILDER_WORKTREE` (or `IOSBUILDER`), see [BUILDING.md](BUILDING.md). It:

1. APFS-clones the patched engine tree the app ships and applies `engine-patches/0[45]*.patch` in name order with
   `patch -p1 --forward`. The first patch that does not apply stops the build and is named.
2. Stops if the shader chain id of the tree differs from `EXPECT_CHAIN` (default `6c53446391cc` since 0536). The chain id names
   the shader library; if it moved, every game's library would have to be rebuilt (`CHAIN_ANY=1` only prints it).
3. Runs the original project's own host build (`load/prebuild_loadable.sh`) from a mirror of its tools, in `HOST_WORK`
   (default `target/host-work`).
4. Verifies each dylib (platform and minimum OS, every symbol the previous host and the app's shim resolve, the chain
   id and the patch markers as strings) and only then writes `dist/host/`: the two dylibs, `HOST-MANIFEST.txt` (what the
   tree is made of, sha256 of every patch and dylib, toolchain) and, with `HOST_TESTS=1`, `HOST-TESTS.txt`.

Use the new host in an app build with `GAMEAPP_HOST=dist/host/host-prebuilt-loadable-{ios,sim}.dylib
scripts/build-app.sh ...`; `app/inputs.tsv` then reports that the pin differs. To go back, leave `GAMEAPP_HOST` unset, or
remove a patch from `engine-patches/` (or point `ENGINE_PATCH_DIR` at another folder) and rebuild. Patches that edit the
same file may need a re-diff after one is removed.

## The patches

`04NN` are upstream nxnative fixes and our own changes; `05NN` are adapted from other MIT-licensed ports. Every patch
header states what it changes, why, and how it was checked. Only 0536 changes the shader chain id (see [Shader chain 2026-11-23](#shader-chain-2026-11-23)).

| area | patches |
|---|---|
| Audio | 0401 render pacing after bursts; 0453 real-time deadline for the audio render thread; 0454 short AudioOut tails (Bluetooth underruns) |
| Memory | 0402 render-target recycle slack capped at 4 MiB; 0403 zero fills in bands that fit the shared buffer; 0404 / 0405 opt-in trimming of idle upload staging and bounded zero-copy sources; 0501 opt-in texture memory saver; 0500 opt-in skip of retired pools; 0512 a memory-pressure trim at the next submission (system pressure, app request, headroom watermark); 0519, 0521-0525 from the [2026-11-23 sync](#upstream-sync-2026-11-23) |
| Pipelines and prewarm | 0406 look-ahead no longer resolves a future sparse attachment; 0412 prepare builds a head of the prewarm lists before the first frame and the tail in the background; 0413 this device's learned list first (`pso_user.tsv`); 0414 an MTLBinaryArchive of the MTLDevice path in the engine; 0415 the prewarm tail resumes where it stopped and waits while the device is hot; 0462 the archive's timed write never blocks compiles; 0535 libraries compiled from Metal source on the phone are compiled for size (`optimizationLevel` Size: faster pipeline setup per Apple, unmeasured here; the app's warm-up must use the same options) |
| Graphics correctness | 0408 composed-mesh arguments checked live (Switch 2 mesh shaders); 0455 / 0456 empty-grid dispatches skipped, interrupted compute compile retried; 0457 sparse descriptor lifetime; 0458 present start-up link rescue; 0460 allocation checked before pages are write-protected; 0506 vertex streams bound to the bytes a draw reads, with a pool-end fallback; 0507 a presented texture keeps its contents when loaded and not written; 0509 the archive leaves out pipelines without a fragment stage, switch for render conditions |
| Input | 0407 Npad style read under the bind lock; 0461 stick diagonals within the Switch circle |
| Upstream sync 2026-11-23 | 0514-0534, see [the section below](#upstream-sync-2026-11-23) |
| Shader chain | 0536 upstream's chain tools, translator sources and library ABI at 033793f0 (chain `6c53446391cc`); 0537 their host side. See [Shader chain 2026-11-23](#shader-chain-2026-11-23) |
| GL and Vulkan games | 0538 shaders from Metal source: with no `<api>_shaders.metallib` and a `<DM_MSL_DIR>/<api>/msl/` folder (the files the phone's translation writes, `crates/ibuilder-glvk`), each function is compiled from its own `.metal` file at first use, with the options of the NVN source compiles (Relaxed math, invariance, `optimizationLevel` Size) |
| Texture decode | 0410 / 0411 BC buffer-to-texture copies and BC decode on the CPU, only on a GPU without BC (A16 and older); 0510 that path on by itself, BC1-5 decoded on the GPU at upload; 0511 BC1-3 stored as ASTC 4x4. See [GPUs without BC textures](#gpus-without-bc-textures) |
| Rewriter | 0409 a conditional select on x18 no longer spills over live stack. This changes the engine's Python rewriter only; the Rust rewriter has its own implementation |
| Diagnostics | 0451 residency staging retired off the feedback queue; 0502 / 0503 tuning switches; 0504 a stall watchdog that logs every guest thread's wait after 5 s without a frame; 0505 a `[pools]` census line; 0508 draw drops counted by reason and the Metal HUD |

`engine-patches/optional/` holds patches that are kept for a decision and never applied (see its README): 0450 and 0452
fail engine self-tests on this tree; 0459 (an 8 GiB alias for one game) is unverified on a phone.

## Upstream sync 2026-11-23

Patches 0514 to 0534 port the worthwhile engine changes of seedds/nxnative `e74c3fa7..184847c5` (89 commits, MIT;
each patch header names its commits, authors, what was adapted and what was tested). Nothing of the shader chain was
taken there: the chain id stayed `b24b25240cad` (it moved later, with 0536). Every patch was built and its tests run on a Mac (M1 Pro); none has run on a
phone yet.

| patch | upstream commits | games | switches |
|---|---|---|---|
| 0514 dirty: PG_NONE fault claimed only when writable | a3d58526 | all (write-protected pools) | - |
| 0515 running-write tables set-associative, sbw grows | a67583a4 | Donkey Kong Bananza; high-draw scenes | - |
| 0516 registered video decoder buffers queued | a41f0270 | Mario Kart World (SDK 19-22 movies) | - |
| 0517 gate dispatch index grows | 184847c5 | games past 8,192 gates | - |
| 0518 offline services: streamplay, prepo, friend IDs, friends no-Uid / blocked users | b768913f, 1364b89a, 656f8844, 977f5cab (not its NotificationQueue hooks: 0513 stays) | Star Fox, Nintendo Switch Sports, Super Kirby Clash, SUPER BOMBERMAN R 2 | - |
| 0519 unmapped stack pages released | 6c50d74e | Cuphead; games mapping stacks | - |
| 0520 mesh draws: deferred updates ordered | 0cf3a702 | Switch 2 mesh titles | - |
| 0521 shader-code pools out of the raw table, guest files wired before the commit, late 30 fps frames | 9143b6ff | Ocarina of Time | - |
| 0522 GPU-only pools out of the raw table, chunks from 256 MB | 3dd1f7a6 | Ocarina of Time | - |
| 0523 sharing and CPU-written tables by pool | da242230 | Ocarina of Time (long sessions) | - |
| 0524 memory report kept in long sessions | 930fbff2 | all (log) | - |
| 0525 idle spares and idle uploaded textures freed, on 0512's trim; system memory logged | d08d7da6 (merged into 0512) | Ocarina of Time on 8 GB iPhones; all | `DM_NVN_TEX_EVICT_IDLE` |
| 0526 linear views size-checked | 3e1ada5b | Nintendo Switch Sports | - |
| 0527 guest clock frozen during prepare and archive open | 8d888054 | 13 Sentinels: Aegis Rim, Unicorn Overlord | - |
| 0528 frame step ignores pauses and stalls | 6ef18a1f (with 0503's `DM_FPS_PAYBACK`) | all capped games | - |
| 0529 explicit CPU / manual audio renderers left to the game | b2d41ac5 | issue #3212 | - |
| 0530 kept index ranges indexed by page | b4588a53 | converted vertex streams | - |
| 0531 present histories under a lock | 7ff0aef4 | all | - |
| 0532 background tasks drain their autoreleases | de348580 | all | - |
| 0533 MSAA downsample into another size | fa59a0a7 (engine part) | Digimon Story Time Stranger | - |
| 0534 loader refuses data in the code's last 16 KB page | 17454fc3 (loader part; the relayout itself is the Rust rewriter's, with 65dfeb33) | FINAL FANTASY VIII Remastered | - |

The memory batch (0521-0525) is what upstream measured on an 8 GB iPhone 17 with Ocarina of Time: page-ins from
6,000-15,600 to 100-400 per 10 s and 14-17 to 27-30 fps in Kokiri Forest (0521), about 274 MB less wired (0522),
206 MB of old-area textures freed at an area change (0525). On our phones it still has to be measured: wired memory
and page-ins (the `system memory at frame` and `memory up to frame` lines, `memstat-*.tsv` with `DM_MEMSTAT`) before
and after.

Not taken: ef393b7e's reserve message (paches/0006 already prints the size and errno there, and that hunk would stop
0006 from applying); 194da5fb, 806fabe1, 5b7dad03 and the OpenGL commits. The chain-moving commits of that range
(a63ca651, 6dc27632, f41613f7, 131bd013, 930e9195, e3f2d21f, acbb26c3, 8cc63cc9) came afterwards with 0536 / 0537.

## Shader chain 2026-11-23

0536 brings the translation chain to upstream nxnative 033793f0: every file `dmsh_chain.py` hashes (`FILES`) is upstream's,
byte for byte, so the chain id is upstream's own, `6c53446391cc` (it was `b24b25240cad`: the files of a3663659 with the
Snowdrop hunk of `shader_split.py`). That takes the 37 chain-moving commits since a3663659, the 27 the earlier reviews
left out (a3663659..e74c3fa7: 736a2ecb, b989137a, c311523c, 02614c26, 32fe34ef, 3ec5b785, caccf926, 2e6f6fd0, 33b0cf51,
ae5d8f51's chain part, 953c5a90 and the split readers) and the 10 since (a63ca651, 6dc27632, f41613f7, 131bd013, 930e9195,
e3f2d21f, acbb26c3, 8cc63cc9, 1ed3e12a, 033793f0). 0537 is their host side: gmem map spans for raw loads, the pulling and
shadow-input entry names when a vertex shader's three libraries are cut from one unit, vertex viewport masks off the lean
geometry path, and task payloads past 16 KB through a buffer.

**Every game's shader library is rebuilt once**: the host loads only `b-6c53446391cc`. The app's Rust stage 3
(`ibuilder-shader`, `ibuilder-shaderbuild`) follows the same Python: on 5,348 shaders of Ocarina of Time (every task and
mesh shader among them) its library is sha256-identical to the one upstream's Python makes through the phone's driver.
The Rust ports had been written against an older engine copy (upstream of 2026-11-15); they are now at 033793f0 too.

The app's translator is compiled from the engine's Ryujinx sources and `dmsh_xlate/Program.cs`: an app build must take
them from the tree with 0536 applied (`NXPOCKET_ENGINE`), not from the unpatched `out-stage` engine.

| patch | upstream commits | what |
|---|---|---|
| 0536 shader chain at 033793f0 | the 37 above | `dm/gfx/tools` chain files and the split readers they import, the translator's Ryujinx sources and `Program.cs`, `dmsh_abi.h`, `shader_lib.py`, `romfs.py` (shared descriptors), `dmsh_chain.h` |
| 0537 host side of the chain | 736a2ecb, b989137a, 2e6f6fd0, acbb26c3 (nvn parts) | `nvn_mtl.m`, `nvn_geometry.inc` |

Not taken: ae5d8f51's host part (the Mac host recovering shaders compiled during play: `nvn_live.inc`, `hle_glslc.c`;
the app translates such shaders itself), `gl_shaders.py` / `vk_shaders.py` (the Mac's OpenGL and Vulkan paths), and the
translator binary `dmsh_xlate_bin/dmsh_xlate` (a patch carries no binary; the Mac oracle used upstream's, which matches
`dmsh_chain.lock`).

## Switches

The host reads `DM_*` environment variables. On the phone they go in a game's `Documents/games/<title id>/env.txt`, one
`NAME=value` per line, or are set by the app's settings. The ones that matter:

| variable | default | meaning |
|---|---|---|
| `DM_PSO_PREPARE_PERCENT=<1..100>` | 100 | share of the prewarm lists built before the first frame (0412) |
| `DM_PSO_PREPARE_TAIL=0\|user` | on | with a percentage below 100: `0` leaves the rest to on-demand builds, `user` builds this device's rows only |
| `DM_PSO_USER_FIRST=0` | on | old list order instead of the learned list first (0413) |
| `DM_PSO_BINARY_ARCHIVE=0` | on | turn the pipeline archive off (0414); `DM_PSO_ARCHIVE_MB` bounds its size |
| `DM_PSO_TAIL_RESUME=0`, `DM_PSO_TAIL_HOT=1` | resume on, wait when hot | tail resume and heat behaviour (0415) |
| `DM_PSO_NOWAIT=0` | set by the app | whether a draw that needs a pipeline waits for its compile |
| `DM_MTL_OPT_SIZE=0` | on | compile libraries from source at the Default level, as before 0535 (also the GL / Vulkan layers' source mode, 0538; A/B only: the app's warm-up compiles for size, so its cache entries are then not used) |
| `DM_PSO_MTL4=0` | per game | use the MTLDevice path instead of Metal 4 pipelines |
| `DM_PROF_TEX_BC_CPU=1` | set by the chain on a GPU without BC textures | accepted, no longer needed since 0510: the no-BC path turns itself on on such a GPU |
| `DM_NO_BC_ASTC=all\|bc1\|0` | `all` | without BC: BC1, BC2 and BC3 stored as ASTC 4x4; `bc1` BC1 only; `0` every BC format decompressed (0511) |
| `DM_NO_BC_GPU=0` | on | without BC: deswizzle and decode / transcode every BC texture on the CPU, as before 0510 (the reference) |
| `DM_TEST_NO_BC=1` | off | test switch: take the no-BC path on a GPU that has BC (the Mac), the only way to run it without an A14-A16 device |
| `DM_MEM_PRESSURE=0` | on | no memory-pressure trim (0512); `DM_MEM_TRIM=<frame>:<tier>,...` and `DM_MEM_HEADROOM=<MB>` are its test switches |
| `DM_NVN_TEX_EVICT_IDLE=0\|1` | on when Metal's working set is under 6 GB | the 10 s sweep of uploaded textures idle 30 s (0525); the memory-pressure trim frees them either way |
| `DM_AUDREN_RT=0` | on | turn the audio real-time deadline off (0453) |
| `DM_AUDREN_TRACE=1` | off | print the first 200 audio updates with their times |
| `DM_NVN_UPLOAD_IDLE_MS=<ms>`, `DM_NVN_UPLOAD_SRC_MB=<MB>` | off | trim idle upload staging, bound zero-copy sources (0404, 0405) |
| `DM_NVN_VBUF_CLAMP=0` | on | the old behaviour of skipping a draw whose vertex stream runs past its pool (0506) |
| `DM_NVN_PRESENT_KEEP=0` | on | do not keep presented textures' contents (0507) |
| `DM_NVN_RENDER_COND=0` | on | ignore render conditions (0509) |
| `DM_STALL_DUMP=0` | on | no stall watchdog (0504) |

Diagnostic switches the app turns on for a game (boot timing, memory statistics, slow frames) are listed in
[DIAGNOSTICS.md](DIAGNOSTICS.md). The engine's own log line `pipeline prepare: ...` in `gameapp.guest.log` says what the
prepare step did.

## GPUs without BC textures

The iPhones 12 to 15 (A14-A16; the 15, not the 15 Pro) and the A14-A16 iPads have Metal 4 but no BC texture formats,
which most Switch games use. Patches 0510 to 0512 port the work of the community fork
[mianmuhammadse/nxnative, branch `no-bc`](https://github.com/mianmuhammadse/nxnative/tree/no-bc) (Mian Muhammad, MIT;
each patch header names its commits) onto our tree, which already had a CPU-only path (0410 / 0411, behind the profile
key `tex_bc_cpu`):

- the path turns itself on when `supportsBCTextureCompression` is false (0510). `tex_bc_cpu` and the app's
  `DM_PROF_TEX_BC_CPU=1` are accepted and no longer needed;
- BC1, BC2 and BC3 2D textures are stored as ASTC 4x4, a quarter of the memory of RGBA8 (0511): the GPU deswizzles the
  blocks and the kernel `bca` transcodes each BC block into one ASTC block at upload (byte-identical to the C
  reference `nvn_bc2astc.c`, which the CPU paths use). A block with varying alpha keeps four alpha levels; the fork
  measured 39.7 dB alpha PSNR over Metroid Dread's BC3 textures;
- BC4 / BC5 (and BC1-3 with `DM_NO_BC_ASTC=bc1` or `0`) are decompressed on the GPU at upload by the kernel `bcd`
  (0510); BC6H / BC7, 3D, sparse-pool and linear textures and buffer-to-texture copies on the CPU (0410 / 0411);
- the upload batch then runs zero fills, the GPU jobs and the copies in that order, with encoder barriers;
- 0512 trims caches when memory runs short (on every device, not only these).

On a GPU with BC none of this code is reached (the `t_bc_astc` test runs it with the path off). Nothing of it has run
on an A14-A16 device yet: the tests run it on the Mac with `DM_TEST_NO_BC=1`. To compare on a device, set
`DM_NO_BC_ASTC=0` (decompressed) or `DM_NO_BC_GPU=0` (all on the CPU, the old path) in the game's `env.txt`.
The fork's per-game BC3 keep table, its Vulkan-layer BC storage and its benchmark on game data were not ported.

## Suspend and resume

The host exports `dm_gfx_suspend(int on)` (both directions), `dm_audio_suspend(int on)`, `guest_clock_freeze_for(reason, on)`
and `dm_focus_set(int)`. GameApp links no engine front end, so the app does what the front end would:
`app/patches/0030-suspend-graphics-in-background.patch` calls them on `DidEnterBackground` / `WillEnterForeground`
(focus 3, audio suspend, clock freeze, graphics suspend; the reverse with graphics first) and on
`WillResignActive` / `DidBecomeActive` (focus 2 / 1).

## Testing

`HOST_TESTS=1 scripts/build-host.sh` runs the engine's host unit tests, the CPU texture tests and the graphics tests that
need no game, and records them in `dist/host/HOST-TESTS.txt`. The no-BC tests (`t_bc_cpu`, `t_bc_astc`, `t_nobc_order`,
`t_bc2astc_gpu`, `t_astc_hw`) force the no-BC path with `DM_TEST_NO_BC=1` (or `g_bc_cpu`) on the Mac's GPU. These run on the Mac; none of them proves behaviour on a
phone. Check a patch on a device through the logs in [DIAGNOSTICS.md](DIAGNOSTICS.md).
