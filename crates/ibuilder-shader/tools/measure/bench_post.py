"""Times shader_post.post_rewrites (the Python chain) alone over a manifest: no file writes, one thread. -> per-stage seconds"""
import os, sys, json, time, collections
os.environ.setdefault('NXPOCKET_ENGINE', os.environ['S'] + '/shader/engine')
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import shader_post as sp
man = sys.argv[1]
jobs = {j['key']: j for j in json.load(open(man + '/jobs.json'))}
keys = [l.split()[0] for l in open(man + '/keys.txt') if l.strip()]
pairs = sp.load_pairs(man + '/profile')
data = []
for k in keys:
    data.append((k, jobs[k], json.load(open(f'{man}/{k}.res.json')), open(f'{man}/{k}.metal').read()))
best = None
for rep in range(int(sys.argv[2]) if len(sys.argv) > 2 else 3):
    t = collections.Counter(); n = collections.Counter(); cpu = time.process_time(); wall = time.perf_counter(); bytes_ = 0
    for k, j, res, src in data:
        t0 = time.process_time()
        try: sp.post_rewrites(src, j, res, k, man, pairs, set())
        except ValueError: pass
        t[j['stage']] += time.process_time() - t0; n[j['stage']] += 1; bytes_ += len(src)
    tot = time.process_time() - cpu
    if best is None or tot < best[0]: best = (tot, dict(t), dict(n), bytes_)
tot, t, n, b = best
print(json.dumps({'shaders': sum(n.values()), 'cpu_s': round(tot, 3), 'per_shader_ms': round(tot * 1e3 / sum(n.values()), 3), 'by_stage_s': {k: round(v, 3) for k, v in t.items()}, 'counts': n, 'MB_msl': round(b / 1e6, 1), 'MB_per_s': round(b / 1e6 / tot, 1)}))
