#!/bin/bash
# scripts/build-app.sh [device|simulator] -- the iBuilder app with the Rust chain, from sources, in one command.
#
#   scripts/build-app.sh device        -> dist/GameApp-rust-<buildid>.ipa (+ .sha256, dist/MANIFEST.txt): the unsigned / ad hoc device .ipa SideStore re-signs
#   scripts/build-app.sh simulator     -> $APP_WORK/simulator/app/GameApp.app, ad hoc signed, NOT installed (app/sim/*.sh install and drive it)
#
# What it builds (docs/ARCHITECTURE.md has the design):
#   1. libibuilder_ffi.a for the target: `cargo build --release -p ibuilder-ffi --features real-stages` (scripts/build-ios-ffi.sh). The features give the chain driver with the REAL stage
#      workers: profile, rewrite, assemble, sign, mods and stage 3 (ibuilder-shadersplit, ibuilder-shaderbuild, ibuilder-shader's finisher); the translator and SPIRV-Cross are callbacks.
#   2. libgameappshim-rust-<kind>.a: app/gameapp_shim_rust.c (the drop-in for gameapp_shim.c), zsign (+ OpenSSL, zlib, minizip) and its adapter, SPIRV-Cross's C wrapper, the NSZ converter
#      and a standard zstd decoder. No Python.h anywhere.
#   3. the .NET iOS app: the C# snapshot in app/gameapp/ (UNCHANGED; app/patches/*.patch are applied to a copy if there are any), the engine's extractor and shader translator sources,
#      `dotnet publish -r ios-arm64` (device) / `dotnet build -r iossimulator-arm64` (simulator). NO CPython, Python.framework, .py or pyext in the project or the bundle.
#   4. the bundle's data: the registered games' tables (dm/games/*/game.tsv), the host sources the profile reads, host-prebuilt.dylib. Nothing else.
#   5. signing (device: ad hoc, with the app's entitlements; simulator: ad hoc inside-out), the .ipa, app/verify_ipa.sh on the archive read back, dist/MANIFEST.txt.
#
# Prerequisites (a Mac): rustup with the aarch64-apple-ios[-sim] targets (Homebrew keg /opt/homebrew/opt/rustup; the script puts it on PATH for itself only), Xcode with the iOS SDK, the
# .NET SDK with the ios workload and the NuGet packages LibHac + SSH.NET in ~/.nuget/packages, and the READ-ONLY inputs below (nothing in them is ever written).
# No machine path is built in: the inputs come from the environment, or from the optional gitignored file scripts/local.env that this script sources first (scripts/local.env.example
# documents it). IOSBUILDER must be set, else the script stops and says so.
#   IOSBUILDER           a checkout of the original iOSBuilder project (IOSBUILDER_MAIN is accepted as an older name): the defaults of ZSIGN_SRC, OPENSSL_XC, ZSTD_SRC below live in it
#   IOSBUILDER_WORKTREE  the iOSBuilder tree with the newest work (default $IOSBUILDER): its out-load-ui/ (prebuilt hosts), out-spvc/ (SPIRV-Cross archives) and out-stage/out-engine/ (the patched engine)
#   NXPOCKET_ENGINE      the patched engine tree (default $IOSBUILDER_WORKTREE/out-stage/out-engine/engine)
#   ZSIGN_SRC, OPENSSL_XC, ZSTD_SRC   zsign checkout, OpenSSL.xcframework, zstd sources (defaults: $IOSBUILDER/out-sign/ and $IOSBUILDER/out/src/zstd/lib)
#   GAMEAPP_HOST         the prebuilt loadable host (default dist/host/host-prebuilt-loadable-{ios,sim}.dylib, the host rebuilt with engine-patches/ and pinned in app/inputs.tsv;
#                        else $IOSBUILDER_WORKTREE/out-load-ui/host-prebuilt-loadable-{ios,sim}.dylib)
#   IBUILDER_TEAM_SUFFIX optional: the team suffix your sideloading service gives the bundle id; adds its exact background-task id to Info.plist (see step 3)
#   APP_VERSION / APP_BUILD_NUMBER optional: CFBundleShortVersionString / CFBundleVersion of the app (a published release sets them; unset they stay 1)
#   FONT_STANDINS_DIR optional: ready-made shared-font stand-ins made only from freely licensed fonts (scripts/make-free-fonts.py); REQUIRED for any IPA you share (NOTICE.md)
# Each is pinned in app/inputs.tsv (size + sha256 as measured when this build was made); a mismatch is a WARNING in the log and in MANIFEST.txt (PIN_STRICT=1 makes it fatal).
# A missing SPIRV-Cross archive or patched engine is rebuilt under $APP_WORK with the worktree's own script (pointed there: nothing is written into the worktree).
#
# Where it writes: only $APP_WORK (default <repo>/target/app-work), $CARGO_TARGET_DIR (default <repo>/target) and dist/. Environment:
#   APP_WORK=<dir>   APP_OUT=<dir> (default <repo>/dist)   CARGO_TARGET_DIR=<dir>   SKIP_RUST=1 (reuse the archive from a previous run)   APP_NAME_SUFFIX=<text>
#   GAMEAPP_EVA_ENTITLEMENT=1 adds extended-virtual-addressing (off: a free Apple ID cannot carry it)   GAMEAPP_RAM_ENTITLEMENT=0 builds without the RAM entitlement
# The FRAMEWORK ID (step 1, GameAppBuild.FrameworkId; scripts/framework-id.sh prints it without building): what a built game's stamp is compared with after an app update. The build id
# (GameAppBuild.Id, <yyyymmdd-hhmmss>-rust<hash>) is new at every build, so comparing it made every game "Needs setup" after every update. The framework id hashes ONLY what shapes a built
# framework and its boot, computed from the files, deterministically (scripts/lib-hash.sh framework_id_listing; the listing goes to <work>/framework-inputs.txt and the MANIFEST):
#   FRAMEWORK_EPOCH (raised by hand), the host dylib's sha256, the shader chain, the cargo features, the stage crates ibuilder-nso ibuilder-a64 ibuilder-rewrite ibuilder-macho
#   ibuilder-sign ibuilder-profile ibuilder-mods ibuilder-nup, ibuilder-pipeline minus the files that never touch a framework (stage 3's shaders, logs, gates, the launch state
#   machine), ibuilder-cli's adapters.rs + Cargo.toml, Cargo.lock, app/game-tables, the engine's dm/games/settings.tsv and dm/games/*/game.tsv, the host sources the profile reads
#   (dm/host, dm/gfx, dm/gfx/nvn, dm/gfx/vk), the extractor (nx_extract/Program.cs + app/patches/nx_extract_merged_update.py).
#   NOT the C# app or its patches, NOT the time, NOT the shader crates (the shader library is versioned by its chain and split version). The same id: "Play", no rebuild; another: rebuild.
# The bundle id is dev.ibuilder.app, fixed: the sideloading service spends one of a small weekly quota of App IDs per distinct id (docs/DECISIONS.md trap 26 of the iOSBuilder repo).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
. "$ROOT/scripts/lib-hash.sh"      # hash_listing, rust_tree_hashes, dir_tree_hash (one xargs pass per tree; same values as the per-file loops)
. "$ROOT/scripts/lib-spawn.sh"     # filter_macho (one batched `file` for a list of paths)
KIND="${1:-device}"
APP_WORK="${APP_WORK:-$ROOT/target/app-work}"
APP_OUT="${APP_OUT:-$ROOT/dist}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# absolute paths (review r5 M6): the staging step cd's into folders and writes the .ipa from there, so a relative APP_WORK / APP_OUT would break step 8 after the whole build
case "$APP_WORK" in /*) ;; *) APP_WORK="$PWD/$APP_WORK" ;; esac
case "$APP_OUT" in /*) ;; *) APP_OUT="$PWD/$APP_OUT" ;; esac
case "$CARGO_TARGET_DIR" in /*) ;; *) CARGO_TARGET_DIR="$PWD/$CARGO_TARGET_DIR"; export CARGO_TARGET_DIR ;; esac
# APP_VERSION / APP_BUILD_NUMBER go into MSBuild properties (-p:): a ';' or ',' would be read as a separator. Digits and dots only (review r5 M4).
[ -z "${APP_VERSION:-}" ] || printf '%s' "$APP_VERSION" | grep -qE '^[0-9]+(\.[0-9]+){0,2}$' || { echo "FAIL: APP_VERSION='$APP_VERSION' is not 1, 1.2 or 1.2.3" >&2; exit 2; }
[ -z "${APP_BUILD_NUMBER:-}" ] || printf '%s' "$APP_BUILD_NUMBER" | grep -qE '^[0-9]+(\.[0-9]+){0,2}$' || { echo "FAIL: APP_BUILD_NUMBER='$APP_BUILD_NUMBER' is not 1, 1.2 or 1.2.3" >&2; exit 2; }
IOSBUILDER="${IOSBUILDER:-${IOSBUILDER_MAIN:-}}"
[ -n "$IOSBUILDER" ] || { echo "FAIL: IOSBUILDER is not set: point it at a checkout of the original iOSBuilder project (environment or scripts/local.env; see scripts/local.env.example)" >&2; exit 2; }
[ -d "$IOSBUILDER" ] || { echo "FAIL: IOSBUILDER=$IOSBUILDER is not a folder" >&2; exit 2; }
W="${IOSBUILDER_WORKTREE:-$IOSBUILDER}"
MAIN_CHECKOUT="$IOSBUILDER"
ENGINE="${NXPOCKET_ENGINE:-$W/out-stage/out-engine/engine}"
ZSIGN="${ZSIGN_SRC:-$MAIN_CHECKOUT/out-sign/signing/zsign}"
OSSL_XC="${OPENSSL_XC:-$MAIN_CHECKOUT/out-sign/signing/OpenSSL/Frameworks/OpenSSL.xcframework}"
ZSTD_SRC="${ZSTD_SRC:-$MAIN_CHECKOUT/out/src/zstd/lib}"
SPVC="${IOSBUILDER_SPVC:-$W/out-spvc}"
APP_ID="dev.ibuilder.app"
APPNAME=GameApp
case "$KIND" in
  device)    SDKNAME=iphoneos;        RID=ios-arm64;          TRIPLE=arm64-apple-ios26.0;            RUST_TARGET=aarch64-apple-ios;     OSSL_PLAT=ios-arm64
             HOST_DYLIB="${GAMEAPP_HOST:-$( [ -f "$ROOT/dist/host/host-prebuilt-loadable-ios.dylib" ] && echo "$ROOT/dist/host/host-prebuilt-loadable-ios.dylib" || echo "$W/out-load-ui/host-prebuilt-loadable-ios.dylib" )}"; SPVC_LIB_NAME=libspvc-ios.a; VERB=publish ;;
  simulator) SDKNAME=iphonesimulator; RID=iossimulator-arm64; TRIPLE=arm64-apple-ios26.0-simulator; RUST_TARGET=aarch64-apple-ios-sim; OSSL_PLAT=ios-arm64_x86_64-simulator
             HOST_DYLIB="${GAMEAPP_HOST:-$( [ -f "$ROOT/dist/host/host-prebuilt-loadable-sim.dylib" ] && echo "$ROOT/dist/host/host-prebuilt-loadable-sim.dylib" || echo "$W/out-load-ui/host-prebuilt-loadable-sim.dylib" )}"; SPVC_LIB_NAME=libspvc-sim.a; VERB=build ;;
  *) echo "usage: $0 [device|simulator]" >&2; exit 2 ;;
esac
if [ "$KIND" = simulator ]; then
  # The simulator build is a TEST VEHICLE, never installed on a phone. A simulator may already hold the real iBuilder (dev.ibuilder.app) with its Documents: a different id
  # gives this build its own data container, so nothing of the existing app is replaced or touched. SIM_APP_ID=dev.ibuilder.app installs over it on purpose.
  APP_ID="${SIM_APP_ID:-dev.ibuilder.app.rusttest}"
  [ "$APP_ID" = "dev.ibuilder.app" ] || printf '\033[1m  simulator build: bundle id %s (a test container of its own; the device .ipa is always dev.ibuilder.app)\033[0m\n' "$APP_ID" >&2
fi
RUST_SUFFIX="${RUST_TARGET#aarch64-apple-}"       # ios | ios-sim
WD="$APP_WORK/$KIND"                               # this kind's working folder
SHARED="$APP_WORK/shared"
SDK="$(xcrun --sdk "$SDKNAME" --show-sdk-path)"
OSSL_FW="$OSSL_XC/$OSSL_PLAT/OpenSSL.framework"
say() { printf '\033[1m%s\033[0m\n' "$*" >&2; }
die() { printf '\033[31mFAIL: %s\033[0m\n' "$*" >&2; exit 1; }
T0=$SECONDS
mkdir -p "$APP_WORK" "$WD" "$SHARED" "$APP_OUT"
INPUTS="$WD/inputs.txt"; : > "$INPUTS"

# ---------------------------------------------------------------- 0. inputs: read-only, pinned, recorded
say "0. the read-only inputs (never written) and their pins (app/inputs.tsv)"
pin_state() {   # name path size sha: compares with app/inputs.tsv, prints MATCH / DIFFERS / UNPINNED
  local name="$1" size="$3" sha="$4" line
  line="$(awk -F'\t' -v n="$name" '$1==n {print $3 "\t" $4}' "$ROOT/app/inputs.tsv" 2>/dev/null)"
  if [ -z "$line" ]; then echo UNPINNED; elif [ "$line" = "$size	$sha" ]; then echo MATCH; else echo "DIFFERS (pinned ${line%%	*} bytes ${line##*	})"; fi
}
note_input() {  # name path description
  local name="$1" path="$2" why="$3" size sha mt st
  if [ -d "$path" ]; then
    size="dir"; sha="$(dir_tree_hash "$path")"
  else size="$(stat -f %z "$path")"; sha="$(shasum -a 256 < "$path" | cut -d' ' -f1)"; fi
  mt="$(stat -f '%Sm' -t '%Y-%m-%dT%H:%M:%S' "$path")"
  st="$(pin_state "$name" "$path" "$size" "$sha")"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$path" "$size" "$sha" "$mt" "$st" "$why" >> "$INPUTS"
  printf '  %-14s %s  (%s bytes, %s)  pin: %s\n' "$name" "$path" "$size" "$mt" "$st" >&2
  case "$st" in DIFFERS*) printf '\033[33m  WARNING: %s is not what app/inputs.tsv pins; the MANIFEST records what was used\033[0m\n' "$name" >&2
                [ "${PIN_STRICT:-0}" = 1 ] && die "PIN_STRICT=1 and $name differs from its pin" ;; esac
}
command -v xcrun >/dev/null && [ -d "$SDK" ] || die "no $SDKNAME SDK (Xcode)"
command -v dotnet >/dev/null || die "no dotnet SDK"
for f in "$ROOT/app/gameapp_shim_rust.c" "$ROOT/app/gameapp/GameAppMain.cs" "$ROOT/crates/ibuilder-ffi/include/ibuilder.h"; do [ -f "$f" ] || die "missing $f"; done

# the patched engine: the extractor and translator sources, the registered games' tables, the host sources the profile reads, the shader chain id
if [ ! -d "$ENGINE/dm/tools/nx_extract" ]; then
  say "  no patched engine at $ENGINE: building one under $SHARED with the worktree's tools/patched_engine.sh (writes only there)"
  ENGINE="$(GIT_OPTIONAL_LOCKS=0 NXPOCKET_ENGINE_SOURCE="$W/engine" bash "$W/tools/patched_engine.sh" "$SHARED/engine-out")" || die "patched_engine.sh failed"
fi
[ -d "$ENGINE/dm/tools/nx_extract" ] || die "no engine at $ENGINE"
[ -f "$(dirname "$ENGINE")/stamp.txt" ] && note_input engine-stamp "$(dirname "$ENGINE")/stamp.txt" "the patched engine's key: the source commit and the sha256 of every paches/engine/*.patch it was cloned with"
note_input "host-$KIND" "$HOST_DYLIB" "prebuilt loadable host (load/prebuild_loadable.sh of the worktree, from the patched engine): the game is grafted onto it; third-party engine code, not part of the Rust rewrite"
note_input zsign "$ZSIGN/src" "zsign sources (patched by iPorterNX: in-memory asset, dylib entitlements) compiled into the app"
note_input "openssl-$KIND" "$OSSL_FW/OpenSSL" "OpenSSL.framework for zsign (embedded in the app, as before)"
note_input zstd-src "$ZSTD_SRC/decompress" "zstd sources: the standard decoder object (NSZ, and a future .nup reader)"
[ -d "$OSSL_FW" ] && [ -d "$ZSIGN/src" ] && [ -f "$ZSTD_SRC/zstd.h" ] && [ -f "$HOST_DYLIB" ] || die "a required input is missing (see above)"
SPVC_LIB="$SPVC/$SPVC_LIB_NAME"
if [ ! -f "$SPVC_LIB" ]; then
  say "  no $SPVC_LIB: building SPIRV-Cross under $SHARED/spvc with the worktree's tools/build_spvc_ios.sh (writes only there)"
  IOSBUILDER_OUT="$SHARED/spvc" SPVC_SRC="${SPVC_SRC:-$MAIN_CHECKOUT/out/src/SPIRV-Cross}" bash "$W/tools/build_spvc_ios.sh" > "$WD/spvc-build.log" 2>&1 || die "build_spvc_ios.sh failed (see $WD/spvc-build.log)"
  SPVC="$SHARED/spvc"; SPVC_LIB="$SPVC/$SPVC_LIB_NAME"
fi
note_input "spvc-$KIND" "$SPVC_LIB" "SPIRV-Cross (pin f11ba9f, tools/build_spvc_ios.sh) as a static library: the C wrapper app/c/spvc_cli.c drives its C API in process"
note_input spvc-include "$SPVC/include" "spirv_cross_c.h and spirv.h of that build"
SHADER_CHAIN="$(grep -o 'DMSH_CHAIN "[0-9a-f]*"' "$ENGINE/dm/gfx/nvn/dmsh_chain.h" | cut -d'"' -f2)"
[ -n "$SHADER_CHAIN" ] || die "no DMSH_CHAIN in $ENGINE/dm/gfx/nvn/dmsh_chain.h"
strings -a "$HOST_DYLIB" > "$WD/host-strings.txt"   # a file, not a pipe: grep -q leaves at its first match and pipefail would report the dying writer
grep -qx "$SHADER_CHAIN" "$WD/host-strings.txt" || die "the host does not carry the shader chain $SHADER_CHAIN of the patched engine: stage 3 would build a library the host refuses"
echo "  shader chain $SHADER_CHAIN: the patched engine's header and the host agree" >&2

# ---------------------------------------------------------------- 1. the Rust archive
SRC_HASH_FILE="$WD/rust-sources.txt"
rust_tree_hashes "$ROOT" > "$SRC_HASH_FILE"        # lib-hash.sh: one line per crate, name TAB sha256 of its src/, Cargo.toml, build.rs, include/
CARGO_LOCK_SHA="$(shasum -a 256 < "$ROOT/Cargo.lock" | cut -d' ' -f1)"   # computed once, printed again in the MANIFEST
RUST_ID="$( { cat "$SRC_HASH_FILE"; echo "$CARGO_LOCK_SHA"; } | shasum -a 256 | cut -c1-8)"
BUILD_ID="${APP_BUILD_ID:-$(date -u +%Y%m%d-%H%M%S)-rust$RUST_ID}"   # APP_BUILD_ID pins it: a rebuild of the simulator app keeps the framework stamp of the data already in the simulator valid
# The Switch 2 package reader (crates/ibuilder-nup, the NupReader adapter of ibuilder-cli, feature `nup`) joins the real adapter set when ibuilder-cli has that feature and it is not already
# a default one: `--features ibuilder-cli/nup` needs no change to any Cargo.toml. Without it the chain's stage 1 for a .nup says "no NUP reader in this build" and this build says so too.
FFI_FEATURES="real-stages"
NUP_STATE="NOT in this build (ibuilder-cli has no feature nup yet: a .nup fails at stage 1 with 'no NUP reader in this build'; NSP / NSZ / XCI games are unaffected)"
if grep -qE '^nup *=' "$ROOT/crates/ibuilder-cli/Cargo.toml" 2>/dev/null; then
  if grep -E '^default *=' "$ROOT/crates/ibuilder-cli/Cargo.toml" | grep -q '"nup"'; then NUP_STATE="in this build (ibuilder-cli default feature nup)"
  else FFI_FEATURES="real-stages,ibuilder-cli/nup"; NUP_STATE="in this build (ibuilder-cli/nup enabled by this script)"; fi
fi
echo "  Switch 2 package reader (.nup): $NUP_STATE" >&2
# the framework id (header above; scripts/lib-hash.sh): stamped with every framework this app builds, and what an updated app compares a built game's stamp with
FRAMEWORK_INPUTS="$WD/framework-inputs.txt"
framework_id_listing "$ROOT" "$ENGINE" "$HOST_DYLIB" "$SHADER_CHAIN" "$FFI_FEATURES" > "$FRAMEWORK_INPUTS"
FRAMEWORK_ID="$(framework_id_of < "$FRAMEWORK_INPUTS")"
grep -q $'\t-$' "$FRAMEWORK_INPUTS" && die "an input of the framework id is missing ($(grep $'\t-$' "$FRAMEWORK_INPUTS" | cut -f1 | tr '\n' ' ')): see $FRAMEWORK_INPUTS"
echo "  framework id $FRAMEWORK_ID (a built game whose stamp has it boots without a rebuild; inputs in $FRAMEWORK_INPUTS)" >&2
say "1. libibuilder_ffi.a for $RUST_TARGET  (build id $BUILD_ID, features $FFI_FEATURES)"
FFI_OUT="$WD/ffi"
FFI_STATE="built by scripts/build-ios-ffi.sh"
if [ "${SKIP_RUST:-0}" = 1 ] && [ -f "$FFI_OUT/libibuilder_ffi-$RUST_SUFFIX.a" ]; then say "  SKIP_RUST=1: reusing $FFI_OUT/libibuilder_ffi-$RUST_SUFFIX.a"; FFI_STATE="SKIP_RUST=1: the archive of a previous run, not checked against this tree"
else
  ( cd "$ROOT" && TARGETS="$RUST_TARGET" FEATURES="$FFI_FEATURES" IPHONEOS_DEPLOYMENT_TARGET=14.0 "$ROOT/scripts/build-ios-ffi.sh" "$FFI_OUT" ) > "$WD/cargo.log" 2>&1 \
    || { tail -30 "$WD/cargo.log" >&2; die "the Rust build failed (full log $WD/cargo.log)"; }
  grep -E '^== ' "$WD/cargo.log" | sed 's/^/  /' >&2
  FFI_STATE="$(grep -h '^== ffi cache' "$WD/cargo.log" | tr '\n' ' ')"; [ -n "$FFI_STATE" ] || FFI_STATE="built by scripts/build-ios-ffi.sh"
fi
RUSTLIB="$FFI_OUT/libibuilder_ffi-$RUST_SUFFIX.a"
[ -f "$RUSTLIB" ] || die "no $RUSTLIB"
RUSTLIB_SHA="$(shasum -a 256 < "$RUSTLIB" | cut -d' ' -f1)"
echo "  $RUSTLIB  $(stat -f %z "$RUSTLIB") bytes  sha256 $RUSTLIB_SHA" >&2

# ---------------------------------------------------------------- 2. zsign + the shim + the C helpers -> one archive
say "2. zsign + libz/minizip + app/gameapp_shim_rust.c + spvc_cli.c + nsz/zstd -> libgameappshim-rust-$KIND.a"
OBJ="$WD/obj"; rm -rf "$OBJ" "$WD/include"; mkdir -p "$OBJ" "$WD/include"
ln -s "$OSSL_FW/Headers" "$WD/include/openssl"
CXXFLAGS=(-target "$TRIPLE" -isysroot "$SDK" -std=c++11 -Oz -fPIC -I"$ZSIGN/src" -I"$ZSIGN/src/common" -I"$ZSIGN/src/third-party/zlib" -I"$ZSIGN/src/third-party/minizip" -I"$WD/include")
failed=0
while IFS= read -r src; do
  [ -n "$src" ] || continue
  xcrun --sdk "$SDKNAME" clang++ "${CXXFLAGS[@]}" -c "$src" -o "$OBJ/z_$(basename "${src%.cpp}").o" || { echo "  FAILED compiling $(basename "$src")" >&2; failed=1; }
done < <(find "$ZSIGN/src" -name '*.cpp' ! -name 'zsign.cpp' | sort)
[ "$failed" = 0 ] || die "zsign did not compile for $KIND"
CFLAGS3=(-target "$TRIPLE" -isysroot "$SDK" -Oz -fPIC -I"$ZSIGN/src/third-party/zlib" -I"$ZSIGN/src/third-party/minizip")
while IFS= read -r src; do
  [ -n "$src" ] || continue
  xcrun --sdk "$SDKNAME" clang "${CFLAGS3[@]}" -c "$src" -o "$OBJ/c_$(basename "${src%.c}").o" || { echo "  FAILED compiling C $(basename "$src")" >&2; failed=1; }
done < <(find "$ZSIGN/src/third-party/zlib" "$ZSIGN/src/third-party/minizip" -name '*.c' ! -name 'iowin32.c' | sort)
[ "$failed" = 0 ] || die "zsign's C libraries did not compile for $KIND"
xcrun --sdk "$SDKNAME" clang++ "${CXXFLAGS[@]}" -DNXPOCKET_WITH_ZSIGN -I"$ROOT/app/c" -c "$ROOT/app/c/inapp_shim.cpp" -o "$OBJ/z_nxpocket_inapp_shim.o" || die "inapp_shim.cpp does not compile against zsign's API ($KIND)"
# the drop-in shim: plain C against ibuilder.h, no Python.h
xcrun --sdk "$SDKNAME" clang -target "$TRIPLE" -isysroot "$SDK" -O1 -g0 -Wall -I"$ROOT/app/c" -I"$ROOT/crates/ibuilder-ffi/include" \
  -c "$ROOT/app/gameapp_shim_rust.c" -o "$OBJ/c_gameapp_shim_rust.o" || die "app/gameapp_shim_rust.c did not compile"
xcrun --sdk "$SDKNAME" clang -target "$TRIPLE" -isysroot "$SDK" -O2 -g0 -I"$SPVC/include" -c "$ROOT/app/c/spvc_cli.c" -o "$OBJ/c_spvc_cli.o" || die "spvc_cli.c did not compile"
# NSZ converter + a STANDARD zstd stream decoder as ONE relocatable object whose only external symbols are gameapp_nsz_to_nsp and gameapp_stdzstd_*: zstd's own symbols become local
# (`ld -r` with an export list), so the standard decoder is the one the NSZ converter uses and nothing clashes. (The worktree's app linked the ZBIC-variant zstd of libpyext_static.a for NSZ.)
ZOBJ="$WD/zstd-obj"; rm -rf "$ZOBJ"; mkdir -p "$ZOBJ"
zo=()
for f in "$ZSTD_SRC"/common/{debug,entropy_common,error_private,fse_decompress,xxhash,zstd_common}.c "$ZSTD_SRC"/decompress/*.c "$ROOT/app/c/stdzstd.c" "$ROOT/app/c/nsz_decompress.c"; do
  o="$ZOBJ/$(basename "$f" .c).o"
  xcrun --sdk "$SDKNAME" clang -target "$TRIPLE" -isysroot "$SDK" -O2 -g0 -DZSTD_DISABLE_ASM=1 -DZSTD_MULTITHREAD=0 -DZSTD_LEGACY_SUPPORT=0 -I"$ZSTD_SRC" -I"$ZSTD_SRC/common" -c "$f" -o "$o" || die "$(basename "$f") did not compile"
  zo+=("$o")
done
printf '_gameapp_stdzstd_new\n_gameapp_stdzstd_feed\n_gameapp_stdzstd_free\n_gameapp_nsz_to_nsp\n' > "$ZOBJ/exports"
xcrun ld -r -arch arm64 -exported_symbols_list "$ZOBJ/exports" "${zo[@]}" -o "$OBJ/c_nsz_stdzstd.o" || die "ld -r of the zstd/nsz object failed"
LIBSHIM="$WD/libgameappshim-rust-$KIND.a"
rm -f "$LIBSHIM"; libtool -static -o "$LIBSHIM" "$OBJ"/*.o 2>/dev/null || die "libtool refused $LIBSHIM"
nm -gU "$LIBSHIM" 2>/dev/null | awk '$2=="T"{print $3}' | sort > "$WD/shim-exports.txt"
echo "  $LIBSHIM ($(stat -f %z "$LIBSHIM") bytes), $(wc -l < "$WD/shim-exports.txt" | tr -d ' ') global functions" >&2
# every [DllImport("__Internal")] gameapp_* the C# declares must be defined here: the contract with the managed half
missing=0
# the snapshot's imports and the ones the C# patches add (their `+` lines: app/patches/0096 imports gameapp_gpu_bc_textures and gameapp_sign_framework_again)
IMPORTS="$( { grep -h 'extern [^;(]*gameapp_[a-z_0-9]*(' "$ROOT"/app/gameapp/GameApp*.cs; cat "$ROOT"/app/patches/*.patch 2>/dev/null | grep '^+' | grep 'extern [^;(]*gameapp_[a-z_0-9]*(' || true; } | grep -o 'gameapp_[a-z_0-9]*(' | tr -d '(' | sort -u)"
# one grep for all the symbols (it was one per symbol): every wanted `_name` that is not a whole line of the export list
MISSING_SYMS="$(printf '_%s\n' $IMPORTS | grep -vxF -f "$WD/shim-exports.txt" || true)"
for sym in $MISSING_SYMS; do echo "  MISSING export: ${sym#_}" >&2; missing=1; done
[ "$missing" = 0 ] || die "the shim does not export every gameapp_* function the C# imports"
echo "  every gameapp_* import of the C# is exported ($(echo "$IMPORTS" | wc -l | tr -d ' ')): $(echo $IMPORTS)" >&2
grep -q '^_gameapp_stdzstd_new$' "$WD/shim-exports.txt" && grep -q '^_gameapp_nsz_to_nsp$' "$WD/shim-exports.txt" || die "the zstd/nsz object lost its exports"

# ---------------------------------------------------------------- 3. the generated .NET project
say "3. the app project (.NET iOS, StartupObject GameAppMain): the C# snapshot + the engine's extractor and shader translator"
rm -rf "$WD/proj"; mkdir -p "$WD/proj"; P="$WD/proj"
cp "$ENGINE/dm/tools/nx_extract/Program.cs" "$P/Program.cs"
sed -i '' 's/static int Main(string\[\] a)/public static int Main(string[] a)/' "$P/Program.cs"
grep -q "public static int Main(string\[\] a)" "$P/Program.cs" || die "the accessibility patch of Program.cs did not apply"
# a package that carries its own update (a merged XCI, a merged NSP) is built WITH it: without this the base game is read and Mario Kart 8 Deluxe's 1.0.0 is a 32-bit program (app/patches/nx_extract_merged_update.py)
python3 "$ROOT/app/patches/nx_extract_merged_update.py" "$P/Program.cs" || die "the merged-update patch of Program.cs did not apply (the engine's extractor source moved?)"
for f in "$ROOT"/app/gameapp/GameApp*.cs; do cp "$f" "$P/"; done
rm -rf "$P/Assets.xcassets"; cp -R "$ROOT/app/gameapp/Assets.xcassets" "$P/Assets.xcassets"
cp "$ROOT/app/gameapp/Info.plist.fragment" "$P/Info.plist.fragment"; cp "$ROOT/app/gameapp/GameApp.entitlements" "$P/GameApp.entitlements"
shopt -s nullglob; PATCHES=("$ROOT"/app/patches/*.patch); shopt -u nullglob
for p in "${PATCHES[@]+"${PATCHES[@]}"}"; do
  ( cd "$P" && patch -p1 --forward --no-backup-if-mismatch < "$p" ) > "$WD/patch-$(basename "$p").log" 2>&1 || { cat "$WD/patch-$(basename "$p").log" >&2; die "$(basename "$p") does not apply to the C# snapshot"; }
  echo "  applied $(basename "$p")" >&2
done
[ "${#PATCHES[@]}" = 0 ] && echo "  no C# patches: the snapshot is built as it is" >&2
# app/patches/0056 permits the exact background-task id an installer gives the app, `dev.ibuilder.app.@TEAM_SUFFIX@.build.*`: the team suffix is not part of this repository, so the
# placeholder is filled in from IBUILDER_TEAM_SUFFIX, and the entry is left out when that is not set (the wildcard of the base id is refused by the system for a suffixed id).
if grep -q '@TEAM_SUFFIX@' "$P/Info.plist.fragment"; then
  if [ -n "${IBUILDER_TEAM_SUFFIX:-}" ]; then
    case "$IBUILDER_TEAM_SUFFIX" in *[!A-Za-z0-9]*) die "IBUILDER_TEAM_SUFFIX=$IBUILDER_TEAM_SUFFIX: letters and digits only" ;; esac
    sed -i '' "s/@TEAM_SUFFIX@/$IBUILDER_TEAM_SUFFIX/g" "$P/Info.plist.fragment"
    echo "  Info.plist: background task id dev.ibuilder.app.<team suffix>.build.* added (IBUILDER_TEAM_SUFFIX)" >&2
  else
    sed -i '' '/@TEAM_SUFFIX@/d' "$P/Info.plist.fragment"
    echo "  Info.plist: no IBUILDER_TEAM_SUFFIX: the team-suffixed background task id is left out (a sideloaded build then cannot keep the shader warm-up going in the background)" >&2
  fi
fi
# the engine's shader translator, compiled into the app (gameapp/xlate/README.md): Ryujinx sources copied from the patched engine, never built in place
XL="$P/xlate"; RYU="$ENGINE/dm/third_party/ryujinx/src"; XT="$ENGINE/dm/gfx/tools/dmsh_xlate"
[ -f "$XT/Program.cs" ] && [ -d "$RYU/Ryujinx.Graphics.Shader" ] || die "no dmsh_xlate / Ryujinx sources in $ENGINE"
mkdir -p "$XL/ryu/Common/Utilities" "$XL/ryu/Common/Memory" "$XL/ryu/Common/Pools"
rsync -a --exclude obj --exclude bin --exclude '._*' --exclude '*.csproj' "$RYU/Ryujinx.Graphics.Shader" "$RYU/Spv.Generator" "$XL/ryu/"
for f in Utilities/BitUtils.cs Utilities/BitfieldExtensions.cs Pools/ObjectPool.cs Memory/ArrayPtr.cs Memory/IArray.cs Memory/StructArrayHelpers.cs Memory/Ptr.cs Memory/Box.cs; do
  cp "$RYU/Ryujinx.Common/$f" "$XL/ryu/Common/$f" || die "missing Ryujinx.Common/$f"
done
cp "$XT/Program.cs" "$XT/dmsh_xlate.csproj" "$XL/"          # UNCHANGED: --source-sha hashes these two files
cp "$ROOT/app/gameapp/xlate/GameAppXlate.cs" "$ROOT/app/gameapp/xlate/XlateCommonShim.cs" "$XL/"
printf 'internal static class GameAppBuild { public const string Id = "%s"; public const string ShaderChain = "%s"; public const string FrameworkId = "%s"; }\n' "$BUILD_ID" "$SHADER_CHAIN" "$FRAMEWORK_ID" > "$P/GameAppBuild.cs"
echo "  build id $BUILD_ID; framework id (what a built game's stamp is compared with) $FRAMEWORK_ID; shader chain $SHADER_CHAIN" >&2
plutil -lint "$P/Info.plist.fragment" >/dev/null 2>&1 || die "Info.plist.fragment is not a valid plist"
plutil -lint "$P/GameApp.entitlements" >/dev/null 2>&1 || die "GameApp.entitlements is not a valid plist"
# plutil is lenient where the SDK's parser is not: a `--` INSIDE an XML comment is legal to plutil and fatal to the SDK. xmllint is strict.
xmllint --noout "$P/Info.plist.fragment" "$P/GameApp.entitlements" 2>/dev/null || die "a plist is not well-formed XML (a '--' inside a comment is the usual cause)"
[ "$(/usr/libexec/PlistBuddy -c 'Print :com.apple.developer.kernel.increased-memory-limit' "$P/GameApp.entitlements" 2>/dev/null)" = true ] || die "GameApp.entitlements has no increased-memory-limit = true"
signing=""; [ "$KIND" = device ] && signing="<EnableCodeSigning>false</EnableCodeSigning>"
cat > "$P/gameapp.csproj" <<XML
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net11.0-ios</TargetFramework>
    <!-- Non-concurrent GC: the shader translator runs J calls at once (gameapp/xlate/GameAppXlate.cs) and allocates ~2.7 MB a shader -->
    <ConcurrentGarbageCollection>false</ConcurrentGarbageCollection>
    <OutputType>Exe</OutputType>
    <AssemblyName>GameApp</AssemblyName>
    <ApplicationId>$APP_ID</ApplicationId>
    <ApplicationTitle>iBuilder</ApplicationTitle>
    <AppIcon>AppIcon</AppIcon>
    <ApplicationVersion>1</ApplicationVersion>
    <SupportedOSPlatformVersion>26.0</SupportedOSPlatformVersion>
    <Nullable>disable</Nullable>
    <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
    <InvariantGlobalization>true</InvariantGlobalization>
    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>
    <StartupObject>GameAppMain</StartupObject>
    $signing
    <MtouchExtraArgs>-lc++</MtouchExtraArgs>
  </PropertyGroup>
  <ItemGroup>
    <Compile Include="$P/Program.cs" />
$(for f in "$P"/GameApp*.cs; do echo "    <Compile Include=\"$f\" />"; done)
    <Compile Include="$P/xlate/ryu/**/*.cs" />
    <Compile Include="$P/xlate/Program.cs" />
    <Compile Include="$P/xlate/GameAppXlate.cs" />
    <Compile Include="$P/xlate/XlateCommonShim.cs" />
    <EmbeddedResource Include="$P/xlate/Program.cs" LogicalName="src0" />
    <EmbeddedResource Include="$P/xlate/dmsh_xlate.csproj" LogicalName="src1" />
    <EmbeddedResource Include="$P/xlate/ryu/Ryujinx.Graphics.Shader/CodeGen/Glsl/HelperFunctions/MultiplyHighS32.glsl" LogicalName="Ryujinx.Graphics.Shader.CodeGen.Glsl.HelperFunctions.MultiplyHighS32.glsl" />
    <EmbeddedResource Include="$P/xlate/ryu/Ryujinx.Graphics.Shader/CodeGen/Glsl/HelperFunctions/MultiplyHighU32.glsl" LogicalName="Ryujinx.Graphics.Shader.CodeGen.Glsl.HelperFunctions.MultiplyHighU32.glsl" />
    <EmbeddedResource Include="$P/xlate/ryu/Ryujinx.Graphics.Shader/CodeGen/Glsl/HelperFunctions/SwizzleAdd.glsl" LogicalName="Ryujinx.Graphics.Shader.CodeGen.Glsl.HelperFunctions.SwizzleAdd.glsl" />
  </ItemGroup>
  <ItemGroup>
    <PackageReference Include="LibHac" Version="0.19.0" />
    <PackageReference Include="SSH.NET" Version="2026.0.1" />
    <PartialAppManifest Include="$P/Info.plist.fragment" />
    <ImageAsset Include="Assets.xcassets/**/*" />
  </ItemGroup>
  <ItemGroup>
    <!-- no Python.framework, no libpyext_static.a: the chain is libibuilder_ffi.a (Rust) behind the shim -->
    <NativeReference Include="$LIBSHIM"><Kind>Static</Kind></NativeReference>
    <NativeReference Include="$RUSTLIB"><Kind>Static</Kind><ForceLoad>true</ForceLoad><SmartLink>false</SmartLink></NativeReference>
    <NativeReference Include="$SPVC_LIB"><Kind>Static</Kind><IsCxx>True</IsCxx></NativeReference>
    <NativeReference Include="$OSSL_FW"><Kind>Framework</Kind></NativeReference>
  </ItemGroup>
