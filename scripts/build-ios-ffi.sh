#!/bin/bash
# scripts/build-ios-ffi.sh -- libibuilder_ffi.a for the iOS device and the iOS simulator, plus an XCFramework the .NET iOS app can reference.
#
#   scripts/build-ios-ffi.sh [<out dir>]            (default: $CARGO_TARGET_DIR or ./target, then /dist/ios)
#   FEATURES=real-stages|mock-stages|""             which stage workers are linked (default real-stages: profile / rewrite / macho / sign / mods and, through ibuilder-cli's default
#                                                   features, stage 3: ibuilder-shadersplit + ibuilder-shaderbuild + ibuilder-shader::finish)
#   TARGETS="aarch64-apple-ios aarch64-apple-ios-sim"   which Rust targets to build (default both; scripts/build-app.sh builds the one it needs). The XCFramework is made only when both are built.
#   FFI_CACHE=0                                     do not reuse the previous library: always run cargo (default: reuse it when the cache key below matches exactly; FFI_CACHE_DIR, default
#                                                   $CARGO_TARGET_DIR/ffi-cache, holds the last library per target and the key it was built with)
#   FFI_CACHE_DRY=1                                 print the key and what would happen (HIT / MISS and why) for each target, build nothing, write nothing, exit
#   IPHONEOS_DEPLOYMENT_TARGET=14.0                 the app's minimum iOS (set it: the C code of capstone and Rust's objects carry it; a mismatch is a linker warning per object)
#
# Needs rustup's toolchain (the Homebrew rustc has no aarch64-apple-ios standard library): the keg is put on PATH for this script only.
# Verified (see docs/chain/PIPELINE-RUST.md, "iOS portability"): both targets build; the exported symbols match include/ibuilder.h; a C client links the simulator archive and runs the chain in a
# booted simulator. The archive linked into the real .NET app: docs/APP-RUST.md (scripts/build-app.sh).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
OUT="${1:-$TARGET_DIR/dist/ios}"
FEATURES="${FEATURES-real-stages}"
TARGETS="${TARGETS:-aarch64-apple-ios aarch64-apple-ios-sim}"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-14.0}"
export CARGO_TARGET_DIR="$TARGET_DIR"
# Nothing of the machine that builds may end up in the binary: panic locations carry the source paths of the crate and of every dependency. The home directory, the cargo registry and this checkout
# are mapped to neutral prefixes (the later, more specific flags win). The exported API and the code are the same; only these strings change.
# CARGO_ENCODED_RUSTFLAGS (flags separated by 0x1f) instead of RUSTFLAGS (split on blanks): a HOME, CARGO_HOME or checkout whose path holds a space would break the --remap-path-prefix pairs (review r5 M5).
# The flags the caller already gave (CARGO_ENCODED_RUSTFLAGS, else RUSTFLAGS split on blanks) stay in front; cargo ignores RUSTFLAGS when the encoded variable is set, so it is unset here.
_flags=()
if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
  IFS=$'\x1f' read -r -a _flags <<< "$CARGO_ENCODED_RUSTFLAGS"
elif [ -n "${RUSTFLAGS:-}" ]; then
  read -r -a _flags <<< "$RUSTFLAGS"
fi
_flags+=("--remap-path-prefix=$HOME=/build/home" "--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/build/cargo" "--remap-path-prefix=$ROOT=/build/iBuilderRust")
CARGO_ENCODED_RUSTFLAGS="$(printf '%s\x1f' "${_flags[@]}")"; CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS%$'\x1f'}"
export CARGO_ENCODED_RUSTFLAGS; unset RUSTFLAGS

command -v rustup >/dev/null || { echo "rustup is not installed (brew install rustup)" >&2; exit 1; }
for t in $TARGETS; do
  case "$t" in aarch64-apple-ios|aarch64-apple-ios-sim) ;; *) echo "unknown target in TARGETS: $t" >&2; exit 1 ;; esac
  rustup target list --installed | grep -qx "$t" || { echo "missing target: rustup target add $t" >&2; exit 1; }
done

cd "$ROOT"
feat=()
[ -n "$FEATURES" ] && feat=(--features "$FEATURES")

