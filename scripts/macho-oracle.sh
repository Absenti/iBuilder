#!/bin/sh
# Regenerate the Python oracle for ibuilder-macho: runs the ORIGINAL iOSBuilder tools (from copies in the scratch folder,
# PYTHONDONTWRITEBYTECODE=1, nothing is written under iOSBuilder) on the same inputs the Rust tests use.
# Needs IBUILDER_SCRATCH_OLD (a folder with ref/venv/bin/python, a Python 3.14 + capstone environment; the oracle goes to $IBUILDER_SCRATCH_OLD/macho), IOSBUILDER (the original
# iOSBuilder checkout) and IOSBUILDER_WORKTREE (default IOSBUILDER), from the environment or scripts/local.env (see scripts/local.env.example).
#   scripts/macho-oracle.sh            -> $M/oracle/*.dylib, *.stdout, ge-*.S, ge-*.flags, mw-*.dylib
# Hosts are frozen copies of the prebuilt host dylibs (sha256 in crates/ibuilder-macho/tests/oracle.rs).
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
SP="${IBUILDER_SCRATCH_OLD:?IBUILDER_SCRATCH_OLD is not set: the scratch folder with ref/venv (environment or scripts/local.env; see scripts/local.env.example)}"
IOB="${IOSBUILDER:?IOSBUILDER is not set: the checkout of the original iOSBuilder project (environment or scripts/local.env; see scripts/local.env.example)}"
WT="${IOSBUILDER_WORKTREE:-$IOB}"
M=$SP/macho
PY=$SP/ref/venv/bin/python
export PYTHONDONTWRITEBYTECODE=1
mkdir -p "$M/py" "$M/flat" "$M/flat-nro" "$M/oracle" "$M/hosts"

# Python sources (copies; the originals stay read-only)
cp "$IOB/tools/assemble.py" "$IOB/tools/macho_write.py" "$IOB/tools/fix_tlv.py" "$IOB/engine/dm/ios/tools/gen_embed.py" "$M/py/"
# assemble.py wants <game_dir>/<name>.patched.bin + .fixups.tsv: symlink the per-module oracle files into one dir
for m in main sdk subsdk0; do
  for e in patched.bin fixups.tsv gates.tsv; do
    ln -sf "$IOB/out-py/pyrewrite/out/$m/$m.$e" "$M/flat/$m.$e"
    ln -sf "$IOB/out-py/pyrewrite/out/$m/$m.$e" "$M/flat-nro/$m.$e"
  done
done
printf '# comment\nsdk\tx\nrefused\tfoo\nsubsdk0\ty\n\n' > "$M/flat-nro/nro.tsv"
head -c 5000 /dev/zero | tr '\0' 'J' > "$M/jit.img"

# frozen host copies (skip a source that is gone: the copy already there is used)
cp_if() { [ -f "$1" ] && cp "$1" "$2" || echo "note: $1 absent, keeping $2"; }
cp_if "$IOB/out-asm/host-prebuilt-sim.dylib" "$M/hosts/asm-sim.dylib"
cp_if "$IOB/out-load/host-prebuilt-loadable-ios.dylib" "$M/hosts/load-ios.dylib"
cp_if "$IOB/out-load/host-prebuilt-loadable-sim.dylib" "$M/hosts/load-sim.dylib"
cp_if "$WT/out-load-ui/host-prebuilt-loadable-ios.dylib" "$M/hosts/wt-ios.dylib"
cp_if "$WT/out-load-ui/host-prebuilt-loadable-sim.dylib" "$M/hosts/wt-sim.dylib"
cp_if "$WT/out-game/gameapp-build-device/bin/Release/net11.0-ios/ios-arm64/GameApp.app/host-prebuilt.dylib" "$M/hosts/wt-app-ios.dylib"
cp_if "$IOB/out-device/gameapp-build-device/bin/Release/net11.0-ios/ios-arm64/GameApp.app/host-prebuilt.dylib" "$M/hosts/dev-4762784.dylib"

H=$M/hosts
asm() { n=$1; shift; "$PY" -I "$M/py/assemble.py" "$@" --out "$M/oracle/$n.dylib" > "$M/oracle/$n.stdout"; echo "oracle $n: $(wc -c < "$M/oracle/$n.dylib") bytes"; }
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

# macho_write.py draws LC_UUID from uuid4(): pin it so the output is reproducible
cat > "$M/py/macho_write_fixed_uuid.py" <<'PYEOF'
import uuid
import macho_write
FIXED = uuid.UUID("00112233-4455-6677-8899-aabbccddeeff")
macho_write.uuidlib.uuid4 = lambda: FIXED
macho_write.main()
PYEOF
(cd "$M/py" && {
  "$PY" macho_write_fixed_uuid.py --out "$M/oracle/mw-sim.dylib" --platform sim > "$M/oracle/mw-sim.stdout"
  "$PY" macho_write_fixed_uuid.py --out "$M/oracle/mw-ios.dylib" --platform ios --name DmGameX --magic 0x12345678 --minos 26.1 --sdk 27.2.1 > "$M/oracle/mw-ios.stdout"
  "$PY" macho_write_fixed_uuid.py --out "$M/oracle/mw-simname.dylib" --name A --magic 0xFFFFFFFF --minos 27.0.5 --sdk 28 > "$M/oracle/mw-simname.stdout"
})

# gen_embed.py
(cd "$M/py" && {
  "$PY" -I gen_embed.py "$M/flat" "$M/oracle/ge-base.S" main subsdk0 sdk > "$M/oracle/ge-base.flags"
  "$PY" -I gen_embed.py --tag T "$M/flat" "$M/oracle/ge-tag.S" main sdk > "$M/oracle/ge-tag.flags"
  "$PY" -I gen_embed.py --jit "$M/jit.img" "$M/flat" "$M/oracle/ge-jit.S" main > "$M/oracle/ge-jit.flags"
  "$PY" -I gen_embed.py --tag Q --jit "$M/jit.img" "$M/flat-nro" "$M/oracle/ge-nro.S" main > "$M/oracle/ge-nro.flags"
})
echo "done: $M/oracle"
