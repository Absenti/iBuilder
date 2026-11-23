"""Mac harness for the phone's stage-3 build (shader_device.build): same driver, same engine Python, tools as callables over the Mac binaries (timed).
usage: run_build.py --engine DIR --keys FILE|all --jobs N --out DIR [--tables 0|1] [--cprofile] [--cas DIR]"""
import argparse, json, os, subprocess as real_subprocess, sys, threading, time, resource
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
S = os.environ['S'] + '/shader'
ap = argparse.ArgumentParser()
ap.add_argument('--engine', default=S + '/engine'); ap.add_argument('--keys', default='all'); ap.add_argument('--jobs', type=int, default=4)
ap.add_argument('--out', required=True); ap.add_argument('--tables', default='0'); ap.add_argument('--cprofile', action='store_true')
ap.add_argument('--cas', default=None); ap.add_argument('--chain', default='6c53446391cc'); ap.add_argument('--tag', default='run')
a = ap.parse_args()
import shader_device as sd
tl = os.path.join(a.engine, 'dm/gfx/tools')
XB, SB = os.path.join(tl, 'dmsh_xlate_bin/dmsh_xlate'), os.path.join(tl, 'spirv_cross_bin/spirv-cross')
ST = {'xlate': [0, 0, 0, 0, 0, 0], 'spirv_cross': [0, 0, 0, 0, 0, 0]}; STL = threading.Lock(); CALLS = []   # calls, wall ns, user s, sys s, max maxrss, jobs
import tempfile
def run(name, b, args):
    njobs = 0
    if name == 'xlate':
        try: njobs = len(json.load(open(args[0])))
        except Exception: pass
    fo, fe = tempfile.TemporaryFile(), tempfile.TemporaryFile()
    t0 = time.perf_counter_ns()
    p = real_subprocess.Popen([b, *args], stdout=fo, stderr=fe)
    _, status, ru = os.wait4(p.pid, 0)
    dt = time.perf_counter_ns() - t0
    p.returncode = os.waitstatus_to_exitcode(status)
    fo.seek(0); fe.seek(0); out, err = fo.read().decode(errors='replace'), fe.read().decode(errors='replace'); fo.close(); fe.close()
    with STL:
        s = ST[name]; s[0] += 1; s[1] += dt; s[2] += ru.ru_utime; s[3] += ru.ru_stime; s[4] = max(s[4], ru.ru_maxrss); s[5] += njobs
        CALLS.append((name, dt / 1e9, ru.ru_utime + ru.ru_stime, ru.ru_maxrss, njobs))
    return p.returncode, out, err
tools = {'xlate': lambda x: run('xlate', XB, x), 'spirv_cross': lambda x: run('spirv_cross', SB, x)}
# time the c1_guard / cb_extent scripts and the whole build
SC = {}
_orig = sd._FakeSubprocess._script
def _script(self, argv):
    t0 = time.perf_counter(); r = _orig(self, argv); SC[os.path.basename(argv[0])] = time.perf_counter() - t0; return r
sd._FakeSubprocess._script = _script
keys = None if a.keys == 'all' else [l.strip() for l in open(a.keys) if l.strip()]
env = {}
if a.cprofile: os.environ['DMSH_CPROFILE'] = '1'
os.environ['DMSH_IT_OUT'] = os.path.join(a.out, 'it.json'); os.makedirs(a.out, exist_ok=True)
logs = []
def log(l):
    logs.append(l)
    if '/' in l and 'shaders' in l and l.split(':')[0].strip() in ('compiling', 'cache check') and int(l.split(':')[1].split('/')[0]) % 1000 == 0: print(time.strftime('%H:%M:%S'), l, flush=True)
t0 = time.perf_counter(); c0 = time.process_time()
img = sd.build(S + '/split-out/pkg', S + '/profile', '0100DFA027504000', a.out, a.chain, tools, log=log, keys=keys, jobs=a.jobs, engine=a.engine,
               keep_work=False, pulling=False, tables=(a.tables == '1'), cas=a.cas)
wall = time.perf_counter() - t0; cpu = time.process_time() - c0
ru = resource.getrusage(resource.RUSAGE_CHILDREN)
res = {'tag': a.tag, 'wall_s': wall, 'cpu_s_python_process': cpu, 'tools': {k: {'calls': v[0], 'wall_s_sum': v[1] / 1e9, 'user_s': v[2], 'sys_s': v[3], 'max_rss_bytes': v[4], 'jobs': v[5]} for k, v in ST.items()},
       'scripts_s': SC, 'children_user_s': ru.ru_utime, 'children_sys_s': ru.ru_stime, 'children_max_rss_bytes': ru.ru_maxrss, 'image': img, 'keys': None if keys is None else len(keys), 'jobs': a.jobs,
       'tail_log': [l for l in logs if not l.startswith(('compiling', 'cache check'))][-40:]}
json.dump(res, open(os.path.join(a.out, 'harness.json'), 'w'), indent=1); json.dump(CALLS, open(os.path.join(a.out, 'calls.json'), 'w'))
print(json.dumps({k: v for k, v in res.items() if k != 'tail_log'}, indent=1))
