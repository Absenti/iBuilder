"""Corpus generator for the round-3 differential tests (tessellation, task / mesh shaders, passthrough geometry): `docs/shaders/SHADER-TESS-MESH-RUST.md` section 3.

    PYTHONDONTWRITEBYTECODE=1 python -I gen_corpus.py <engine copy (the dir holding dm/)> <out corpus dir> [--scale F] [--seed N] [--only tess,mesh,lean,compose]

Writes `<out>/cases.tsv` (`id<TAB>step<TAB>path of the input MSL<TAB>args json`, the format of tools/difftest/) and the inputs under `<out>/in/`.

The inputs are made of
* REAL tool output: small GLSL programs (random shapes) -> `glslangValidator -V` -> `spirv-opt --strip-debug` (the translator's SPIR-V has no names either: SPIRV-Cross then names
  its variables `m_<id>`, as `dmsh_tess` expects) -> the engine's own `spirv-cross` with the flags of `dmsh_build.XC`, for hull (tesc) and domain (tese) shaders;
* hand-made MSL in the shapes the engine's own tests use (`test_dmsh_meshstage.py` KERNEL, `test_dmsh_geometry.py` LEAN_KERNEL / MESH_VS), varied and mutated: the kernel's head,
  its declarations, its parameters, the lean kernel's capture reads and stores, the vertex entries' resources;
* mutations of both (a character, a token, a line damaged / duplicated / swapped; SPIR-V instructions dropped or their literals changed), so that the error paths fire.

No game data is read or written."""
import json, os, random, re, struct, subprocess, sys, tempfile

ENGINE = os.path.abspath(sys.argv[1]); OUT = os.path.abspath(sys.argv[2]); args = sys.argv[3:]
SCALE = float(args[args.index('--scale') + 1]) if '--scale' in args else 1.0
SEED = int(args[args.index('--seed') + 1]) if '--seed' in args else 20261119
ONLY = set(args[args.index('--only') + 1].split(',')) if '--only' in args else {'tess', 'mesh', 'lean', 'compose'}
TOOLS = os.path.join(ENGINE, 'dm/gfx/tools')
sys.path.insert(0, TOOLS)
import dmsh_tess as T, dmsh_meshstage as MS, dmsh_geometry as G, dmsh_mesh as M, dmsh_abi as abi  # noqa: E402
XC = ['--msl', '--msl-version', '20300', '--msl-argument-buffers', '--msl-texture-buffer-native', '--msl-pad-fragment-output']
SPIRV_CROSS = os.path.join(TOOLS, 'spirv_cross_bin/spirv-cross')
rnd = random.Random(SEED)
os.makedirs(OUT + '/in', exist_ok=True)
CASES = []
NEXT = [0]
STATS = {}


def N(n): return max(1, int(n * SCALE))


def put(step, text, args=None, ext='metal'):
    cid = NEXT[0]; NEXT[0] += 1
    path = f'{OUT}/in/{cid}.{ext}'
    with open(path, 'w', encoding='utf-8', newline='') as f: f.write(text)
    CASES.append(f'{cid}\t{step}\t{path}\t{json.dumps(args) if args is not None else ""}')
    STATS[step] = STATS.get(step, 0) + 1
    return cid


def put_spv(data):
    cid = NEXT[0]; NEXT[0] += 1
    path = f'{OUT}/in/{cid}.spv'
    with open(path, 'wb') as f: f.write(data)
    return path


def run(cmd, **kw): return subprocess.run(cmd, capture_output=True, **kw)


def glsl(src, stage):
    with tempfile.TemporaryDirectory() as d:
        p = f'{d}/s.{stage}'; open(p, 'w').write(src)
        r = run(['glslangValidator', '-V', p, '-o', f'{d}/s.spv'], text=True)
        if r.returncode: return None
        r = run(['spirv-opt', '--strip-debug', f'{d}/s.spv', '-o', f'{d}/t.spv'], text=True)
        if r.returncode: return None
        return open(f'{d}/t.spv', 'rb').read()


def cross(spv, extra=()):
    with tempfile.TemporaryDirectory() as d:
        p = f'{d}/s.spv'; open(p, 'wb').write(spv)
        r = run([SPIRV_CROSS, *XC, *extra, p], text=True)
        return r.stdout if r.returncode == 0 else None


