#!/bin/zsh
# e2e.sh <name> [--random N] [--seed S]: a synthetic game (make_game.py), the Python build (phone driver, real translator + fake answers) and the Rust build (shaderbuild --finisher rust)
# over it, then libdiff. Env: S2, OLD (see run.sh), IOSBUILDER, TARGET (cargo target dir). Output: $S2/sh3/<name>/{game,py,rs}
ROOT=$(cd "$(dirname "$0")/../../../.." && pwd)   # the repository
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"   # scripts/local.env.example
S2=${S2:-${IBUILDER_SCRATCH:?IBUILDER_SCRATCH is not set (the scratch folder; environment or scripts/local.env, see scripts/local.env.example)}}
OLD=${OLD:-${IBUILDER_SCRATCH_OLD:-$S2}}
TARGET=${TARGET:-$S2/target-sh3}
T=$ROOT/crates/ibuilder-shader/tools/tessmesh
PY=$OLD/ref/venv/bin/python
REAL=${IOSBUILDER:?IOSBUILDER is not set (the original iOSBuilder checkout; environment or scripts/local.env, see scripts/local.env.example)}/engine/dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate
N=$1; shift
D=$S2/sh3/$N; rm -rf $D; mkdir -p $D
GAME_TSV=$OLD/shader/profile/0100DFA027504000/game.tsv PYTHONDONTWRITEBYTECODE=1 $PY -I $T/make_game.py $S2/sh3/engine $S2/sh3/enginetests $D/game "$@" || exit 2
export TM_FAKE=$D/game/fake TM_REAL_XLATE=$REAL
# the Rust build's engine folder: the real spirv-cross and a dmsh_xlate that is fake_xlate.py with its environment baked in (tests/tessmesh_e2e.rs of ibuilder-shaderbuild uses it as it is)
mkdir -p $D/eng/dm/gfx/tools/dmsh_xlate_bin $D/eng/dm/gfx/tools/spirv_cross_bin
ln -sf $OLD/shader/engine/dm/gfx/tools/spirv_cross_bin/spirv-cross $D/eng/dm/gfx/tools/spirv_cross_bin/spirv-cross
cat > $D/eng/dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate <<EOS
#!/bin/sh
TM_FAKE=$D/game/fake TM_REAL_XLATE=$REAL TM_JOBLOG=\${TM_JOBLOG:-$D/jobs-test} exec $PY -I $T/fake_xlate.py "\$@"
EOS
chmod +x $D/eng/dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate
TM_JOBLOG=$D/jobs-py PYTHONDONTWRITEBYTECODE=1 $PY -I $T/run_py_build.py --driver $S2/sh3/bin --engine $S2/sh3/engine --pkg $D/game/pkg --profile $D/game/profile --out $D/py > $D/py.log 2>&1 || { tail -20 $D/py.log; exit 3; }
mkdir -p $D/rs
TM_JOBLOG=$D/jobs-rs $TARGET/release/shaderbuild build --pkg $D/game/pkg --profile $D/game/profile --game 0100DFA027504000 --out $D/rs --chain 6c53446391cc --engine $D/eng --finisher rust --jobs 2 --tables 0 --pulling 0 --cas $D/rs/cas > $D/rs.log 2>&1 || { tail -20 $D/rs.log; exit 4; }
grep -E "^shaders|mesh libraries" $D/py.log $D/rs.log
$TARGET/release/libdiff $D/py/b-6c53446391cc.img $D/rs/b-6c53446391cc.img
# the translator's job files, as each build wrote them (job_for): the same jobs and the same code bytes
diff -rq $D/jobs-py $D/jobs-rs && echo "translator jobs: $(ls $D/jobs-py | wc -l) job files, identical in both builds (job text and code sha256)"
