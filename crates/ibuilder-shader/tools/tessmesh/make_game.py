"""A synthetic game for the end-to-end differential test of the round-3 shader kinds (docs/shaders/SHADER-TESS-MESH-RUST.md section 3.3): a split package folder (`pkg/`), a profile folder
(`profile/<id>/`) and the translator answers of the shaders that are not translated by the real translator (`fake/`).

    PYTHONDONTWRITEBYTECODE=1 python -I make_game.py <engine copy> <engine tests copy> <out dir> [--seed N]

Where the shaders come from
* REAL translator output: the engine's own hand-assembled test programs (`test_dmsh_hull_cbe.py` hull, `test_dmsh_geometry.py` passthrough geometry shader, `test_dmsh_ampere_mem.py` stage-memory
  kernels with their encodings) wrapped in split records; `dmsh_xlate` (the real binary) translates them, whichever of the two builds runs.
* `fake/`: small GLSL programs (hull / domain / vertex / pixel stages) compiled by `glslangValidator`, stripped of names (`spirv-opt --strip-debug`: SPIRV-Cross then names variables `m_<id>`
  as for the translator's own SPIR-V) with a minimal answer json; `fake_xlate.py` hands them to the build in place of the translator.
No game data is used: the records are made here from the engine's tests, the control sections from the facts `job_for` reads.
Writes `<out>/manifest.json` (key -> what it is)."""
import hashlib, json, os, random, re, shutil, struct, subprocess, sys, tempfile

ENGINE, TESTS, OUT = (os.path.abspath(a) for a in sys.argv[1:4])
SEED = int(sys.argv[sys.argv.index('--seed') + 1]) if '--seed' in sys.argv else 7
rnd = random.Random(SEED)
sys.path.insert(0, os.path.join(ENGINE, 'dm/gfx/tools')); sys.path.insert(0, TESTS)
import test_dmsh_hull_cbe as HULL   # noqa: E402
import test_dmsh_ampere_mem as AMP   # noqa: E402
GAME_ID = '0100DFA027504000'
GAME_TSV = os.environ.get('GAME_TSV')   # a game.tsv to copy (the profile's identity file: no game data)
for d in ('pkg', f'profile/{GAME_ID}', 'fake'): os.makedirs(os.path.join(OUT, d), exist_ok=True)
SHADERS = []   # dict(name, stage, key, rec, fake=(spv, res)|None, facts)
XC = ['--msl', '--msl-version', '20300', '--msl-argument-buffers', '--msl-texture-buffer-native', '--msl-pad-fragment-output']


def key_of(name): return hashlib.sha256(name.encode()).hexdigest()[:16]


def record(key, code, ctl):
    rec = bytearray(b'BTCDBTCD' + bytes.fromhex(key) + bytes(0x28)); struct.pack_into('<2I', rec, 0x28, len(ctl), len(code)); rec += code + ctl
    assert rec.count(b'BTCDBTCD') == 1 and rec.find(bytes.fromhex('34127698'), 0x38) == 0x38 + len(code)
    return bytes(rec)


def ctl1(tf=None, tess=None, early=False):
    """an NVN1 control section: the fields job_for reads (c1 range +0x700 / +0x704, transform feedback +0x6dc.., tessellation mode +0x738..)"""
    c = bytearray(0x880); c[:4] = bytes.fromhex('34127698'); struct.pack_into('<I', c, 4, 1)
    if tess: struct.pack_into('<2I', c, 0x738, tess[0], tess[1]); struct.pack_into('<I', c, 0x744, tess[2])
    if early: c[0x718] = 1
    if tf:   # [(stride, [(location, components)])] per stream
        for b, s in enumerate(tf):
            if not s: continue
            stride, outs = s
            struct.pack_into('<H', c, 0x6dc + 2 * b, stride); c[0x6e8 + b] = len(outs)
            for i, (loc, n) in enumerate(outs): c[0xdc + 0x80 * b + i] = 0x20 + loc; c[0x4dc + 0x80 * b + i] = n
    return bytes(c)


