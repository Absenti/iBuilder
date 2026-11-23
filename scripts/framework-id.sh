#!/bin/bash
# scripts/framework-id.sh [device|simulator] [--id] -- the FRAMEWORK ID scripts/build-app.sh would give this tree, without building anything.
#
# The framework id (GameAppBuild.FrameworkId) is what a built game's stamp is compared with after an app update: the same id boots the game already built ("Play"), another id
# rebuilds it. It hashes only what shapes a built framework and its boot (scripts/lib-hash.sh, framework_id_listing, has the list and why each input is there).
#
# The inputs are found as build-app.sh finds them: scripts/local.env if present, NXPOCKET_ENGINE (default target/host-work/engine when scripts/build-host.sh left it there, else $IOSBUILDER_WORKTREE/out-stage/out-engine/engine, IOSBUILDER_WORKTREE
# defaulting to $IOSBUILDER), GAMEAPP_HOST (default dist/host/host-prebuilt-loadable-{ios,sim}.dylib, else the worktree's out-load-ui/). Nothing is written.
#
# usage: scripts/framework-id.sh                 the listing (name TAB value), then "framework id <16 hex>"
#        scripts/framework-id.sh simulator --id  the simulator build's id alone
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
. "$ROOT/scripts/lib-hash.sh"
KIND=device; ONLY_ID=0
for a in "$@"; do
  case "$a" in device|simulator) KIND="$a" ;; --id) ONLY_ID=1 ;; *) echo "usage: $0 [device|simulator] [--id]" >&2; exit 2 ;; esac
done
IOSBUILDER="${IOSBUILDER:-${IOSBUILDER_MAIN:-}}"
W="${IOSBUILDER_WORKTREE:-$IOSBUILDER}"
ENGINE="${NXPOCKET_ENGINE:-$( [ -d "$ROOT/target/host-work/engine/dm/tools/nx_extract" ] && echo "$ROOT/target/host-work/engine" || echo "$W/out-stage/out-engine/engine")}"   # as build-app.sh finds it: the tree scripts/build-host.sh leaves in target/host-work, else the out-stage engine
case "$KIND" in device) SUF=ios ;; simulator) SUF=sim ;; esac
HOST_DYLIB="${GAMEAPP_HOST:-$( [ -f "$ROOT/dist/host/host-prebuilt-loadable-$SUF.dylib" ] && echo "$ROOT/dist/host/host-prebuilt-loadable-$SUF.dylib" || echo "$W/out-load-ui/host-prebuilt-loadable-$SUF.dylib" )}"
SHADER_CHAIN="$(grep -o 'DMSH_CHAIN "[0-9a-f]*"' "$ENGINE/dm/gfx/nvn/dmsh_chain.h" 2>/dev/null | cut -d'"' -f2)"
[ -n "$SHADER_CHAIN" ] || echo "WARNING: no DMSH_CHAIN in $ENGINE/dm/gfx/nvn/dmsh_chain.h: the id below is not the one a build would make" >&2
[ -f "$HOST_DYLIB" ] || echo "WARNING: no host at $HOST_DYLIB: the id below is not the one a build would make" >&2
# the cargo features exactly as build-app.sh step 1 chooses them
FFI_FEATURES="real-stages"
if grep -qE '^nup *=' "$ROOT/crates/ibuilder-cli/Cargo.toml" 2>/dev/null && ! grep -E '^default *=' "$ROOT/crates/ibuilder-cli/Cargo.toml" | grep -q '"nup"'; then
  FFI_FEATURES="real-stages,ibuilder-cli/nup"
fi
LISTING="$(framework_id_listing "$ROOT" "$ENGINE" "$HOST_DYLIB" "$SHADER_CHAIN" "$FFI_FEATURES")"
ID="$(printf '%s\n' "$LISTING" | framework_id_of)"
if [ "$ONLY_ID" = 1 ]; then echo "$ID"; else printf '%s\n' "$LISTING"; echo "framework id $ID"; fi