# ------------------------------------------------------------------------------------------------------------------------------------------ mutation
def mutate_text(t, r):
    """1-3 small damages that keep the line structure mostly: a character, a token, a digit, a line deleted / duplicated / swapped / truncated"""
    lines = t.split('\n')
    for _ in range(r.randrange(1, 4)):
        k = r.randrange(9)
        if not lines: break
        i = r.randrange(len(lines)); ln = lines[i]
        if k == 0 and ln: j = r.randrange(len(ln)); lines[i] = ln[:j] + ln[j + 1:]
        elif k == 1 and ln: j = r.randrange(len(ln)); lines[i] = ln[:j] + r.choice('abcxyz_0123 ;[]()<>{}*,.') + ln[j:]
        elif k == 2: lines[i] = re.sub(r'\d+', lambda m: str(int(m.group()) + r.choice([-1, 1, 2, 7, 100])), ln, count=1)
        elif k == 3: del lines[i]
        elif k == 4: lines.insert(i, ln)
        elif k == 5 and i + 1 < len(lines): lines[i], lines[i + 1] = lines[i + 1], lines[i]
        elif k == 6 and ln: lines[i] = ln[:r.randrange(len(ln) + 1)]
        elif k == 7: lines[i] = ln.replace('float4', r.choice(['float3', 'int4', 'uint4', 'half4', 'float', 'double4']), 1)
        else: lines[i] = ln.replace(' [[', ' [[ ', 1) if ' [[' in ln else ln + ' '
    return '\n'.join(lines)


def spv_instrs(data):
    w = list(struct.unpack(f'<{len(data) // 4}I', data)); i = 5; out = []
    while i < len(w):
        n = w[i] >> 16
        if not n or i + n > len(w): return None
        out.append(w[i:i + n]); i += n
    return w[:5], out


def mutate_spv(data, r):
    p = spv_instrs(data)
    if not p: return data
    hdr, ins = p
    for _ in range(r.randrange(1, 3)):
        k = r.randrange(7)
        decos = [j for j, x in enumerate(ins) if x[0] & 0xffff == 71]
        if k == 0 and decos: del ins[r.choice(decos)]
        elif k == 1 and decos:
            x = ins[r.choice(decos)]
            if len(x) >= 4 and x[2] == 30: x[3] = r.choice([0, 1, 5, 31, 32, 40, x[3] + 1])
        elif k == 2 and decos: ins.append(list(ins[r.choice(decos)]))
        elif k == 3:   # a Patch decoration on a random decorated id
            if decos: ins.insert(r.choice(decos), [71 | 3 << 16, ins[r.choice(decos)][1], 15])
        elif k == 4:   # drop a variable instruction
            v = [j for j, x in enumerate(ins) if x[0] & 0xffff == 59]
            if v: del ins[r.choice(v)]
        elif k == 5:   # the storage class of a variable
            v = [j for j, x in enumerate(ins) if x[0] & 0xffff == 59 and len(x) >= 4]
            if v: ins[r.choice(v)][3] = r.choice([1, 3, 6, 7, 12])
        else:
            decos = [x for x in ins if x[0] & 0xffff == 71 and len(x) >= 4 and x[2] == 11]
            if decos: r.choice(decos)[3] = r.choice([0, 1, 7, 9, 10])
    flat = hdr + [v for x in ins for v in x]
    out = struct.pack(f'<{len(flat)}I', *flat)
    if r.random() < 0.05: out = out[:r.randrange(20, len(out))]   # truncated
    return out


# ------------------------------------------------------------------------------------------------------------------------------------------ tessellation
GLTYPE = {'vec4': 'vec4', 'ivec4': 'ivec4', 'uvec4': 'uvec4', 'vec3': 'vec3', 'vec2': 'vec2', 'float': 'float', 'vec4[2]': 'vec4'}


def pick_type(r, bad):
    if r.random() < bad: return r.choice(['ivec4', 'uvec4', 'vec3', 'vec2', 'float', 'vec4a'])
    return 'vec4'


def decl(kind, ty, loc, name, per_cp=True, patch=False):
    q = ('patch ' if patch else '') + kind
    if ty == 'vec4a': return f'layout(location = {loc}) {q} vec4 {name}[2];' if patch else f'layout(location = {loc}) {q} vec4 {name}[][2];'
    return f'layout(location = {loc}) {q} {ty} {name}{"[]" if per_cp and not patch else ""};'


def expr_of(ty, src):
    if ty == 'vec4a': return f'{src}[0] + {src}[1]'
    return src


def assign_to(ty, dst, e):
    if ty == 'vec4a': return f'{dst}[0] = {e}; {dst}[1] = {e};'
    return f'{dst} = {e};'


def val(ty, e):   # a vec4 expression converted to the declared type
    return {'vec4': e, 'ivec4': f'ivec4({e})', 'uvec4': f'uvec4({e})', 'vec3': f'({e}).xyz', 'vec2': f'({e}).xy', 'float': f'({e}).x', 'vec4a': e}[ty]