def ctl2(inst_len, stage_byte=0, threads=32, payload=0, task=0, verts=3, max_v=0, max_p=0, prim_at=0, early=False):
    c = bytearray(0x100); c[:4] = bytes.fromhex('34127698'); struct.pack_into('<I', c, 4, 0xc9)
    struct.pack_into('<I', c, 0x38, inst_len); struct.pack_into('<II', c, 0x40, inst_len, inst_len)
    c[0x56] = stage_byte; struct.pack_into('<2H', c, 0x58, threads, payload); c[0x5e] = task; c[0x5f] = verts; struct.pack_into('<2H', c, 0x60, max_v, max_p)
    if prim_at: struct.pack_into('<I', c, 0x64, prim_at); c[0x68] = 0x80
    if early: c[0x5b] = 1
    return bytes(c)


def add(name, stage, rec_fn, fake=None, facts=None, note=''):
    key = key_of(name)
    SHADERS.append(dict(name=name, stage=stage, key=key, rec=rec_fn(key), fake=fake, facts=facts, note=note))
    return key


def run(cmd, **kw): return subprocess.run(cmd, capture_output=True, text=True, **kw)


def glsl_spv(src, stage):
    with tempfile.TemporaryDirectory() as d:
        open(f'{d}/s.{stage}', 'w').write(src)
        r = run(['glslangValidator', '-V', f'{d}/s.{stage}', '-o', f'{d}/s.spv']); assert r.returncode == 0, r.stdout
        r = run(['spirv-opt', '--strip-debug', f'{d}/s.spv', '-o', f'{d}/t.spv']); assert r.returncode == 0, r.stderr
        return open(f'{d}/t.spv', 'rb').read()


RES = {'cbufs': [], 'sbufs': [], 'textures': [], 'images': [], 'notes': []}
NV1_CODE = bytes.fromhex('78563412') + bytes(0x2c)   # a Maxwell code area's 0x30-byte header


# ---------------------------------------------------------------------------------------------------------------- real translator: hull, passthrough geometry, stage-memory kernels
add('hull_cbe', 'hs', lambda k: record(k, NV1_CODE + HULL.hull(), ctl1()), note='test_dmsh_hull_cbe.hull() through the real translator')
add('hull_cbe_term', 'hs', lambda k: record(k, NV1_CODE + HULL.hull(True), ctl1()), note='its variant with an address that is not base + lane')


def gs_code(topology=7, vertices=1, invocations=1, passthrough=True, exit_only=True):
    table = open(os.environ['IOSBUILDER'] + '/engine/dm/third_party/ryujinx/src/Ryujinx.Graphics.Shader/Decoders/InstTable.cs').read()   # the original iOSBuilder checkout (read only)
    pat = re.search(r'Add\("([01x]{64})", InstName\.Exit\s*,', table)[1]
    exit_i = int(pat.replace('x', '0'), 2) | (7 << 16) | 15
    header = bytearray(0x50)
    struct.pack_into('<5I', header, 0, (int(passthrough) << 24) | (4 << 10) | (3 << 5) | 1, 0, invocations << 24, topology << 24, vertices)
    return bytes(header) + struct.pack('<QQ', 0, exit_i)


add('gs_pass', 'gs', lambda k: record(k, NV1_CODE + gs_code(), ctl1()), note='test_dmsh_geometry passthrough geometry shader (real translator)')
add('gs_pass_tf', 'gs', lambda k: record(k, NV1_CODE + gs_code(), ctl1(tf=[(16, [(0, 4)]), None, None, None])), note='the same with a transform feedback layout (geometry reader gets vs_tf)')
add('gs_pass2', 'gs', lambda k: record(k, NV1_CODE + gs_code(), ctl1(tf=[(32, [(1, 4), (3, 4)]), (16, [(2, 4)]), None, None])), note='two feedback streams')
add('gs_plain', 'gs', lambda k: record(k, NV1_CODE + gs_code(6, 3, 1, passthrough=False), ctl1()), note='not passthrough: no lean kernel')


def sph(pos=0xf0, generics=b'', sys0=0, imap=None):
    s = bytearray(0x80); s[0x34] = sys0; s[0x35] = pos; s[0x36:0x36 + len(generics)] = generics
    for n, m in (imap or {}).items(): s[0x18 + n] = m
    return bytes(s)


