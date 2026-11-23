#!/bin/bash
# scripts/e2e-grizzy.sh -- the WHOLE chain on Grizzy and the Lemmings through the Mac CLI (profile -> rewrite -> shaders -> assemble -> ad hoc sign), and its checks.
# docs/reference/pipeline.md describes the end-to-end check.
#
#   scripts/e2e-grizzy.sh run    NAME [ibuilder options...]     run the CLI into $E2E_ROOT/NAME under /usr/bin/time -l (logs: NAME.{stdout,time,report.tsv,uptime.*}); exit status = the CLI's
#   scripts/e2e-grizzy.sh kill   NAME SECONDS [options...]      the same, but SIGTERM after SECONDS (an interrupted stage 3), then what it left behind
#   scripts/e2e-grizzy.sh verify NAME                           (2) the 21 rewrite files vs the Python oracle, (3) the shader library vs the Python's, (4) codesign on the Mach-O
#
# The three runs of the docs:  run cold  /  run warm   (same NAME again: everything cached)  /  kill resume 30, then run resume --shader-jobs 6  /  verify cold
# Build first (two commands, as they were run):  cargo build --release -p ibuilder-cli --features shader-host-tools     (the CLI)
#                                               cargo build --release -p ibuilder-shaderbuild --features host-tools,f2-finish     (libdiff)
#
# Environment (all read only inputs; nothing is ever written outside $E2E_ROOT):
#   IBUILDER   the CLI binary                [target/release/ibuilder]        LIBDIFF   libdiff (ibuilder-shaderbuild)   [target/release/libdiff]
#   E2E_ROOT   where the output folders go   [$TMPDIR/ibuilder-e2e]           IOSBUILDER  the checkout holding the Grizzy inputs, the host dylib, the oracle  [required: environment or scripts/local.env]
#   ENGINE     the engine folder (dm/gfx/tools/{dmsh_xlate_bin,spirv_cross_bin} are RUN; dm/games and dm/host are READ as the profile bundle)   [$IOSBUILDER/engine]
#   CHAIN      the host's shader chain       [6c53446391cc]                   PYLIB     the Python's full library image of Grizzy (21,990 shaders)  [none: (3) is skipped without it]
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
IOS="${IOSBUILDER:?IOSBUILDER is not set: the checkout of the original iOSBuilder project (environment or scripts/local.env; see scripts/local.env.example)}"
BIN="${IBUILDER:-$ROOT/target/release/ibuilder}"
LIBDIFF="${LIBDIFF:-$ROOT/target/release/libdiff}"
E2E="${E2E_ROOT:-${TMPDIR:-/tmp}/ibuilder-e2e}"
ENGINE="${ENGINE:-$IOS/engine}"
CHAIN="${CHAIN:-6c53446391cc}"
TID=0100DFA027504000
cmd="${1:-}"; shift || true

cli() { # <out dir> <report> [options...]
  local out="$1" report="$2"; shift 2
  exec "$BIN" --exefs "$IOS/out/ref-out-grizzy/ExeFS" --out "$out" --bundle "$ENGINE" --host "$IOS/out-load/host-prebuilt-loadable-ios.dylib" \
    --romfs "$IOS/out/ref-img-grizzy/RomFS.img" --shaders --shader-chain "$CHAIN" --engine "$ENGINE" --report "$report" "$@"
}

case "$cmd" in
run)
  name="${1:?usage: run NAME [options]}"; shift
  mkdir -p "$E2E"; out="$E2E/$name"; uptime > "$E2E/$name.uptime.before"
  /usr/bin/time -l "$BIN" --exefs "$IOS/out/ref-out-grizzy/ExeFS" --out "$out" --bundle "$ENGINE" --host "$IOS/out-load/host-prebuilt-loadable-ios.dylib" \
    --romfs "$IOS/out/ref-img-grizzy/RomFS.img" --shaders --shader-chain "$CHAIN" --engine "$ENGINE" --report "$E2E/$name.report.tsv" "$@" \
    > "$E2E/$name.stdout" 2> "$E2E/$name.time"
  rc=$?; echo "exit=$rc" >> "$E2E/$name.stdout"; uptime > "$E2E/$name.uptime.after"
  cat "$E2E/$name.stdout"; grep -E "real|maximum resident|peak memory" "$E2E/$name.time"; exit $rc ;;