def asv4(ty, src):   # a declared-type value as a vec4
    return {'vec4': src, 'ivec4': f'vec4({src})', 'uvec4': f'vec4({src})', 'vec3': f'vec4({src}, 1.0)', 'vec2': f'vec4({src}, 0.0, 1.0)', 'float': f'vec4({src})', 'vec4a': f'({src}[0] + {src}[1])'}[ty]


def gen_tess(r):
    bad = r.choice([0, 0, 0, 0.1, 0.3])
    ncp = r.choice([1, 2, 3, 3, 3, 4, 4, 5, 6, 8, 16, 32])
    quads = r.random() < 0.45
    locs = r.sample(range(0, 32), 26)
    L = iter(locs)
    # per control point inputs (hull) / outputs; patch outputs
    hin = [(next(L), pick_type(r, bad), f'inp{i}') for i in range(r.randrange(0, 5))]
    hout = [(next(L), pick_type(r, bad), f'outp{i}') for i in range(r.randrange(0, 5))]
    hpatch = [(next(L), pick_type(r, bad), f'pat{i}') for i in range(r.randrange(0, 4))]
    gl_pos_in = r.random() < 0.85
    tc = ['#version 450', f'layout(vertices = {ncp}) out;']
    tc += [decl('in', t, l, n) for l, t, n in hin] + [decl('out', t, l, n) for l, t, n in hout] + [decl('out', t, l, n, patch=True) for l, t, n in hpatch]
    body = []
    acc = ' + '.join([asv4(t, f'{n}[gl_InvocationID]') for l, t, n in hin]) or 'vec4(1.0)'
    for l, t, n in hout: body.append(assign_to(t, f'{n}[gl_InvocationID]', val(t, acc)))
    if gl_pos_in: body.append('gl_out[gl_InvocationID].gl_Position = gl_in[gl_InvocationID].gl_Position;')
    lv = ['gl_TessLevelOuter[0] = 4.0; gl_TessLevelOuter[1] = 3.0; gl_TessLevelOuter[2] = 5.0; gl_TessLevelInner[0] = 2.0;']
    if quads: lv[0] += ' gl_TessLevelOuter[3] = 2.0; gl_TessLevelInner[1] = 6.0;'
    body.append('if (gl_InvocationID == 0) {')
    body += lv
    for l, t, n in hpatch: body.append(assign_to(t, n, val(t, acc.replace('[gl_InvocationID]', '[0]'))))
    body.append('}')
    tc += ['void main() {'] + body + ['}']
    # domain
    din = [(next(L), pick_type(r, bad), f'cin{i}') for i in range(r.randrange(0, 4))]
    dpatch = [(next(L), pick_type(r, bad), f'dpat{i}') for i in range(r.randrange(0, 4))]
    dout = [(next(L), 'vec4', f'dout{i}') for i in range(r.randrange(0, 3))]
    spacing = r.choice(['equal_spacing', 'fractional_odd_spacing', 'fractional_even_spacing'])
    # the translator always declares the default domain (triangles, equal spacing, see dmsh_tess.py), so spirv-cross writes a float3 gl_TessCoord; the job's tess_mode (below) is what varies. A few
    # programs keep their own layout (quads: spirv-cross writes a float2 coordinate, which the Python refuses)
    glmode = 'triangles' if r.random() < 0.93 else r.choice(['quads', 'isolines'])
    mode = r.choice(['quads', 'triangles', 'isolines'] if r.random() < 0.15 else ['quads', 'triangles'])
    order = r.choice(['cw', 'ccw'])
    te = ['#version 450', f'layout({glmode}, {spacing}, {order}) in;']
    te += [decl('in', t, l, n) for l, t, n in din] + [decl('in', t, l, n, patch=True) for l, t, n in dpatch] + [decl('out', t, l, n, per_cp=False) for l, t, n in dout]
    k = min(ncp, 3)
    pos = ' + '.join(f'gl_TessCoord.{c} * gl_in[{i}].gl_Position' for i, c in enumerate('xyz'[:k])) if gl_pos_in else 'vec4(gl_TessCoord, 1.0)'
    acc = ' + '.join(asv4(t, f'{n}[{i}]') for l, t, n in din for i in range(k)) or 'vec4(0.5)'
    acc += ''.join(' + ' + asv4(t, n) for l, t, n in dpatch)
    tb = [f'gl_Position = {pos};'] + [f'{n} = {acc};' for l, t, n in dout]
    te += ['void main() {'] + tb + ['}']
    return '\n'.join(tc), '\n'.join(te), mode, spacing, order