def inst(c): return AMP.inst(*c)


def ampere_stage(name, stage, threads, payload, code, max_v=0, max_p=0, task=1, verts=3, prim_sph=None, vertex_sph=None):
    insts = b''.join(inst(c) for c in code) + inst(AMP.EXIT)
    first = vertex_sph or sph(0xf0, b'\x30')
    body = first + insts
    prim_at = 0
    if prim_sph is not None:
        body += bytes(-len(body) % 16); prim_at = len(body); body += prim_sph
    area = body
    stage_byte = 6 if stage == 'ts' else 7
    return add(name, stage, lambda k: record(k, area, ctl2(len(first) + len(insts), stage_byte, threads, payload, task, verts, max_v, max_p, prim_at)), note='stage-memory kernel of test_dmsh_ampere_mem (real translator)')


MS = AMP.StageMemory
stage_shader = ampere_stage


stage_shader('ts_payload', 'ts', threads=32, payload=256, code=[MS.PAYLOAD])
stage_shader('ts_wg', 'ts', threads=64, payload=1408, code=[MS.WORKGROUP, MS.KEEP_R0])
stage_shader('ms_task', 'ms', threads=32, payload=128, max_v=128, max_p=64, task=1, code=[MS.PAYLOAD], prim_sph=sph(0, b'\x01'))
stage_shader('ms_notask', 'ms', threads=32, payload=0, max_v=64, max_p=126, task=0, code=[MS.INPUT, MS.KEEP_R19], vertex_sph=sph(0xf0, b'\x30'))
stage_shader('ms_point', 'ms', threads=32, payload=0, max_v=32, max_p=32, task=0, verts=1, code=[MS.ATTRIBUTE], vertex_sph=sph(0xf8))
stage_shader('ms_big', 'ms', threads=32, payload=64, max_v=128, max_p=126, task=0, verts=3, code=[MS.READBACK, MS.KEEP_R8, MS.COUNT], vertex_sph=sph(0xf0, bytes([0x33] * 8)), prim_sph=sph(0, b'\x0f', sys0=0), )
stage_shader('ms_prim_id', 'ms', threads=32, payload=0, max_v=128, max_p=126, task=0, code=[MS.INPUT, MS.KEEP_R19], vertex_sph=sph(0xf0), prim_sph=sph(0, b'', sys0=1))
stage_shader('ms_refused', 'ms', threads=32, payload=0, max_v=32, max_p=32, task=0, code=[(0x000fe20000000000, 0x0000000000007984)], vertex_sph=sph(0xf0))   # LDS: shared memory of its own
stage_shader('ms_toobig', 'ms', threads=32, payload=0, max_v=300, max_p=64, task=0, code=[MS.INPUT, MS.KEEP_R19], vertex_sph=sph(0xf0))   # exceeds a Metal mesh: to_stage refuses

# ---------------------------------------------------------------------------------------------------------------- fake translator answers (glslang)
def attrs(n, ty='vec4'): return '\n'.join(f'layout(location = {i}) in {ty} a{i};' for i in range(n))


def vs_glsl(nin, nout, point=True):
    body = ' + '.join(f'a{i}' for i in range(nin)) or 'vec4(1.0)'
    return ('#version 450\n' + attrs(nin) + '\n' + '\n'.join(f'layout(location = {i}) out vec4 v{i};' for i in range(nout)) + f'\nvoid main() {{\n    vec4 s = {body};\n' +
            ''.join(f'    v{i} = s * {i + 1}.0;\n' for i in range(nout)) + '    gl_Position = s;\n' + ('    gl_PointSize = 2.0;\n' if point else '') + '}\n')


def ps_glsl(locs):
    return ('#version 450\n' + '\n'.join(f'layout(location = {i}) in vec4 v{i};' for i in locs) + '\nlayout(location = 0) out vec4 o0;\nvoid main() {\n    o0 = ' + (' + '.join(f'v{i}' for i in locs) or 'vec4(1.0)') + ';\n}\n')


