import hashlib, json, os, sys
ROOT = sys.argv[1]; MODE = sys.argv[2] if len(sys.argv) > 2 else 'scenario'
def norm(v, base):
    if isinstance(v, str): return v.replace(base, '<R>')
    if isinstance(v, list): return [norm(x, base) for x in v]
    if isinstance(v, dict): return {k: norm(x, base) for k, x in v.items()}
    return v
def tree(base):
    out = {}
    for root, dirs, files in os.walk(base):
        for d in dirs: out[os.path.relpath(os.path.join(root, d), base) + '/'] = None
        for f in files:
            p = os.path.join(root, f)
            try: out[os.path.relpath(p, base)] = hashlib.sha256(open(p, 'rb').read()).hexdigest() if not os.path.islink(p) else 'link'
            except OSError: out[os.path.relpath(p, base)] = 'unreadable'
    return out
ops = json.load(open(os.path.join(ROOT, 'ops_py.json')))
py = json.load(open(os.path.join(ROOT, 'out_py.json'))); rs = json.load(open(os.path.join(ROOT, 'out_rs.json')))
bad = 0; excs = {}; total_steps = 0; logs = 0
for i, (a, b) in enumerate(zip(py, rs)):
    if MODE == 'scenario':
        base_py = os.path.dirname(ops[i]['docs'])
        base_rs = base_py.replace('/py/', '/rs/')
        a, b = norm(a, base_py), norm(b, base_rs)
        problems = []
        if a.get('logs') != b.get('logs'): problems.append(('logs', a.get('logs'), b.get('logs')))
        for k, (x, y) in enumerate(zip(a['steps'], b['steps'])):
            total_steps += 1
            if 'exc' in x: excs[x['exc']] = excs.get(x['exc'], 0) + 1
            if x != y: problems.append(('step %d %s' % (k, ops[i]['steps'][k]['k']), x, y))
        ta, tb = tree(base_py), tree(base_rs)
        if ta != tb: problems.append(('tree', {k: ta.get(k) for k in set(ta) ^ set(tb) | {k for k in ta if ta.get(k) != tb.get(k)}}, {k: tb.get(k) for k in set(ta) ^ set(tb) | {k for k in ta if ta.get(k) != tb.get(k)}}))
        logs += len(a.get('logs', []))
        if problems:
            bad += 1
            if bad <= 5:
                print('CASE', i, base_py)
                for p in problems[:4]: print('   ', str(p)[:700])
    else:
        total_steps += 1
        if 'exc' in a: excs[a['exc']] = excs.get(a['exc'], 0) + 1
        if a != b:
            bad += 1
            if bad <= 8: print('OP', i, json.dumps(ops[i])[:300], '\n   py', str(a)[:300], '\n   rs', str(b)[:300])
print('%s: %d cases, %d steps/ops, %d log lines, %d differing; python exceptions seen: %s' % (MODE, len(py), total_steps, logs, bad, excs))
sys.exit(1 if bad else 0)