</Project>
XML
if sed -e 's/<!--//g' -e 's/-->//g' "$P/gameapp.csproj" | grep -q -- '--'; then die "the generated csproj carries a double hyphen (MSB4025)"; fi
if sed -e 's/<!--.*-->//' "$P/gameapp.csproj" | grep -qiE 'python|pyext'; then die "the generated csproj references Python (outside a comment)"; fi

# ---------------------------------------------------------------- 4. dotnet build / publish
OUTDIR="$WD/out"; rm -rf "$OUTDIR"
say "4. dotnet $VERB ($RID)  (log: $WD/dotnet.log)"
DN0=$SECONDS
# APP_VERSION (CFBundleShortVersionString, what an installer compares: SideStore offers an update only when it is higher) and APP_BUILD_NUMBER (CFBundleVersion) are optional;
# unset, the csproj keeps its own value (1), as every earlier build.
VERARGS=(); [ -n "${APP_VERSION:-}" ] && VERARGS+=("-p:ApplicationDisplayVersion=$APP_VERSION"); [ -n "${APP_BUILD_NUMBER:-}" ] && VERARGS+=("-p:ApplicationVersion=$APP_BUILD_NUMBER")
dotnet "$VERB" "$P/gameapp.csproj" -c Release -r "$RID" -nologo -o "$OUTDIR" ${VERARGS[@]+"${VERARGS[@]}"} > "$WD/dotnet.log" 2>&1
DNRC=$?
grep -E ' error |error MSB|Build FAILED|warning MT|warning IL' "$WD/dotnet.log" | head -20 >&2
[ "$DNRC" = 0 ] || { tail -30 "$WD/dotnet.log" >&2; die "dotnet $VERB failed (rc $DNRC)"; }
echo "  dotnet $VERB: $((SECONDS - DN0)) s" >&2
APP=""
for candidate in "$OUTDIR" "$P/bin"; do
  [ -d "$candidate" ] || continue
  APP="$(find "$candidate" -maxdepth 6 -name "$APPNAME.app" -type d | head -1)"
  [ -n "$APP" ] && break
