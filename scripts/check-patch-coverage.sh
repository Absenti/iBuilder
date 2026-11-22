#!/usr/bin/env bash
# Checks that the Rust rewriter (ibuilder-rewrite) covers everything the engine's per-game patch files register, so that an engine update cannot silently leave it stale:
#   1. src/tables.rs must be what tools/gen_patch_tables.py makes of the engine's patch files (CUTS / OBS / CAPABILITIES / MATCH_CUTS and the data around the stub functions);
#   2. every code-valued registration (FRAME_STEP / STUBS by build ID, MATCH_FRAME_STEP / MATCH_STUBS by rule) must have a ported stub function (src/stubs_games/) that was
#      verified against the patch file's current code (the code digest): `ibuilder-rewrite --list-ported` must list every row the engine registers.
# Fails (exit 1) listing every build / rule the Rust cannot rewrite and every stale table. Skips loudly (exit 0, "SKIP" on stdout) when there is no engine tree.
#
# usage: scripts/check-patch-coverage.sh [<engine>/dm]      (or IBUILDER_ENGINE_DM=<engine>/dm; IBUILDER_PYTHON=python3; IBR_BIN=<ibuilder-rewrite binary>, else cargo builds it)
# After an engine update:  python3 -B crates/ibuilder-rewrite/tools/gen_patch_tables.py <engine>/dm --coverage crates/ibuilder-rewrite/coverage.tsv > crates/ibuilder-rewrite/src/tables.rs
# then port what this lists (tools/stubtest.py is the differential test against the real Python) and put the new code digest in the game's `GamePort`.
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"   # may set IBUILDER_ENGINE_DM (scripts/local.env.example)
DM=${1:-${IBUILDER_ENGINE_DM:-}}
if [ -z "$DM" ] || [ ! -d "$DM/games" ] || [ ! -f "$DM/tools/patch_nso.py" ]; then
    echo "SKIP check-patch-coverage: no engine tree (pass <engine>/dm, or set IBUILDER_ENGINE_DM)"
    exit 0
fi
PY=${IBUILDER_PYTHON:-python3}
TMP=$(mktemp -d "${TMPDIR:-/tmp}/patchcov.XXXXXX") || exit 1
trap 'rm -rf "${TMP:?}"' EXIT
GEN="$ROOT/crates/ibuilder-rewrite/tools/gen_patch_tables.py"
if ! PYTHONDONTWRITEBYTECODE=1 "$PY" -B "$GEN" "$DM" --coverage "$TMP/engine.tsv" > "$TMP/tables.rs" 2> "$TMP/gen.err"; then
    echo "FAIL: tools/gen_patch_tables.py could not read the engine's patch files:"; cat "$TMP/gen.err"; exit 1
fi
BIN=${IBR_BIN:-}
if [ -z "$BIN" ]; then
    (cd "$ROOT" && cargo build --release --offline -q -p ibuilder-rewrite --bin ibuilder-rewrite) || { echo "FAIL: cargo build"; exit 1; }
    BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/ibuilder-rewrite"
fi
"$BIN" --list-ported > "$TMP/rust.tsv" || { echo "FAIL: $BIN --list-ported"; exit 1; }
fail=0
if ! cmp -s "$TMP/tables.rs" "$ROOT/crates/ibuilder-rewrite/src/tables.rs"; then
    echo "STALE: crates/ibuilder-rewrite/src/tables.rs is not what the engine's patch files generate (first differences):"
    diff "$ROOT/crates/ibuilder-rewrite/src/tables.rs" "$TMP/tables.rs" | cut -c1-200 | head -12
    echo "  regenerate: python3 -B crates/ibuilder-rewrite/tools/gen_patch_tables.py $DM --coverage crates/ibuilder-rewrite/coverage.tsv > crates/ibuilder-rewrite/src/tables.rs"
    fail=1
fi
# the engine's rows against what this build can rewrite (kind, profile or file, key, code digest)
LC_ALL=C grep -v '^#' "$TMP/engine.tsv" | cut -f1-4 | LC_ALL=C sort > "$TMP/e.txt"
LC_ALL=C grep -v '^#' "$TMP/rust.tsv" | cut -f1-4 | LC_ALL=C sort > "$TMP/r.txt"
LC_ALL=C comm -23 "$TMP/e.txt" "$TMP/r.txt" > "$TMP/missing.txt"
LC_ALL=C comm -13 "$TMP/e.txt" "$TMP/r.txt" > "$TMP/extra.txt"
if [ -s "$TMP/missing.txt" ]; then
    echo "CANNOT REWRITE: the engine registers these and this build of the rewriter does not (not ported, or the port is older than the patch file's code):"
    while IFS=$'\t' read -r kind who key digest; do
        game=$(LC_ALL=C grep -F "$(printf '%s\t%s\t%s\t%s\t' "$kind" "$who" "$key" "$digest")" "$TMP/engine.tsv" | head -1 | cut -f5)
        printf '  %-17s %s  %s  [%s]  %s\n' "$kind" "$who" "$key" "$digest" "$game"
    done < "$TMP/missing.txt"
    fail=1
fi
if [ -s "$TMP/extra.txt" ]; then
    echo "STALE: this build lists registrations the engine no longer has:"
    sed 's/^/  /' "$TMP/extra.txt"
    fail=1
fi
if [ "$fail" = 0 ]; then
    echo "OK: all $(wc -l < "$TMP/e.txt" | tr -d ' ') registrations of the engine's patch files ($(LC_ALL=C grep -c -E '^(FRAME_STEP|STUBS|MATCH_FRAME_STEP|MATCH_STUBS)' "$TMP/engine.tsv") code-valued) are covered by this build of the rewriter"
fi
exit $fail
