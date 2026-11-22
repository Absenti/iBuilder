#!/usr/bin/env python3
"""mkreal.py <capture dir> <out dir> [--limit N]: cases.tsv over the captured real shaders (<key>/{raw.metal, job.json}): every step on every shader of a stage it belongs to, and a sample
of the other stages (a wrong-stage text is a legitimate input of the function: both sides must agree on what happens to it)."""
import json, os, random, sys
cap, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)
ANY = ['check_msl', 'wide_atomics', 'precise', 'membar_fence', 'bit0_mask', 'select_guards', 'mul_extended', 'float_add_msl', 'gather_offsets', 'hmma_msl', 'shared_store_msl',
       'pool_arrays', 'wide_loads', 'tg_fence', 'border_emu', 'uniform_tests', 'shared_body']
PS = ['per_vertex', 'frag_z_snap', 'blend_zero', 'fs_uint', 'fs_dual']
CS = ['first_store', 'tg_spill', 'max_threads']
VS = ['vs_init', 'vs_layer', 'vs_base_instance', 'drop_point_size', 'vs_depth', 'vs_discard', 'vs_layer_entry', 'capture', 'vs_tf']
rng = random.Random(7)
keys = sorted(os.listdir(cap))
lines = []; n = 0
for k in keys:
    p = f'{cap}/{k}/raw.metal'
    if not os.path.isfile(p): continue
    j = json.load(open(f'{cap}/{k}/job.json')); st = j['stage']
    args = {'local': j.get('local'), 'tf': j.get('tf')}
    steps = list(ANY) + {'ps': PS, 'cs': CS, 'vs': VS}.get(st, [])
    others = [s for s in PS + CS + VS if s not in steps]
    steps += rng.sample(others, 3)   # a few steps of other stages on every text
    for s in steps:
        a = args if s in ('first_store', 'max_threads', 'vs_tf') else {}
        if s == 'vs_tf' and not j.get('tf'): a = {'tf': [{'stride': 16, 'outputs': [[0, 4]]}, {}, {}, {}]}
        lines.append(f'{n}\t{s}\t{os.path.abspath(p)}\t{json.dumps(a)}'); n += 1
# the whole chain on every real shader: its own job fields, its own answer json, random pair sets for the vertex shaders
for k in keys:
    p = f'{cap}/{k}/raw.metal'
    if not os.path.isfile(p): continue
    j = json.load(open(f'{cap}/{k}/job.json'))
    if j['stage'] not in ('ps', 'vs', 'cs'): continue
    a = {'stage': j['stage'], 'local': j.get('local'), 'tf': j.get('tf'), 'res': os.path.abspath(f'{cap}/{k}/res.json'), 'dual_want': rng.random() < 0.3, 'partner': (rng.sample(range(0, 8), rng.randint(0, 5)) if rng.random() < 0.6 else None)}
    lines.append(f'{n}\tchain\t{os.path.abspath(p)}\t{json.dumps(a)}'); n += 1
open(f'{out}/cases.tsv', 'w').write('\n'.join(lines) + '\n')
print(n, 'cases over', len(keys), 'shaders', file=sys.stderr)
