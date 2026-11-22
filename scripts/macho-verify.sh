#!/bin/sh
# Build ibuilder-assemble / ibuilder-macho-write / ibuilder-gen-embed and byte-compare their output with the Python
# oracle made by scripts/macho-oracle.sh. Prints IDENTICAL / DIFFERENT per case; exit status 1 if any differs.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
SP="${IBUILDER_SCRATCH_OLD:?IBUILDER_SCRATCH_OLD is not set: the scratch folder scripts/macho-oracle.sh wrote to (environment or scripts/local.env; see scripts/local.env.example)}"
IOB="${IOSBUILDER:?IOSBUILDER is not set: the checkout of the original iOSBuilder project (environment or scripts/local.env; see scripts/local.env.example)}"
M=$SP/macho
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$SP/target-macho}
B=$CARGO_TARGET_DIR/release
H=$M/hosts
O=$M/oracle
R=$M/rust-out
mkdir -p "$R"
# Falls back to the private scratch workspace that holds only this crate when the repo workspace does not build.
if ! (cd "$(dirname "$0")/.." && cargo build --release -p ibuilder-macho 2>/dev/null); then
  (cd "$M/ws" && cargo build --release --offline -p ibuilder-macho) || exit 2
fi
bad=0
cmpc() { if cmp -s "$1" "$2"; then echo "IDENTICAL  $3"; else echo "DIFFERENT  $3"; bad=1; fi; }
asm() { n=$1; shift; "$B/ibuilder-assemble" "$@" --out "$R/$n.dylib" > "$R/$n.stdout" && cmpc "$R/$n.dylib" "$O/$n.dylib" "assemble $n ($(wc -c < "$R/$n.dylib") bytes)"; }
G="--game $M/flat"
asm dev-4762784 --host "$H/dev-4762784.dylib" $G --modules main subsdk0 sdk
asm asm-sim     --host "$H/asm-sim.dylib"     $G --modules main subsdk0 sdk
asm load-ios    --host "$H/load-ios.dylib"    $G --modules main subsdk0 sdk
asm load-sim    --host "$H/load-sim.dylib"    $G --modules main subsdk0 sdk
asm wt-ios      --host "$H/wt-ios.dylib"      $G --modules main subsdk0 sdk
asm wt-sim      --host "$H/wt-sim.dylib"      $G --modules main subsdk0 sdk
asm wt-app-ios  --host "$H/wt-app-ios.dylib"  $G --modules main subsdk0 sdk
asm tag-sdk-sub --host "$H/load-ios.dylib"    $G --modules sdk subsdk0 --tag T
asm main-only   --host "$H/wt-ios.dylib"      $G --modules main
asm syn-asm-sim --synthetic --host "$H/asm-sim.dylib" --modules main subsdk0 sdk
asm syn-tag-wt-sim --synthetic --tag G --host "$H/wt-sim.dylib" --modules main sdk
cmpc "$M/rust-out/dev-4762784.dylib" "$IOB/out-load/device/DmGame0100DFA027504000.framework/DmGame0100DFA027504000" "assemble dev-4762784 vs the framework stored by the original pipeline (out-load/device)"
U=00112233-4455-6677-8899-aabbccddeeff
"$B/ibuilder-macho-write" --out "$R/mw-sim.dylib" --uuid $U --platform sim > /dev/null; cmpc "$R/mw-sim.dylib" "$O/mw-sim.dylib" "macho-write sim"
"$B/ibuilder-macho-write" --out "$R/mw-ios.dylib" --uuid $U --platform ios --name DmGameX --magic 0x12345678 --minos 26.1 --sdk 27.2.1 > /dev/null; cmpc "$R/mw-ios.dylib" "$O/mw-ios.dylib" "macho-write ios"
"$B/ibuilder-macho-write" --out "$R/mw-simname.dylib" --uuid $U --name A --magic 0xFFFFFFFF --minos 27.0.5 --sdk 28 > /dev/null; cmpc "$R/mw-simname.dylib" "$O/mw-simname.dylib" "macho-write name/magic/version"
ge() { n=$1; shift; "$B/ibuilder-gen-embed" "$@" > "$R/ge-$n.flags"; cmpc "$R/ge-$n.S" "$O/ge-$n.S" "gen-embed $n (.S)"; cmpc "$R/ge-$n.flags" "$O/ge-$n.flags" "gen-embed $n (flags)"; }
ge base "$M/flat" "$R/ge-base.S" main subsdk0 sdk
ge tag --tag T "$M/flat" "$R/ge-tag.S" main sdk
ge jit --jit "$M/jit.img" "$M/flat" "$R/ge-jit.S" main
ge nro --tag Q --jit "$M/jit.img" "$M/flat-nro" "$R/ge-nro.S" main
exit $bad