done
[ -n "$APP" ] || die "no $APPNAME.app came out of the $VERB"
# the app the SDK published is moved to a stable place: everything below edits this one copy
rm -rf "$WD/app"; mkdir -p "$WD/app"; ditto "$APP" "$WD/app/$APPNAME.app" || die "could not copy the bundle"
APP="$WD/app/$APPNAME.app"
say "  the bundle: $APP"

# ---------------------------------------------------------------- 5. the data the Rust chain reads from the bundle (no Python, no engine scripts)
say "5. the bundle's data: registered games' tables, host sources for the hook set, the prebuilt host"
mkdir -p "$APP/dm/games" "$APP/dm/host" "$APP/dm/gfx/nvn" "$APP/dm/gfx/vk"
cp "$ENGINE/dm/games/settings.tsv" "$APP/dm/games/"
tables=0; extras=0
for p in "$ENGINE"/dm/games/*/game.tsv; do
  [ -f "$p" ] || continue
  d="$(basename "$(dirname "$p")")"; mkdir -p "$APP/dm/games/$d"; cp "$p" "$APP/dm/games/$d/game.tsv"; tables=$((tables + 1))
  # The game's other small INPUTS of the shader build and of the first boot, which the stage-3 pack takes from this folder (every *.tsv of it): `bf_want.tsv` / `dual_want.tsv` (which pixel
  # shaders get the extra variants), `pso_manifest.tsv` (the pipeline prewarm seed list: without it a first start builds every pipeline of the first frames on demand, minutes of black screen
  # on a cold Metal cache: device log 2026-11-19, Mario Kart 8 Deluxe "[RED] pipeline prewarm: seed list does not exist"), `bench.tsv`; and the two inputs the Rust build READS from the game's
  # folder as the engine's Mac build does: `geo_mesh.tsv` (vertex/geometry pairs that get a mesh library: dmsh_chain.INPUT_TABLES, mesh_pairs) and `shader_facts_manual.tsv` (hand-written
  # facts merged over the split's: shader_split.py). The list is the one of ibuilder-pipeline's PACK_INPUT_TABLES (a pack made before a table was bundled is topped up from it at the boot).
  # The split's own tables (shader_map, shader_pairs, pso_pairs, shader_facts) are made on the phone from the game's files and are NOT copied.
  for extra in bf_want.tsv dual_want.tsv geo_mesh.tsv shader_facts_manual.tsv pso_manifest.tsv bench.tsv; do
    [ -f "$(dirname "$p")/$extra" ] && cp "$(dirname "$p")/$extra" "$APP/dm/games/$d/$extra" && extras=$((extras + 1))
  done
  # this repository's own tables for the game (app/game-tables/<id>/*.tsv) replace the engine's of the same name: what was learned on a real phone (docs/DIAGNOSTICS.md)
  for over in "$ROOT/app/game-tables/$d"/*.tsv; do
    [ -f "$over" ] && cp "$over" "$APP/dm/games/$d/$(basename "$over")" && echo "  game table $d/$(basename "$over"): this repository's, not the engine's" >&2
  done
done
# a game this repository has a table for and the engine has none (ported from a newer engine: South Park, Stick of Truth) gets its folder too, so the phone renders its
# registered profile instead of deriving a provisional one
for over_dir in "$ROOT"/app/game-tables/*/; do
  d="$(basename "$over_dir")"
  [ -f "$over_dir/game.tsv" ] || continue
  [ -d "$APP/dm/games/$d" ] && continue
  mkdir -p "$APP/dm/games/$d"
  for over in "$over_dir"*.tsv; do [ -f "$over" ] && cp "$over" "$APP/dm/games/$d/$(basename "$over")"; done
  tables=$((tables + 1)); echo "  game table $d: this repository's only (the engine has none)" >&2
