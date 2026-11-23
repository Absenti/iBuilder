#!/usr/bin/env bash
# scripts/build-glslang.sh [ios] [sim] [mac] -- glslang as static libraries for the app: the in-process replacement of `glslangValidator`, which the OpenGL port of
# crates/ibuilder-glvk (gl_shaders.py) calls. app/c/glslang_cli.cpp (gameapp_glslang) drives glslang's C++ API in these archives, as app/c/spvc_cli.c drives SPIRV-Cross.
#
#   scripts/build-glslang.sh                -> $GLSLANG_OUT/libglslang-ios.a, libglslang-sim.a, libglslang-mac.a, include/, mac/glslang-cli
#   GLSLANG_OUT=<dir> scripts/build-glslang.sh ios
#
# The source: the release GLSLANG_TAG (16.6.0, commit e1b562a8bed273a02f30b59b66a5d499793cede5, the version Homebrew's glslangValidator that wrote the tests' oracle reports,
# "Glslang Version: 11:16.6.0"), fetched once as GitHub's release tarball into $GLSLANG_OUT/dl, checked against GLSLANG_SHA256 and unpacked into $GLSLANG_OUT/src. Never into the
# repository. A tarball with another hash is refused (nothing is built from it).
# The build, per platform (CMake, Release, static): ENABLE_OPT=OFF (no SPIRV-Tools: the port passes no -O), ENABLE_GLSLANG_BINARIES=OFF (no glslangValidator: no process is started
# on the phone), ENABLE_HLSL=OFF (GLSL only; the HLSL front end changes nothing for a GLSL input), GLSLANG_TESTS=OFF, BUILD_EXTERNAL=OFF, ENABLE_PCH=OFF; iOS device arm64
# (iphoneos) and simulator arm64 (iphonesimulator) with a minimum of GLSLANG_MIN_IOS (18.0, as SPIRV-Cross's archives), the Mac arm64 for the proof (the CLI below). The
# libraries glslang, MachineIndependent, GenericCodeGen, OSDependent, SPIRV and glslang-default-resource-limits are merged into one archive per platform.
# include/ (after a build): the headers of glslang/ and SPIRV/, StandAlone/DirStackFileIncluder.h (glslangValidator's #include resolver, used as is), and the two generated ones the CLI reads:
# glslang/build_info.h (CMake, the version macros) and glslang/glsl_intrinsic_header.h (gen_extension_headers.py over glslang/ExtensionHeaders, as glslangValidator's build makes it).
# mac/glslang-cli: app/c/glslang_cli.cpp built for the Mac with -DGAMEAPP_GLSLANG_MAIN (a main() over gameapp_glslang): the proof runs the crate's oracle test with it as GLSLANG.
#
# Idempotent: an archive that is present is kept (delete it to rebuild). Writes only $GLSLANG_OUT (default <repo>/target/glslang). Licences: NOTICE.md, section 3 (glslang).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${GLSLANG_TAG:-16.6.0}"
COMMIT=e1b562a8bed273a02f30b59b66a5d499793cede5
SHA256="${GLSLANG_SHA256:-9c09b901149c729df745057dafa815278aaa101b84d2b6e14f16a42de52f97f2}"
URL="https://github.com/KhronosGroup/glslang/archive/refs/tags/$TAG.tar.gz"
OUT="${GLSLANG_OUT:-$ROOT/target/glslang}"
MIN="${GLSLANG_MIN_IOS:-18.0}"
KINDS=("$@"); [ ${#KINDS[@]} -gt 0 ] || KINDS=(ios sim mac)
say() { printf '[glslang] %s\n' "$*" >&2; }
die() { printf '[glslang] FAIL: %s\n' "$*" >&2; exit 1; }
command -v cmake >/dev/null || die "no cmake"
mkdir -p "$OUT/dl"
SRC="$OUT/src/glslang-$TAG"

# ---- 1. the pinned source
if [ ! -f "$SRC/glslang/Public/ShaderLang.h" ]; then
  TGZ="$OUT/dl/glslang-$TAG.tar.gz"
  [ -f "$TGZ" ] || { say "fetching $URL"; curl -fsSL -o "$TGZ.part" "$URL" && mv "$TGZ.part" "$TGZ"; } || die "cannot fetch $URL"
  got="$(shasum -a 256 "$TGZ" | cut -d' ' -f1)"
  [ "$got" = "$SHA256" ] || die "$TGZ has sha256 $got, the pin is $SHA256: refused (delete it to fetch again)"
  rm -rf "$OUT/src.tmp"; mkdir -p "$OUT/src.tmp"
  tar -xzf "$TGZ" -C "$OUT/src.tmp" || die "cannot unpack $TGZ"
  [ -f "$OUT/src.tmp/glslang-$TAG/glslang/Public/ShaderLang.h" ] || die "$TGZ does not hold glslang-$TAG/"
  mkdir -p "$OUT/src"; rm -rf "$SRC"; mv "$OUT/src.tmp/glslang-$TAG" "$SRC"; rm -rf "$OUT/src.tmp"
fi
say "source $SRC (release $TAG, commit $COMMIT, tarball sha256 $SHA256)"

# ---- 2. the archives
export SOURCE_DATE_EPOCH=0 TZ=UTC ZERO_AR_DATE=1   # ZERO_AR_DATE: libtool writes no time stamps, so an archive is the same bytes on every build (its pin in app/inputs.tsv holds)
build() {   # <kind> -> $OUT/libglslang-<kind>.a
  local kind=$1 a="$OUT/libglslang-$1.a" b="$OUT/build-$1" sys=()
  if [ -f "$a" ]; then say "$a present, kept"; return; fi
  case "$kind" in
    ios) sys=(-DCMAKE_SYSTEM_NAME=iOS -DCMAKE_OSX_SYSROOT=iphoneos -DCMAKE_OSX_DEPLOYMENT_TARGET="$MIN") ;;
    sim) sys=(-DCMAKE_SYSTEM_NAME=iOS -DCMAKE_OSX_SYSROOT=iphonesimulator -DCMAKE_OSX_DEPLOYMENT_TARGET="$MIN") ;;
    mac) sys=(-DCMAKE_OSX_DEPLOYMENT_TARGET=14.0) ;;
    *) die "unknown kind $kind (ios, sim, mac)" ;;
  esac
  rm -rf "$b"
  cmake -S "$SRC" -B "$b" "${sys[@]}" -DCMAKE_OSX_ARCHITECTURES=arm64 -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
    -DENABLE_OPT=OFF -DENABLE_GLSLANG_BINARIES=OFF -DENABLE_HLSL=OFF -DGLSLANG_TESTS=OFF -DBUILD_EXTERNAL=OFF -DENABLE_PCH=OFF \
    -DGLSLANG_ENABLE_INSTALL=OFF -DENABLE_SPIRV=ON > "$OUT/cfg-$kind.log" 2>&1 || { tail -20 "$OUT/cfg-$kind.log" >&2; die "cmake configure ($kind) failed"; }
  cmake --build "$b" -j "$(sysctl -n hw.ncpu)" > "$OUT/bld-$kind.log" 2>&1 || { tail -30 "$OUT/bld-$kind.log" >&2; die "cmake build ($kind) failed"; }
  local libs=()
  while IFS= read -r l; do libs+=("$l"); done < <(find "$b" -name 'lib*.a' | sort)
  [ ${#libs[@]} -gt 0 ] || die "no archive under $b"
  for want in libglslang.a libMachineIndependent.a libGenericCodeGen.a libOSDependent.a libglslang-default-resource-limits.a; do
    printf '%s\n' "${libs[@]}" | grep -q "/$want\$" || die "$kind: $want was not built"
  done
  libtool -static -o "$a.tmp" "${libs[@]}" 2>/dev/null || die "libtool refused the $kind archives"
  mv "$a.tmp" "$a"
  say "$a: $(stat -f %z "$a") bytes, $(lipo -archs "$a") ($(printf '%s ' "${libs[@]##*/}"))"
}
for k in "${KINDS[@]}"; do build "$k"; done

# ---- 3. the headers (copied: the app's build reads this folder, never the source tree)
INC="$OUT/include"
if [ ! -f "$INC/glslang/glsl_intrinsic_header.h" ] || [ ! -f "$INC/glslang/build_info.h" ]; then
  rm -rf "$INC.tmp"; mkdir -p "$INC.tmp/StandAlone"
  ( cd "$SRC" && find glslang SPIRV \( -name '*.h' -o -name '*.hpp' -o -name '*.hpp11' \) -print | while IFS= read -r h; do mkdir -p "$INC.tmp/$(dirname "$h")"; cp "$h" "$INC.tmp/$h"; done )
  cp "$SRC/StandAlone/DirStackFileIncluder.h" "$INC.tmp/StandAlone/"
  BI="$(find "$OUT"/build-* -path '*/include/glslang/build_info.h' | head -1)"; [ -f "$BI" ] || die "no build_info.h in a CMake build folder"
  cp "$BI" "$INC.tmp/glslang/build_info.h"
  python3 -I "$SRC/gen_extension_headers.py" -i "$SRC/glslang/ExtensionHeaders" -o "$INC.tmp/glslang/glsl_intrinsic_header.h" || die "gen_extension_headers.py failed"
  rm -rf "$INC"; mv "$INC.tmp" "$INC"
fi

# ---- 4. the Mac CLI over the wrapper (the proof: GLSLANG=$OUT/mac/glslang-cli cargo test -p ibuilder-glvk --features host-tools)
case " ${KINDS[*]} " in *" mac "*)
  mkdir -p "$OUT/mac"
  xcrun clang++ -std=c++17 -O2 -arch arm64 -mmacosx-version-min=14.0 -DGAMEAPP_GLSLANG_MAIN -I"$INC" -I"$INC/glslang" \
    "$ROOT/app/c/glslang_cli.cpp" "$OUT/libglslang-mac.a" -o "$OUT/mac/glslang-cli.tmp" || die "app/c/glslang_cli.cpp did not build for the Mac"
  mv "$OUT/mac/glslang-cli.tmp" "$OUT/mac/glslang-cli"
  say "$OUT/mac/glslang-cli: $("$OUT/mac/glslang-cli" --version | head -1)" ;;
esac
