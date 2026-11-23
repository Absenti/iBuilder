"""Differential cases for the parts of `ibuilder_shader::finish` that the Grizzy library does not exercise (bindless textures, computed driver-segment indices, vertex raster facts, readable-tile
samples through gl_FragCoord, ...): small HAND-MADE synthetic MSL / SPIR-V inputs, with the Python's answers computed by the engine's own modules. The Rust test (tests/finish_cases.rs)
runs the Rust functions on the same inputs and compares the answers.
usage: ENGINE=<engine dir with dm/gfx/tools> python gen_diff_cases.py > tests/data/finish_cases.json"""
import os, sys, json, random, struct, re, itertools, hashlib, collections
E = os.environ['ENGINE']
sys.path.insert(0, E + '/dm/gfx/tools')
import dmsh_reflect as R, dmsh_fetch as F, dmsh_side as S, dmsh_vin as V, dmsh_pull as P, dmsh_entry as EN, dmsh_geometry as G, dmsh_abi as abi
import tempfile
TMP = os.environ.get('F2_TMP') or tempfile.mkdtemp()   # scratch folder for the SPIR-V files the Python functions read from a path
os.makedirs(TMP, exist_ok=True)
SPV_FILE = os.path.join(TMP, '_f2_spv.bin')
rnd = random.Random(20261119)
OUT = {}
SCALE = float(os.environ.get('CASES_SCALE', '1'))   # fixture size: 1 = about 5 MB
def N(n): return max(1, int(n * SCALE))

ST = collections.Counter(); KINDS = set()   # what the cases exercise (printed on stderr)
def dj(v):
    """the text the Rust test compares with `J::dumps_compact()` (Python's default separators); a long one is stored as `sha256:<hex>:<length>` to keep the fixture small"""
    s = json.dumps(v)
    return s if len(s) <= 3000 else f'sha256:{hashlib.sha256(s.encode()).hexdigest()}:{len(s)}'
def attempt(fn, *a):
    try: v = fn(*a); attempt.raw = v; return {'ok': dj(v)}
    except Exception as e: attempt.raw = None; return {'err': type(e).__name__}
attempt.raw = None