done
cp "$ENGINE/dm/host/"*.c "$APP/dm/host/"; cp "$ENGINE/dm/gfx/"*.c "$APP/dm/gfx/"; cp "$ENGINE/dm/gfx/nvn/"*.c "$APP/dm/gfx/nvn/"; cp "$ENGINE/dm/gfx/vk/"*.m "$APP/dm/gfx/vk/"
cp "$HOST_DYLIB" "$APP/host-prebuilt.dylib"
# The shared-font stand-ins (dm/font/: standard.ttf, the Chinese/Korean cuts, nintendo_ext.ttf with the button icons), which the host serves for the system shared fonts and the pipeline hands over as
# DM_FONT_DIR. The engine's note "Shared fonts on iOS": without them GetSharedFontLoadState says "not loaded", and a game that waits for the font never boots (device log 2026-11-19: Mario Kart 8 Deluxe,
# black screen, "[RED] cannot open the standard shared font"). The latest iOSBuilder app does not ship them either. They are made here by the engine's own dm/tools/make_shared_fonts.py (fontTools:
# FONT_PYTHON names a python that has it; STD_FONT names another source font than macOS's Arial Unicode, whose licence is Apple's/Monotype's: for a private build only).
FONT_PYTHON="${FONT_PYTHON:-python3}"
mkdir -p "$APP/dm/font"
# FONT_STANDINS_DIR: a folder of ready-made stand-ins made ONLY from freely licensed fonts (scripts/make-free-fonts.py): used as they are, nothing is made from macOS's fonts. This is the way to build an IPA
# that may be shared (a published release); without it the stand-ins come from macOS's Arial Unicode (below) and the IPA is for your own phone only (NOTICE.md).
FONTS_STATE=""
if [ -n "${FONT_STANDINS_DIR:-}" ]; then
  FONT_FILES="standard.ttf chinese_simplified.ttf chinese_simplified_ext.ttf chinese_traditional.ttf korean.ttf nintendo_ext.ttf"
  for f in $FONT_FILES; do [ -f "$FONT_STANDINS_DIR/$f" ] || die "FONT_STANDINS_DIR=$FONT_STANDINS_DIR: $f is missing"; done
  # exactly the six files the host serves (and LICENSE-FONTS.txt, which must be there): another .ttf in that folder never reaches the bundle (review r5 M10)
  for f in $FONT_FILES; do cp "$FONT_STANDINS_DIR/$f" "$APP/dm/font/" || die "cannot copy $f from FONT_STANDINS_DIR"; done
  [ -f "$FONT_STANDINS_DIR/LICENSE-FONTS.txt" ] || die "FONT_STANDINS_DIR=$FONT_STANDINS_DIR: LICENSE-FONTS.txt is missing (the licences of the fonts travel with them)"
  cp "$FONT_STANDINS_DIR/LICENSE-FONTS.txt" "$APP/dm/font/" || die "cannot copy LICENSE-FONTS.txt"
  FONTS_STATE="fonts: $(basename "$FONT_STANDINS_DIR") (free: the six stand-ins and LICENSE-FONTS.txt of FONT_STANDINS_DIR)"
  echo "  shared-font stand-ins: $(ls "$APP/dm/font" | wc -l | tr -d ' ') files from FONT_STANDINS_DIR (freely licensed sources)" >&2
