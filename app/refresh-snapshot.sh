#!/bin/bash
# app/refresh-snapshot.sh -- (re)take the snapshot of the app sources this build starts from.
#
# The newest iBuilder app lives in the iOSBuilder worktree ($IOSBUILDER_WORKTREE, read only here; it may keep changing). The Rust build does NOT read that
# folder for the C# or the C: it builds from the copies in app/gameapp/ and app/c/, so a build is reproducible. This script copies them again,
# and writes app/SNAPSHOT.tsv: relative path, size, mtime (source), sha256, source path (written as $IOSBUILDER_WORKTREE/<path>: no machine path is stored). Every file is read twice (stat before, copy, hash
# the copy, stat after): a source that changed while it was being copied is copied again, three times, then the script stops.
#
#   app/refresh-snapshot.sh            re-copy and rewrite SNAPSHOT.tsv (then re-apply the patches: scripts/build-app.sh does that on a copy)
#   app/refresh-snapshot.sh --check    only compare the sources with SNAPSHOT.tsv: prints what changed in the worktree since the snapshot
#
# Nothing is written outside iBuilderRust/app. IOSBUILDER_WORKTREE comes from the environment or from scripts/local.env (scripts/local.env.example); unset, the script stops and says so.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
[ -f "$HERE/../scripts/local.env" ] && . "$HERE/../scripts/local.env"
W="${IOSBUILDER_WORKTREE:?IOSBUILDER_WORKTREE is not set: the iOSBuilder tree that holds gameapp/ and sign/ (environment or scripts/local.env; see scripts/local.env.example)}"
TSV="$HERE/SNAPSHOT.tsv"
mode="${1:-copy}"

# <destination relative to app/> <source>
FILES=()
for f in "$W"/gameapp/GameApp*.cs; do FILES+=("gameapp/$(basename "$f")|$f"); done
FILES+=("gameapp/Info.plist.fragment|$W/gameapp/Info.plist.fragment"
        "gameapp/GameApp.entitlements|$W/gameapp/GameApp.entitlements"
        "gameapp/xlate/GameAppXlate.cs|$W/gameapp/xlate/GameAppXlate.cs"
        "gameapp/xlate/XlateCommonShim.cs|$W/gameapp/xlate/XlateCommonShim.cs"
        "c/spvc_cli.c|$W/gameapp/spvc_cli.c"
        "c/stdzstd.c|$W/gameapp/stdzstd.c"
        "c/nsz_decompress.c|$W/gameapp/nsz_decompress.c"
        "c/inapp_shim.cpp|$W/sign/inapp_shim.cpp"
        "c/inapp_shim.h|$W/sign/inapp_shim.h")
while IFS= read -r f; do FILES+=("gameapp/Assets.xcassets/${f#$W/gameapp/Assets.xcassets/}|$f"); done < <(find "$W/gameapp/Assets.xcassets" -type f ! -name .DS_Store | LC_ALL=C sort)

st() { stat -f '%z %Sm' -t '%Y-%m-%dT%H:%M:%S' "$1"; }
if [ "$mode" = "--check" ]; then
  bad=0
  while IFS=$'\t' read -r rel size mtime sha src; do
    [ "$rel" = "path" ] && continue
    src="${src/#\$IOSBUILDER_WORKTREE/$W}"   # the TSV stores the source relative to the placeholder
    if [ ! -f "$src" ]; then echo "GONE     $rel ($src)"; bad=1; continue; fi
    now="$(st "$src")"; h="$(shasum -a 256 < "$src" | cut -d' ' -f1)"
    [ "$h" = "$sha" ] || { echo "CHANGED  $rel  (snapshot $size $mtime, now $now)"; bad=1; }
  done < "$TSV"
  for e in "${FILES[@]}"; do dst="${e%%|*}"; grep -q "^$dst	" "$TSV" || { echo "NEW      $dst"; bad=1; }; done
  [ "$bad" = 0 ] && echo "the worktree sources equal the snapshot ($(($(wc -l < "$TSV") - 1)) files)"
  exit "$bad"
fi

tmp="$(mktemp)"; printf 'path\tsize\tmtime\tsha256\tsource\n' > "$tmp"
for e in "${FILES[@]}"; do
  dst="${e%%|*}"; src="${e#*|}"
  [ -f "$src" ] || { echo "missing source: $src" >&2; exit 1; }
  mkdir -p "$(dirname "$HERE/$dst")"
  ok=0
  for try in 1 2 3; do
    before="$(st "$src")"; cp -f "$src" "$HERE/$dst"; after="$(st "$src")"
    if [ "$before" = "$after" ] && [ "$(stat -f %z "$HERE/$dst")" = "${before%% *}" ]; then ok=1; break; fi
    echo "  $src changed while it was copied (try $try)" >&2; sleep 1
  done
  [ "$ok" = 1 ] || { echo "could not take a stable copy of $src" >&2; exit 1; }
  printf '%s\t%s\t%s\t%s\t%s\n' "$dst" "${before%% *}" "${before#* }" "$(shasum -a 256 < "$HERE/$dst" | cut -d' ' -f1)" "\$IOSBUILDER_WORKTREE${src#"$W"}" >> "$tmp"
done
mv "$tmp" "$TSV"
echo "snapshot: $(($(wc -l < "$TSV") - 1)) files -> $TSV"