# ---------------------------------------------------------------------------------------------- reflect
PREFIX = ['fp', 'vp', 'cs', 'gp']
def hexs(h): return '%X' % h
def tex_name(p):
    slots = rnd.choice([2, 3, 7, 10, 16, 17, 65538, 131075, 196610, 196615, 262147, 720898, 0, 4, 12])
    ty = rnd.choice([0, 0, 0, 1, 2, 3])
    words = [0x12, 0x5A, 0x48, 0x28, 0x2A, 0x5C, 0x168 // 4 + 3, 0x568 // 4 + 2, 0x120 // 4 + 1, 0x20 // 4, 0, rnd.randrange(0, 0x3fff), rnd.randrange(0x10, 0x200)]
    o0, o1 = rnd.choice(words), rnd.choice(words)
    h = (ty << 28) | (o1 << 14) | o0
    kind = rnd.choice(['t', 't2d', 't3d', 'tcube', 't2da', 'tb', 'ti', 'i', 'i2d', 'i3d', 'ib'])
    if rnd.random() < 0.12: return f'{p}_{kind}_tcb_{hexs(h)}'
    if rnd.random() < 0.1: return f'{p}_{kind}_cb0_40000000'
    return f'{p}_{kind}_cb{slots}_{hexs(h)}'
TYPES = ['texture2d<float>', 'texture3d<float>', 'depth2d<float>', 'texture_buffer<float>', 'texture2d<float, access::read_write>', 'texture2d<uint, access::read_write>', 'texturecube<float>',
         'array<texture2d<float>, 8>', 'array<texture2d_array<float>, 4>', 'array<texture_buffer<float>, 2>', 'array<texture2d<float>, 40>', 'texture2d_array<float>']
def member(p, n):
    r = rnd.random()
    if r < 0.30:
        nm = tex_name(p); ty = rnd.choice(TYPES)
        if rnd.random() < 0.35: return ('sampler' if not ty.startswith('array<') else ('array<sampler, ' + ty.split(', ')[-1]), nm + 'Smplr' + ('_w' if rnd.random() < 0.15 else ''))
        return (ty, nm)
    if r < 0.50: return ('constant _10*', f'{p}_c{rnd.choice([0, 1, 2, 3, 4, 5, 9, 16, 17, 24, 25, 26, 27, 28, 29])}')
    if r < 0.62: return ('device _9*', f'{p}_s{rnd.randrange(0, 5)}')
    if r < 0.66: return ('constant _12&', 'support_buffer')
    if r < 0.70: return ('constant _10*', 'dm_border')
    if r < 0.74: return rnd.choice([('device _9*', 'geometry_vb_output'), ('device uint*', 'vertex_output'), ('texture_buffer<float>', 'trb_data'), ('constant _5*', 'vb_info')])
    return ('texture2d<float>', f'{p}_t_cb2_{hexs(rnd.randrange(0, 0x200))}')
def reflect_case():
    stage = rnd.choice(['ps', 'vs', 'cs', 'gs', 'ds', 'ps', 'cs'])
    p = {'ps': 'fp', 'vs': 'vp', 'cs': 'cs', 'gs': 'gp', 'ds': 'vp'}[stage]
    ms = [member(p, i) for i in range(rnd.randrange(1, 12))]
    pools = ''
    extra = []
    if rnd.random() < 0.25:
        pn = f'{p}_t_cb0_40000000'; ms.append((f'constant spvPool_{pn}*', pn)); pools += f'struct spvPool_{pn}\n{{\n    uint n;\n    ulong seen;\n    array<texture2d<float>, 1> e;\n}};\n\n'
    if rnd.random() < 0.2:
        h = rnd.choice(['2A', '2C', '40000000'])
        pn = f'{p}_t_cb7_{h}'
        ms.append((f'constant spvPool_{pn}*', pn)); pools += f'struct spvPool_{pn}\n{{\n    uint n;\n    ulong seen;\n    array<{rnd.choice(["texture2d<float>", "sampler"])}, 1> e;\n}};\n\n'
    lines = []
    for i, (ty, nm) in enumerate(ms):
        attrs = ', raster_order_group(0)' if rnd.random() < 0.05 else ''
        lines.append(f'    {ty} {nm} [[id({i}{attrs})]];')
    body = ''.join(f'    {nm}.atomic_fetch_add(x);\n' if rnd.random() < 0.3 else f'    uint w = spvDescriptorSet0.{nm}.get_width(0);\n' for ty, nm in ms[:4] if not ty.startswith('constant'))
    msl = pools + 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(lines) + '\n};\n\nkernel void main0()\n{\n' + body + '}\n'
    if rnd.random() < 0.05: msl += '\nstruct oddity { int x; };\n'
    res = {'cbufs': [{'binding': i, 'slot': s, 'fixed': rnd.choice([0, 0, 140, 464])} for i, s in enumerate([1, 2, 3, 4, 5, 6, 7, 9, 25])], 'sbufs': [], 'textures': [], 'images': [], 'notes': []}
    for i in range(rnd.randrange(0, 4)):
        res['sbufs'].append({'binding': rnd.randrange(0, 9), 'slot': rnd.randrange(0, 6), 'cb_slot': rnd.choice([0, 0, 0, 255, 24, 5, 3, 1]), 'cb_offset': rnd.choice([0x110 // 4, 0x310 // 4, 0x510 // 4, 0x520 // 4, 1, 4, 5, 0x210 // 4, 0x220 // 4, 0x400 // 4, 0x30]), 'flags': rnd.choice([0, 1, 2, 3])})
    if rnd.random() < 0.3: res['notes'] = rnd.choice([['operation "Store" traced to no storage buffer'], ['x', 'operation "AtomicAdd" traced to no storage buffer'], ['operation "Load" traced to no storage buffer']])
    for kind in ('textures', 'images'):
        for i in range(rnd.randrange(0, 3)):
            res[kind].append({'binding': i, 'cbuf_slot': rnd.choice([2, 196610, 65538, -1, 7, 720898]), 'handle': rnd.choice([0x12, 0x2A, 274104410, 0x5A, 0x40000000]) , 'array': rnd.choice([1, 1, 4]), **({'tbx': rnd.randrange(0, 40)} if rnd.random() < 0.6 else {})})
    if rnd.random() < 0.3: res['fetch1d'] = [rnd.randrange(0, 20) for _ in range(3)]
    ex = attempt(R.reflect, 'k', stage, msl, res)
    ST['reflect ' + ('ok' if 'ok' in ex else 'err')] += 1
    for b in attempt.raw or []: KINDS.add(b['kind'])
    return {'stage': stage, 'msl': msl, 'res': res, 'expect': ex}
OUT['reflect'] = [reflect_case() for _ in range(N(300))]

def sbuf_case():
    """storage-buffer-only argument buffers: every branch of the sbuf -> c0 slot mapping (ubo_ptr, sbuf_push, sbuf_ubo, gmem log / map, bounded sbx, plain reg)"""
    stage = rnd.choice(['cs', 'cs', 'ps', 'vs', 'gs', 'ds'])
    p = {'ps': 'fp', 'vs': 'vp', 'cs': 'cs', 'gs': 'gp', 'ds': 'vp'}[stage]
    n = rnd.randrange(1, 5)
    ms = [('device _9*', f'{p}_s{i}') for i in range(n)]
    if rnd.random() < 0.4: ms.append(('constant _10*', f'{p}_c{abi.SBLEN_BANK}'))
    lines = [f'    {ty} {nm} [[id({i})]];' for i, (ty, nm) in enumerate(ms)]
    msl = 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(lines) + '\n};\n'
    base = R.SSBO.get(stage, 0)
    res = {'cbufs': [], 'sbufs': [], 'textures': [], 'images': [], 'notes': rnd.choice([[], ['operation "Store" traced to no storage buffer']])}
    for i in range(n + rnd.randrange(0, 2)):
        kind = rnd.choice(['plain', 'plain', 'ubo_ptr', 'push', 'ubo', 'log', 'map', 'weird'])
        cb_slot, cb_off = {'plain': (0, (base + 0x10 * rnd.randrange(0, 18)) // 4), 'ubo_ptr': (0, (abi.C0_CS_UBO + 0x10 * rnd.randrange(0, 18)) // 4), 'push': (24, rnd.randrange(0, 64)), 'ubo': (rnd.randrange(3, 17), rnd.randrange(0, 64)),
                           'log': (255, 1), 'map': (255, rnd.choice([0, 2, 5])), 'weird': (rnd.choice([1, 2]), rnd.randrange(0, 9))}[kind]
        res['sbufs'].append({'binding': rnd.randrange(0, 20), 'slot': rnd.choice([0, 1, 2, 31, 32, 40, -1]), 'cb_slot': cb_slot, 'cb_offset': cb_off, 'flags': rnd.choice([0, 1, 2, 3, 2, 3])})
    ex = attempt(R.reflect, 'k', stage, msl, res)
    ST['sbuf reflect ' + ('ok' if 'ok' in ex else 'err')] += 1
    for b in attempt.raw or []: KINDS.add(b['kind'])
    return {'stage': stage, 'msl': msl, 'res': res, 'expect': ex}
OUT['reflect'] += [sbuf_case() for _ in range(N(120))]

# ---------------------------------------------------------------------------------------------- drv_reads / unfillable / index_range
def expr(depth=0, names=('gl_SampleID', '_6')):
    r = rnd.random()
    if depth > 3 or r < 0.25: return rnd.choice([*names, *names, f'{rnd.randrange(0, 20)}u', f'{rnd.randrange(0, 4)}', 'gl_SampleID'])
    if r < 0.4: return f'{rnd.choice(["uint", "int", "as_type<uint>", "as_type<int>", "as_type<uint >"])}({expr(depth + 1, names)})'
    op = rnd.choice(['+', '+', '-', '*', '>>', '<<', '&', '|', '^', '/', '%'])
    a, b = expr(depth + 1, names), expr(depth + 1, names)
    if op in ('>>', '<<'): b = f'{rnd.randrange(0, 6)}u'
    s = f'{a} {op} {b}'
    return f'({s})' if rnd.random() < 0.7 else s
def drv_case():
    defs = []
    avail = ['gl_SampleID']
    for n in (6, 7, 8, 9, 10):
        if rnd.random() < 0.8:
            defs.append(f'    {rnd.choice(["uint", "int"])} _{n} = {expr(1, tuple(avail[-2:]))};'); avail.append(f'_{n}')
        if rnd.random() < 0.1: defs.append(f'    _{n} = {expr(2, tuple(avail[-1:]))};')
    if rnd.random() < 0.3: defs.append('    float _11 = as_type<float>(gl_SampleID);')
    uses = []
    for _ in range(rnd.randrange(1, 5)):
        bank = rnd.choice([0, 0, 2, 2, 0])
        idx = rnd.choice(['_6', '_7', '_8', '_9', f'{rnd.randrange(0, 80)}', f'{rnd.randrange(0, 80)}u', '_10', 'as_type<uint>(_11)', expr(1, tuple(avail[-2:]))])
        uses.append(f'    float4 v{len(uses)} = (*spvDescriptorSet0.fp_c{bank})._m0[{idx}];')
    msl = 'fragment main0_out main0()\n{\n' + '\n'.join(defs + uses) + '\n}\n'
    d_ = R.drv_reads(msl); ST['drv dynamic'] += any(x['dynamic'] for x in d_.values()); ST['drv ranged'] += any(len(x['vec4_offsets']) > 1 for x in d_.values())
    return {'msl': msl, 'expect': dj(R.drv_reads(msl)), 'unfillable': {st: dj(R.unfillable(R.drv_reads(msl), st)) for st in ('ps', 'vs', 'cs', 'gs', 'ts', 'ms')}}
OUT['drv'] = [drv_case() for _ in range(N(200))]
# ---------------------------------------------------------------------------------------------- raster_facts / screen_varyings (vertex)
def vs_msl():
    nattr = rnd.randrange(1, 4)
    ins = '\n'.join(f'    float4 m_{20 + i} [[attribute({i})]];' for i in range(nattr))
    nloc = rnd.randrange(0, 4)
    outs = ''.join(f'\n    float4 m_{40 + i} [[user(locn{rnd.choice([1, 2, 3, 4, 5, 6, 7, 8])})]];' for i in range(nloc))
    body = ['    main0_out out = {};', '    out.gl_PointSize = 1.0;']
    a = f'm_{20}'
    for c in 'xyz': body.append(f'    float _{50 + "xyz".index(c)} = in.{a}.{c};')
    body.append('    float _60 = in.m_20.w;')
    mode = rnd.choice(['passthrough', 'passthrough_wattr', 'branch', 'swizzle', 'far', 'far_const', 'clip', 'screen', 'screen_fma', 'affine', 'other'])
    px, py, pz, pw = '_50', '_51', '_52', '1.0' if mode != 'passthrough_wattr' else '_60'
    if mode == 'far': body += ['    float _70 = _50 * 2.0;', '    out.gl_Position.x = _50;', '    out.gl_Position.y = _51;', '    out.gl_Position.z = _70;', '    out.gl_Position.w = _70;']
    elif mode == 'far_const': body += ['    out.gl_Position.x = _50;', '    out.gl_Position.y = _51;', '    out.gl_Position.z = 1.0;', '    out.gl_Position.w = 1.0;']
    elif mode == 'screen' or mode == 'screen_fma':
        body += ['    float _71 = _50 * 3.0;', '    float _72 = _51 * 3.0;', '    float _73 = _52 + 1.0;', '    float _74 = _60 * 2.0;', '    out.gl_Position.x = _71;', '    out.gl_Position.y = _72;', '    out.gl_Position.z = _73;', '    out.gl_Position.w = _74;']
        for i, k in enumerate((p for p in re.findall(r'm_(\d+) \[\[user', outs))):
            if rnd.random() < 0.7:
                if mode == 'screen': body += [f'    out.m_{k}.x = (out.gl_Position.x + out.gl_Position.w) * 0.5;', f'    out.m_{k}.y = (out.gl_Position.w - out.gl_Position.y) * 0.5;', f'    out.m_{k}.w = out.gl_Position.w;']
                else: body += [f'    float _8{i} = fma(_71, 0.5, _74 * 0.5);', f'    out.m_{k}.x = _8{i};', f'    out.m_{k}.y = (_74 - _72) / 2.0;', f'    out.m_{k}.w = _74;']
    elif mode == 'affine':
        body += ['    float _75 = fma(_50, (*spvDescriptorSet0.vp_c4)._m0[1].x, (*spvDescriptorSet0.vp_c4)._m0[2].y);', '    float _76 = fma(_51, (*spvDescriptorSet0.vp_c4)._m0[3].z, (*spvDescriptorSet0.vp_c5)._m0[0].w);']
        body += ['    out.gl_Position.x = _50;', '    out.gl_Position.y = _51;', '    out.gl_Position.z = _52;', '    out.gl_Position.w = 1.0;']
        for k in re.findall(r'm_(\d+) \[\[user', outs)[:1]: body += [f'    out.m_{k}.x = _75;', f'    out.m_{k}.y = _76;']
    else:
        body += [f'    out.gl_Position.x = {px};', f'    out.gl_Position.y = {py};', f'    out.gl_Position.z = {pz};', f'    out.gl_Position.w = {pw};']
        for k in re.findall(r'm_(\d+) \[\[user', outs):
            if rnd.random() < 0.7: body += [f'    out.m_{k}.x = _50;', f'    out.m_{k}.y = _51;']
    if mode == 'branch': body = body[:3] + ['    if (_50 > 0.0)', '    {', '        out.gl_Position.w = 2.0;', '    }'] + body[3:]
    if mode == 'swizzle': body.append('    out.gl_Position.xy = float2(_50, _51);')
    if mode == 'clip': body.append('    out.gl_ClipDistance[0] = _50;')
    body.append('    return out;')
    gp = '    float4 gl_Position [[position, invariant]];\n    float gl_PointSize [[point_size]];'
    msl = ('struct main0_out\n{\n' + gp + outs + '\n};\n\nstruct main0_in\n{\n' + ins + '\n};\n\n'
           'vertex main0_out main0(main0_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])\n{\n' + '\n'.join(body) + '\n}\n')
    if rnd.random() < 0.5:   # the second entry point, as vs_depth leaves it
        msl += '\n\nvertex main0_out main0_z(main0_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])\n{\n' + '\n'.join(body) + '\n}\n'
    return msl
def vs_case():
    msl = vs_msl()
    r_ = R.raster_facts('vs', msl); ST['vs pos_attr'] += r_.get('pos_attr', -1) != -1; ST['vs far'] += 'far' in r_; ST['vs screen'] += bool(F.screen_varyings(msl)); ST['vs uv'] += bool(r_.get('uv_from_attr')); ST['vs affine'] += bool(r_.get('uv_affine'))
    return {'msl': msl, 'raster': dj(R.raster_facts('vs', msl)), 'screen': dj(F.screen_varyings(msl)), 'attributes': dj([[m.group(1), m.group(2), int(m.group(3))] for m in R.attributes(msl)])}
OUT['vs'] = [vs_case() for _ in range(N(250))]
def ps_msl():
    n = rnd.randrange(1, 5)
    cols = ''.join(f'\n    {rnd.choice(["float4", "float4", "uint4", "float", "half4"])} m_{3 + i} [[color({i})]];' for i in range(n))
    if rnd.random() < 0.15: cols += '\n    float gl_SampleMask [[sample_mask]];'
    body = ['    main0_out out = {};']
    if rnd.random() < 0.3: body.append('    discard_fragment();')
    return 'struct main0_out\n{' + cols + '\n};\n\nfragment main0_out main0(main0_in in [[stage_in]], uint pid [[primitive_id]])\n{\n' + '\n'.join(body) + '\n    return out;\n}\n'
OUT['ps'] = [{'msl': (m := ps_msl()), 'raster': dj(R.raster_facts('ps', m))} for _ in range(N(60))]

# ---------------------------------------------------------------------------------------------- fragment units: fold, unit (alpha / logic), shared body
def frag_msl(shared=False):
    nt = rnd.randrange(1, 4)
    tex = ''.join(f'    texture2d<float> fp_t_cb2_{20 + i} [[id({i})]];\n    sampler fp_t_cb2_{20 + i}Smplr [[id({10 + i})]];\n' for i in range(nt))
    ncol = rnd.randrange(1, 4)
    cols = ''.join(f'\n    {rnd.choice(["float4", "float4", "uint4"])} m_{3 + i} [[color({i}{rnd.choice(["", "", ", index(0)"])})]];' for i in range(ncol))
    nin = rnd.randrange(1, 4)
    ins = ''.join(f'\n    float4 m_{9 + i} [[user(locn{i + 1})]];' for i in range(nin))
    body = ['    main0_out out = {};']
    for i in range(nt):
        t = f'fp_t_cb2_{20 + i}'
        kind = rnd.choice(['varying', 'varying', 'proj', 'proj_div', 'frag', 'frag_scale', 'other'])
        ux, uy = f'_{30 + 4 * i}', f'_{31 + 4 * i}'
        if kind == 'varying': body += [f'    float {ux} = in.m_9.x;', f'    float {uy} = in.m_9.y;']
        elif kind in ('proj', 'proj_div'):
            b0 = 200 + 10 * i
            body += [f'    float _{b0} = in.m_9.x;', f'    float _{b0 + 1} = in.m_9.y;', f'    float _{b0 + 2} = in.m_9.w;']
            if kind == 'proj': body += [f'    float _{b0 + 3} = 1.0 / _{b0 + 2};', f'    float {ux} = _{b0} * _{b0 + 3};', f'    float {uy} = _{b0 + 1} * _{b0 + 3};']
            else: body += [f'    float {ux} = _{b0} / _{b0 + 2};', f'    float {uy} = _{b0 + 1} / _{b0 + 2};']
        elif kind == 'frag':
            b0 = 100 + 10 * i
            body += [f'    float _{b0} = gl_FragCoord.x;', f'    float {ux} = _{b0} * (*spvDescriptorSet0.fp_c4)._m0[1].x;', f'    float _{b0 + 1} = gl_FragCoord.y;', f'    float {uy} = (*spvDescriptorSet0.fp_c4)._m0[1].y * _{b0 + 1};']
        elif kind == 'frag_scale':
            b0 = 100 + 10 * i
            body += [f'    float _{b0} = gl_FragCoord.x;', f'    float _{b0 + 1} = (*spvDescriptorSet0.support_buffer)._m5[0].x;', f'    float _{b0 + 2} = _{b0} / _{b0 + 1};', f'    float {ux} = _{b0 + 2} * (*spvDescriptorSet0.fp_c3)._m0[0].z;',
                     f'    float _{b0 + 3} = gl_FragCoord.y;', f'    float _{b0 + 4} = _{b0 + 3} / _{b0 + 1};', f'    float {uy} = (*spvDescriptorSet0.fp_c3)._m0[0].w * _{b0 + 4};']
        else: body += [f'    float {ux} = in.m_9.x * 2.0;', f'    float {uy} = in.m_9.y;']
        body.append(f'    float2 _{40 + i} = float2({ux}, {uy});')
        smp = f'spvDescriptorSet0.{t}.sample(spvDescriptorSet0.{t}Smplr, select(_{40 + i}, float2(0.0), isnan(_{40 + i})))'
        if rnd.random() < 0.25: smp = f'dm_bs({smp}, spvDescriptorSet0.dm_border, 3u, spvDescriptorSet0.{t}, spvDescriptorSet0.{t}Smplr_w, select(_{40 + i}, float2(0.0), isnan(_{40 + i})))'
        body.append(f'    float4 _{50 + i} = {smp};')
    for i in range(ncol): body.append(f'    out.m_{3 + i} = {"_50" if i == 0 else "float4(1.0)"};')
    body.append('    return out;')
    attr = rnd.choice(['', '', '[[ early_fragment_tests ]] '])
    msl = ('#include <metal_stdlib>\n#include <simd/simd.h>\n\nusing namespace metal;\n\nstruct main0_out\n{' + cols + '\n};\n\nstruct main0_in\n{' + ins + '\n};\n\n'
           'struct spvDescriptorSetBuffer0\n{\n' + tex + '};\n\n' + attr + 'fragment main0_out main0(main0_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], float4 gl_FragCoord [[position]])\n{\n' + '\n'.join(body) + '\n}\n')
    if rnd.random() < 0.3 and not attr:
        msl += '\nfragment main0_out main0_u(main0_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], float4 gl_FragCoord [[position]])\n{\n    main0_out out = {};\n    return out;\n}\n'
    if shared: msl = S.shared_body(msl) or msl
    return msl, nt
def binds_for(msl):
    return [{'msl_name': m.group(1), 'msl_type': 'texture2d<float>', 'arg_id': i, 'kind': rnd.choice(['tex', 'tex', 'tex', 'img']), 'reg': rnd.choice([i, i + 1, None])} for i, m in enumerate(re.finditer(r'texture2d<float> (\w+) \[\[id\(\d+\)\]\];', msl))]
def unit_case():
    msl, nt = frag_msl(rnd.random() < 0.4)
    src, info = S.unit(msl)
    b = binds_for(msl); om = rnd.random() < 0.3
    folded = attempt(lambda: F.fold(src or msl, b, om_only_ok=om))
    ST['unit'] += src is not None
    if attempt.raw and attempt.raw[0] is not None:
        ST['fold ok'] += 1
        for call in attempt.raw[1]['calls']: ST['fold ' + ('proj' if call.get('proj') else 'frag_pos' if 'frag_pos' in call else 'varying')] += 1
        ST['fold mixed'] += bool(attempt.raw[1]['mixed_regs'])
    return {'msl': msl, 'binds': b, 'om_only_ok': om, 'unit': dj([src, info]), 'fold': folded,
            'shared': dj(S.shared_body(msl)), 'entries': attempt(lambda: [[e['attr'], e['ret'], e['name'], e['params'], e['decl'], e['args']] for e in S.entries(msl)])}
OUT['frag'] = [unit_case() for _ in range(N(260))]
# ---------------------------------------------------------------------------------------------- typed-output entry point (fs_uint, typed_ops) and the pixel shader's side libraries
# Own random stream (the categories after this one keep their inputs): frag_msl's shaders with random colour members (float / uint / int / half of each width, index(0), a
# colour past 7, members without an attribute), extra parameters (an attachment read refuses the logic operation, buffers 29 and 30 taken), an entry point of another return type
# or no output struct. Each case: fs_uint, typed_ops, the source post.rs writes (shared body + main0_u + main0_d), its unit and fold, and ps_libraries as shader_device runs it.
_ps_src = open(E + '/dm/gfx/tools/dmsh_build.py').read()
class _SourceMTL:   # shader_device._source_mode: the unit goes under every cut name
    def compile(self, t, msl, lib): open(lib, 'w').write(open(msl).read())
    def compile_split(self, t, msl, libs, split):
        for lib in libs: open(lib, 'w').write(open(msl).read())
_PS = {'re': re, 'os': os, 'subprocess': __import__('subprocess'), 'dmsh_side': S, 'dmsh_fetch': F, 'MTL': _SourceMTL(), 'TARGETS': {'ios'}, 'out': None,
       'SIDE_STATS': collections.Counter(), 'BF_STATS': collections.Counter()}
exec(_ps_src[_ps_src.index('def ps_libraries'):_ps_src.index('pc_of, loc, RT_KEYS')], _PS)
def ps_libraries_source(key, full, binds, om):
    d = tempfile.mkdtemp(dir=TMP); _PS.update(IOSOUT=d, w=d, BF_WANT={key} if om else set())
    plain = os.path.join(d, 'plain.metal'); open(plain, 'w').write(full)
    side = _PS['ps_libraries'](key, plain, full, binds)
    files = {n[:-len('.metallib')] + '.metal': open(os.path.join(d, n)).read() for n in sorted(os.listdir(d)) if n.endswith(('_bx.metallib', '_bf.metallib'))}
    for n in os.listdir(d): os.unlink(os.path.join(d, n))
    os.rmdir(d)
    return [files, side]
def typed_msl(r):
    global rnd
    saved, rnd = rnd, r
    try: msl, _ = frag_msl(r.random() < 0.3)
    finally: rnd = saved
    tys = ['float4', 'float4', 'float', 'float2', 'float3', 'uint4', 'uint', 'uint2', 'int4', 'int', 'int3', 'half4']
    mem = []
    for i in range(r.randrange(1, 4)):
        c = r.choice([i, i, i, i, 0, 8]) if i else r.choice([0, 0, 0, 0, 1])
        mem.append(f'    {r.choice(tys)} m_{3 + i} ' + r.choice([f'[[color({c})]]'] * 6 + [f'[[color({c}), index(0)]]', '']) + ';')
    if r.random() < 0.2: mem.append('    float gl_FragDepth [[depth(any)]];')
    if r.random() < 0.2: mem.append('    uint gl_SampleMask [[sample_mask]];')
    msl = re.sub(r'struct main0_out\n\{.*?\n\};', lambda _: 'struct main0_out\n{\n' + '\n'.join(mem) + '\n};', msl, count=1, flags=re.S)
    extra = r.choice(['', '', '', ', float4 dm_fb0 [[color(0)]]', ', constant uint& b29 [[buffer(29)]]', ', constant uint& b30 [[buffer(30)]]', ', uint gl_PrimitiveID [[primitive_id]]'])
    msl = msl.replace('float4 gl_FragCoord [[position]])', 'float4 gl_FragCoord [[position]]' + extra + ')', 1)
    if r.random() < 0.05: msl = msl.replace('fragment main0_out main0(', 'fragment float4 main0(', 1)
    if r.random() < 0.03: msl = msl.replace('struct main0_out\n', 'struct main0_res\n', 1)
    return msl
def typed_case(r):
    msl = typed_msl(r); om, dual = r.random() < 0.3, r.random() < 0.3
    saved_rnd = globals()['rnd']; globals()['rnd'] = r
    try: b = binds_for(msl)
    finally: globals()['rnd'] = saved_rnd
    fu = EN.fs_uint(msl); fd = EN.fs_dual(msl) if dual else None
    shared = S.shared_body(msl) if fu or fd else None
    full = shared + (fu or '') + (fd or '') if shared else msl
    src, info = S.unit(full)
    ST['typed'] += fu is not None; ST['typed alpha'] += bool(fu and EN.typed_ops(fu)['alpha']); ST['typed logic'] += bool(fu and EN.typed_ops(fu)['logic'])
    return {'msl': msl, 'binds': b, 'om_only_ok': om, 'dual': dual, 'fu': dj(fu), 'ops': dj(EN.typed_ops(fu) if fu else None), 'full': dj(full),
            'unit': dj([src, info]), 'fold': attempt(lambda: F.fold(src or full, b, om_only_ok=om)), 'pslib': attempt(ps_libraries_source, 'k0', full, b, om)}
_trnd = random.Random(20261123)
OUT['typed'] = [typed_case(_trnd) for _ in range(N(300))]
# ---------------------------------------------------------------------------------------------- vertex libraries: int_inputs, vin, pull
def vs_lib_msl():
    n = rnd.randrange(0, 5)
    mem = [(rnd.choice(['float4', 'float4', 'uint4', 'int4']), f'm_{rnd.choice([0, 1, 2, 3, 5, 8, 15, 16, 20])}' + rnd.choice(['', '', '_0']), i) for i, _ in enumerate(range(n))]
    mem = [(t, nm, (rnd.choice([i, i, 16]) if rnd.random() < 0.1 else i)) for t, nm, i in mem]
    ins = '\n'.join(f'    {t} {nm} [[attribute({i})]];' for t, nm, i in mem)
    outs = '    float4 gl_Position [[position, invariant]];\n    float gl_PointSize [[point_size]];\n    float4 m_30 [[user(locn1)]];'
    body = ['    main0_out out = {};']
    for t, nm, i in mem:
        r = rnd.random()
        if r < 0.4: body += [f'    float _{100 + i} = in.{nm}.x;', f'    uint _{110 + i} = as_type<uint>(_{100 + i});']
        elif r < 0.6: body += [f'    float _{100 + i} = in.{nm}.y;', f'    out.m_30.x = _{100 + i};']
        elif r < 0.8: body += [f'    int _{120 + i} = as_type<int>(in.{nm}.z);']
        else: body += [f'    float _{100 + i} = in.{nm}.x;', f'    float _{130 + i} = _{100 + i};', f'    out.m_30.y = as_type<uint>(_{130 + i});']
    body += ['    out.gl_Position.x = 0.0;', '    return out;']
    pars = rnd.choice(['', ', uint gl_VertexIndex [[vertex_id]]', ', uint gl_InstanceIndex [[instance_id]], uint gl_VertexIndex [[vertex_id]]', ', uint bi [[base_instance]]'])
    stage_in = 'main0_in in [[stage_in]]'
    sig = f'vertex main0_out main0({stage_in}, constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]{pars})'
    msl = ('#include <metal_stdlib>\n#include <simd/simd.h>\n\nusing namespace metal;\n\nstruct spvDescriptorSetBuffer0\n{\n    constant _16* vp_c4 [[id(0)]];\n};\n\nstruct main0_out\n{\n' + outs + '\n};\n\n' +
           ('struct main0_in\n{\n' + ins + '\n};\n\n' if mem else '') + sig.replace(stage_in + ', ', '' if not mem else stage_in + ', ') + '\n{\n' + '\n'.join(body) + '\n}\n')
    if not mem: msl = msl.replace('main0_in in [[stage_in]], ', '')
    if rnd.random() < 0.7:
        msl += '\n\n' + sig.replace('main0(', 'main0_z(') + '\n{\n' + '\n'.join(body) + '\n}\n'
    if rnd.random() < 0.2: msl += '\nvertex void main0_tf(main0_in in [[stage_in]])\n{\n    return;\n}\n'
    if rnd.random() < 0.1: msl = msl.replace('[[buffer(0)]]', '[[buffer(16)]]')
    return msl
def vlib_case():
    msl = vs_lib_msl()
    ii = EN.int_inputs(msl); ST['int_inputs'] += bool(ii)
    vin_ = attempt(V.variant, msl); ST['vin'] += bool(attempt.raw and attempt.raw[0] is not None)
    pull_ = attempt(P.variant, msl); ST['pull'] += bool(attempt.raw and attempt.raw[0] is not None); ST['pull wide-line'] += bool(attempt.raw and attempt.raw[0] is not None and 'entry_wl' in attempt.raw[1])
    return {'msl': msl, 'int_inputs': dj(ii), 'vin': vin_, 'pull': pull_}
OUT['vlib'] = [vlib_case() for _ in range(N(260))]
# ---------------------------------------------------------------------------------------------- SPIR-V: spv_single_lane / flatten_sets / geometry helpers
def sw(op, *ops): return [(len(ops) + 1) << 16 | op, *ops]
def name_words(s):
    b = s.encode() + b'\0'; b += b'\0' * (-len(b) % 4)
    return list(struct.unpack('<%dI' % (len(b) // 4), b))
def spv_module(kind):
    # a minimal, structurally valid-looking module: header, capability, extension, entry point, decorations, types, variable, function with loads and group ops
    bound = 40
    ins = [sw(17, 1), sw(17, 61 if kind != 'nocap' else 1), sw(10, *name_words('SPV_KHR_shader_ballot'))] if rnd.random() < 0.8 else [sw(17, 1)]
    ins += [[(5 + len(name_words('main'))) << 16 | 15, 0, 1, *name_words('main'), 20, 21]]
    bi = rnd.choice([36, 41, 4416, 4417, 4418, 4419, 4420])
    ins += [sw(5, 20, *name_words('gl_SubgroupSize')), sw(71, 20, 11, bi), sw(71, 21, 11, rnd.choice([41, 4416]))]
    ins += [sw(21, 3, 32, 0), sw(20, 4), sw(23, 5, 3, 4)] if rnd.random() < 0.7 else [sw(21, 3, 32, 0)]
    ins += [sw(32, 7, 1, 3), sw(59, 7, 20, 1), sw(59, 7, 21, 1)]
    ins += [sw(54, 2, 1, 0, 8), sw(248, 9)]
    ins += [sw(61, 3, 30, 20), sw(61, 3, 31, 21)]
    op = rnd.choice([333, 334, 335, 336, 337, 338, 339, 345, 346, 347, 348, 340, 350, 355, 366, 100])
    if kind == 'ballot': op = 339
    ins += [sw(op, 3, 32, 2, 3, 30, 31)] if op != 100 else [sw(100, 3, 32)]
    if rnd.random() < 0.25: ins += [sw(65, 7, 33, 20, 3)]
    if rnd.random() < 0.2: ins += [sw(60, 7, 34, 20, 3, 3)]
    ins += [sw(253), sw(56)]
    words = [0x07230203, 0x10300, 0, bound, 0] + [w for i in ins for w in i]
    return struct.pack('<%dI' % len(words), *words)
# spv_single_lane lives in dmsh_build (which runs a build when imported): lift it and its tables out of the source text
src = open(E + '/dm/gfx/tools/dmsh_build.py').read()
SG = {'struct': struct}
exec(src[src.index('SG_OPS = '):src.index('def geometry_finish')], SG)
def run_single_lane(d):
    p = SPV_FILE; open(p, 'wb').write(d)
    r = SG['spv_single_lane'](p)
    return [r, open(p, 'rb').read().hex()]
OUT['spv'] = []
for _ in range(N(200)):
    d = spv_module(rnd.choice(['x', 'ballot', 'nocap']))
    res = attempt(run_single_lane, d); ST['spv rewritten'] += bool(attempt.raw and attempt.raw[0] is True)
    OUT['spv'].append({'spv': d.hex(), 'expect': res})
def flat_module():
    words = [0x07230203, 0x10300, 0, 100, 0]
    ins = []
    for ident in range(10, 16):
        ins += [sw(71, ident, 34, rnd.choice([0, 0, 1, 2, 3])), sw(71, ident, 33, rnd.randrange(0, 20))]
        if rnd.random() < 0.2: ins += [sw(71, ident, 30, 5)]
    if rnd.random() < 0.1: ins += [[0]]
    for w in ins: words += w
    return struct.pack('<%dI' % len(words), *words)
def run_flatten(d):
    p = SPV_FILE; open(p, 'wb').write(d); G.flatten_sets(p); return open(p, 'rb').read().hex()
OUT['flatten'] = [{'spv': (d := flat_module()).hex(), 'expect': attempt(run_flatten, d)} for _ in range(N(60))]


# ---------------------------------------------------------------------------------------------- gs_slice (the layer-only geometry shader): the GLSL spirv-cross writes, read by regular expressions
import types
_gs_ns = {'re': re, 'subprocess': None, 'SPIRV_CROSS': 'spirv-cross'}
exec(src[src.index('def gs_slice'):src.index('SG_OPS = ')], _gs_ns)
def py_gs_slice(glsl):
    _gs_ns['subprocess'] = types.SimpleNamespace(run=lambda *a, **k: types.SimpleNamespace(returncode=0, stdout=glsl, stderr=''))
    return _gs_ns['gs_slice']('x.spv')
def gs_glsl():
    nin, nout = rnd.randrange(1, 4), rnd.randrange(1, 4)
    head = ['#version 450', 'layout(triangles) in;', 'layout(max_vertices = 3, triangle_strip) out;', '']
    head += [f'layout(location = {i + 1}) in vec4 in_{i}[3];' for i in range(nin)] + [f'layout(location = {i + 1}) out vec4 out_{i};' for i in range(nout)] + ['', 'void main()', '{']
    body = ['    int _9 = 0;'] if rnd.random() < 0.8 else []
    layer_src = (rnd.randrange(0, nin), rnd.choice('xyzw'))
    n = 20
    for v in range(3):
        body.append('    if (false)' if rnd.random() < 0.1 else '    int _%d = %d;' % (n, v)); n += 1
        for c in 'xyzw':
            if rnd.random() < 0.8: body += [f'    float _{n} = gl_in[_9].gl_Position.{c};', f'    gl_Position.{c} = _{n};']; n += 1
        for i in range(nin):
            for c in 'xyzw':
                if rnd.random() < 0.3: body += [f'    float _{n} = in_{i}[_9].{c};', f'    out_{rnd.randrange(0, nout) if rnd.random() < 0.3 else i}.{c} = _{n};']; n += 1
        li, lc = layer_src if rnd.random() < 0.85 else (rnd.randrange(0, nin), rnd.choice('xyzw'))
        body += [f'    float _{n} = in_{li}[_9].{lc};', f'    gl_Layer = floatBitsToInt(_{n});']; n += 1
        if rnd.random() < 0.05: body.append('    gl_Layer = floatBitsToInt(_20);')
        body.append('    EmitVertex();')
    if rnd.random() < 0.08: body.append('    EndPrimitive();')
    if rnd.random() < 0.08: body.insert(3, '    vec4 weird = vec4(1.0);')
    if rnd.random() < 0.05: body = body[:-2]
    glsl = '\n'.join(head + body + ['}', ''])
    if rnd.random() < 0.05: glsl = glsl.replace('triangle_strip', 'line_strip')
    return glsl
OUT['gs_slice'] = []
for _ in range(N(120)):
    g = gs_glsl(); ex = attempt(py_gs_slice, g); ST['gs_slice hit'] += bool(attempt.raw)
    OUT['gs_slice'].append({'glsl': g, 'expect': ex})

# ---------------------------------------------------------------------------------------------- geometry text helpers
def gs_msl():
    trb = rnd.choice([True, True, False])
    s = ['struct spvDescriptorSetBuffer0', '{', '    device uint* vertex_output [[id(0)]];' if rnd.random() < 0.7 else '    device float* other [[id(0)]];', '    texture_buffer<float> trb_data [[id(1)]];' if trb else '    int zzz [[id(1)]];', '};', '']
    s += ['kernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])', '{']
    for k in range(rnd.randrange(0, 4)): s.append(f'    uint _{k + 5} = {rnd.choice(["spvDescriptorSet0.trb_data", "trb_data"])}.read({rnd.choice(["_3", "(_4 + 1u)", "uint(_5)"])}){rnd.choice([".x", ".x", ".y"])};')
    s += ['}', '']
    if rnd.random() < 0.3: s[0:0] = ['float4 helper(texture_buffer<float> trb_data, uint i)', '{', '    return trb_data.read(i);', '}']
    return '\n'.join(s)
def geo_case():
    m = gs_msl()
    return {'msl': m, 'topology_pointer': attempt(G.topology_pointer, m), 'capture_input': attempt(G.capture_input, m)}
OUT['geo'] = [geo_case() for _ in range(N(80))]
def reader_msl():
    mask = rnd.choice(['gl_BuiltIn_5253', 'gl_BuiltIn_5253', 'other'])
    s = ['struct main0_out', '{', '    float4 gl_Position [[position, invariant]];', '};', '', 'vertex main0_out main0(uint vid [[vertex_id]])', '{', '    main0_out out = {};', '    float _5 = 1.0;']
    for _ in range(rnd.randrange(0, 3)): s.append(f'    spvArrayCopyFromStackToStack({mask}, as_type<int>(_5).elements);')
    if rnd.random() < 0.2: s.append(f'    spvArrayCopyFromStackToStack({mask}, other(_5).elements);')
    s += ['    return out;', '}', '']
    if rnd.random() < 0.15: s[0:0] = ['uint dm_gs_viewport_mask = 0u;']
    return '\n'.join(s)
OUT['reader'] = [{'msl': (m := reader_msl()), 'expect': attempt(G.reader, m)} for _ in range(N(60))]
OUT['output_layout'] = [{'code': (c := bytes(rnd.randrange(0, 256) for _ in range(rnd.choice([8, 20, 32])))).hex(), 'expect': [attempt(G.output_layout, c, t) for t in range(6)]} for _ in range(N(60))]
# well-formed headers too
def good_code(pt):
    w = [(1 << 24) if pt else 0, 0, (rnd.randrange(1, 40) << 24) | rnd.randrange(0, 1 << 24), (rnd.choice([1, 6, 7, 3]) << 24) | rnd.randrange(0, 1 << 20), rnd.randrange(1, 200)]
    return struct.pack('<5I', *w)
OUT['output_layout'] += [{'code': (c := good_code(rnd.random() < 0.3)).hex(), 'expect': [attempt(G.output_layout, c, t) for t in range(6)]} for _ in range(N(60))]

# ---------------------------------------------------------------------------------------------- more reflection cases, from a random stream of their own (the cases above keep theirs)
# buffer textures through uniform handles (their sampler word), wrapped 1D fetches (row_bias: a second offset table group, tbx and fetch1d on the texture), BUFFER pools (uint bias),
# fixed pointer sites of the raw pool table (gptr_sites, gmem_map's pointers), integer colour outputs (raster_facts)
def with_rnd(r, f):
    global rnd
    old, rnd = rnd, r
    try: return f()
    finally: rnd = old
BIAS_NAME = re.compile(r'[a-z]+_([ti])[a-z0-9]*_(?:cb(\d+)|tcb)_([0-9A-F]+)(?:_|$)')   # buffer_image_bias's name pattern
def gptr_notes(stage):
    base = R.SSBO.get(stage, 0x110)
    n = rnd.choice([0, 1, 2, 3, 5, 65])
    ks = list(range(n))
    r = rnd.random()
    if r < 0.08 and ks: ks[-1] += 1   # a gap
    elif r < 0.14 and ks: ks.append(ks[0])   # a site noted twice
    elif r < 0.2: rnd.shuffle(ks)
    out = []
    for k in ks:
        slot = rnd.choice([0, 0, 0, 1, 24])
        word = rnd.choice([(base + 0x10 * rnd.randrange(0, 18)) // 4, (base + 0x10 * rnd.randrange(0, 16)) // 4 + rnd.choice([0, 1]), rnd.randrange(0, 0x800)])
        out.append(f'translator: gptr site {k}: c{slot}[{word}] +{rnd.choice([0, 8, 16, 0x38])}' + rnd.choice(['', '', '', '\n', ' x']))
    if rnd.random() < 0.2: out.insert(0, 'operation "Store" traced to no storage buffer')
    if rnd.random() < 0.1: out.append('translator: gptr site 0x: c0[1] +0')
    return out
def reflect_head_case():
    stage = rnd.choice(['ps', 'vs', 'cs', 'gs', 'ds', 'ts', 'ms'])
    p = {'ps': 'fp', 'vs': 'vp', 'cs': 'cs', 'gs': 'gp', 'ds': 'vp', 'ts': 'tp', 'ms': 'mp'}[stage]
    ms = []
    for _ in range(rnd.randrange(1, 8)):
        nm = tex_name(p)
        ty = rnd.choice(['texture_buffer<float>', 'texture_buffer<uint>', 'texture2d<float>', 'texture2d<float>', 'array<texture2d<float>, 4>', 'array<texture_buffer<float>, 2>', 'texture2d<float, access::read_write>', 'texture1d<float>'])
        if rnd.random() < 0.3: ms.append(('sampler', nm + 'Smplr'))
        ms.append((ty, nm))
    pools = ''
    if rnd.random() < 0.35:
        pn = rnd.choice([f'{p}_t_cb0_40000000', f'{p}_t_cb7_2A', f'{p}_t_cb7_2C', f'{p}_tb_cb0_40000000'])
        el = rnd.choice(['texture_buffer<float>', 'texture_buffer<uint>', 'texture2d<float>', 'sampler'])
        bias = '    uint bias;\n' if 'texture_buffer<' in el and rnd.random() < 0.85 else ''
        ms.append((f'constant spvPool_{pn}*', pn)); pools += f'struct spvPool_{pn}\n{{\n    uint n;\n{bias}    ulong seen;\n    array<{el}, 1> e;\n}};\n\n'
    if rnd.random() < 0.4: ms.append(('constant _10*', f'{p}_c{abi.TBIAS_BANK}'))
    nsb = rnd.randrange(0, 3)
    for i in range(nsb): ms.append(('device _9*', f'{p}_s{i}'))
    rnd.shuffle(ms)
    lines = [f'    {ty} {nm} [[id({i})]];' for i, (ty, nm) in enumerate(ms)]
    body = ''.join(f'    uint w = spvDescriptorSet0.{nm}.get_width(0);\n' for ty, nm in ms if not ty.startswith('constant') and rnd.random() < 0.3)
    msl = pools + 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(lines) + '\n};\n\nkernel void main0()\n{\n' + body + '}\n'
    res = {'cbufs': [], 'sbufs': [], 'textures': [], 'images': [], 'notes': []}
    for ty, nm in ms:
        m = BIAS_NAME.match(nm)
        if not m or rnd.random() < 0.2: continue
        t = {'binding': len(res['textures']), 'cbuf_slot': int(m.group(2)) if m.group(2) else -1, 'handle': int(m.group(3), 16), 'array': rnd.choice([1, 1, 2, 4]), 'tbx': rnd.randrange(0, 40)}
        if rnd.random() < 0.6: t['row_bias'] = rnd.choice([True, True, False, 1, 0])
        res['images' if m.group(1) == 'i' else 'textures'].append(t)
    for t in res['textures'][:]:
        if rnd.random() < 0.15: res['textures'].append({**t, 'tbx': rnd.randrange(0, 40), 'row_bias': True})   # an entry no binding of the shader names
    for i in range(nsb):
        res['sbufs'].append({'binding': i, 'slot': rnd.randrange(0, 4), 'cb_slot': rnd.choice([255, 255, 255, 0]), 'cb_offset': rnd.choice([0, 0, 1, 2]), 'flags': rnd.choice([0, 1])})
    if rnd.random() < 0.3: res['fetch1d'] = [rnd.randrange(0, 20) for _ in range(3)]
    if rnd.random() < 0.85: res['notes'] = gptr_notes(stage)
    ex = attempt(R.reflect, 'k', stage, msl, res)
    ST['head reflect ' + ('ok' if 'ok' in ex else 'err')] += 1
    for b in attempt.raw or []:
        KINDS.add(b['kind'])
        ST['head row_bias'] += b.get('fetch1d') is True and 'tbx' in b
        ST['head pointers'] += bool(b.get('pointers'))
        ST['head tbuf_ubo soff'] += b['kind'] == abi.KIND_TBUF_UBO and 'soff' in b
    return {'stage': stage, 'msl': msl, 'res': res, 'expect': ex}
def gptr_case():
    stage = rnd.choice(['ps', 'vs', 'cs', 'gs', 'ds', 'ts', 'ms'])
    res = {'notes': gptr_notes(stage)}
    try: ex = {'ok': dj(R.gptr_sites(stage, res))}
    except ValueError as e: ex = {'err': str(e)}
    ST['gptr ' + ('ok' if 'ok' in ex else 'err')] += 1
    return {'stage': stage, 'res': res, 'expect': ex}
def ps_head_msl():
    n = rnd.randrange(1, 6)
    cols = ''.join(f'\n    {rnd.choice(["float4", "uint4", "int4", "uint", "int", "int2", "half4", "uint3", "short4", "ushort2"])} m_{3 + i} [[color({rnd.choice([i, i, 7 - i])})]];' for i in range(n))
    if rnd.random() < 0.15: cols += '\n    float gl_SampleMask [[sample_mask]];'
    return 'struct main0_out\n{' + cols + '\n};\n\nfragment main0_out main0(main0_in in [[stage_in]])\n{\n    main0_out out = {};\n    return out;\n}\n'
def head_cases():
    OUT['reflect'] += [reflect_head_case() for _ in range(N(200))]
    OUT['gptr'] = [gptr_case() for _ in range(N(120))]
    OUT['ps'] += [{'msl': (m := ps_head_msl()), 'raster': dj(R.raster_facts('ps', m))} for _ in range(N(60))]
with_rnd(random.Random(20261123), head_cases)
# ---------------------------------------------------------------------------------------------- vertex entry points and libraries (HEAD): vs_viewport_mask, finish()'s vertex section, vs_layer_entry(full, source), vs_unit, vs_libraries
# A random stream of its own, so the categories above keep their cases.
import ast as _ast, contextlib as _ctx, io as _io, shutil as _shutil
import dmsh_reflect as _R
VR = random.Random(20261123)
def _verr(fn, *a):   # {'ok': json} or {'err': the ValueError's message}
    try: return {'ok': dj(fn(*a))}
    except ValueError as e: return {'err': str(e)}
def vsrc(types=None, ids=None, ret=None, mask=None, layer=None, write=None, psize=None, loc=0):
    """a vertex shader in the translator's shape, main0 alone (what finish() has before vs_init)"""
    types = types if types is not None else [VR.choice(['float4', 'float4', 'uint4', 'int4']) for _ in range(VR.randrange(0, 4))]
    ids = ids if ids is not None else VR.choice(['', ', uint gl_VertexIndex [[vertex_id]]', ', uint gl_InstanceIndex [[instance_id]]', ', uint gl_InstanceIndex [[instance_id]], uint gl_VertexIndex [[vertex_id]]', ', uint gl_ViewIndex [[amplification_id]]'])
    ret = VR.random() < 0.5 if ret is None else ret
    mask = mask if mask is not None else VR.choice(['none', 'none', 'lit', 'arg', 'cbuf', 'cbuf_hex', 'bad_vid', 'bad_two', 'bad_cond', 'bad_return', 'bad_device', 'bad_index', 'bad_local', 'cmt'])
    layer = VR.random() < 0.25 if layer is None else layer
    write = VR.random() < 0.25 if write is None else write
    psize = VR.random() < 0.5 if psize is None else psize
    if layer and 'gl_InstanceIndex' not in ids: ids += ', uint gl_InstanceIndex [[instance_id]]'
    ins = ''.join(f'    {t} m_{l} [[attribute({l + loc})]];\n' for l, t in enumerate(types))
    use = ''.join(f'    acc += float4(as_type<float4>(in.m_{l}));\n' if t != 'float4' else f'    acc += in.m_{l};\n' for l, t in enumerate(types))
    outs = '    float4 c [[user(locn0)]];\n    float4 m_1 [[user(locn1)]];\n    float4 gl_Position [[position, invariant]];\n' + ('    float gl_PointSize [[point_size]];\n' if psize else '')
    extra = ''
    params = ('main0_in in [[stage_in]], ' if types else '') + 'constant float4& u [[buffer(0)]]' + ids
    head = ''
    st = ''
    if mask in ('cbuf', 'cbuf_hex', 'bad_device', 'bad_index'):
        head = 'struct Words { float4 _m0[64]; };\nstruct Descriptors { ' + ('device' if mask == 'bad_device' else 'constant') + ' Words* vp_c5 [[id(1)]]; };\n\n'
        params += ', constant Descriptors& spvDescriptorSet0 [[buffer(1)]]'
        word = {'cbuf': '_m0[50].y', 'cbuf_hex': '_m0[0x3fu].w', 'bad_device': '_m0[5].x', 'bad_index': '_m0[64].x'}[mask]
        st = f'    gl_BuiltIn_5253[0] = as_type<int>((*spvDescriptorSet0.vp_c5).{word});\n'
    elif mask in ('arg', 'bad_two', 'bad_cond', 'bad_return', 'bad_local', 'cmt'):
        params += ', constant uint& mask [[buffer(2)]]'
        st = '    gl_BuiltIn_5253[0] = as_type<int>(mask);\n'
        if mask == 'bad_two': st += st
        if mask == 'bad_cond': st = '    if (acc.x > 0.0) { ' + st.strip() + ' }\n'
        if mask == 'bad_return': st = '    if (acc.y > 2.0) { return out; }\n' + st
        if mask == 'bad_local': st += '    int dm_vertex_viewport_mask = 0;\n'
        if mask == 'cmt': st = '    // a note: gl_BuiltIn_5253 é\n' + st.replace(';', ' /* ü */;')
    elif mask == 'lit': st = f'    gl_BuiltIn_5253[0] = {VR.choice(["1", "0u", "0x3fU", "-1", "int(1)", "uint(63)"])};\n'
    elif mask == 'bad_vid': st = '    gl_BuiltIn_5253[0] = int(acc.x);\n'
    if VR.random() < 0.05: params += ', constant bool& fc [[buffer(3), function_constant(dm_fc)]]'
    if write:
        params += ', texture2d<uint, access::write> image [[texture(0)]]'
        extra += '    image.write(uint4(1u), uint2(0));\n'
    early = '    if (acc.x > 5.0)\n    {\n        return out;\n    }\n    out.c = acc;\n' if ret else ''
    lay = '    out.m_1.x = as_type<float>(int(gl_InstanceIndex));\n' if layer else ''
    ps = '    out.gl_PointSize = 4.0;\n' if psize else ''
    body = f'    main0_out out = {{}};\n    float4 acc = u;\n{use}{st}{extra}{lay}{ps}{early}    out.gl_Position = acc;\n    return out;\n'
    return ('#include <metal_stdlib>\n#include <simd/simd.h>\n\nusing namespace metal;\n\n' + head + 'struct main0_out\n{\n' + outs + '};\n\n'
            + ('struct main0_in\n{\n' + ins.rstrip('\n') + '\n};\n\n' if types else '') + f'vertex main0_out main0({params})\n{{\n{body}}}\n')
# finish()'s vertex section (from `pad = []` to `fu = ...`), run as the upstream test does
_bsrc = open(E + '/dm/gfx/tools/dmsh_build.py').read(); _tree = _ast.parse(_bsrc)
_fin = next(n for n in _tree.body if isinstance(n, _ast.FunctionDef) and n.name == 'finish')
_a = next(i for i, n in enumerate(_fin.body) if isinstance(n, _ast.Assign) and any(isinstance(t, _ast.Name) and t.id == 'pad' for t in n.targets))
_b = next(i for i, n in enumerate(_fin.body) if isinstance(n, _ast.Assign) and any(isinstance(t, _ast.Name) and t.id == 'fu' for t in n.targets))
_tf = next(n for n in _tree.body if isinstance(n, _ast.FunctionDef) and n.name == 'vs_tf')
_wrap = _ast.parse('def build(src, j, res):\n    pass\n').body[0]
_wrap.body = [*_fin.body[_a:_b], *_ast.parse('return full, locals()').body]
_flow = {'re': re, 'sys': sys, 'dmsh_entry': EN, 'dmsh_geometry': G, 'dmsh_reflect': _R, 'key': 'fixture', 'PAIRS': {}, 'msl': os.path.join(TMP, '_vflow.metal'), 'TF_SLOT': 8, 'tess': None}
exec(compile(_ast.fix_missing_locations(_ast.Module(body=[_tf, _wrap], type_ignores=[])), 'dmsh_build.py', 'exec'), _flow)
RES = {'sbufs': [], 'textures': [], 'images': [], 'notes': []}
TFJ = [{'stride': 16, 'outputs': [[1, 4]]}, [], [], []]
# vs_libraries, vin_finish, pull_finish with source mode's Metal cache (the MSL under each library name), the iOS target only
_libs = {n.name: n for n in _tree.body if isinstance(n, _ast.FunctionDef) and n.name in ('vs_libraries', 'vin_finish', 'pull_finish')}
class _SrcMTL:
    def compile(self, target, msl, lib): _shutil.copyfile(msl, lib)
    def compile_split(self, target, msl, libs, split):
        for l in libs: _shutil.copyfile(msl, l)
_LIBDIR = os.path.join(TMP, '_vlib_ios'); _W = os.path.join(TMP, '_vlib_w')
_libns = {'os': os, 're': re, 'subprocess': __import__('subprocess'), 'collections': collections, 'dmsh_pull': P, 'dmsh_vin': V, 'dmsh_side': S, 'MTL': _SrcMTL(), 'out': os.path.join(TMP, '_vlib_mac'), 'IOSOUT': _LIBDIR,
          'w': _W, 'TARGETS': {'ios'}, 'PULL_STATS': collections.Counter(), 'VIN_STATS': collections.Counter()}
exec(compile(_ast.fix_missing_locations(_ast.Module(body=list(_libs.values()), type_ignores=[])), 'dmsh_build.py', 'exec'), _libns)
def vs_libraries_case(full, pulling):
    for d in (_LIBDIR, _W, _libns['out']): _shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    _libns['IOS_PULL'] = pulling
    msl = os.path.join(_W, 'k.metal'); open(msl, 'w').write(full)
    buf = _io.StringIO()
    with _ctx.redirect_stdout(buf): side, cut = _libns['vs_libraries']('k', msl, full)
    files = {f[:-len('.metallib')] + '.metal': dj(open(os.path.join(_LIBDIR, f)).read()) for f in sorted(os.listdir(_LIBDIR)) if f.endswith('.metallib')}
    return {'files': files, 'side': dj(side), 'cut': 'ios' in cut, 'log': buf.getvalue().splitlines()}
GOOD = ['none', 'none', 'none', 'lit', 'arg', 'arg', 'cbuf', 'cbuf_hex', 'cmt']
def vflow_case():
    src = vsrc(mask=VR.choice(GOOD) if VR.random() < 0.8 else None, loc=VR.choice([0, 0, 0, 14, 16, 30])); tf = VR.random() < 0.2
    j = {'stage': 'vs', **({'tf': TFJ} if tf else {})}
    r = _flow['build'](src, j, RES)
    if len(r) == 3:
        ST['vflow refused'] += 1
        return {'msl': src, 'tf': tf, 'status': r[0], 'detail': r[1]}
    full, sc = r
    ST['vflow mask'] += sc['viewport_mask']; ST['vflow layer'] += bool(sc['lay']); ST['vflow discard'] += sc['discard']
    c = {'msl': src, 'tf': tf, 'full': dj(full), 'single': dj(sc['src']), 'plain_len': len(sc['plain'].encode()), 'viewport_mask': sc['viewport_mask'], 'discard': sc['discard'], 'lay': dj(sc['lay']),
         'libs': [vs_libraries_case(full, p) for p in (True, False)]}
    for p in c['libs']: ST['vslib cut'] += p['cut']; ST['vslib red'] += bool(p['log'])
    return c
OUT['vflow'] = [vflow_case() for _ in range(N(200))]
def vmask_case():
    src = vsrc()
    if VR.random() < 0.3: src = EN.vs_init(src)
    return {'msl': src, 'expect': _verr(EN.vs_viewport_mask, src)}
OUT['vmask'] = [vmask_case() for _ in range(N(120))]
def vlayer_case():   # the input is the source after vs_layer; the test makes `full` as finish() does (checked by its hash) and passes the unculled one as `source` or None
    src = EN.vs_layer(EN.vs_init(vsrc(layer=VR.random() < 0.7, mask=VR.choice(['none', 'arg', 'lit']))))
    raster, unculled = EN.vs_viewport_mask(src)
    full = EN.vs_depth(raster)[1] + G.capture(unculled)
    source = EN.vs_depth(unculled)[1] if VR.random() < 0.7 else None
    return {'msl': src, 'full': dj(full), 'with_source': source is not None, 'expect': _verr(EN.vs_layer_entry, full, source)}
OUT['vlayer'] = [vlayer_case() for _ in range(N(60))]
TF_TAIL = '\nvertex void main0_tf(main0_in in [[stage_in]], constant float4& u [[buffer(0)]], uint dm_vid [[vertex_id]], device uint* dm_tf0 [[buffer(8)]])\n{\n    dm_tf0[dm_vid] = as_type<uint>(in.m_0.x + u.x);\n}\n'
def unit_full(src, tf, two, k):
    """the build's source for vs_unit (main0 with the depth line, main0_z, main0_gs, the rarer entry points) from the source after vs_init, then one of the shapes vs_unit refuses (k)"""
    raster, unculled = EN.vs_viewport_mask(src)
    full = EN.vs_depth(raster)[1] + G.capture(unculled) + (TF_TAIL if tf else '')
    if two: full = full.replace('    return out;\n}\n\nvertex void main0_gs', '    if (acc.w > 1.0) { return out; } else { return out; }\n}\n\nvertex void main0_gs', 1)
    i = full.find('vertex void main0_gs(')
    bad = {1: lambda: full.replace('    ' + S.VS_DEPTH + '\n', '', 1), 2: lambda: full.replace('vertex main0_out main0_z(', 'vertex main0_out main0_y('),
           3: lambda: full.replace('main0_z(main0_in in [[stage_in]], ', 'main0_z(main0_in in [[stage_in]], uint x [[vertex_id]], '),
           4: lambda: full + '\nvertex main0_out main0_p(main0_in in [[stage_in]])\n{\n    main0_out out = {};\n    return out;\n}\n',
           5: lambda: full.replace('constant float4& u [[buffer(0)]]', 'constant float4& u [[buffer(0), function_constant(c)]]'),
           6: lambda: full[:i] + full[i:].replace('float4 acc = u;', 'float4 acc = u + 1.0;'),
           7: lambda: full[:i] + full[i:].replace(' return; }', ' }'),
           8: lambda: full[:i] + full[i:].replace('constant float4& u [[buffer(0)]]', 'constant float4& v [[buffer(0)]]'),
           9: lambda: full.replace('vertex main0_out main0_z(', 'vertex float4 main0_z('),
           13: lambda: full + '\nstatic int dm_wl_in(int x) { return x; }\n'}
    return bad[k]() if k in bad else full
def vsunit_case():
    """vs_unit on the build's source and its pulling and shadow-input variants (none, the variant, the variant with another body, the source itself); a share of them refused"""
    src = EN.vs_init(vsrc(types=[VR.choice(['float4', 'uint4', 'int4']) for _ in range(VR.randrange(1, 4))], mask=VR.choice(['none', 'none', 'arg', 'lit'])))
    tf, two, k = VR.random() < 0.2, VR.random() < 0.15, VR.randrange(0, 30)
    full = unit_full(src, tf, two, k)
    pk = VR.choice(['none', 'variant', 'variant']) if k != 10 else 'edited'
    vk = VR.choice(['none', 'variant', 'variant']) if k not in (11, 12) else ('edited' if k == 11 else 'full')
    pv = None if pk == 'none' else P.variant(full)[0]
    vv = None if vk == 'none' else V.variant(full)[0]
    if pv and pk == 'edited': pv = pv.replace('float4 acc = u;', 'float4 acc = u + 1.0;')
    if vv and vk == 'edited': vv = vv.replace('float4 acc = u;', 'float4 acc = u + 1.0;')
    if vk == 'full': vv = full
    r = _verr(lambda: list(S.vs_unit(full, pv, vv)))
    ST['vsunit ok' if 'ok' in r else 'vsunit refused'] += 1
    return {'msl': src, 'tf': tf, 'two': two, 'k': k, 'full': dj(full), 'pulled': pk, 'shadowed': vk, 'expect': r}
OUT['vsunit'] = [vsunit_case() for _ in range(N(160))]
def vsentries_case():   # vs_entries on a vs_unit case's source with an entry point that never closes after it, cut short
    i = VR.randrange(len(OUT['vsunit'])); c = OUT['vsunit'][i]
    tail = VR.choice(['', '\nvertex main0_out main0_q(int a)\n{\n    return {};'])
    m = unit_full(c['msl'], c['tf'], c['two'], c['k']) + tail
    n = len(m) if VR.random() < 0.5 else VR.randrange(len(m) // 2, len(m) + 1)
    return {'case': i, 'tail': tail, 'len': n, 'expect': _verr(lambda: {k: [v['ret'], v['params'], v['start'], v['body'], v['end']] for k, v in S.vs_entries(m[:n]).items()})}
OUT['vsentries'] = [vsentries_case() for _ in range(N(60))]
json.dump(OUT, sys.stdout, separators=(',', ':'))
print(json.dumps({'cases': {k: len(v) for k, v in OUT.items()}, 'exercised': dict(ST), 'binding kinds': sorted(KINDS)}), file=sys.stderr)