elif "$FONT_PYTHON" -c 'import fontTools' 2>/dev/null; then
  printf '\033[33m  WARNING: %s\033[0m\n' "FONT_STANDINS_DIR is not set: the shared-font stand-ins are made from macOS's Arial Unicode (Apple's / Monotype's licence). This IPA is for YOUR phone only: do NOT share it (NOTICE.md, Fonts). Set FONT_STANDINS_DIR for a shareable build." >&2
  PYTHONDONTWRITEBYTECODE=1 "$FONT_PYTHON" -B "$ENGINE/dm/tools/make_shared_fonts.py" "$APP/dm/font" ${STD_FONT:+"$STD_FONT"} >&2 || die "make_shared_fonts.py failed"
  [ -f "$APP/dm/font/standard.ttf" ] && [ -f "$APP/dm/font/nintendo_ext.ttf" ] || die "the shared-font stand-ins were not made"
  FONTS_STATE="fonts: macOS Arial Unicode (private build only)"; [ -z "${STD_FONT:-}" ] || FONTS_STATE="fonts: STD_FONT source (private build only unless that font's licence allows sharing)"
  echo "  shared-font stand-ins: $(ls "$APP/dm/font" | wc -l | tr -d ' ') files, $(du -sk "$APP/dm/font" | cut -f1) KB" >&2
