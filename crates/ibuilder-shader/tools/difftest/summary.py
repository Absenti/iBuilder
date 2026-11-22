#!/usr/bin/env python3
"""summary.py <corpus dir>...: one table over all corpora (each with cases.tsv and res/{py,rs}.tsv): per step the cases, how many are identical (status, same-flag, length, crc32, meta),
how many different, how many the step changed / how many the Python failed on."""
import collections, sys
rows = collections.defaultdict(collections.Counter)
for c in sys.argv[1:]:
    step = {}
    for l in open(f'{c}/cases.tsv', encoding='utf-8'):
        p = l.rstrip('\n').split('\t', 3)
        if len(p) >= 3: step[p[0]] = p[1]
    def load(f):
        d = {}
        for l in open(f, encoding='utf-8'):
            x = l.rstrip('\n').split('\t')
            if x and x[0]: d[x[0]] = x
        return d
    py, rs = load(f'{c}/res/py.tsv'), load(f'{c}/res/rs.tsv')
    for cid, a in py.items():
        s = step.get(cid, '?'); b = rs.get(cid); r = rows[s]
        r['cases'] += 1
        if b is not None and a[:6] == b[:6]: r['identical'] += 1
        else: r['different'] += 1
        if a[1] == 'ERR': r['py failed'] += 1
        elif a[2] == '0': r['changed'] += 1
tot = collections.Counter()
print('| step | cases | identical | different | text changed by the step | Python raised |\n|---|---|---|---|---|---|')
for s in sorted(rows):
    r = rows[s]; tot.update(r)
    print(f"| {s} | {r['cases']:,} | {r['identical']:,} | {r['different']} | {r['changed']:,} | {r['py failed']:,} |")
print(f"| **all** | {tot['cases']:,} | {tot['identical']:,} | {tot['different']} | {tot['changed']:,} | {tot['py failed']:,} |")
