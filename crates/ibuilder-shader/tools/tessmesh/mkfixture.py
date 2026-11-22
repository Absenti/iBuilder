"""mkfixture.py <corpus dir (with cases.tsv and res/py.tsv)> <out json> [--per-step N] [--budget BYTES]: a committed subset of a differential corpus (tests/data/tessmesh_cases.json).
Per step: the distinct error messages (up to half of N), then the Python's successes spread over the corpus, up to N cases; inputs (the MSL, the SPIR-V as hex, the second MSL of a mesh composition) are embedded, and the
Python's result line (`status same len crc32 meta`) is kept for the Rust test to compare. usage as in docs/shaders/SHADER-TESS-MESH-RUST.md."""
import collections, json, os, sys
corp, out = sys.argv[1], sys.argv[2]
per = int(sys.argv[sys.argv.index('--per-step') + 1]) if '--per-step' in sys.argv else 80
budget = int(sys.argv[sys.argv.index('--budget') + 1]) if '--budget' in sys.argv else 4_500_000
rows = {}
for l in open(corp + '/cases.tsv', encoding='utf-8'):
    p = l.rstrip('\n').split('\t', 3)
    if len(p) >= 3: rows[p[0]] = (p[1], p[2], json.loads(p[3]) if len(p) > 3 and p[3] else None)
py = {}
for l in open(corp + '/res/py.tsv', encoding='utf-8'):
    c = l.rstrip('\n').split('\t')
    if c and c[0]: py[c[0]] = c
by = collections.defaultdict(list)
for cid, (step, path, args) in rows.items():
    if cid in py: by[step].append(cid)
pick = []
for step, ids in sorted(by.items()):
    seen, chosen = set(), []
    for cid in ids:   # distinct error messages first (up to half of the quota), then the Python's successes spread over the corpus, then whatever is left
        c = py[cid]
        if c[1] != 'ERR': continue
        k = c[6][:60]
        if k not in seen and len(chosen) < per // 2: seen.add(k); chosen.append(cid)
    oks = [cid for cid in ids if py[cid][1] != 'ERR']
    stride = max(1, len(oks) // max(1, per - len(chosen)))
    for cid in oks[::stride]:
        if len(chosen) < per: chosen.append(cid)
    for cid in ids:
        if len(chosen) >= per: break
        if cid not in chosen: chosen.append(cid)
    pick += chosen[:per]
cases, size = [], 0
for cid in pick:
    step, path, args = rows[cid]
    text = open(path, encoding='utf-8', newline='').read()
    a = dict(args) if args else {}
    if 'spv' in a: a['spv'] = open(a['spv'], 'rb').read().hex()
    if 'gs' in a: a['gs'] = open(a['gs'], encoding='utf-8', newline='').read()
    c = py[cid]
    cases.append({'step': step, 'in': text, 'args': a, 'py': '\t'.join(c[1:6]), 'msg': c[6]})
    size += len(text) + len(json.dumps(a))
    if size > budget: break
json.dump(cases, open(out, 'w', encoding='utf-8'), ensure_ascii=False, separators=(',', ':'))
print(len(cases), 'cases', os.path.getsize(out), 'bytes', dict(collections.Counter(c['step'] for c in cases)), file=sys.stderr)
