# Architecture

iBuilder turns a Nintendo Switch game, from the user's own dump, into a native iOS game, on the iPhone. This page
follows the chain from the input file to the running game. Details of each part are in [reference/](reference/).

## The chain

```
 input (.nsp .xci .nsz, or .nup for a Switch 2 game)
   |
   0  extract       C# (LibHac) reads the container; ibuilder-nup reads a .nup
   1  profile       ibuilder-profile derives the per-game profile (profile.tsv, game.tsv)
   2  rewrite       ibuilder-rewrite (+ ibuilder-a64, ibuilder-nso) patches the ARM64 NSO modules
   3  shaders       ibuilder-shadersplit -> translate (Ryujinx translator, SPIRV-Cross) -> ibuilder-shaderbuild
                    (+ ibuilder-shader) -> a shader library image per game
   4  assemble      ibuilder-macho links the engine host + rewritten modules into one Mach-O framework
      sign          ad hoc signature (ibuilder-sign on the Mac; zsign with the Keychain identity in the app; with no .p12
                    in the app, zsign ad hoc: an export-only build, stamped signature=adhoc-export and never booted)
   5  boot          the app dlopens the framework and calls the host's entry point: the game runs in-process
```

Stages 1 to 4 run in one Rust library, `ibuilder-pipeline`, which owns the stage order, the settings, the run markers
(so an interrupted build resumes), the timing and memory/thermal gates, and the logs. It is reached through a stable C
ABI (`ibuilder-ffi`, `include/ibuilder.h`) or the Mac command line (`ibuilder-cli`). See
[reference/pipeline.md](reference/pipeline.md).

Rule of the chain: for every valid input, each stage's output is byte-identical to what the original Python toolchain
(iOSBuilder) produces. That is the test oracle (see [../CONTRIBUTING.md](../CONTRIBUTING.md)). The pieces that run
on the phone use no `fork`, `exec` or embedded interpreter.

## The crates

| crate | what |
|---|---|
| `ibuilder-pipeline`, `ibuilder-ffi`, `ibuilder-cli`, `ibuilder-alloc` | chain driver, C ABI, Mac CLI, allocator that returns big blocks to the kernel |
| `ibuilder-nup`, `ibuilder-profile` | Switch 2 container; per-game profile ([reference/profile-and-nup.md](reference/profile-and-nup.md)) |
| `ibuilder-nso`, `ibuilder-a64`, `ibuilder-rewrite` | NSO modules, AArch64 decoding, the module rewriter ([reference/rewrite.md](reference/rewrite.md)) |
| `ibuilder-shadersplit`, `ibuilder-shader`, `ibuilder-shaderbuild` | stage 3 ([reference/shaders.md](reference/shaders.md), [reference/shader-split.md](reference/shader-split.md)) |
| `ibuilder-macho`, `ibuilder-sign`, `ibuilder-pack` | assembler, signer, `.nxgame` packager ([reference/assemble-sign-pack.md](reference/assemble-sign-pack.md)) |
| `ibuilder-mods` | Atmosphere-style mods and cheats ([reference/mods.md](reference/mods.md)) |
| `ibuilder-testkit` | shared test helpers (`skip`, `have`, `compare_files`, `local_env`, ...) |

## The app

The app is a .NET iOS application (C#, `app/gameapp/`, a snapshot of the original GameApp sources plus the patches in
`app/patches/`) with a small C layer:

```
 GameApp (C#)  --P/Invoke "__Internal"-->  libgameappshim-rust-<kind>.a
                                             app/gameapp_shim_rust.c : Keychain identity, nsz, host symbols,
                                                                       translator trampoline, live-shader notification
                                             zsign + OpenSSL, SPIRV-Cross wrapper, zstd
                                                  |  ibuilder.h
                                             libibuilder_ffi.a  (Rust, --features real-stages)
```

* The C# side owns the UI, the game library, imports (files, NAS, cloud), saves, exports, and decides when to start, stop
  or resume a build. It reads the files the Rust library writes (`gameapp.stage`, run markers, logs, `build_times.tsv`)
  and the chain's result code.
* The result code of a build: 0 ok, 4 no shader library, 5 not assembled or signed, 75 stopped on purpose before stage 3
  (next launch continues), 130 cancelled, 131 paused. A build that returns 4 or 5 is never stamped as ready.
