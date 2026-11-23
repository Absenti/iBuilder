"""The Python half of the end-to-end test: the phone's driver (shader_device.build) over a synthetic game, with the translator callable running fake_xlate.py.
usage: TM_FAKE=<dir> TM_REAL_XLATE=<dmsh_xlate> run_py_build.py --driver <dir with shader_device.py> --engine <engine copy> --pkg <dir> --profile <dir> --out <dir> [--jobs N] [--pulling 0|1] [--chain C]"""
import argparse, os, subprocess, sys
ap = argparse.ArgumentParser()
for a in ('--driver', '--engine', '--pkg', '--profile', '--out'): ap.add_argument(a, required=True)
ap.add_argument('--game', default='0100DFA027504000'); ap.add_argument('--chain', default='6c53446391cc'); ap.add_argument('--jobs', type=int, default=2); ap.add_argument('--pulling', default='0')
a = ap.parse_args()
sys.path.insert(0, a.driver)
import shader_device as sd
HERE = os.path.dirname(os.path.abspath(__file__))
SC = os.path.join(a.engine, 'dm/gfx/tools/spirv_cross_bin/spirv-cross')
def run(cmd):
    r = subprocess.run(cmd, capture_output=True, text=True); return r.returncode, r.stdout, r.stderr
tools = {'xlate': lambda x: run([sys.executable, '-I', os.path.join(HERE, 'fake_xlate.py'), *x]), 'spirv_cross': lambda x: run([SC, *x])}
logs = []
img = sd.build(a.pkg, a.profile, a.game, a.out, a.chain, tools, log=lambda l: (logs.append(l), print(l, flush=True)), keys=None, jobs=a.jobs, engine=a.engine, keep_work=False,
               pulling=(a.pulling == '1'), tables=False, cas=None)
print('image', img)
