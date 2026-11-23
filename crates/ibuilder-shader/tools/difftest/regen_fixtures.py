#!/usr/bin/env python3
"""regen_fixtures.py <engine dir> <fixture dir>: recompute the `expect` of every committed step fixture (tests/fixtures/<step>.<n>.{in,args.json,expect}) with the engine's own Python function,
in mkfixtures.py's format (first line the status, second the meta json, then the text or the message). The inputs are kept as they are; prints the fixtures whose expectation changed."""
import json, os, sys
eng, fdir = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], eng]
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stepdiff as sd
changed = total = 0
for n in sorted(os.listdir(fdir)):
    if not n.endswith('.in'): continue
    base = os.path.join(fdir, n[:-3]); step = n.split('.')[0]
    text = open(base + '.in', newline='').read(); args = json.load(open(base + '.args.json'))
    try: out, meta = sd.STEPS[step](text, args); kind = 'OK'
    except (ValueError, SystemExit) as e: kind = 'ERR'; out, meta = str(e if isinstance(e, ValueError) else e.code), None
    new = kind + '\n' + json.dumps(meta, sort_keys=True, separators=(',', ':'), ensure_ascii=False) + '\n' + out
    old = open(base + '.expect', newline='').read()
    total += 1
    if new != old:
        changed += 1; print('changed', n[:-3]); open(base + '.expect', 'w', newline='').write(new)
print(f'{changed} of {total} expectations changed', file=sys.stderr)