elif [ "${ALLOW_NO_FONTS:-0}" = 1 ]; then
  rmdir "$APP/dm/font" 2>/dev/null; printf '\033[33m  WARNING: no fontTools in %s: the bundle has NO shared-font stand-ins (games that wait for the system font will not boot)\033[0m\n' "$FONT_PYTHON" >&2
  FONTS_STATE="fonts: none (ALLOW_NO_FONTS=1)"
else
  die "no fontTools for $FONT_PYTHON: make the shared-font stand-ins (python3 -m venv V && V/bin/pip install fonttools, then FONT_PYTHON=V/bin/python3) or set ALLOW_NO_FONTS=1 to build without them"
fi
echo "  registered game tables: $tables game.tsv + $extras build-input tables (bf_want, dual_want, geo_mesh, shader_facts_manual, pso_manifest, bench); host sources: dm/host $(ls "$APP/dm/host" | wc -l | tr -d ' ') .c, dm/gfx $(ls "$APP/dm/gfx" | wc -l | tr -d ' ') .c" >&2
# the whole point: nothing of the interpreter is in the bundle
badpy="$(find "$APP" \( -name '*.py' -o -name '*.pyc' -o -name '*.fwork' -o -name 'Python.framework' -o -name 'pyext' -o -name 'lib-dynload' -o -name 'shaderengine' -o -name 'libpyext*' \) | head -5)"
[ -z "$badpy" ] || die "the bundle carries Python pieces: $badpy"
otool -L "$APP/$APPNAME" | grep -qiE 'python' && die "the executable links Python"
[ -z "$(find "$APP" -type l | head -1)" ] || die "the bundle has symbolic links (the packer keeps files only)"

