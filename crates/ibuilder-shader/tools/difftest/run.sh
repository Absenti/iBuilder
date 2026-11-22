#!/bin/zsh
# run.sh <corpus dir> [step]: Python and Rust over <corpus>/cases.tsv, then compare.py. Env: S2 (the scratch folder, default $IBUILDER_SCRATCH), OLD (the earlier scratch folder with the reference environment, default $IBUILDER_SCRATCH_OLD), JOBS
ROOT=$(cd "$(dirname "$0")/../../../.." && pwd)   # the repository
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"   # scripts/local.env.example
S2=${S2:-${IBUILDER_SCRATCH:?IBUILDER_SCRATCH is not set (the scratch folder; environment or scripts/local.env, see scripts/local.env.example)}}
OLD=${OLD:-${IBUILDER_SCRATCH_OLD:-$S2}}
T=$ROOT/crates/ibuilder-shader/tools/difftest
C=$1; ONLY=${2:+--only $2}
mkdir -p $C/res
PYTHONDONTWRITEBYTECODE=1 nice $OLD/ref/venv/bin/python -I $T/stepdiff.py $S2/f1/engine $C/cases.tsv $C/res/py.tsv --jobs ${JOBS:-4} ${=ONLY} || exit 2
$S2/target-f1/release/step_diff --cases $C/cases.tsv --out $C/res/rs.tsv ${=ONLY} || exit 2
python3 $T/compare.py $C/res/py.tsv $C/res/rs.tsv $C/cases.tsv