# ---- the cache (lever 17): the library of the previous build is reused ONLY when everything that goes into it is exactly the same ------------------------------------------------------
# The key, one line per ingredient so a MISS can say which one changed: the sha256 of each of the crates' sources (src/, include/, Cargo.toml, build.rs: the same fingerprints build-app.sh
# prints as the tree id), Cargo.lock, the workspace Cargo.toml (profiles) and any .cargo/config*, `rustc -vV` and `cargo -V`, RUSTFLAGS (which carry the path remaps), the cargo features,
# the target, the build-related environment (CARGO_*, RUSTC*, CC/CFLAGS, IPHONEOS_*, SDKROOT), the iPhoneOS/simulator SDK and the C compiler (the crates compile C), and the sha256 of this
# script and of scripts/lib-hash.sh. A key that differs by one byte is a miss and cargo runs. FFI_CACHE=0 never looks at the cache and never writes to it.
. "$ROOT/scripts/lib-hash.sh"
CACHE_DIR="${FFI_CACHE_DIR:-$TARGET_DIR/ffi-cache}"
sha_of() { shasum -a 256 | cut -d' ' -f1; }
ffi_key() {   # $1 = rust target; prints the key text
  local t="$1" sdk
  case "$t" in aarch64-apple-ios) sdk=iphoneos ;; *) sdk=iphonesimulator ;; esac
  echo "key-format 2"
  echo "target $t"
  echo "features ${FEATURES:-<none>}"
  echo "rustflags $(printf '%s' "$CARGO_ENCODED_RUSTFLAGS" | tr '\037' ' ')"
  echo "deployment-target $IPHONEOS_DEPLOYMENT_TARGET"
  echo "rustc $(rustc -vV | tr '\n' '|')"
  echo "cargo $(cargo -V)"
  echo "sdk $(xcrun --sdk "$sdk" --show-sdk-version 2>/dev/null || echo unknown) $(xcrun --sdk "$sdk" --show-sdk-path 2>/dev/null || echo unknown)"
  echo "clang $(xcrun --sdk "$sdk" clang --version 2>/dev/null | head -1 || echo unknown)"
  echo "env $(env | grep -E '^(CARGO_|RUSTC|RUSTUP_|CC|CXX|AR|CFLAGS|CXXFLAGS|CPPFLAGS|LDFLAGS|TARGET_|HOST_|CRATE_|IPHONEOS_|SDKROOT|MACOSX_|DEVELOPER_DIR|LIBRARY_PATH)' | grep -v '^CARGO_TARGET_DIR=' | LC_ALL=C sort | tr '\n' '|')"
  echo "Cargo.lock $(sha_of < Cargo.lock)"
  echo "Cargo.toml $(sha_of < Cargo.toml)"
  # every cargo config cargo itself would read: ./.cargo and the .cargo of each parent folder of the checkout, then $CARGO_HOME (review r5 L4)
  echo "cargo-config $( { d="$PWD"; while :; do cat "$d/.cargo/config" "$d/.cargo/config.toml" 2>/dev/null || true; [ "$d" = / ] && break; d="$(dirname "$d")"; done
                          cat "${CARGO_HOME:-$HOME/.cargo}/config" "${CARGO_HOME:-$HOME/.cargo}/config.toml" 2>/dev/null || true; } | sha_of)"
  echo "script $(sha_of < "$ROOT/scripts/build-ios-ffi.sh") lib-hash $(sha_of < "$ROOT/scripts/lib-hash.sh")"
  rust_tree_hashes "$ROOT" | awk -F'\t' '{ print "crate " $1 " " $2 }'
}
cache_status() {   # $1 = target; sets KEY, KEYHASH, CSTATE (HIT | MISS: reason)
  local t="$1" kf="$CACHE_DIR/$t.key" lib="$CACHE_DIR/$t.a"
  KEY="$(ffi_key "$t")"; KEYHASH="$(printf '%s\n' "$KEY" | sha_of)"
  if [ "${FFI_CACHE:-1}" = 0 ]; then CSTATE="MISS: FFI_CACHE=0"
  elif [ ! -f "$kf" ] || [ ! -f "$lib" ] || [ ! -f "$lib.sha256" ]; then CSTATE="MISS: no cached library for $t"
  elif [ "$(cat "$kf")" != "$KEY" ]; then
    CSTATE="MISS: the key changed ($( { diff <(cat "$kf") <(printf '%s\n' "$KEY") || true; } | awk '/^[<>] /{ print ($2 == "crate" ? "crate-" $3 : $2) }' | sort -u | tr '\n' ' ' | sed 's/ $//'))"
  elif [ "$(sha_of < "$lib")" != "$(cat "$lib.sha256")" ]; then CSTATE="MISS: the cached library does not match its recorded sha256"
  else CSTATE="HIT"; fi
}
if [ "${FFI_CACHE_DRY:-0}" = 1 ]; then
  for t in $TARGETS; do cache_status "$t"; echo "== ffi cache $t: $CSTATE (key $KEYHASH)"; printf '%s\n' "$KEY" | sed 's/^/     /'; done
  exit 0
