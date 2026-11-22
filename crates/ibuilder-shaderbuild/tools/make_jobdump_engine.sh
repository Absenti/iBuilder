#!/bin/zsh
# An instrumented COPY of the engine's Python for the job oracle (never the original): dmsh_build.py's translate() also writes, for each shader it translates,
# <JOBDUMP>/<key>/{job.json (json.dumps of the job), code.bin (the bytes of <key>.bin), c1.bin} right after job_for, and with JOBDUMP_RES=1 tr.spv / tr.res.json (the translator's answers).
# usage: make_jobdump_engine.sh <engine copy dir that holds dm/gfx/tools>      (the copy is edited in place)
set -e
f="$1/dm/gfx/tools/dmsh_build.py"
python3 - "$f" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
old = """    for k in miss: jobs[k], c1s[k] = job_for(k, loc[k][0], rec_of(k)[1], wb)
    def xl(ch):"""
new = """    for k in miss: jobs[k], c1s[k] = job_for(k, loc[k][0], rec_of(k)[1], wb)
    _JD = os.environ.get('JOBDUMP')
    if _JD:
        for k in miss:
            d = os.path.join(_JD, k); os.makedirs(d, exist_ok=True)
            open(d + '/job.json', 'w').write(json.dumps(jobs[k]))
            open(d + '/code.bin', 'wb').write(open(jobs[k]['code'], 'rb').read())
            open(d + '/c1.bin', 'wb').write(c1s[k])
    def xl(ch):"""
assert old in s; s = s.replace(old, new)
old2 = """        for r in ex.map(xl, [c for c in (miss[i::NJ] for i in range(NJ)) if c]): xlate_errors(r)
    return wb"""
new2 = """        for r in ex.map(xl, [c for c in (miss[i::NJ] for i in range(NJ)) if c]): xlate_errors(r)
    if _JD and os.environ.get('JOBDUMP_RES'):
        import shutil as _sh
        for k in miss:
            for ext in ('.spv', '.res.json'):
                if os.path.exists(os.path.join(wb, k + ext)): _sh.copyfile(os.path.join(wb, k + ext), os.path.join(_JD, k, 'tr' + ext))
    return wb"""
assert old2 in s; s = s.replace(old2, new2)
open(p, 'w').write(s)
PY
echo "patched $f"