def tess_cases(count):
    made = 0
    while made < count:
        tc, te, mode, spacing, order = gen_tess(rnd)
        hspv, dspv = glsl(tc, 'tesc'), glsl(te, 'tese')
        if not hspv or not dspv: STATS['glsl failed'] = STATS.get('glsl failed', 0) + 1; continue
        hm, dm = cross(hspv), cross(dspv)
        if hm is None or dm is None: STATS['spirv-cross failed'] = STATS.get('spirv-cross failed', 0) + 1; continue
        made += 1
        for kind, spv, msl in (('hull', hspv, hm), ('domain', dspv, dm)):
            variants = [(spv, msl)]
            for _ in range(rnd.choice([0, 1, 1, 2])):
                if rnd.random() < 0.5: variants.append((spv, mutate_text(msl, rnd)))
                else: variants.append((mutate_spv(spv, rnd), msl))
            for s, m in variants:
                path = put_spv(s)
                if kind == 'hull': put('tess_hull', m, {'spv': path})
                else:
                    modes = [{'mode': mode, 'spacing': spacing_name(spacing), 'winding': order}]
                    if rnd.random() < 0.4: modes.append(rnd.choice([None, {'mode': 'quads', 'spacing': rnd.choice(['equal', 'fractional_odd', 'fractional_even']), 'winding': rnd.choice(['cw', 'ccw'])},
                                                                   {'mode': 'triangles', 'spacing': rnd.choice(['equal', 'fractional_odd', 'fractional_even']), 'winding': rnd.choice(['cw', 'ccw'])},
                                                                   {'mode': 'isolines', 'spacing': 'equal', 'winding': 'cw'}]))
                    for md in modes: put('tess_domain', m, {'spv': path, 'mode': md})
    # control sections
    for _ in range(N(300)):
        ctl = bytearray(rnd.choice([0x747, 0x748, 0x748, 0x760, 0x800]))
        if len(ctl) >= 0x748:
            struct.pack_into('<2I', ctl, 0x738, rnd.choice([4, 7, 0x8E7A, 5, 0, 7, 4]), rnd.choice([0x202, 0x8E7B, 0x8E7C, 0x203, 0, 0x202]))
            struct.pack_into('<I', ctl, 0x744, rnd.choice([0x900, 0x901, 0x902, 0, 0x900]))
        put('tess_mode', '', {'ctl': ctl.hex()})


def spacing_name(s): return {'equal_spacing': 'equal', 'fractional_odd_spacing': 'fractional_odd', 'fractional_even_spacing': 'fractional_even'}[s]


# ------------------------------------------------------------------------------------------------------------------------------------------ task / mesh shaders
HEAD = """#include <metal_stdlib>
using namespace metal;
template<typename T, size_t Num>
struct spvUnsafeArray
{
    T elements[Num ? Num : 1];
    thread T& operator [] (size_t pos) thread { return elements[pos]; }
    threadgroup T& operator [] (size_t pos) threadgroup { return elements[pos]; }
    device T& operator [] (size_t pos) device { return elements[pos]; }
};
struct B { uint _m0[4]; };
struct spvDescriptorSetBuffer0 { constant B* cp_c3 [[id(0)]]; };
"""
PARAMS = ['constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]', 'uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]]', 'uint3 gl_WorkGroupID [[threadgroup_position_in_grid]]',
          'uint gl_LocalInvocationIndex [[thread_index_in_threadgroup]]', 'device B* x [[buffer(3)]]', 'uint3 gl_GlobalInvocationID [[thread_position_in_grid]]',
          'uint gl_LocalInvocationID_x [[thread_position_in_threadgroup]]', 'constant uint& n [[buffer(4)]]', 'array<texture2d<float>, 4> t [[texture(0)]]', 'sampler smp [[sampler(0)]]',
          'uint dm_lid [[thread_position_in_threadgroup]]', 'uint3 dm_tg [[threadgroup_position_in_grid]]']
LOCALS = ['uint dm_lg = 0u; (void)dm_lg;', 'uint _5 = 0u;', 'float4 v = float4(0.0);', 'int a = 1; int b = 2;', 'spvUnsafeArray<float, 4> _9;'] * 4 + ['if (x) { y; }', 'uint _7 = 1u', 'threadgroup uint q;', '    ']
BODY = ['    if (gl_LocalInvocationID.x != 0u) return;', '    _23[%d] = (*spvDescriptorSet0.cp_c3)._m0[0];', '    threadgroup_barrier(mem_flags::mem_threadgroup);',
        '    threadgroup_barrier(mem_flags::mem_device | mem_flags::mem_threadgroup);', '    int _11 = 3; int _12 = 7; dmsh_shared_store8_0(_11, _12, _23);',
        '    mem_flags f = mem_flags::mem_threadgroup;', '    threadgroup_barrier(mem_flags::mem_threadgroup); // threadgroup', '    threadgroup uint* p = &_23[0];', '    atomic_store_explicit((threadgroup atomic_uint*)&_23[1], 1u, memory_order_relaxed);']