fi
HIT_LIB=""
for t in $TARGETS; do
  cache_status "$t"
  if [ "$CSTATE" = HIT ]; then
    echo "== ffi cache $t: HIT (key ${KEYHASH:0:12}): cargo not run, the library of the previous build with this exact key is reused"
    HIT_LIB="$HIT_LIB $t"   # the copy below reads it from the cache; target/ is left alone (cargo's own fingerprints there must not be fooled by an older file)
    continue
  fi
  echo "== ffi cache $t: $CSTATE (key ${KEYHASH:0:12})"
  echo "== cargo build --release --target $t -p ibuilder-ffi ${feat[*]:-}"
  cargo build --release --target "$t" -p ibuilder-ffi "${feat[@]+"${feat[@]}"}"
  if [ "${FFI_CACHE:-1}" != 0 ]; then   # store the fresh library with the key it was built with (key written last: a half-written entry never matches)
    mkdir -p "$CACHE_DIR" && rm -f "$CACHE_DIR/$t.key" \
      && cp "$TARGET_DIR/$t/release/libibuilder_ffi.a" "$CACHE_DIR/$t.a" && sha_of < "$CACHE_DIR/$t.a" > "$CACHE_DIR/$t.a.sha256" \
      && printf '%s\n' "$KEY" > "$CACHE_DIR/$t.key" \
      || echo "== ffi cache $t: could not store the library (the build is fine, the next one rebuilds)" >&2
  fi
done

rm -rf "$OUT"; mkdir -p "$OUT"
cp crates/ibuilder-ffi/include/ibuilder.h "$OUT/ibuilder.h"
# the exported surface must be exactly the header's (Xcode's nm cannot read the bitcode Rust embeds: --no-llvm-bc)
want="$(grep -oE '\bibuilder_[a-z_0-9]+\(' crates/ibuilder-ffi/include/ibuilder.h | tr -d '(' | grep -v '^ibuilder_test_panic$' | sed 's/^/_/' | sort -u)"
for t in $TARGETS; do
  a="${t#aarch64-apple-}"                        # ios | ios-sim
  case " $HIT_LIB " in *" $t "*) src="$CACHE_DIR/$t.a" ;; *) src="$TARGET_DIR/$t/release/libibuilder_ffi.a" ;; esac
  cp "$src" "$OUT/libibuilder_ffi-$a.a"
  have="$(nm -gU --no-llvm-bc "$OUT/libibuilder_ffi-$a.a" 2>/dev/null | awk '$2=="T" && $3 ~ /^_ibuilder_/ {print $3}' | grep -v '^_ibuilder_test_panic$' | sort -u)"
  if [ "$want" != "$have" ]; then echo "exported symbols of $a differ from ibuilder.h:" >&2; diff <(echo "$want") <(echo "$have") >&2 || true; exit 1; fi
  echo "== $a: $(echo "$have" | wc -l | tr -d ' ') exported ibuilder_* symbols, identical to the header"
done

case " $TARGETS " in
  *" aarch64-apple-ios "*aarch64-apple-ios-sim*|*" aarch64-apple-ios-sim "*aarch64-apple-ios*)
    xcodebuild -create-xcframework \
      -library "$OUT/libibuilder_ffi-ios.a"     -headers crates/ibuilder-ffi/include \
      -library "$OUT/libibuilder_ffi-ios-sim.a" -headers crates/ibuilder-ffi/include \
      -output "$OUT/ibuilder.xcframework" ;;
  *) echo "== one target only: no XCFramework" ;;
esac
echo "== $OUT"; ls -la "$OUT"
