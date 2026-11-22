#!/bin/zsh
# dump.sh <corpus dir> <case id>: both outputs of one case in <corpus>/dump/{py,rs}/ and their diff
ROOT=$(cd "$(dirname "$0")/../../../.." && pwd)   # the repository
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"   # scripts/local.env.example
S2=${S2:-${IBUILDER_SCRATCH:?IBUILDER_SCRATCH is not set (the scratch folder; environment or scripts/local.env, see scripts/local.env.example)}}
OLD=${OLD:-${IBUILDER_SCRATCH_OLD:-$S2}}
T=$ROOT/crates/ibuilder-shader/tools/difftest
C=$1; ID=$2
mkdir -p $C/dump/py $C/dump/rs; awk -F"\t" -v id=$ID '$1==id' $C/cases.tsv > $C/dump/one.tsv
PYTHONDONTWRITEBYTECODE=1 $OLD/ref/venv/bin/python -I $T/stepdiff.py $S2/f1/engine $C/dump/one.tsv $C/dump/py.tsv --jobs 1 --dump $C/dump/py $ID
$S2/target-f1/release/step_diff --cases $C/dump/one.tsv --out $C/dump/rs.tsv --dump $C/dump/rs $ID
cat $C/dump/py.tsv | cut -c1-300; cat $C/dump/rs.tsv | cut -c1-300
diff $C/dump/py/$ID.* $C/dump/rs/$ID.* | head -${3:-30}