STORE_FN = 'void dmsh_shared_store8_0(thread const int& _9, thread const int& _10, threadgroup spvUnsafeArray<uint, %d>& _23)\n{\n    ((threadgroup uchar*)&_23[0])[uint(_9)] = uchar(uint(_10));\n}\n\n'


def sph(r, pos=None, generics=None):
    s = bytearray(0x80)
    s[0x35] = pos if pos is not None else r.choice([0xf0, 0xf8, 0x30, 0xf1, 0xf9, 0x00, r.randrange(256)])
    for i in range(0x36, 0x46): s[i] = (r.choice([0, 0, 0x03, 0x0f, 0x30, 0xf0, 0xff, r.randrange(256)]) if generics is None else generics) if r.random() < 0.5 else 0
    s[0x34] = r.choice([0, 0, 0, 1, 0x10, r.randrange(256)])
    return bytes(s)


def mesh_info_json(r):
    kind = r.choice(['ts', 'ms', 'ms', 'ms'])
    i = {'kind': kind, 'threads': r.choice([32, 32, 32, 64, 128, 1, 33, 256]), 'payload': r.choice([0, 4, 16, 128, 256, 1408, 77, 2000])}
    if kind == 'ms':
        comps = []
        if r.random() < 0.2: comps.append(['psiz', 0])
        for c in r.sample(range(4), r.choice([4, 4, 4, 3, 2])): comps.append([None, c])
        comps.sort(key=lambda x: (x[0] is not None, x[1]) if False else 0)
        for n in sorted(r.sample(range(32), r.randrange(0, 5))):
            for c in range(4):
                if r.random() < 0.7: comps.append([n, c])
        if r.random() < 0.05: comps = [x for x in comps if x[0] is not None]   # no position
        if r.random() < 0.05: comps = []
        prim = []
        for n in sorted(r.sample(range(32), r.randrange(0, 4))):
            for c in range(4):
                if r.random() < 0.6: prim.append([n, c])
        sysn = r.choice([0, 0, 0, 1, 1, 2]); bi = r.choice([[], [], [8], [0, 8], [8, 9], [3]])
        i.update(task=r.random() < 0.5, verts=r.choice([3, 3, 3, 3, 2, 1, 0, 4, 5]), max_v=r.choice([32, 64, 128, 128, 160, 256, 257, 33, 1, 0, 300]), max_p=r.choice([32, 64, 126, 128, 64, 512, 513, 1, 0]),
                 vertex=comps, primitive=prim, primitive_system=sysn, primitive_builtins=bi)
        if r.random() < 0.15: del i['verts']
        if r.random() < 0.1: del i['primitive_system']
        if r.random() < 0.1: del i['primitive_builtins']
    return i


def tup(i):
    i = dict(i)
    for k in ('vertex', 'primitive'):
        if k in i: i[k] = [tuple(x) for x in i[k]]
    return i


def kernel_text(r, words, extra_store=False):
    nparams = r.choice([1, 2, 2, 3, 3, 4])
    params = r.sample(PARAMS, nparams)
    if r.random() < 0.8 and PARAMS[0] not in params: params.insert(0, PARAMS[0])
    locs = r.sample(LOCALS, r.choice([0, 0, 1, 1, 2, 3]))
    body = r.sample(BODY, r.choice([1, 2, 3])) + (['    _23[%d] = 1u;'] if r.random() < 0.3 else [])
    nl = '\n'
    head = HEAD
    if extra_store or r.random() < 0.2: head += STORE_FN % words
    lines = [f'kernel void main0({", ".join(params)}){r.choice(["", " ", "  "])}', '{' + r.choice(['', '', ' ']) if False else '{']
    lines += [f'    {x}' for x in locs] + [f'    threadgroup spvUnsafeArray<uint, {words}> _23;'] + [(b % r.randrange(0, max(words, 1)) if '%d' in b else b) for b in body] + ['}']
    t = head + nl.join(lines) + '\n'
    if r.random() < 0.1: t = t.replace('{\n    threadgroup', '{\n    threadgroup', 1).replace('threadgroup spvUnsafeArray<uint, %d> _23;' % words, 'threadgroup spvUnsafeArray<uint,%d>  _23;' % words)
    if r.random() < 0.05: t = t.replace('kernel void main0', 'kernel  void  main0', 1)
    return t