def fake_shader(name, stage, glsl, gl_stage, ctl, code=None, extra_res=None):
    spv = glsl_spv(glsl, gl_stage)
    res = dict(RES, **(extra_res or {}))
    return add(name, stage, lambda k: record(k, code or (NV1_CODE + bytes(0x60)), ctl), fake=(spv, res))


vs_keys = {}
vs_keys['vs_a'] = fake_shader('vs_a', 'vs', vs_glsl(2, 3), 'vert', ctl1())
vs_keys['vs_b'] = fake_shader('vs_b', 'vs', vs_glsl(1, 2, point=False), 'vert', ctl1())
vs_keys['vs_tf'] = fake_shader('vs_tf', 'vs', vs_glsl(2, 3), 'vert', ctl1(tf=[(32, [(0, 4), (2, 4)]), None, None, None]))
ps_keys = {}
ps_omap = bytearray(0x60); struct.pack_into('<I', ps_omap, 72, 0xf)
ps_keys['ps_01'] = fake_shader('ps_01', 'ps', ps_glsl([0, 1]), 'frag', ctl1(), code=NV1_CODE + sph(0xf0, imap={0: 0xff, 1: 0x55})[:0x50] + bytes(0x10))
ps_keys['ps_2'] = fake_shader('ps_2', 'ps', ps_glsl([2]), 'frag', ctl1(), code=NV1_CODE + sph(0xf0, imap={2: 0x03})[:0x50] + bytes(0x10))
ps_keys['ps_n2'] = fake_shader('ps_n2', 'ps', ps_glsl([0, 3, 5]), 'frag', ctl2(0x80), code=sph(0xf0, imap={0: 0x0f, 3: 0xc0, 5: 0xff}) + bytes(0x20))   # an NVN2 record: its SPH leads the code
ps_keys['ps_n2b'] = fake_shader('ps_n2b', 'ps', ps_glsl([1, 2, 3, 4]), 'frag', ctl2(0x80), code=sph(0xf0, imap={1: 0xff, 2: 0xff, 4: 0x0c}) + bytes(0x20))


def tesc(ncp, ins, outs, patches, quads=False, pos=True):
    t = ['#version 450', f'layout(vertices = {ncp}) out;']
    t += [f'layout(location = {l}) in vec4 i{l}[];' for l in ins] + [f'layout(location = {l}) out vec4 o{l}[];' for l in outs] + [f'layout(location = {l}) patch out vec4 p{l};' for l in patches]
    acc = ' + '.join(f'i{l}[gl_InvocationID]' for l in ins) or 'vec4(1.0)'
    t += ['void main() {'] + [f'    o{l}[gl_InvocationID] = {acc};' for l in outs] + (['    gl_out[gl_InvocationID].gl_Position = gl_in[gl_InvocationID].gl_Position;'] if pos else [])
    t += ['    if (gl_InvocationID == 0) {', '        gl_TessLevelOuter[0] = 4.0; gl_TessLevelOuter[1] = 3.0; gl_TessLevelOuter[2] = 5.0; gl_TessLevelInner[0] = 2.0;'] + ([] if not quads else ['        gl_TessLevelOuter[3] = 2.0; gl_TessLevelInner[1] = 6.0;'])
    t += [f'        p{l} = {acc.replace("[gl_InvocationID]", "[0]")};' for l in patches] + ['    }', '}']
    return '\n'.join(t)


def tese(ins, patches, outs, ncp=3, layout='triangles, equal_spacing, cw'):
    t = ['#version 450', f'layout({layout}) in;'] + [f'layout(location = {l}) in vec4 c{l}[];' for l in ins] + [f'layout(location = {l}) patch in vec4 q{l};' for l in patches] + [f'layout(location = {l}) out vec4 w{l};' for l in outs]
    pos = ' + '.join(f'gl_TessCoord.{c} * gl_in[{i}].gl_Position' for i, c in enumerate('xyz'[:ncp]))
    acc = ' + '.join([f'c{l}[{i}]' for l in ins for i in range(min(ncp, 3))] + [f'q{l}' for l in patches]) or 'vec4(0.5)'
    return '\n'.join(t + ['void main() {', f'    gl_Position = {pos};'] + [f'    w{l} = {acc};' for l in outs] + ['}'])


