#!/bin/bash
# app/sim/place-grizzy.sh <app Documents folder> -- leave Documents exactly as the app's own stage 1 (the extraction) would, for Grizzy and the Lemmings, WITHOUT keys.
#
# Stage 1 needs prod.keys (never touched here: this script does not read, copy or name a key file). What the extraction leaves for the chain is read from the app's own
# code (GameAppMain.ConfigText: extract_out / exe_dir / romfs, and the harness gameapp/run_gameapp.sh --chain-only): extract/ExeFS/{main,sdk,subsdk0,main.npdm,rtld}, extract/RomFS.img
# (the DMROMFS1 image), extract/control.nacp, extract/icon.jpg. The one-shot request `gameapp.args` = "chain-only" makes the app's next launch skip the extraction and run the chain
# on them by itself (GameAppMain.AutoRunFromArgs). Everything is an APFS clone (`cp -c`): instant, no disk cost, the inputs stay read only.
#
#   IOSBUILDER  the checkout of the original iOSBuilder project (read only)  [required: environment or scripts/local.env, see scripts/local.env.example]
#   ROMFS       the Grizzy RomFS image (DMROMFS1, 2,498,033,693 bytes, mtime 2026-11-15 18:15:09). The first of these that exists is used:
#                 $IOSBUILDER/out/ref-img-grizzy/RomFS.img           (deleted from the checkout on 2026-11-19 ~12:09)
#                 $IOSBUILDER/out-load/0100DFA027504000.nxgame/romfs.img   (the same image packed into the nxgame; deleted ~12:24)
#                 an APFS clone another package of this round made: $IBUILDER_SCRATCH/mem/ios/out/ref-img-grizzy/RomFS.img   (SCRATCHPAD is the older name of IBUILDER_SCRATCH)
#                 the pack oracle's copy of an earlier round: $IBUILDER_SCRATCH_OLD/pack-fix/real-rs/0100DFA027504000.nxgame/romfs.img
set -euo pipefail
DOCS="${1:?usage: place-grizzy.sh <app Documents folder>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
IOS="${IOSBUILDER:?IOSBUILDER is not set: the checkout of the original iOSBuilder project (environment or scripts/local.env; see scripts/local.env.example)}"
SRC="$IOS/out/ref-out-grizzy"
SCR="${SCRATCHPAD:-${IBUILDER_SCRATCH:-/nonexistent-IBUILDER_SCRATCH}}"
PREV="${IBUILDER_SCRATCH_OLD:-/nonexistent-IBUILDER_SCRATCH_OLD}"
if [ -z "${ROMFS:-}" ]; then
  for c in "$IOS/out/ref-img-grizzy/RomFS.img" "$IOS/out-load/0100DFA027504000.nxgame/romfs.img" "$SCR/mem/ios/out/ref-img-grizzy/RomFS.img" "$PREV/pack-fix/real-rs/0100DFA027504000.nxgame/romfs.img"; do
    [ -f "$c" ] && [ "$(stat -f %z "$c")" = 2498033693 ] && { ROMFS="$c"; break; }
  done
fi
[ -d "$SRC/ExeFS" ] && [ -f "${ROMFS:-/nonexistent}" ] || { echo "inputs missing: $SRC/ExeFS or the RomFS image (ROMFS=${ROMFS:-none found})" >&2; exit 1; }
echo "RomFS image: $ROMFS ($(stat -f %z "$ROMFS") bytes)"
case "$DOCS" in */Documents) ;; *) echo "not a Documents folder: $DOCS" >&2; exit 1 ;; esac
mkdir -p "$DOCS/extract"
clone() { cp -c -R "$1" "$2" 2>/dev/null || cp -R "$1" "$2"; }
# a previous placement is replaced (only these four names under extract/)
rm -rf -- "$DOCS/extract/ExeFS" "$DOCS/extract/RomFS.img" "$DOCS/extract/control.nacp" "$DOCS/extract/icon.jpg"
clone "$SRC/ExeFS" "$DOCS/extract/ExeFS"
clone "$SRC/control.nacp" "$DOCS/extract/control.nacp"
clone "$SRC/icon.jpg" "$DOCS/extract/icon.jpg"
clone "$ROMFS" "$DOCS/extract/RomFS.img"
printf 'chain-only\n' > "$DOCS/gameapp.args"
echo "placed under $DOCS:"; ls -la "$DOCS/extract" "$DOCS/extract/ExeFS" | sed 's/^/  /'
echo "  gameapp.args: $(cat "$DOCS/gameapp.args")"
