# Building and releasing

## What you need

A Mac with:

* Rust (rustup) with the `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets, and Xcode with the iPhoneOS SDK.
  With Homebrew's `rustup` keg, put it on `PATH` for the command only
  (`PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build ...`); the build scripts do it for themselves.
* the .NET SDK with the iOS workload, and the NuGet packages `LibHac` and `SSH.NET` in `~/.nuget/packages`.
* a checkout of the original iOSBuilder project, used only as a read-only input. The app build takes from it the
  prebuilt engine host, the SPIRV-Cross archives, the patched engine tree and the zsign, OpenSSL and zstd sources.
  Nothing in this repository writes there.

Your dumps, keys and certificates are never needed by the build and must never be committed.

## Configure

```bash
cp scripts/local.env.example scripts/local.env     # ignored by git; edit it
```

`IOSBUILDER` (the checkout) is enough to build the app. Optional: `IOSBUILDER_WORKTREE` (a tree with newer work,
default `$IOSBUILDER`), `IBUILDER_SCRATCH` (generated oracle outputs for the byte-compare tests), `IBUILDER_TEAM_SUFFIX`
(the team suffix your sideloading service gives the bundle id, adds the matching background-task id to `Info.plist`),
`IBUILDER_ENGINE_DM`. Every script sources `scripts/local.env` first and so do the Rust tests; an exported variable wins
over the file. A script that needs a missing variable stops and names it.

## Build and test the Rust workspace

```bash
cargo build --release --workspace
scripts/test-report.sh                               # all tests, then "N passed, K skipped for missing fixtures"
IBUILDER_STRICT_FIXTURES=1 scripts/test-report.sh    # the same, failing on any skipped test
scripts/build-ios-ffi.sh                             # libibuilder_ffi.a for device and simulator (+ xcframework)
```

## Build the IPA

```bash
scripts/build-app.sh device        # dist/GameApp-rust-<buildid>.ipa, .sha256, MANIFEST.txt, <ipa>.verify/
scripts/build-app.sh simulator     # an ad hoc signed GameApp.app for the iOS simulator (not installed by the script)
```

The device IPA is ad hoc signed; SideStore or AltStore re-sign it with the user's Apple ID. The script:

1. builds `libibuilder_ffi.a` (`--features real-stages`) for the target;
2. builds the C shim, zsign + OpenSSL, the SPIRV-Cross wrapper, the NSZ converter and zstd;
3. builds the .NET app from `app/gameapp/` with `app/patches/*.patch` applied in order to a copy;
4. adds the bundle data, signs, zips the IPA and runs `app/verify_ipa.sh` on the archive read back.

Useful variables: `APP_WORK`, `APP_OUT`, `CARGO_TARGET_DIR`, `SKIP_RUST=1` (reuse the Rust archive),
`APP_VERSION` / `APP_BUILD_NUMBER` (`CFBundleShortVersionString` / `CFBundleVersion`; a release must raise the version),
`GAMEAPP_HOST` (a host dylib other than the default, see [ENGINE.md](ENGINE.md)), `PIN_STRICT=1`,
`GAMEAPP_EVA_ENTITLEMENT=1` (adds extended-virtual-addressing; a free Apple ID cannot carry it).

Inputs are pinned in `app/inputs.tsv` (size and sha256); a mismatch is a warning in the log and in `MANIFEST.txt`, fatal
with `PIN_STRICT=1`. `app/refresh-snapshot.sh [--check]` refreshes the C# snapshot from the iOSBuilder tree.

### The framework id

Every build has a build id (`GameAppBuild.Id`, `<yyyymmdd-hhmmss>-rust<hash>`) and a **framework id**
(`GameAppBuild.FrameworkId`, 16 hex digits). The chain stamps both into a built game's `Documents/gameapp.framework`
(`app_build=`, `framework_id=`). After an app update, a built game whose stamp names the same framework id is ready: Play
boots it, nothing is rebuilt. A different framework id means the host or a stage crate changed, so the game is rebuilt. A stamp
written before framework ids existed is compared by build id, so the first update to an app with framework ids rebuilds every
game one last time.

The framework id is the first 16 hex digits of the sha256 of a listing of only the inputs that shape a built framework and its
boot (`scripts/lib-hash.sh`, `framework_id_listing`). `scripts/build-app.sh` writes that listing to
`<work>/framework-inputs.txt` and `MANIFEST.txt`. The inputs are:

- `FRAMEWORK_EPOCH`, raised by hand when something not listed here changes what a build makes or how a built game boots
- the sha256 of the host dylib the bundle carries (`dist/host/host-prebuilt-loadable-{ios,sim}.dylib`, pinned in `app/inputs.tsv`)
- the shader chain (`dmsh_chain.h`) and the `ibuilder-ffi` cargo features
- the source trees of `ibuilder-nso`, `ibuilder-a64`, `ibuilder-rewrite`, `ibuilder-macho`, `ibuilder-sign`, `ibuilder-profile`,
  `ibuilder-mods` and `ibuilder-nup`
- `ibuilder-pipeline`, except the files that never touch a framework or its boot (stage 3's shaders, logs, memory and thermal
  gates, the stop request, the launch state machine and the stamp); a new file counts until it is excluded by name
- `crates/ibuilder-cli/src/adapters.rs` and its `Cargo.toml`, and `Cargo.lock`
- `app/game-tables/`, and the engine's `dm/games/settings.tsv`, `dm/games/*/game.tsv` and the host sources the profile reads
  (`dm/host`, `dm/gfx`, `dm/gfx/nvn`, `dm/gfx/vk`)
- the extractor: the engine's `dm/tools/nx_extract/Program.cs` and `app/patches/nx_extract_merged_update.py`

Some things are deliberately left out: the C# app and its patches, the build time, the shader crates (the shader library has its
own versioning: chain and split version), `ibuilder-ffi`, the shim, and zsign/OpenSSL. The rewrite cache is keyed on the
framework id too, so an update that keeps the framework id also keeps the cached rewrites. `scripts/framework-id.sh
[device|simulator] [--id]` prints the id and its inputs without building anything.

### Fonts

The game needs the Switch's shared fonts. A build with the defaults embeds stand-ins derived from macOS's Arial
Unicode: that IPA is for your own phone and must never be shared (see [../NOTICE.md](../NOTICE.md)). For an IPA you share,
make free stand-ins from a freely licensed font and point the build at them:

```bash
python3 scripts/make-free-fonts.py <engine dm/tools dir> NotoSansCJKjp-Regular.otf <out dir>     # needs fonttools
FONT_STANDINS_DIR=<out dir> scripts/build-app.sh device
```

### The verify folder

Next to the IPA the build writes `<ipa>.verify/`: a copy of the verifier (`app/verify_ipa.sh` and the scripts it
sources) that measured the archive, so what is kept is exactly what checked it. A release can attach it as
`<ipa>.verify.tar.gz`; the release workflow runs it again on the downloaded IPA.
`app/compare_ipa.sh <old> <new>` lists what differs in structure and size between two IPAs.

## Other checks

```bash
scripts/check-patches-apply.sh            # app patches apply, in order, to a fresh copy of the snapshot
scripts/check-no-spawn.sh <GameApp.app>   # the app imports no fork / exec / spawn
scripts/check-forbidden-files.sh          # no key, certificate, profile, game container or Switch module in the tree
scripts/usb-install.sh <GameApp.app>      # sign a COPY with Xcode's provisioning and install it over USB (developer use)
```

## What CI does

`.github/workflows/ci.yml` runs on every push to `main` and every pull request: `fmt`, `clippy`, `test` (cargo nextest
plus doc tests, with skipped tests counted), `c-tests` (`app/c`), `no-spawn`, `patches`, `scripts`, `forbidden-files`,
`secrets` (gitleaks), `ios-ffi` (both iOS targets build) and `workflows` (actionlint). The IPA is not built in CI: it
needs inputs that are not in this repository.

## Releasing, and the SideStore / AltStore source

A *source* is a JSON file (`apps.json`) listing apps: name, bundle id, icon and, per version, the IPA URL, size, sha256
and minimum iOS. SideStore and AltStore read it, install the IPA from its URL, and offer an update when the listed
version is newer than the installed one. The IPAs are assets of the GitHub releases, not files in the repository.

Add the source in the client with this URL, or open `sidestore://source?url=<URL>` / `altstore://source?url=<URL>`:

```
https://raw.githubusercontent.com/Absenti/iBuilder/main/apps.json
```

The client re-signs the IPA with the user's certificate; the kind of certificate decides what iBuilder can do (see the
README). The same holds for a game the app exports: with no `.p12` imported its framework is signed ad hoc (stamped
`signature=adhoc-export`, never booted inside the app), and the installer re-signs it with the rest of the IPA. The size and sha256 in the source are those of the file as released. Each release has `<ipa>.sha256` next to the
IPA; compare it with `shasum -a 256` and with the entry in `apps.json`.

To release:

1. `scripts/build-app.sh device` (with `APP_VERSION` raised and `FONT_STANDINS_DIR` set).
2. Publish a GitHub release (not a pre-release) with the IPA, its `.sha256` and optionally `<ipa>.verify.tar.gz`
   (`tar -czf <ipa>.verify.tar.gz -C dist <ipa>.verify`).
3. `.github/workflows/release.yml` runs on `release: published`: it downloads the IPA, checks it against the `.sha256`
   asset and GitHub's digest, runs `app/verify_ipa.sh` (and the shipped verifier) and `scripts/check-no-spawn.sh`, then
   runs `scripts/update-source.py` (version, build and minimum iOS read from the IPA's `Info.plist`; the date from the
   release) and opens a pull request `release/apps-json-<tag>` that changes `apps.json` only.
4. Review and merge it. Merging publishes the update: the clients read `main`.

By hand, from the IPA on disk:

```bash
python3 -I scripts/update-source.py --tag v0.1.0 --ipa dist/GameApp-rust-<build>.ipa
python3 -I scripts/update-source.py --validate --require-versions
```

Things to know:

* Enable *Settings > Actions > General > Allow GitHub Actions to create and approve pull requests*, or the workflow can
  only push the branch. A pull request opened by a workflow does not start `pull_request` workflows: close and reopen
  it, or run `gh workflow run ci.yml --ref release/apps-json-<tag>`.
* A release must raise `CFBundleShortVersionString`. `update-source.py` replaces the entry with the same version and
  build, so repeating the pair gives the phones nothing to update to. The version is set in the app build, not in
  `apps.json`.
* The source also declares the app's permissions (`appPermissions`: the increased-memory-limit entitlement and the
  photo-library usage text); AltStore refuses an install whose declared permissions differ from the IPA's, so
  `update-source.py` compares them when it can read the IPA.
* `apps.json` keeps the newest 10 versions, newest first, in the v2 format (`versions[]`).
* Pre-releases are not listed automatically: run the workflow by hand (*Actions > Release > Run workflow*, with the tag).