* With no signing identity (the signer's rc 10, 12, 13) stage 4b signs ad hoc through `ibuilder_options.sign_adhoc` and the
  build is 0, stamped for export only: the app offers Export IPA, not Play, and "Sign again to play" once a `.p12` is imported.
* The shim asks Metal whether the GPU reads BC textures; without them (`IBUILDER_FLAG_GPU_NO_BC`) every boot sets
  `DM_PROF_TEX_BC_CPU=1` and the host decodes them on the CPU. A GPU with them gets no change.
* The patches in `app/patches/` are applied, in order, to a copy of the snapshot by `scripts/build-app.sh`;
  `scripts/check-patches-apply.sh` proves they apply.
* The bundle carries only the data the Rust chain reads (`dm/games/*/game.tsv`, `settings.tsv`, host sources for the
  profile, `host-prebuilt.dylib`). No Python, no `.py` files.
* An exported game is this app's bundle plus the built game (`ExportedGame/manifest.tsv` makes the executable run as
  that game's player). The layout is described at the end of this page.

## The engine host

The game does not run on a translated CPU. The rewritten NSO modules are native ARM64 code that runs in the iPhone's
process, and the Switch's system services and GPU API are provided by a C host (the nxnative engine with the NX2Apple
patches; third party, pinned). The host is a prebuilt dylib that the assembler grafts the game onto, which becomes
`DmGameProbe.framework`. This repository adds a series of patches to it (`engine-patches/`, see [ENGINE.md](ENGINE.md)).
Graphics go through Metal; shaders are translated ahead of time by stage 3 and the library is read by the host
(`DM_MSL_DIR`).

## Where data lives on the phone

Under the app's `Documents/` (visible in the Files app, except the dot folder):

| path | content |
|---|---|
| `extract/` | the extracted dump: `ExeFS` modules, `RomFS.img`, and `flat/` (the rewriter's outputs: `<module>.patched.bin` and the fixup/site/gate/cut tables) |
| `DmGameProbe.framework/` | the assembled and signed game |
| `shaders/<title id>/` | the shader library `b-<chain>.img` and the tables the host reads beside it |
| `games/<title id>/` | per-game files: icon, `env.txt` (host switches), mods, learned pipeline lists |
| `profile.tsv`, `profile-derived/` | runtime profile and derived game tables |
| `gameapp.config`, `gameapp.settings`, `gameapp.stage`, run markers, `*.log`, `build_times.tsv` | build configuration, progress and logs |
| `.diag/` | hidden diagnostics ([DIAGNOSTICS.md](DIAGNOSTICS.md)) |

## Exported game IPA layout

The app can export a built game as an `.ipa` ("Light": the RomFS stays beside it as a `.romfs` clone; "Full": the RomFS
is inside). The layout, for a tool that wants to post-process one:

```
Payload/<Name>.app/
  Info.plist                      generated: bundle id <app id>.g<title id>, the game's name and icon
  GameApp                         the app executable (becomes the player because ExportedGame/manifest.tsv exists)
  host-prebuilt.dylib, dm/...     data the Rust chain reads; not needed to play an export
  Frameworks/DmGameProbe.framework/DmGameProbe   the game: host + rewritten modules, signed
  ExportedGame/
    manifest.tsv                  key<TAB>value: title_id, dump, name, app_build, exported (UTC); Light adds romfs_*
    extract/flat/                 the 21 rewriter files (7 per module: main, sdk, subsdk0)
    extract/RomFS.img             Full export only
    shaders/<title id>/           shader library and tables
    documents/                    first-launch seed for Documents (files already there are never overwritten)
```

At launch in player mode the app seeds Documents from `ExportedGame/documents/`, points the chain at the bundle and
Play runs a boot-only chain (no build stage runs). Do not add `_CodeSignature/` or `embedded.mobileprovision`: the
sideloading service re-signs every Mach-O and the bundle. A replaced `DmGameProbe`, `patched.bin` or shader library must
stay consistent with the other files of the same game (the host loads `<m>.patched.bin`, `.fixups.tsv` and
`.gates.tsv` from `extract/flat/` at every boot; the library file name carries the host's chain id).