def mesh_cases(count):
    for _ in range(count):
        i = mesh_info_json(rnd)
        try:
            lay, size = MS.memory(tup(i))
            words = size // 4
        except Exception: words = 64
        if rnd.random() < 0.25: words = rnd.choice([1, 4, 7, 100, 1500, 1501, 1536, 3000, 4096, 4097, 9000, 100000])
        t = kernel_text(rnd, words)
        if rnd.random() < 0.15: t = mutate_text(t, rnd)
        reads = None
        if rnd.random() < 0.6:
            reads = {str(n): rnd.choice([0, 1, 3, 7, 8, 0xf, 0xc, 4]) for n in rnd.sample(range(32), rnd.randrange(0, 8))}
        put('mesh_to_stage', t, {'info': i, 'reads': reads})
    # control sections and areas
    for _ in range(N(1500)):
        ctl = bytearray(rnd.choice([0x80, 0x80, 0x80, 0x100, 0x5f, 0x6a, 0x60, 0x58, 0x40]))
        if len(ctl) > 0x56: ctl[0x56] = rnd.choice([6, 7, 7, 7, 6, 7, 6, 7, 1, 0, 5])
        if len(ctl) >= 0x5c: struct.pack_into('<2H', ctl, 0x58, rnd.choice([32, 64, 1, 128, 0, 65535]), rnd.choice([0, 128, 256, 1408, 77, 4, 65535]))
        if len(ctl) > 0x5f: ctl[0x5e] = rnd.choice([0, 1, 1]); ctl[0x5f] = rnd.choice([0, 1, 2, 3, 3, 4])
        if len(ctl) >= 0x64: struct.pack_into('<2H', ctl, 0x60, rnd.choice([32, 64, 128, 0, 300, 65535]), rnd.choice([32, 64, 126, 0, 600]))
        prim_at = rnd.choice([0, 0x80, 0x180, 0x100, 0x1000]) if len(ctl) >= 0x68 else 0
        if len(ctl) > 0x68:
            if prim_at: struct.pack_into('<I', ctl, 0x64, prim_at); ctl[0x68] = rnd.choice([0x80, 0x80, 0xff, 0x7f])
            else: ctl[0x68] = rnd.choice([0, 0x7f])
        area = sph(rnd) + (bytes(rnd.choice([0, 0x80, 0x100])) + sph(rnd) if rnd.random() < 0.7 else b'')
        if rnd.random() < 0.05: area = area[:rnd.choice([0, 0x30, 0x35, 0x40, 0x45, 0x80])]
        if prim_at and rnd.random() < 0.8 and len(area) < prim_at + 0x80:
            area = area.ljust(prim_at, b'\0') + sph(rnd)
        code = bytes(rnd.randrange(256) for _ in range(rnd.choice([0, 16, 64, 160]))) if rnd.random() < 0.4 else b''
        if code and rnd.random() < 0.7:
            code = bytearray(code); o = rnd.randrange(0, len(code) - 15, 16) if len(code) >= 16 else 0
            if len(code) >= 16: code[o:o + 2] = struct.pack('<H', rnd.choice([0x984, 0x388, 0x988, 0x38c, 0x98c, 0xf8c, 0x984 | 0x1000, 0x985, 0x7918]))
            code = bytes(code)
        put('mesh_info', '', {'ctl': bytes(ctl).hex(), 'area': area.hex(), 'code': code.hex()})
    for _ in range(N(300)):
        s = bytearray(rnd.choice([0x80, 0x80, 0x38, 0x37, 0x40, 0]))
        for k in range(0x18, min(len(s), 0x38)): s[k] = rnd.choice([0, 0, 0, 0x55, 0xff, 0x0f, 0x03, 0x80, rnd.randrange(256)])
        put('mesh_pixel_reads', '', {'sph': bytes(s).hex()})
    for _ in range(N(60)): put('mesh_prims_for', '', {'n': rnd.choice([0, 4, 100, 388, 1000, 2730, 5460, 10922, 10923, 32768, 100000, rnd.randrange(0, 11000)])})
    for p in (1, 2, 4, 8, 16, 32):   # the exact limits of each primitive count: the largest vertex size that fits, and the next one (32768 = 3 * p * vbytes + 4 * p)
        edge = (32768 - 4 * p) // (3 * p)
        for n in (edge - 1, edge, edge + 1): put('mesh_prims_for', '', {'n': n})