# A binary that may be shared must not carry the build machine's home directory (a static library built earlier, zsign, still has the path it was compiled in): blanked in place, same size,
# before the signature is made (scripts/scrub-build-paths.py; the Rust build maps its own paths away in scripts/build-ios-ffi.sh).
# The WHOLE bundle is scrubbed, not only the executable (review r5 H1: host-prebuilt.dylib carries object paths in its symbol table, Microsoft.iOS.dll its .pdb path): every file that holds the
# user's home gets it blanked, same length. scrubbed.txt lists them; the Mach-O ones are signed again below (a modified Mach-O has an invalid signature). app/verify_ipa.sh greps the whole bundle.
python3 -I "$ROOT/scripts/scrub-build-paths.py" --changed-list "$WD/scrubbed.txt" "$APP" >&2 || die "scrub-build-paths.py failed"
echo "  scrubbed files: $(grep -c . "$WD/scrubbed.txt" || true)" >&2

# ---------------------------------------------------------------- 6. signing
if [ "$KIND" = simulator ]; then
  say "6. signing inside-out, ad hoc (every nested Mach-O first, the bundle last)"
  LIST="$WD/nested.txt"; : > "$LIST"
  find "$APP" -type f \( -name '*.so' -o -name "$APPNAME" -o -name '*.dylib' -o -name 'OpenSSL' \) > "$LIST"
  find "$APP/Frameworks" -path '*.framework/*' -type f ! -name '*.plist' ! -name '*.origin' >> "$LIST" 2>/dev/null
  # the Mach-O ones, found with one batched `file` (it was one `file | grep` per path), signed in the same order
  while IFS= read -r f; do codesign --force --sign - --timestamp=none "$f" >/dev/null 2>&1; done < <(filter_macho < "$LIST")
  codesign --force --sign - --timestamp=none "$APP" >/dev/null 2>&1 || die "codesign refused the bundle"
  codesign --verify --strict "$APP" >/dev/null 2>&1 || die "the bundle does not verify"
  # an ad hoc signature carrying increased-memory-limit is refused by the simulator's launchd at launch (docs of the iOSBuilder repo): the simulator build carries no entitlements
  codesign -d --entitlements :- "$APP" 2>/dev/null | grep -q 'increased-memory' && die "the simulator bundle carries the increased-memory-limit entitlement: it would not bootstrap"
else
  say "6. device bundle: every nested Mach-O signed ad hoc, then the executable ad hoc with the app's entitlements (SideStore re-signs everything on install)"
  # Every nested Mach-O gets a fresh ad hoc signature (review r5 M3 / verify_ipa.sh `codesign --verify`): the scrub above, and the build's own edits of the .NET frameworks, leave a signature that
  # does not match the bytes (or none at all: host-prebuilt.dylib, the R2R image). Found by magic, not by name.
  NSIGN=0
  while IFS= read -r f; do
    [ "$f" = "$APP/$APPNAME" ] && continue
    codesign --force --sign - --timestamp=none "$f" >/dev/null 2>&1 || die "codesign refused ${f#"$APP/"}"
    NSIGN=$((NSIGN + 1))
  done < <(list_macho "$APP")
  echo "  $NSIGN nested Mach-O file(s) signed ad hoc" >&2
  if [ "${GAMEAPP_RAM_ENTITLEMENT:-1}" = 0 ]; then
    say "  GAMEAPP_RAM_ENTITLEMENT=0: the RAM entitlement is NOT embedded"
    codesign --force --sign - --timestamp=none "$APP/$APPNAME" || die "codesign refused the executable"
  else
    SIGN_ENT="$P/GameApp.entitlements"
    if [ "${GAMEAPP_EVA_ENTITLEMENT:-0}" != 1 ]; then
      SIGN_ENT="$WD/GameApp.signed.entitlements"; cp "$P/GameApp.entitlements" "$SIGN_ENT"
      /usr/libexec/PlistBuddy -c 'Delete :com.apple.developer.kernel.extended-virtual-addressing' "$SIGN_ENT" >/dev/null 2>&1
      say "  GAMEAPP_EVA_ENTITLEMENT not 1: extended-virtual-addressing left out (a free Apple ID cannot carry it)"
    fi
    codesign --force --sign - --timestamp=none --entitlements "$SIGN_ENT" "$APP/$APPNAME" || die "codesign refused to embed the entitlements"
    codesign -d --entitlements :- "$APP/$APPNAME" 2>/dev/null | grep -q 'increased-memory-limit' || die "the executable does not carry increased-memory-limit after signing"
    echo "  embedded in the executable's own signature: com.apple.developer.kernel.increased-memory-limit = true" >&2
  fi
fi

