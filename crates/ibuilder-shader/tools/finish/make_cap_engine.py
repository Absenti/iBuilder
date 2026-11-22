"""F2 oracle harness: instrument a COPY of the engine's dmsh_build.py (scratch folder only, never the original) so that every call of `finish(key, w, j, c1)` first keeps its INPUTS
(`<key>.spv`, `<key>.res.json`, `<key>.bin` of the working folder, the job dict as `job.json` before `finish` mutates it, c1 as `c1.hex`) and afterwards the Python's own
result (`result.json`: status, detail, the reflection dict as `finish` returns it, i.e. before `MTL.with_compiler` and before the json spelling).
usage: S2=$SCRATCH SRC=<engine copy dir> DST=<instrumented copy dir> python make_cap_engine.py        run later with DMSH_CAPF=<capture dir>"""
import os, shutil
src, dst = os.environ['SRC'], os.environ['DST']
shutil.rmtree(dst, ignore_errors=True)
shutil.copytree(src, dst, ignore=shutil.ignore_patterns('__pycache__'), symlinks=True)
p = dst + '/dm/gfx/tools/dmsh_build.py'
t = open(p).read()
INSTR = r'''
# ======================= F2 INSTRUMENTATION (scratch copy only) =======================
_CAPF = os.environ.get('DMSH_CAPF')
# DMSH_MUTATE=tf,discard (comma list): exercise what Grizzy never reaches, on real shaders: `tf` gives every vertex shader a transform feedback layout, `discard` puts a comment
# naming `access::write` after `main0_out out = {};` (a vertex shader with side effects gets main0_discard). The Rust harness (shader_finish --mutate) applies the same edits.
_MUT = set((os.environ.get('DMSH_MUTATE') or '').split(','))
MUT_TF = [{'stride': 16, 'outputs': [[0, 4]]}, [], [], []]
MUT_DISCARD = ('main0_out out = {};', 'main0_out out = {};\n    // access::write')   # (find, replace), the first occurrence
import dmsh_atomic as _da
_orig_check = _da.check_msl
import threading as _th2
_CUR = _th2.local()
def _check(src):
    return _orig_check(src.replace(*MUT_DISCARD, 1) if 'discard' in _MUT and getattr(_CUR, 'stage', None) == 'vs' else src)
_da.check_msl = _check
_orig_finish = finish
def finish(key, w, j, c1):
    _CUR.stage = j['stage']
    if 'tf' in _MUT and j['stage'] == 'vs': j['tf'] = MUT_TF
    if _CAPF:
        d = os.path.join(_CAPF, key); os.makedirs(d, exist_ok=True)
        for ext in ('spv', 'res.json', 'bin'):
            sp = os.path.join(w, key + '.' + ext)
            if os.path.exists(sp): shutil.copyfile(sp, os.path.join(d, key + '.' + ext))
        with open(os.path.join(d, 'job.json'), 'w') as f: json.dump(j, f)
        with open(os.path.join(d, 'c1.hex'), 'w') as f: f.write(c1.hex())
    r = _orig_finish(key, w, j, c1)
    if _CAPF:
        with open(os.path.join(d, 'result.json'), 'w') as f: json.dump({'status': r[0], 'detail': r[1], 'refl': r[2]}, f, default=repr)
    return r
'''
marker = 'keys = build_keys(want, loc, PAIRS)\n'
assert t.count(marker) == 1
open(p, 'w').write(t.replace(marker, INSTR + marker))
print('instrumented', p)
