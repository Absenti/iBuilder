"""The same question as mutate.py, answered from the other side and without rebuilding anything: one edit of the ENGINE's Python (in a private copy of the engine, never the original) at a time, the Python half re-run
over the corpus's cases of the steps the edited function belongs to, and the cases counted whose result no longer equals the Rust half's stored result (res/rs.tsv, from the unmodified Rust).
usage: python3 mutate_py.py <engine copy to edit (a throw-away copy)> <corpus dir with res/rs.tsv> <python with the stepdiff.py dependencies>"""
import os, shutil, subprocess, sys
eng, corp, py = sys.argv[1], sys.argv[2], sys.argv[3]
T = eng + '/dm/gfx/tools/'
STEPDIFF = os.path.join(os.path.dirname(os.path.abspath(__file__)), '../difftest/stepdiff.py')
MUTS = [   # (name, file, old, new, steps)
    ('tess hull: in_bytes += 16 -> 15', 'dmsh_tess.py', 'in_bytes += 16', 'in_bytes += 15', ['tess_hull']),
    ('tess _location: location < 32 -> <= 32', 'dmsh_tess.py', '0 <= v[\'location\'] < 32', '0 <= v[\'location\'] <= 32', ['tess_hull', 'tess_domain']),
    ('tess mode_bits: winding bit 3 -> 2', 'dmsh_tess.py', '(mode[\'winding\'] == \'cw\') << 3', '(mode[\'winding\'] == \'cw\') << 2', ['tess_domain']),
    ('tess domain: patch input location test patch flag', 'dmsh_tess.py', "patch_loads.append((mem, ty, _location(io, mem, 'in', True)))", "patch_loads.append((mem, ty, _location(io, mem, 'in', False)))", ['tess_domain']),
    ('tess control_mode: spacing 0x8E7B -> 0x8E7D', 'dmsh_tess.py', '0x8E7B: \'fractional_odd\'', '0x8E7D: \'fractional_odd\'', ['tess_mode']),
    ('meshstage memory: payload rounding + 15 -> + 16', 'dmsh_meshstage.py', "out = INPUT + (i['payload'] + 15 & ~15)", "out = INPUT + (i['payload'] + 16 & ~15)", ['mesh_info', 'mesh_to_stage']),
    ('meshstage to_stage: spill at SPILL_MIN <= -> <', 'dmsh_meshstage.py', 'SPILL_MIN <= words * 4 <= abi.MESH_SCRATCH_STRIDE', 'SPILL_MIN < words * 4 <= abi.MESH_SCRATCH_STRIDE', ['mesh_to_stage']),
    ('meshstage _width: component 3 mask & 8 -> & 4', 'dmsh_meshstage.py', 'read) & 8 and written[3] is None', 'read) & 4 and written[3] is None', ['mesh_to_stage']),
    ('meshstage own_shared: opcode 0x388 dropped', 'dmsh_meshstage.py', '(0x984, 0x388, 0x988', '(0x984, 0x988', ['mesh_info']),
    ('meshstage pixel_reads: 2 * c -> c', 'dmsh_meshstage.py', 'sph[0x18 + n] >> (2 * c) & 3', 'sph[0x18 + n] >> c & 3', ['mesh_pixel_reads']),
    ('meshstage to_stage: the point topology name', 'dmsh_meshstage.py', "TOPOLOGY = {1: 'point', 2: 'line', 3: 'triangle'}", "TOPOLOGY = {1: 'points', 2: 'line', 3: 'triangle'}", ['mesh_to_stage']),
    ('mesh prims_for: 4 * p -> 5 * p (3 * p would be an equivalent mutant: no vertex size lies between the two limits)', 'dmsh_mesh.py', '3 * p * vbytes + 4 * p <= MESH_BYTES', '3 * p * vbytes + 5 * p <= MESH_BYTES', ['mesh_prims_for', 'mesh_compose']),
    ('mesh compose: run slots < RUN_SLOTS -> <=', 'dmsh_mesh.py', 'int(m[3]) < RUN_SLOTS]', 'int(m[3]) <= RUN_SLOTS]', ['mesh_compose']),
    ('mesh out_bytes: half is 2 bytes -> 4', 'dmsh_mesh.py', "'half': 2", "'half': 4", ['mesh_out_bytes', 'mesh_compose']),
    ('lean: MASK_WORD 5 -> 6', 'dmsh_geometry.py', 'MASK_WORD = 5', 'MASK_WORD = 6', ['geo_lean', 'mesh_compose']),
    ('lean: dead-code use count -= 1 -> -= 2', 'dmsh_geometry.py', 'uses[t] -= 1', 'uses[t] -= 2', ['geo_lean']),
    ('lean: record base used elsewhere test > -> >=', 'dmsh_geometry.py', "'\\b' + b + r'\\b', text)) - 1 > sum(", "'\\b' + b + r'\\b', text)) - 1 >= sum(", ['geo_lean']),
    ('lean: renumbered stride * 144 -> * 145', 'dmsh_geometry.py', "f'* {max(len(reads), 1)};'", "f'* {max(len(reads), 1) + 1};'", ['geo_lean']),
]
rs = {}
for l in open(corp + '/res/rs.tsv', encoding='utf-8'):
    c = l.rstrip('\n').split('\t')
    if c and c[0]: rs[c[0]] = c
rows = {}
for l in open(corp + '/cases.tsv', encoding='utf-8'):
    p = l.rstrip('\n').split('\t', 3)
    if len(p) >= 3: rows[p[0]] = p[1]
out = []
for name, f, old, new, steps in MUTS:
    path = T + f
    text = open(path).read()
    if text.count(old) != 1: print(f'SKIP {name}: the text occurs {text.count(old)} times'); continue
    sub = corp + '/res/mutpy_cases.tsv'
    with open(sub, 'w') as g:
        for l in open(corp + '/cases.tsv', encoding='utf-8'):
            if l.split('\t', 3)[1] in steps: g.write(l)
    shutil.copy(path, path + '.orig')
    try:
        open(path, 'w').write(text.replace(old, new))
        r = subprocess.run([py, '-I', STEPDIFF, eng, sub, corp + '/res/mutpy.tsv', '--jobs', '2'], capture_output=True, text=True, env={**os.environ, 'PYTHONDONTWRITEBYTECODE': '1', 'STEPDIFF_TIMEOUT': '30'})
    finally:
        shutil.move(path + '.orig', path)
    if r.returncode: print('stepdiff failed', r.stderr[-300:]); continue
    n = d = 0
    for l in open(corp + '/res/mutpy.tsv', encoding='utf-8'):
        c = l.rstrip('\n').split('\t')
        n += 1
        if c[:6] != rs[c[0]][:6]: d += 1
    print(f'{name:70} {n:6} cases, {d:5} differ', flush=True)