# ------------------------------------------------------------------------------------------------------------------------------------------ passthrough geometry: lean and compose
def lean_kernel(r, stride=None, bad=0.0):
    stride = stride if stride is not None else r.choice([134, 133, 134, 7, 40, 1, 12])
    nreads = r.choice([1, 2, 3, 4, 6])
    words = r.sample([0, 1, 3, 4, 17, 20, 36, 40, 55, 76, 143, 5], nreads)
    mask_src = r.choice(words)
    ids = iter(range(10, 400, r.choice([1, 2, 2, 3])))
    nid = lambda: f'_{next(ids)}'
    lines = ['struct _22 { float _m0[1]; };', 'struct spvDescriptorSetBuffer0', '{', '    device _22* vertex_output [[id(1)]];', '    device _22* geometry_vb_output [[id(2)]];']
    if r.random() < 0.5: lines.append('    device uint* gp_s0 [[id(4)]];')
    lines += ['};', 'kernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint3 gl_GlobalInvocationID [[thread_position_in_grid]])', '{']
    arr = nid(); lines.append(f'    spvUnsafeArray<float, {stride}> {arr};')
    inv = nid(); lines.append(f'    int {inv} = int(gl_GlobalInvocationID.x);')
    vals = {}
    for w in words:
        base, off = nid(), nid()
        lines.append(f'    int {base} = {inv} * 144;')
        idx = base
        if w:
            lines.append(f'    int {off} = {base} + {w};'); idx = off
        v = nid(); lines.append(f'    float {v} = (*spvDescriptorSet0.vertex_output)._m0[{idx}];'); vals[w] = v
        if r.random() < 0.6 and w != mask_src and w < stride: lines.append(f'    {arr}[{w}] = {v};')
    lines.append(f'    {arr}[5] = as_type<float>(as_type<uint>({vals[mask_src]}) & 1u);')
    if r.random() < 0.4: lines.append(f'    uint {nid()} = atomic_fetch_add_explicit((device atomic_uint*)spvDescriptorSet0.gp_s0, 1u, memory_order_relaxed);')
    ob = nid(); lines.append(f'    int {ob} = {inv} * {stride};')
    stores = [0, 5] + r.sample([17, 36, 4, 20], r.choice([0, 1, 2]))
    for s in stores:
        ix = ob
        if s:
            ix = nid(); lines.append(f'    int {ix} = {ob} + {s};')
        v = nid(); lines.append(f'    float {v} = {arr}[{s}];')
        lines.append(f'    (*spvDescriptorSet0.geometry_vb_output)._m0[{ix}] = {v};')
    lines.append('}')
    t = 'template<typename T, size_t Num>\nstruct spvUnsafeArray\n{\n    T elements[Num ? Num : 1];\n};\n' + '\n'.join(lines) + '\n'
    if bad:
        r2 = rnd
        k = r2.randrange(8)
        if k == 0: t = t.replace('_m0[', '_m0[(', 1) if False else re.sub(r'(geometry_vb_output\)\._m0\[)(_\d+)\]', r'\1\2 + 1]', t, count=1)
        elif k == 1: t = re.sub(r'int (_\d+) = (_\d+) \+ (\d+);', lambda m: f'int {m.group(1)} = {m.group(2)} + {m.group(2)};', t, count=1)
        elif k == 2: t = t.replace(' + 5;', ' + 6;', 1)
        elif k == 3: t = t.replace('* 144;', '* 143;', 1)
        elif k == 4: t = t.replace('device uint* gp_s0', 'device uint* gp_s0', 1) + '    int _999 = 1;\n'
        elif k == 5: t = t.replace(f'{arr}[5] = ', f'{arr}[5 + 0] = ', 1)
        elif k == 6: t = mutate_text(t, rnd)
        else: t = t.replace('kernel void main0(', 'kernel void main1(', 1)
    return t, stride


def lean_cases(count):
    made = []
    for _ in range(count):
        t, stride = lean_kernel(rnd, bad=rnd.choice([0, 0, 0, 1]))
        put('geo_lean', t, {'stride': stride})
        made.append((t, stride))
    return made


MESH_VS = """#include <metal_stdlib>
using namespace metal;

struct spvDescriptorSetBuffer0
{
    constant float4* c [[id(0)]];
};

struct main0_out
{
%(outs)s
};

struct main0_in
{
%(ins)s
};

vertex main0_out main0(main0_in in [[stage_in]], %(params)s)
{
    main0_out out = {};
    out.%(o0)s = in.%(i0)s;
%(point)s    out.gl_Position = in.%(i0)s * spvDescriptorSet0.c[0];
    return out;
}

vertex main0_out main0_z(main0_in in [[stage_in]], %(params)s)
{
    main0_out out = {};
    out.%(o0)s = in.%(i0)s;
%(point)s    out.gl_Position = in.%(i0)s;
    return out;
}
"""


