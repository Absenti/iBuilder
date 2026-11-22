#!/bin/sh
# Differential harness: crates/ibuilder-mods against gameapp/mods.py.
#   usage: run.sh <scratch dir> <python with lz4> <path of a COPY of gameapp/mods.py> [cases per seed=300] [seeds="1 2 3"]
# Runs, per seed: N synthetic scenarios (mod folders + NSO-like modules, ~13 steps each: plan, apply, mark, signature, boot, cheats ... compared with the logs and the final file trees),
# 9 x N micro-operations (ips, pchtxt, repr, bid, stem, sections, NSO image/patch) and 3 x 40,000 LZ4 streams (valid, mutated, truncated, garbage).
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"; S="$1"; PY="$2"; export MODS_PY="$3"; N="${4:-300}"; SEEDS="${5:-1 2 3}"
export PYTHONDONTWRITEBYTECODE=1
REPO="$(cd "$HERE/../../../.." && pwd)"
CARGO_TARGET_DIR="$S/target" cargo build --release --manifest-path "$REPO/Cargo.toml" -p ibuilder-mods --features diff --bin mods_diff
RS="$S/target/release/mods_diff"
FAIL=0
for seed in $SEEDS; do
  R="$S/run-$seed"; rm -rf "$R"; mkdir -p "$R/micro" "$R/lz4"
  "$PY" -I "$HERE/gen_cases.py" "$N" "$seed" "$R"
  "$PY" -I "$HERE/mods_diff.py" "$R/ops_py.json" "$R/out_py.json"
  "$RS" "$R/ops_rs.json" "$R/out_rs.json"
  "$PY" -I "$HERE/compare.py" "$R" scenario || FAIL=1
  "$PY" -I "$HERE/gen_micro.py" "$seed" "$((N * 5))" "$HERE/gen_cases.py" "$R/micro/ops_py.json"
  "$PY" -I "$HERE/mods_diff.py" "$R/micro/ops_py.json" "$R/micro/out_py.json"
  "$RS" "$R/micro/ops_py.json" "$R/micro/out_rs.json"
  "$PY" -I "$HERE/compare.py" "$R/micro" micro || FAIL=1
  "$PY" -I "$HERE/fuzz_lz4.py" 40000 "$seed" "$R/lz4/ops_py.json"
  "$PY" -I "$HERE/mods_diff.py" "$R/lz4/ops_py.json" "$R/lz4/out_py.json"
  "$RS" "$R/lz4/ops_py.json" "$R/lz4/out_rs.json"
  "$PY" -I "$HERE/compare_lz4.py" "$R/lz4/ops_py.json" "$R/lz4/out_py.json" "$R/lz4/out_rs.json" || FAIL=1
done
echo "differential harness: $([ $FAIL = 0 ] && echo ALL IDENTICAL || echo MISMATCHES)"
exit $FAIL