TRI = (0x0004, 0x0202, 0x0900)
fake_shader('hs_fake_a', 'hs', tesc(3, [0, 1], [0], [2]), 'tesc', ctl1())
fake_shader('hs_fake_b', 'hs', tesc(4, [3], [1, 2], [], quads=True), 'tesc', ctl1())
fake_shader('hs_fake_bad', 'hs', tesc(3, [0], [], [5]).replace('vec4 p5', 'vec3 p5').replace('p5 = ', 'p5 = (').replace(';\n    }', ').xyz;\n    }'), 'tesc', ctl1())   # a vec3 patch output: not a 4-component slot
fake_shader('ds_tri', 'ds', tese([0], [2], [0]), 'tese', ctl1(tess=TRI))
fake_shader('ds_quads', 'ds', tese([0, 1], [], [0, 1]), 'tese', ctl1(tess=(0x0007, 0x8E7B, 0x0901)))
fake_shader('ds_evens', 'ds', tese([0], [2, 4], [3]), 'tese', ctl1(tess=(0x0004, 0x8E7C, 0x0901)))
fake_shader('ds_isolines', 'ds', tese([0], [], [0]), 'tese', ctl1(tess=(0x8E7A, 0x0202, 0x0900)))   # isolines: left as translated, 'unsupported'
fake_shader('ds_nomode', 'ds', tese([0], [], [0]), 'tese', ctl1())   # no tess_mode in its control section
fake_shader('ds_badin', 'ds', tese([0], [], [0]).replace('vec4 c0[]', 'vec3 c0[]').replace('c0[0]', 'vec4(c0[0], 1.0)').replace('c0[1]', 'vec4(c0[1], 1.0)').replace('c0[2]', 'vec4(c0[2], 1.0)'), 'tese', ctl1(tess=TRI))

