#!/bin/bash
# scripts/check-patches-apply.sh -- do the C# patches of app/patches/*.patch apply, in order, to a fresh copy of the C# snapshot? Nothing is built and app/gameapp/ is never written.
#
# Exactly what scripts/build-app.sh does with them (a copy of app/gameapp/GameApp*.cs, then `patch -p1 --forward` for every app/patches/*.patch in glob order), with a `--dry-run` of each patch
# before the real one, so a patch that does not apply says which and why. A C# compiler is not involved: this proves the patches APPLY, not that the result compiles (docs/REVIEW-TRIAGE-2026-11-19.md).
#
# usage: scripts/check-patches-apply.sh        exit 0 = every patch applied, 1 = one did not
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
W="$(mktemp -d "${TMPDIR:-/tmp}/check-patches.XXXXXX")" || exit 2
trap 'rm -rf "${W:?}"' EXIT
P="$W/proj"
mkdir -p "$P"
for f in "$ROOT"/app/gameapp/GameApp*.cs "$ROOT"/app/gameapp/Info.plist.fragment; do cp "$f" "$P/"; done
shopt -s nullglob
rc=0
for p in "$ROOT"/app/patches/*.patch; do
  name="$(basename "$p")"
  if ! ( cd "$P" && patch -p1 --forward --no-backup-if-mismatch --dry-run < "$p" ) > "$W/dry.log" 2>&1; then
    echo "DOES NOT APPLY: $name (dry run)"; cat "$W/dry.log"; rc=1; break
  fi
  if ! ( cd "$P" && patch -p1 --forward --no-backup-if-mismatch < "$p" ) > "$W/real.log" 2>&1; then
    echo "DOES NOT APPLY: $name"; cat "$W/real.log"; rc=1; break
  fi
  echo "applied $name: $(grep '^patching file' "$W/real.log" | sed 's/^patching file //' | tr '\n' ' ')"
  # a hunk that needed fuzz or an offset applied, but not exactly where the patch says: said, not hidden
  grep -E 'fuzz|offset' "$W/real.log" | sed 's/^/    /'
  true
done
[ "$rc" = 0 ] && echo "all $(ls "$ROOT"/app/patches/*.patch | wc -l | tr -d ' ') patches apply, in order, to a fresh copy of app/gameapp/GameApp*.cs"
exit "$rc"
