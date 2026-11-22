#!/bin/bash
# scripts/test-report.sh -- `cargo test --release --workspace --no-fail-fast`, then what the green number really means.
#
# libtest counts a test that returns early as PASSED and hides its stderr, so "N passed" also counted every test that compared nothing because a fixture (a game's files, the Python oracle, a tool)
# is not on this machine (docs/REVIEW-TRIAGE-2026-11-19.md, finding 3.2). Every such test calls `ibuilder_testkit::skip(reason)`, which prints, appends one line `crate<TAB>test<TAB>reason` to a
# log file, and PANICS when IBUILDER_STRICT_FIXTURES=1. This script runs the suite with that log pointed at a fresh file, then prints
#
#     N passed, K of them skipped (fully or partly) for missing fixtures      <- the verifications that did NOT happen are K of the N
#     ... by crate, with the reasons ...
#
# and exits non-zero when a test failed or (IBUILDER_STRICT_FIXTURES=1) when anything was skipped. In strict mode a skip is itself a test failure, so the run fails the way a failing test does.
#
# usage: scripts/test-report.sh [extra cargo test args, e.g.  --exclude ibuilder-cli   or   -- --include-ignored   or   -- --test-threads=4]   (always the whole workspace)
# env:   IBUILDER_STRICT_FIXTURES=1  fail on any skip          CARGO_TARGET_DIR  as for cargo (the log lives under it)
#        IOSBUILDER, IOSBUILDER_WORKTREE, IBUILDER_SCRATCH, IBUILDER_SCRATCH_OLD: where the fixtures are (environment or scripts/local.env, see scripts/local.env.example); without them the
#        tests that need those inputs skip, and the report says so.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"   # where the fixtures of this machine are (scripts/local.env.example); the tests read the same file
cd "$ROOT" || exit 2
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
mkdir -p "$TARGET"
LOG="$TARGET/skipped-tests.log"
OUT="$TARGET/test-report.out"
: > "$LOG"
export IBUILDER_SKIP_LOG="$LOG"

cargo test --release --workspace --no-fail-fast "$@" 2>&1 | tee "$OUT"
RC="${PIPESTATUS[0]}"

# every "test result:" line of every test binary: "test result: ok. P passed; F failed; I ignored; ..."
read -r PASSED FAILED IGNORED < <(awk '/^test result:/ { p += $4; f += $6; i += $8 } END { print p + 0, f + 0, i + 0 }' "$OUT")
# distinct (crate, test) pairs that skipped at least once: one test may skip many times (a loop over cases)
SKIPPED="$(cut -f1,2 "$LOG" | sort -u | wc -l | tr -d ' ')"

echo
echo "================================================================================================"
echo "$PASSED passed, $SKIPPED of them skipped (fully or partly) for missing fixtures; $FAILED failed, $IGNORED ignored (not run)"
if [ "$SKIPPED" -gt 0 ]; then
  echo "skipped tests, by crate (a skip is a verification that did not happen on this machine):"
  cut -f1,2 "$LOG" | sort -u | cut -f1 | sort | uniq -c | awk '{ printf "  %-24s %s\n", $2, $1 }'
  echo "reasons (log: $LOG):"
  sort -u "$LOG" | awk -F'\t' '{ printf "  %-22s %-58s %s\n", $1, substr($2, 1, 58), $3 }' | cut -c1-230
else
  echo "no test skipped: everything that was compared was compared"
fi
echo "================================================================================================"

if [ "$RC" -ne 0 ] || [ "$FAILED" -gt 0 ]; then
  echo "cargo test failed (exit $RC)" >&2
  [ "$RC" -ne 0 ] && exit "$RC"
  exit 1
fi
if [ "${IBUILDER_STRICT_FIXTURES:-0}" = 1 ] && [ "$SKIPPED" -gt 0 ]; then
  echo "IBUILDER_STRICT_FIXTURES=1 and $SKIPPED test(s) skipped: not a verified run" >&2
  exit 1
fi
exit 0