# ---------------------------------------------------------------------------------------------------------------- random task / mesh shaders and pixel shaders (--random N)
N_RAND = int(sys.argv[sys.argv.index('--random') + 1]) if '--random' in sys.argv else 0
RAND_PAIRS = []
rand_ps = []
for i in range(max(2, N_RAND // 4) if N_RAND else 0):
    locs = sorted(rnd.sample(range(0, 12), rnd.randrange(1, 5)))
    imap = {l: rnd.choice([0x0f, 0xff, 0x55, 0x03, 0xc0, 0x30, 0xaa, 0x01]) for l in locs if rnd.random() < 0.8}
    if rnd.random() < 0.5: fake_shader(f'rps{i}', 'ps', ps_glsl(locs), 'frag', ctl2(0x80), code=sph(0xf0, imap=imap) + bytes(0x20))
    else: fake_shader(f'rps{i}', 'ps', ps_glsl(locs), 'frag', ctl1(), code=NV1_CODE + sph(0xf0, imap=imap)[:0x50] + bytes(0x10))
    rand_ps.append(f'rps{i}')
CODES = [[MS.PAYLOAD], [MS.WORKGROUP, MS.KEEP_R0], [MS.INPUT, MS.KEEP_R19], [MS.ATTRIBUTE], [MS.READBACK, MS.KEEP_R8], [MS.COUNT], [MS.INDEX], [MS.COUNT_ZERO, MS.COUNT], [MS.INPUT, MS.KEEP_R19, MS.ATTRIBUTE, MS.COUNT]]
for i in range(N_RAND):
    kind = rnd.choice(['ts', 'ms', 'ms', 'ms'])
    gens = bytes(rnd.choice([0, 0, 0x03, 0x0f, 0x30, 0xf0, 0xff, 0x11, 0x33]) for _ in range(rnd.choice([0, 1, 2, 4, 8])))
    vsph = sph(rnd.choice([0xf0, 0xf0, 0xf8, 0x30, 0xf1]), gens, sys0=rnd.choice([0, 0, 1]))
    psph = sph(0, bytes(rnd.choice([0, 0x01, 0x0f, 0x03]) for _ in range(rnd.choice([0, 1, 2]))), sys0=rnd.choice([0, 1, 0x100 & 0xff])) if rnd.random() < 0.5 else None
    nm = f'r{kind}{i}'
    if kind == 'ts': ampere_stage(nm, 'ts', threads=rnd.choice([32, 64, 1]), payload=rnd.choice([16, 128, 256, 1408, 77]), code=rnd.choice(CODES))
    else: ampere_stage(nm, 'ms', threads=rnd.choice([32, 32, 32, 64, 128]), payload=rnd.choice([0, 16, 128, 256]), max_v=rnd.choice([32, 64, 128, 128, 160, 256, 1]), max_p=rnd.choice([32, 64, 126, 128, 512, 513]),
                       task=rnd.choice([0, 1]), verts=rnd.choice([3, 3, 3, 2, 1, 0, 4]), code=rnd.choice(CODES), vertex_sph=vsph, prim_sph=psph)
    for p in rnd.sample(rand_ps, rnd.randrange(0, min(4, len(rand_ps)) + 1)): RAND_PAIRS.append((nm, p))

# ---------------------------------------------------------------------------------------------------------------- tables, packages, fakes
PAIRS = [('vs_a', 'ps_01'), ('vs_b', 'ps_2'), ('vs_tf', 'ps_01'), ('ms_task', 'ps_n2'), ('ms_notask', 'ps_01'), ('ms_notask', 'ps_n2b'), ('ms_big', 'ps_n2'), ('ms_prim_id', 'ps_01'), ('ms_point', 'ps_n2b'),
         ('ms_refused', 'ps_01'), ('ms_toobig', 'ps_01'), ('ts_payload', 'ps_01')]
MESH_PAIRS = [('vs_a', 'gs_pass'), ('vs_b', 'gs_pass'), ('vs_tf', 'gs_pass_tf'), ('vs_a', 'gs_pass2'), ('vs_b', 'gs_plain'), ('vs_a', 'gs_missing'), ('hull_cbe', 'gs_pass')]
PAIRS += [(a, b) for a, b in RAND_PAIRS]
by_name = {s['name']: s for s in SHADERS}
stages = {}
for s in SHADERS: stages.setdefault(s['stage'], []).append(s)
rows = []
for st, lst in sorted(stages.items()):
    path = f'{OUT}/pkg/{st}_main.bin'
    with open(path, 'wb') as f:
        for i, s in enumerate(lst):
            f.write(s['rec']); rows.append(f'{st}_main.bin\t{i}\t{st}\t{s["key"]}\tsynthetic#{s["name"]}')
prof = f'{OUT}/profile/{GAME_ID}'
if GAME_TSV: shutil.copy(GAME_TSV, f'{prof}/game.tsv')
open(f'{prof}/shader_map.tsv', 'w').write('# synthetic game of tools/tessmesh/make_game.py\npackage\tindex\tstage\tdm_ctl_sha16\tsource\n' + '\n'.join(rows) + '\n')
open(f'{prof}/shader_map.tsv.conflicts.tsv', 'w').write('# none\n')
open(f'{prof}/shader_facts.tsv', 'w').write('# synthetic answer table: every query answered with defaults\n')
open(f'{prof}/shader_pairs.tsv', 'w').write('# vertex / mesh key <TAB> pixel key\n' + ''.join(f'{by_name[a]["key"]}\t{by_name[b]["key"]}\n' for a, b in PAIRS))
open(f'{prof}/geo_mesh.tsv', 'w').write('# vertex key, geometry key\n' + ''.join(f'{by_name[a]["key"]}\t{by_name[b]["key"] if b in by_name else key_of(b)}\n' for a, b in MESH_PAIRS))
for s in SHADERS:
    if s['fake']:
        open(f'{OUT}/fake/{s["key"]}.spv', 'wb').write(s['fake'][0]); json.dump(dict(s['fake'][1], key=s['key'], stage=s['stage']), open(f'{OUT}/fake/{s["key"]}.res.json', 'w'), indent=2)
json.dump({s['key']: dict(name=s['name'], stage=s['stage'], fake=bool(s['fake']), note=s['note']) for s in SHADERS}, open(f'{OUT}/manifest.json', 'w'), indent=1)
print(f'{len(SHADERS)} shaders ({sum(1 for s in SHADERS if s["fake"])} with a fake translation) in {OUT}', file=sys.stderr)
