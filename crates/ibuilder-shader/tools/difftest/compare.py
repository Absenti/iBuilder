#!/usr/bin/env python3
"""compare.py py.tsv rs.tsv cases.tsv: per step, how many cases are identical (status, same-flag, length, crc32, meta) and the first mismatches. Messages of two ERR results are compared too
and reported on their own (the build prints them; the library does not depend on them). Exit 1 on any mismatch."""
import sys, collections
def load(p):
    d = {}
    for l in open(p, encoding='utf-8'):
        c = l.rstrip('\n').split('\t')
        if c and c[0]: d[c[0]] = c
    return d
py, rs = load(sys.argv[1]), load(sys.argv[2])
step = {}
for l in open(sys.argv[3], encoding='utf-8'):
    p = l.rstrip('\n').split('\t', 3)
    if len(p) >= 3: step[p[0]] = (p[1], p[2])
stat = collections.defaultdict(lambda: collections.Counter()); bad = collections.defaultdict(list)
for cid, a in py.items():
    s = step.get(cid, ('?', '?'))[0]
    b = rs.get(cid)
    if b is None: stat[s]['missing in rust'] += 1; bad[s].append(cid); continue
    stat[s]['cases'] += 1
    stat[s]['py ' + a[1]] += 1
    if a[1] == 'ERR': stat[s]['py err'] += 0
    if a[:6] == b[:6]:
        stat[s]['identical'] += 1
        if a[1] == 'ERR':
            if a[6] == b[6]: stat[s]['err message identical'] += 1
        elif a[2] == '0': stat[s]['changed text'] += 1
    else:
        stat[s]['DIFFERENT'] += 1; bad[s].append(cid)
rc = 0
for s in sorted(stat):
    c = stat[s]; print(f"{s:18} cases {c['cases']:6}  identical {c['identical']:6}  different {c['DIFFERENT']:4}  (py ok {c['py OK']}, py err {c['py ERR']}; texts changed by the step {c['changed text']}; err messages identical {c['err message identical']})")
    if c['DIFFERENT'] or c['missing in rust']: rc = 1
    for cid in bad[s][:5]:
        print('    MISMATCH', cid, step.get(cid, ('?', '?'))[1], '\n       py', py[cid][1:6], '\n       rs', rs.get(cid, ['-'])[1:6], '\n       py msg', py[cid][6][:100], '\n       rs msg', (rs.get(cid, ['-'] * 7)[6])[:100])
sys.exit(rc)