def vs_text(r):
    nin = r.randrange(1, 4); ins = [(r.choice(['float4', 'float4', 'float4', 'uint4', 'int4']), f'm_{r.randrange(0, 90)}', i) for i, _ in enumerate(range(nin))]
    nout = r.randrange(1, 4)
    outs = [f'    float4 m_{r.randrange(100, 190)} [[user(locn{l})]];' for l in r.sample(range(0, 20), nout)]
    if r.random() < 0.15: outs[0] = f'    float3 m_{r.randrange(100, 190)} [[user(locn{r.randrange(0, 20)})]];'
    if r.random() < 0.06: outs.append('    float4 weird [[user(locn40)]];')
    outs.append('    float4 gl_Position [[position, invariant]];')
    if r.random() < 0.75: outs.append('    float gl_PointSize [[point_size]];')
    if r.random() < 0.03: outs.append('    float gl_ClipDistance [[clip_distance]] [2];')
    params = r.choice(['constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint gl_VertexIndex [[vertex_id]]',
                       'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint gl_VertexIndex [[vertex_id]], uint gl_InstanceIndex [[instance_id]]',
                       'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4* u [[buffer(2)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4* u [[buffer(13)]]',
                       'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4* u [[buffer(40)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint b [[base_vertex]], uint bi [[base_instance]]',
                       'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4& u [[buffer(31)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4& u [[buffer(30)]]',
                       'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint q [[sample_id]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4& u [[buffer]]'] if r.random() < 0.1 else ['constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint gl_VertexIndex [[vertex_id]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint4* u [[buffer(2)]], uint b [[base_vertex]]'])
    point = '    out.gl_PointSize = 1.0;\n' if any('point_size' in o for o in outs) else ''
    t = MESH_VS % {'outs': '\n'.join(outs), 'ins': '\n'.join(f'    {t} {n} [[attribute({l})]];' for t, n, l in ins), 'params': params, 'o0': re.search(r'(m_\d+|weird)', outs[0]).group(1), 'i0': ins[0][1], 'point': point}
    if r.random() < 0.12: t = t.replace('main0_z', 'main0_q')
    return t


def compose_cases(lean_kernels, count):
    for _ in range(count):
        vs = vs_text(rnd)
        t, stride = rnd.choice(lean_kernels)
        # the lean kernel as the build makes it (with the Python function); a kernel it refuses is used as it is
        try: gs, reads = G.lean(t, stride)
        except ValueError: gs, reads = t, [36, 17]
        if rnd.random() < 0.12: gs = rnd.choice([gs.replace('uint3 gl_GlobalInvocationID [[thread_position_in_grid]]', 'uint3 gl_GlobalInvocationID [[thread_position_in_grid]], threadgroup uint* tg [[threadgroup(0)]]'),
                                                  gs.replace('kernel void main0(', 'kernel void main0(uint k [[thread_index_in_simdgroup]], ', 1), gs + '\n// threadgroup\n', mutate_text(gs, rnd),
                                                  gs.replace('constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(1)]]', 1)])
        if rnd.random() < 0.08: reads = rnd.choice([[], [2, 3], reads + [2], [0], [36, 3, 4, 19, 23, 143]])
        if rnd.random() < 0.15: vs = mutate_text(vs, rnd)
        gpath = f'{OUT}/in/{NEXT[0]}.gs.metal'; open(gpath, 'w', newline='').write(gs)
        put('mesh_compose', vs, {'gs': gpath, 'reads': reads})
    for _ in range(N(200)):
        members = []
        for _ in range(rnd.randrange(0, 6)):
            ty = rnd.choice(['float', 'float', 'int', 'uint', 'half', 'short', 'ushort', 'double', 'float']); k = rnd.choice(['', '', '2', '3', '4', '4', '4'])
            arr = rnd.choice(['', '', '', ' [8]']); attr = rnd.choice(['', ' [[user(locn1)]]', ' [[position, invariant]]', ' [[point_size]]'])
            members.append(f'    {ty}{k} m_{rnd.randrange(50)}{arr}{attr};')
        t = 'struct main0_out\n{\n' + '\n'.join(members) + '\n};\n'
        if rnd.random() < 0.08: t = 'struct other { int x; };\n'
        put('mesh_out_bytes', t)


# ------------------------------------------------------------------------------------------------------------------------------------------
if 'tess' in ONLY: tess_cases(N(400))
if 'mesh' in ONLY: mesh_cases(N(4000))
lk = lean_cases(N(2500)) if ('lean' in ONLY or 'compose' in ONLY) else []
if 'compose' in ONLY and lk: compose_cases(lk, N(2500))
open(OUT + '/cases.tsv', 'w').write('\n'.join(CASES) + '\n')
print(json.dumps({'cases': len(CASES), **STATS}, sort_keys=True), file=sys.stderr)
