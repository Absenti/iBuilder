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
2. Stops if the shader chain id of the tree differs from `EXPECT_CHAIN` (default `b24b25240cad`). The chain id names
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
header states what it changes, why, and how it was checked. They never change the shader chain id.

| area | patches |
|---|---|
| Audio | 0401 render pacing after bursts; 0453 real-time deadline for the audio render thread; 0454 short AudioOut tails (Bluetooth underruns) |
| Memory | 0402 render-target recycle slack capped at 4 MiB; 0403 zero fills in bands that fit the shared buffer; 0404 / 0405 opt-in trimming of idle upload staging and bounded zero-copy sources; 0501 opt-in texture memory saver; 0500 opt-in skip of retired pools; 0512 a memory-pressure trim at the next submission (system pressure, app request, headroom watermark) |
| Pipelines and prewarm | 0406 look-ahead no longer resolves a future sparse attachment; 0412 prepare builds a head of the prewarm lists before the first frame and the tail in the background; 0413 this device's learned list first (`pso_user.tsv`); 0414 an MTLBinaryArchive of the MTLDevice path in the engine; 0415 the prewarm tail resumes where it stopped and waits while the device is hot; 0462 the archive's timed write never blocks compiles |
| Graphics correctness | 0408 composed-mesh arguments checked live (Switch 2 mesh shaders); 0455 / 0456 empty-grid dispatches skipped, interrupted compute compile retried; 0457 sparse descriptor lifetime; 0458 present start-up link rescue; 0460 allocation checked before pages are write-protected; 0506 vertex streams bound to the bytes a draw reads, with a pool-end fallback; 0507 a presented texture keeps its contents when loaded and not written; 0509 the archive leaves out pipelines without a fragment stage, switch for render conditions |
| Input | 0407 Npad style read under the bind lock; 0461 stick diagonals within the Switch circle |
| Texture decode | 0410 / 0411 BC buffer-to-texture copies and BC decode on the CPU, only on a GPU without BC (A16 and older); 0510 that path on by itself, BC1-5 decoded on the GPU at upload; 0511 BC1-3 stored as ASTC 4x4. See [GPUs without BC textures](#gpus-without-bc-textures) |
| Rewriter | 0409 a conditional select on x18 no longer spills over live stack. This changes the engine's Python rewriter only; the Rust rewriter has its own implementation |
| Diagnostics | 0451 residency staging retired off the feedback queue; 0502 / 0503 tuning switches; 0504 a stall watchdog that logs every guest thread's wait after 5 s without a frame; 0505 a `[pools]` census line; 0508 draw drops counted by reason and the Metal HUD |

`engine-patches/optional/` holds patches that are kept for a decision and never applied (see its README): 0450 and 0452
fail engine self-tests on this tree; 0459 (an 8 GiB alias for one game) is unverified on a phone.

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
| `DM_PSO_MTL4=0` | per game | use the MTLDevice path instead of Metal 4 pipelines |
| `DM_PROF_TEX_BC_CPU=1` | set by the chain on a GPU without BC textures | accepted, no longer needed since 0510: the no-BC path turns itself on on such a GPU |
| `DM_NO_BC_ASTC=all\|bc1\|0` | `all` | without BC: BC1, BC2 and BC3 stored as ASTC 4x4; `bc1` BC1 only; `0` every BC format decompressed (0511) |
| `DM_NO_BC_GPU=0` | on | without BC: deswizzle and decode / transcode every BC texture on the CPU, as before 0510 (the reference) |
| `DM_TEST_NO_BC=1` | off | test switch: take the no-BC path on a GPU that has BC (the Mac), the only way to run it without an A14-A16 device |
| `DM_MEM_PRESSURE=0` | on | no memory-pressure trim (0512); `DM_MEM_TRIM=<frame>:<tier>,...` and `DM_MEM_HEADROOM=<MB>` are its test switches |
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
