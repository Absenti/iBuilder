#!/usr/bin/env python3
"""mkmut.py <capture dir> <out dir> [--n N] [--seed S]: mutated copies of captured real shaders (1-3 line-structure-preserving mutations of fuzz.mutate each), with every step of their stage as a case."""
import json, os, random, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fuzz
cap, out = sys.argv[1], sys.argv[2]
a = sys.argv[3:]
n = int(a[a.index('--n') + 1]) if '--n' in a else 600
seed = int(a[a.index('--seed') + 1]) if '--seed' in a else 3
rng = random.Random(seed); fuzz.rng.seed(seed)
os.makedirs(out + '/msl', exist_ok=True)
ANY = ['check_msl', 'wide_atomics', 'precise', 'bit0_mask', 'select_guards', 'mul_extended', 'float_add_msl', 'gather_offsets', 'hmma_msl', 'shared_store_msl', 'pool_arrays', 'wide_loads', 'tg_fence', 'border_emu', 'uniform_tests', 'shared_body', 'raw_noinline', 'mesh_subgroup_noinline']
PS = ['per_vertex', 'frag_z_snap', 'blend_zero', 'fs_uint', 'fs_dual']
CS = ['first_store', 'tg_spill', 'compute_loops', 'max_threads']
VS = ['vs_init', 'vs_layer', 'vs_base_instance', 'drop_point_size', 'vs_depth', 'vs_discard', 'vs_layer_entry', 'capture', 'vs_tf']
keys = sorted(k for k in os.listdir(cap) if os.path.isfile(f'{cap}/{k}/raw.metal'))
by = {}
for k in keys: by.setdefault(json.load(open(f'{cap}/{k}/job.json'))['stage'], []).append(k)
cases = []; cid = 0
for st, ks in by.items():
    if st not in ('ps', 'vs', 'cs'): continue
    for k in rng.sample(ks, min(len(ks), n if st == 'ps' else n // 2)):
        j = json.load(open(f'{cap}/{k}/job.json'))
        text = open(f'{cap}/{k}/raw.metal').read()
        for rep in range(2):
            t = fuzz.mutate(text, 3)
            p = f'{os.path.abspath(out)}/msl/{cid}.msl'; open(p, 'w', newline='').write(t)
            args = {'local': j.get('local'), 'tf': j.get('tf') or [{'stride': 16, 'outputs': [[0, 4]]}, [], [], []]}
            for s in ANY + {'ps': PS, 'cs': CS, 'vs': VS}[st]:
                cases.append(f'{cid}.{s}\t{s}\t{p}\t{json.dumps(args if s in ("first_store", "max_threads", "vs_tf") else {})}')
            a = {'stage': st, 'local': j.get('local'), 'tf': j.get('tf') or ([{'stride': 16, 'outputs': [[0, 4]]}, [], [], []] if rng.random() < 0.2 else None), 'res': os.path.abspath(f'{cap}/{k}/res.json'),
                 'dual_want': rng.random() < 0.3, 'partner': (rng.sample(range(0, 8), rng.randint(0, 5)) if rng.random() < 0.6 else None)}
            cases.append(f'{cid}.chain\tchain\t{p}\t{json.dumps(a)}')
            cid += 1
open(out + '/cases.tsv', 'w').write('\n'.join(cases) + '\n')
print(len(cases), 'cases over', cid, 'mutated shaders', file=sys.stderr)