# ---------------------------------------------------------------- 7. the structure report
say "7. the executable"
EXE="$APP/$APPNAME"
otool -l "$EXE" | grep -A4 LC_BUILD_VERSION | grep -E 'platform|minos|sdk' | sed 's/^/  /' >&2
echo "  linked libraries:" >&2; otool -L "$EXE" | tail -n +2 | awk '{print "    " $1}' >&2
echo "  embedded frameworks: $(ls "$APP/Frameworks" 2>/dev/null | tr '\n' ' ')" >&2
nm -gU "$EXE" 2>/dev/null > "$WD/nm-exe.txt"
echo "  Rust entry points in the executable (nm -gU): $(grep -c ' T _ibuilder_' "$WD/nm-exe.txt")  gameapp_*: $(grep -c ' T _gameapp_' "$WD/nm-exe.txt")" >&2
echo "  initializer sections (__mod_init_func / __init_offsets): $(otool -l "$EXE" | grep -cE 'sectname (__mod_init_func|__init_offsets)')" >&2
du -sk "$APP"/* 2>/dev/null | sort -rn | head -12 | awk '{printf "    %8.1f MB  %s\n", $1/1024, $2}' | sed "s|$APP/||" >&2

if [ "$KIND" = simulator ]; then
  say "done (simulator). $APP  ($(du -sh "$APP" | awk '{print $1}'))  elapsed $((SECONDS - T0)) s"
  echo "$APP" > "$APP_WORK/last-simulator-app.txt"
  exit 0
fi

# ---------------------------------------------------------------- 8. the .ipa, deterministic, Payload/ only
IPA="$APP_OUT/GameApp-rust-$BUILD_ID${APP_NAME_SUFFIX:-}.ipa"
say "8. the .ipa: Payload/ only, no Symbols/, files sorted, fixed time stamps -> $IPA"
STAGE="$(mktemp -d "$WD/stage.XXXXXX")"; mkdir -p "$STAGE/Payload"
ditto --norsrc --noextattr --noacl "$APP" "$STAGE/Payload/$APPNAME.app" || die "staging copy failed"
find "$STAGE/Payload" \( -name '.DS_Store' -o -name '._*' -o -name '__MACOSX' \) -print -delete >&2
( cd "$STAGE" && find Payload -print0 | xargs -0 touch -t "${IPA_TIMESTAMP:-200101010000}" )
( cd "$STAGE" && find Payload -type f | LC_ALL=C sort > "$STAGE/files.txt" && rm -f "$IPA" && zip -X -D -q -9 "$IPA" -@ < "$STAGE/files.txt" ) || die "zip failed"
rm -rf "$STAGE"
IPA_SHA="$(shasum -a 256 < "$IPA" | cut -d' ' -f1)"   # the 40 MB archive is hashed once: the .sha256 file and the MANIFEST both use it
printf '%s  %s\n' "$IPA_SHA" "$(basename "$IPA")" > "$IPA.sha256"
say "9. the .ipa read back from the archive (app/verify_ipa.sh)"
# The verifier travels WITH the archive (review r5 P12: a PASS recorded by sha256 only cannot be replayed once the script has changed and there is no VCS): app/verify_ipa.sh and the
# scripts/lib-spawn.sh it sources are copied to dist/<ipa name>.verify/ with their relative layout, and it is that COPY that runs here, so what is kept is exactly what measured the archive.
# replay.sh there runs it again on the archive next to it, with the environment this build gave it.
VERIFY_DIR="${IPA%.ipa}.verify"
rm -rf "$VERIFY_DIR"; mkdir -p "$VERIFY_DIR/app" "$VERIFY_DIR/scripts" || die "cannot create $VERIFY_DIR"
cp -p "$ROOT/app/verify_ipa.sh" "$VERIFY_DIR/app/verify_ipa.sh" && cp -p "$ROOT/scripts/lib-spawn.sh" "$VERIFY_DIR/scripts/lib-spawn.sh" || die "cannot copy the verifier to $VERIFY_DIR"
V_EVA="${GAMEAPP_EVA_ENTITLEMENT:-0}"; V_RAM="${GAMEAPP_RAM_ENTITLEMENT:-1}"
{
  echo '#!/bin/bash'
  echo "# replays the check this build ran on $(basename "$IPA"), with the same verifier (app/verify_ipa.sh + scripts/lib-spawn.sh, copied here at build time) and the same environment"
  echo 'HERE=$(cd "$(dirname "$0")" && pwd)'
  echo 'IPA="${1:-$HERE/../'"$(basename "$IPA")"'}"'
  printf 'GAMEAPP_EVA_ENTITLEMENT=%q GAMEAPP_RAM_ENTITLEMENT=%q SHADER_CHAIN=%q FRAMEWORK_ID=%q APP_BUILD_NUMBER=%q APP_VERSION=%q exec "$HERE/app/verify_ipa.sh" "$IPA" %q\n' \
    "$V_EVA" "$V_RAM" "$SHADER_CHAIN" "$FRAMEWORK_ID" "${APP_BUILD_NUMBER:-}" "${APP_VERSION:-}" "$APP_ID"
} > "$VERIFY_DIR/replay.sh" && chmod +x "$VERIFY_DIR/replay.sh" || die "cannot write $VERIFY_DIR/replay.sh"
GAMEAPP_EVA_ENTITLEMENT="$V_EVA" GAMEAPP_RAM_ENTITLEMENT="$V_RAM" SHADER_CHAIN="$SHADER_CHAIN" FRAMEWORK_ID="$FRAMEWORK_ID" \
  APP_BUILD_NUMBER="${APP_BUILD_NUMBER:-}" APP_VERSION="${APP_VERSION:-}" \
  "$VERIFY_DIR/app/verify_ipa.sh" "$IPA" "$APP_ID" 2>&1 | tee "$WD/verify_ipa.log" >&2
VRC="${PIPESTATUS[0]}"
cp -p "$WD/verify_ipa.log" "$VERIFY_DIR/verify_ipa.log" 2>/dev/null || true
if [ "$VRC" != 0 ]; then
  # an archive that failed its own check must not sit in dist/ under its release name with a .sha256 that invites publishing it (review r5 L5)
  FAILED="${IPA%.ipa}.FAILED.ipa"; mv -f "$IPA" "$FAILED" && rm -f "$IPA.sha256" && IPA="$FAILED"
  FAILED_V="${IPA%.ipa}.verify"; rm -rf "$FAILED_V"; mv -f "$VERIFY_DIR" "$FAILED_V" && VERIFY_DIR="$FAILED_V" \
    && sed -i '' "s|/\.\./GameApp-rust-[^}]*\.ipa}|/../$(basename "$IPA")}|" "$VERIFY_DIR/replay.sh"
  printf '\033[31m  verify_ipa.sh failed: the archive is now %s (no .sha256)\033[0m\n' "$IPA" >&2
fi

# ---------------------------------------------------------------- 10. MANIFEST
say "10. $APP_OUT/MANIFEST.txt"
{
  echo "GameApp (iBuilder) with the Rust chain -- build $BUILD_ID, $(date -u '+%Y-%m-%d %H:%M:%S') UTC, $((SECONDS - T0)) s"
  echo
  echo "ipa              $(basename "$IPA")"
  echo "size             $(stat -f %z "$IPA") bytes"
  echo "sha256           $IPA_SHA"
  echo "bundle id        $APP_ID  (CFBundleVersion $(plutil -extract CFBundleVersion raw "$APP/Info.plist" 2>/dev/null), executable $APPNAME)"
  echo "signature        $(codesign -dv "$APP/$APPNAME" 2>&1 | grep -E 'Signature|flags' | tr '\n' ' ')"
  echo "entitlements     $(codesign -d --entitlements :- "$APP/$APPNAME" 2>/dev/null | grep -o 'com.apple.developer.kernel[a-z-]*' | tr '\n' ' ')"
  echo "shader chain     $SHADER_CHAIN (the patched engine's header = the host's)"
  echo "framework id     $FRAMEWORK_ID (GameAppBuild.FrameworkId: a game built by an app with this id boots without a rebuild; its inputs below)"
  echo "version          CFBundleShortVersionString ${APP_VERSION:-from the csproj}, CFBundleVersion ${APP_BUILD_NUMBER:-1}"
  echo "$FONTS_STATE"
  echo "scrubbed files   $(grep -c . "$WD/scrubbed.txt" || true) (home directory blanked in the bundle; verify_ipa.sh greps the whole bundle for it)"
  # the PASS count belongs to a revision of the checker (review r4 §3.3: a new check changes the count), so the checker's own sha256 travels with it (and the shared spawn list it reads)
  echo "verify_ipa.sh    exit $VRC  ($(grep -c $'\033\[32mPASS' "$WD/verify_ipa.log") PASS, $(grep -c $'\033\[31mFAIL' "$WD/verify_ipa.log") FAIL; script sha256 $(shasum -a 256 < "$VERIFY_DIR/app/verify_ipa.sh" | cut -c1-16), scripts/lib-spawn.sh sha256 $(shasum -a 256 < "$VERIFY_DIR/scripts/lib-spawn.sh" | cut -c1-16))"
  echo "                 $(grep -E '^  [0-9]+ checks: ' "$WD/verify_ipa.log" | sed 's/^ *//')"
  echo "                 the verifier that ran, kept with the archive: $(basename "$VERIFY_DIR")/ (app/verify_ipa.sh sha256 $(shasum -a 256 < "$VERIFY_DIR/app/verify_ipa.sh" | cut -d' ' -f1), scripts/lib-spawn.sh sha256 $(shasum -a 256 < "$VERIFY_DIR/scripts/lib-spawn.sh" | cut -d' ' -f1)); replay: $(basename "$VERIFY_DIR")/replay.sh"
  echo
  echo "== toolchain =="
  echo "rustc            $(PATH=/opt/homebrew/opt/rustup/bin:$PATH rustc --version)"
  echo "cargo features   ibuilder-ffi: $FFI_FEATURES  (ibuilder-cli default: profile rewrite macho sign mods shaders shader-finish shadersplit)"
  echo "NUP reader       $NUP_STATE"
  echo "dotnet           $(dotnet --version)  workloads: $(dotnet workload list 2>/dev/null | awk '/^ios/{print $1 " " $2}')"
  echo "Xcode            $(xcodebuild -version 2>/dev/null | tr '\n' ' ')"
  echo "iPhoneOS SDK     $SDK"
  echo
  echo "== the Rust libraries: sha256 of each crate's sources (src/ include/ Cargo.toml build.rs), then of the whole set + Cargo.lock =="
  while IFS=$'\t' read -r n h; do printf '%-24s %s\n' "$n" "$h"; done < "$SRC_HASH_FILE"
  echo "Cargo.lock               $CARGO_LOCK_SHA"
  echo "tree id                  rust$RUST_ID"
  echo "libibuilder_ffi.a        $(stat -f %z "$RUSTLIB") bytes  sha256 $RUSTLIB_SHA"
  echo "ffi library              $FFI_STATE"
  echo
  echo "== the framework id's inputs (scripts/lib-hash.sh framework_id_listing): framework id $FRAMEWORK_ID =="
  while IFS=$'\t' read -r n h; do printf '%-24s %s\n' "$n" "$h"; done < "$FRAMEWORK_INPUTS"
  echo
  echo "== the C# snapshot (app/SNAPSHOT.tsv sha256 $(shasum -a 256 < "$ROOT/app/SNAPSHOT.tsv" | cut -c1-16)); C# patches applied: ${#PATCHES[@]} =="
  echo "app/gameapp_shim_rust.c  $(shasum -a 256 < "$ROOT/app/gameapp_shim_rust.c" | cut -d' ' -f1)"
  echo
  echo "== read-only inputs used (name, path, size, sha256, mtime, pin state) =="
  while IFS=$'\t' read -r n pth sz sha mt st why; do printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$n" "$pth" "$sz" "$sha" "$mt" "$st"; done < "$INPUTS"
  echo
  echo "== what is inside (top level of Payload/$APPNAME.app, uncompressed MB) =="
  du -sk "$APP"/* | sort -rn | awk '{printf "%9.2f  %s\n", $1/1024, $2}' | sed "s|$APP/||"
  echo "files in the archive: $(unzip -Z1 "$IPA" | wc -l | tr -d ' ');  *.py: $(unzip -Z1 "$IPA" | grep -c '\.py$');  Python.framework: $(unzip -Z1 "$IPA" | grep -c 'Python.framework')"
  echo
  echo "== frameworks =="
  ls -1 "$APP/Frameworks" | sed 's/^/  /'
} > "$APP_OUT/MANIFEST.txt"
cat "$APP_OUT/MANIFEST.txt" | head -12 >&2
[ "$VRC" = 0 ] || die "app/verify_ipa.sh failed: the .ipa is NOT a good build (dist/MANIFEST.txt says so)"
say "done. $IPA"
echo "  sha256 $(cut -d' ' -f1 "$IPA.sha256")   $(stat -f %z "$IPA") bytes   elapsed $((SECONDS - T0)) s" >&2
