#!/usr/bin/env python3
"""mkfixtures.py <engine copy> <fixture dir> <corpus dir>...: the smallest changed / failing cases of the fuzz corpora as committed regression fixtures (tests/fixtures/<step>.<n>.{in,args.json,expect}),
with the Python's result (the engine's own function) as `expect`: first line the status, second the meta json, then the text (OK) or the message (ERR)."""
import json, os, sys
eng, outdir, *corpora = sys.argv[1:]
sys.argv = [sys.argv[0], eng]
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stepdiff as sd
os.makedirs(outdir, exist_ok=True)
picked = {}
for c in corpora:
    for l in open(f'{c}/cases.tsv').read().split('\n'):
        if not l: continue
        cid, step, path, args = l.split('\t', 3)
        if step == 'chain' or step not in sd.STEPS: continue
        picked.setdefault(step, []).append((os.path.getsize(path), cid, path, json.loads(args)))
n = 0
for step, rows in sorted(picked.items()):
    rows.sort(key=lambda r: r[0])
    got = {'changed': 0, 'same': 0, 'err': 0}
    for size, cid, path, args in rows:
        if size > 6000: break
        text = open(path, newline='').read()
        sd.signal.setitimer(sd.signal.ITIMER_REAL, 1)
        try: out, meta = sd.STEPS[step](text, args); kind = 'changed' if out != text else 'same'
        except (ValueError, SystemExit) as e: kind = 'err'; out, meta = str(e if isinstance(e, ValueError) else e.code), None
        except Exception: continue
        finally: sd.signal.setitimer(sd.signal.ITIMER_REAL, 0)
        if got[kind] >= (3 if kind == 'changed' else 1): continue
        if kind == 'same' and got['changed'] == 0: continue
        got[kind] += 1; n += 1
        k = sum(got.values())
        base = f'{outdir}/{step}.{k}'
        open(base + '.in', 'w', newline='').write(text)
        json.dump(args, open(base + '.args.json', 'w'))
        open(base + '.expect', 'w', newline='').write(('ERR' if kind == 'err' else 'OK') + '\n' + json.dumps(meta, sort_keys=True, separators=(',', ':'), ensure_ascii=False) + '\n' + out)
print(n, 'fixtures', file=sys.stderr)