kill)
  name="${1:?usage: kill NAME SECONDS [options]}"; secs="${2:?usage: kill NAME SECONDS [options]}"; shift 2
  mkdir -p "$E2E"; out="$E2E/$name"
  count() { find "$out/shadercache" -type f -not -name '*.tmp*' 2>/dev/null | wc -l | tr -d ' '; }
  echo "before: cache entries $(count)"
  cli "$out" "$E2E/$name.kill.report.tsv" "$@" > "$E2E/$name.kill.stdout" 2> "$E2E/$name.kill.stderr" &
  pid=$!; sleep "$secs"
  echo "stage file at the kill: $(cat "$out/gameapp.stage" 2>/dev/null)"
  kill -TERM "$pid"; wait "$pid" 2>/dev/null; rc=$?
  for _ in 1 2 3 4 5 6 7 8 9 10; do pgrep -f "$out/" > /dev/null || break; sleep 1; done   # the translator children of the killed CLI finish the call they were in
  echo "killed after ${secs} s: exit status $rc (143 = SIGTERM)"
  echo "after the kill: cache entries $(count), stray tmp files $(find "$out/shadercache" -type f -name '*.tmp*' 2>/dev/null | wc -l | tr -d ' ')"
  ls "$out/shaders"/*.work 2>&1 | head -10
  echo "split jobs kept: $(ls "$out/shaders"/*.work/splitjobs 2>/dev/null | wc -l | tr -d ' ')" ;;
verify)
  name="${1:?usage: verify NAME}"; out="$E2E/$name"; bad=0
  n=0; d=0
  for m in main sdk subsdk0; do for e in patched.bin fixups.tsv sites.tsv gates.tsv cuts.tsv obs.tsv changed.tsv; do
    if cmp "$out/extract/rewrite/$m/$m.$e" "$IOS/out-py/pyrewrite/out/$m/$m.$e"; then n=$((n+1)); else d=$((d+1)); fi
  done; done
  echo "(2) rewrite files identical to the Python oracle: $n of 21, different: $d"; [ "$d" = 0 ] || bad=1
  lib="$out/shaders/$TID/b-$CHAIN.img"
  if [ -n "${PYLIB:-}" ] && [ -f "$PYLIB" ] && [ -x "$LIBDIFF" ]; then
    echo "(3) libdiff, our library vs the Python's:"
    "$LIBDIFF" "$lib" "$PYLIB" --ignore-first-line c1_full.tsv --ignore-first-line cb_extent.tsv
    echo "    exit $? (3 files differ by design when the profile differs: c1_full.tsv, cb_extent.tsv, inputs.tsv: see docs/reference/pipeline.md)"
  else echo "(3) skipped: set PYLIB to the Python's b-$CHAIN.img and build libdiff"; fi
  # (4) the Mach-O alone: copied out of the bundle folder (inside X.framework/ codesign also wants _CodeSignature/CodeResources, which ibuilder-sign does not write)
  tmp="$(mktemp -d)"; cp "$out/DmGameProbe.framework/DmGameProbe" "$tmp/DmGameProbe"
  echo "(4) codesign --verify --strict on the signed Mach-O:"; codesign --verify --strict --verbose=2 "$tmp/DmGameProbe" 2>&1 | sed "s|$tmp/||"; [ "${PIPESTATUS[0]}" = 0 ] || bad=1
  rm -rf "$tmp"; exit $bad ;;
*) sed -n '2,13p' "$0"; exit 2 ;;
esac
