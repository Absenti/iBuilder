#!/usr/bin/env python3
"""fuzz.py <out dir> [--seed N] [--n N] [--steps a,b,...]: synthetic and mutated MSL for the per-step differential test. For each step a generator builds documents from the step's own
idiom (line templates written from its docstring and regexes, so that every branch can fire), composes them with distractor lines in random order and indentation, and sometimes
damages them (one to three character / line / token mutations, truncation, bracket imbalance) so that the ValueError paths and the near-misses of every regex are exercised.
The Python is the oracle: no expected output is written here. Output: <out>/msl/<n>.msl and <out>/cases.tsv (id, step, path, args)."""
import json, os, random, sys

rng = random.Random(1)
def ch(*a): return rng.choice(a)
def pick(seq): return rng.choice(list(seq))
def maybe(p=0.5): return rng.random() < p
def ind(n=None): return ' ' * (rng.choice([0, 4, 4, 4, 8, 2]) if n is None else n)
def tmp(): return f'_{rng.randint(1, 60)}'

DISTRACT = [
    'float4 _{n} = float4(1.0, 2.0, 3.0, 4.0);', 'uint _{n} = uint(_{m});', 'bool _{n} = _{m} < _{k};', 'int _{n} = _{m} + 1;', 'out.gl_Position = float4(_{n});',
    'if (_{n})', '{', '}', 'else', '    _{n} = _{m};', 'float _{n} = as_type<float>(_{m});', 'threadgroup_barrier(mem_flags::mem_threadgroup);', '// a comment _{n}',
    'return out;', 'uint _{n} = _{m} >> uint(2);', 'int _{n} = _{m} & 1;', 'bool _{n} = _{m} != 0;', 'int _{n} = _{m} ? (-1) : 0;', '_{n} = _{m} + _{k};', 'x = _{n};',
    'using namespace metal;', 'struct main0_out', 'fragment main0_out main0(main0_in in [[stage_in]])', 'kernel void main0(uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]])',
]
def distractor():
    t = rng.choice(DISTRACT)
    for key in ('{n}', '{m}', '{k}'): t = t.replace(key, str(rng.randint(1, 80)))
    return t

def mutate(text, strength=2):
    for _ in range(rng.randint(1, strength)):
        if not text: break
        k = rng.randint(0, 9)
        i = rng.randrange(len(text) + 1)
        if k == 0: text = text[:i] + text[i + 1:]                                   # drop a char
        elif k == 1: text = text[:i] + rng.choice(' ;(){}[]<>=,._0123456789abcxyzu') + text[i:]   # insert a char (never a newline: spirv-cross writes one declaration per line, line-wise damage is k == 2, 3, 4, 6)
        elif k == 2:                                                                  # drop a line
            ls = text.split('\n')
            if ls: del ls[rng.randrange(len(ls))]
            text = '\n'.join(ls)
        elif k == 3:                                                                  # duplicate a line
            ls = text.split('\n')
            if ls: j = rng.randrange(len(ls)); ls.insert(j, ls[j])
            text = '\n'.join(ls)
        elif k == 4: text = text[:i]                                                  # truncate
        elif k == 5:                                                                  # change a digit
            ds = [j for j, c in enumerate(text) if c.isdigit()]
            if ds: j = rng.choice(ds); text = text[:j] + str((int(text[j]) + rng.randint(1, 9)) % 10) + text[j + 1:]
        elif k == 6:                                                                  # swap two lines
            ls = text.split('\n')
            if len(ls) > 1: a, b = rng.randrange(len(ls)), rng.randrange(len(ls)); ls[a], ls[b] = ls[b], ls[a]
            text = '\n'.join(ls)
        elif k == 7:                                                                  # collapse or add whitespace
            text = text.replace('  ', ' ', 1) if maybe() else text[:i] + '  ' + text[i:]
        elif k == 8: text = text.replace('\n', '\r\n', 1)
        else:                                                                         # swap a token for another
            toks = [t for t in ('uint', 'int', 'float', 'bool', 'thread', 'device', 'threadgroup', 'constant') if t in text]
            if toks: t = rng.choice(toks); text = text.replace(t, rng.choice(['uint', 'int', 'float', 'half', 'ulong']), 1)
    return text

def shuffle_in(core, n_extra=None):
    """core lines with distractors in between, in the original order"""
    out = []
    for l in core:
        for _ in range(rng.randint(0, 2) if n_extra is None else n_extra):
            out.append(ind() + distractor())
        out.append(l)
    return out

GENS = {}
def gen(name):
    def deco(f): GENS[name] = f; return f
    return deco

# ----------------------------------------------------------------------------------------------------------------------------- bit0_mask
@gen('bit0_mask')
def g_bit0(args):
    def func():
        n = rng.randint(1, 3); lines = []
        for _ in range(n):
            a, b, m = tmp(), tmp(), tmp()
            ty = ch('int', 'uint')
            x = ch('as_type<int>(_3)', '_9', 'in.x', '(_4 + _5)', '_4 | _5', '(_4 | _5)', '_4 || _5', 'foo(_1, _2)', '_1 ? _2 : _3', 'a, b', '(a ? b : c)', '(_4 && _5)', 'x & y', '((_1))', '_4 ^ _5', '(_1 + (_2 | _3))')
            lines += [ind() + f'{ty} {a} = {x} & 1{ch("", "u", "", "")};']
            if maybe(0.9): lines += [ind() + f'bool {b} = {a} != 0{ch("", "u", "")};']
            if maybe(0.9): lines += [ind() + f'int {m} = {b} ? (-1) : 0;']
            if maybe(0.15): lines += [ind() + ch(f'{a} = 3;', f'{a} += 1;', f'{b} = true;', f'{a}++;', f'++{a};', f'foo(&{a});', f'{b}--;', f'{a} <<= 1;', f'{a} == 3;', f'{a} <= 3;', f'if ({a} != {b}) x = 1;', f'{a} -= 1;')]
            if maybe(0.15): lines += [ind() + f'int {tmp()} = {b} ? (-1) : 0;']
        lines = shuffle_in(lines)
        return '\n'.join(lines)
    parts = ['using namespace metal;', 'struct main0_out', '{', '    float4 gl_Position [[position]];', '};', '']
    for _ in range(rng.randint(1, 3)):
        parts += [ch('void f()', 'kernel void main0()', 'static inline int g(int x)'), '{', func(), '}' if maybe(0.93) else '  }', '']
    return '\n'.join(parts) + ('\n' if maybe(0.9) else '')


# ----------------------------------------------------------------------------------------------------------------------------- shared real-shaped fragments
SPV_ARRAY = """template<typename T, size_t Num>
struct spvUnsafeArray
{
    T elements[Num ? Num : 1];
    
    thread T& operator [] (size_t pos) thread
    {
        return elements[pos];
    }
    constexpr const thread T& operator [] (size_t pos) const thread
    {
        return elements[pos];
    }
    
    device T& operator [] (size_t pos) device
    {
        return elements[pos];
    }
    threadgroup T& operator [] (size_t pos) threadgroup
    {
        return elements[pos];
    }
};
"""
PRELUDE = '#pragma clang diagnostic ignored "-Wmissing-prototypes"\n\n#include <metal_stdlib>\n#include <simd/simd.h>\n\nusing namespace metal;\n\n'

def vs_text(extra_body=(), params=None, point_size=True, stores=True, layer=False, user_outs=3):
    outs = ''.join(f'    float4 m_{20 + i} [[user(locn{i})]];\n' for i in range(user_outs))
    st = 'struct main0_out\n{\n' + outs + '    float4 gl_Position [[position, invariant]];\n' + ('    float gl_PointSize [[point_size]];\n' if point_size else '') + ('    uint gl_Layer [[render_target_array_index]];\n' if layer else '') + '};\n\n'
    if params is None:
        params = ch('constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint gl_VertexIndex [[vertex_id]]',
                    'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint gl_VertexIndex [[vertex_id]], uint gl_InstanceIndex [[instance_id]]',
                    'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint dmv [[vertex_id]], float4 in_a [[attribute(1)]], uint dmi [[instance_id]]',
                    'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]',
                    'uint gl_VertexIndex [[vertex_id]]', '')
    body = ['    main0_out out = {};']
    if point_size: body.append('    out.gl_PointSize = 1.0;')
    body += list(extra_body)
    if stores: body += [f'    out.m_{20 + i}.{c} = _{rng.randint(1, 90)};' for i in range(user_outs) for c in 'xyzw' if maybe(0.5)]
    body += ['    out.gl_Position.x = _50;', '    out.gl_Position.w = 1.0;']
    if maybe(0.3): body += ['    if (_77)', '    {', '        return out;', '    }']
    body += ['    return out;']
    fn = f'vertex main0_out main0({params})\n{{\n' + '\n'.join(body) + '\n}\n'
    return PRELUDE + 'struct spvDescriptorSetBuffer0\n{\n    constant _16* vp_c0 [[id(0)]];\n};\n\n' + st + fn

# ----------------------------------------------------------------------------------------------------------------------------- vs_base_instance
BI = '(*spvDescriptorSet0.vp_c0)._m0[100].y'
@gen('vs_base_instance')
def g_vsbi(args):
    body = []
    for _ in range(rng.randint(0, 3)):
        body.append(f'    float _{rng.randint(1, 90)} = {ch(BI, "fma(" + BI + ", 2.0, 1.0)", "(*spvDescriptorSet0.vp_c0)._m0[100].x", "(*spvDescriptorSet0.vp_c0)._m0[101].y", BI + BI)};')
    params = ch(None, None, None, 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint gl_VertexIndex [[vertex_id]], uint gl_BaseInstance [[base_instance]]',
                'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant (*x)(int) y [[buffer(3)]]', 'a((b)) [[x((1))]]', 'a (b [[c(1)]], d [[e(2)]])', 'float4 x [[attribute(0)]]')
    t = vs_text(body, params)
    if maybe(0.3): t = t.replace('vertex main0_out main0(', ch('vertex  main0_out  main0 (', 'vertex\nmain0_out\nmain0\t(', 'vertex void main0(', 'vertex main0_out main0_z(', 'xvertex main0_out main0('), 1)
    if maybe(0.3): t = t + '\n' + t.split('struct main0_out')[0][-0:] + vs_text(body, params).split('};\n\n')[-1].replace('main0(', 'main0_z(')
    return t

# ----------------------------------------------------------------------------------------------------------------------------- drop_point_size
@gen('drop_point_size')
def g_dps(args):
    body = []
    for _ in range(rng.randint(0, 3)):
        body.append(ch('    _88(out.gl_Position, out.gl_PointSize);', '    out.gl_PointSize = 2.0;', '    out.gl_PointSize = _5;', '    foo(out.gl_PointSize);', '    float _9 = out.gl_PointSize;', '    out.gl_PointSize = fma(a, b, c);', '    out.gl_PointSize = 1.0; out.gl_PointSize = 2.0;'))
    t = vs_text(body, point_size=maybe(0.5))
    if maybe(0.3): t = t.replace('main0_out out = {};', 'main0_out out = {};\n    out.gl_PointSize = 1.0;', 1)
    if maybe(0.03): t += '\nstatic inline void h(thread main0_out& out)\n{\n    foo(out.gl_PointSize);\n}\n'   # a use outside any vertex function: the engine's loop never ends (a timeout here)
    if maybe(0.2): t = ch('static void k(thread float& p)\n{\n    p = 1.0;\n}\n\n', 'vertex void other(int a)\n{\n    p = 1;\n}\n') + t
    if maybe(0.2): t = t + '\n' + vs_text(body, point_size=True).split('};\n\n', 1)[-1].replace('main0(', 'main1(')
    return t

# ----------------------------------------------------------------------------------------------------------------------------- vs_tf
@gen('vs_tf')
def g_vstf(args):
    n_out = rng.randint(0, 4)
    body = []
    t = vs_text(body, user_outs=rng.randint(1, 4))
    tf = []
    for b in range(4):
        if maybe(0.5): tf.append([] if maybe(0.7) else None); continue
        outs = [[rng.randint(0, 5), rng.randint(0, 4)] for _ in range(rng.randint(0, 3))]
        tf.append({'stride': 4 * sum(n for _, n in outs) + (4 if maybe(0.1) else 0), 'outputs': outs})
    if maybe(0.1): tf = tf[:rng.randint(0, 3)]
    args['tf'] = tf
    if maybe(0.1): t = t.replace('return out;', 'return out;\n    // return out;', 1)
    return t

# ----------------------------------------------------------------------------------------------------------------------------- first_store
@gen('first_store')
def g_first(args):
    two_d = maybe(0.4)
    gx, gy, gz, lx, ly = [tmp() for _ in range(5)]
    n = [f'_{rng.randint(100, 200) + i}' for i in range(20)]
    sx, sy = rng.choice([0, 1, 2, 3, 4, 5, 6, 7, 8, 30, 62]), rng.choice([0, 1, 2, 3, 5])   # local sizes that fit an i64 (1 << 63 is a JSON number serde_json reads as u64)
    sp = lambda: ch('\n    ', '\n    ', '\n', ' ', '\n\n    ')
    if not two_d:
        guard = n[5]
        head = [f'float {n[0]} = as_type<float>(gl_WorkGroupID.x);', f'float {n[1]} = as_type<float>(gl_LocalInvocationID.x);', f'int {n[2]} = as_type<int>({n[0]}) << {sx};',
                f'int {n[3]} = ~as_type<int>({n[1]});', f'int {n[4]} = {n[3]} + 1;', f'bool {n[5]} = {n[4]} != {n[2]};']
        args['local'] = [1 << sx if maybe(0.9) else ch(64, 8), 1 if maybe(0.92) else 2, 1]
        g = f'!{guard}'
    else:
        head = [f'float {n[0]} = as_type<float>(gl_WorkGroupID.y);', f'float {n[1]} = as_type<float>(gl_LocalInvocationID.y);', f'float {n[2]} = as_type<float>(gl_LocalInvocationID.x);',
                f'float {n[3]} = as_type<float>(gl_WorkGroupID.x);', f'float {n[4]} = as_type<float>(gl_WorkGroupID.z);',
                f'int {n[5]} = as_type<int>({n[0]}) << {sy};', f'int {n[6]} = ~as_type<int>({n[1]});', f'int {n[7]} = {n[6]} + 1;', f'bool {n[8]} = {n[7]} != {n[5]};',
                f'int {n[9]} = as_type<int>({n[3]}) << {sx};', f'int {n[10]} = ~as_type<int>({n[2]});', f'int {n[11]} = {n[10]} + 1;', f'bool {n[12]} = {n[11]} == {n[9]};',
                f'bool {n[13]} = !{n[8]};', f'bool {n[14]} = {n[12]} && {n[13]};', f'bool {n[15]} = ' + ch(f'as_type<int>({n[4]}) == 0', f'0 == as_type<int>({n[4]})') + ';',
                f'bool {n[16]} = {n[15]} && {n[14]};']
        opt = maybe(0.5)
        if opt: head.append(f'bool {n[17]} = !{n[16]};')
        g = f'!{n[17]}' if opt else n[16]
        args['local'] = [1 << sx if maybe(0.9) else ch(8, 16), 1 << sy if maybe(0.9) else 4, 1]
    if maybe(0.06): args['local'] = args['local'][:rng.randint(1, 2)] if maybe(0.5) else args['local'][:2] + [2]
    stores = []
    plan = ([0] if maybe(0.5) else []) + [1] * rng.choice([1, 1, 1, 2, 2, 3, 5, 1]) + [2] * rng.choice([0, 0, 1, 2]) + ([3] if maybe(0.3) else [])
    if maybe(0.7): plan.sort(key=lambda x: x if x != 2 else 0.5)
    else: rng.shuffle(plan)
    for k in plan:   # mostly the idiom's own lines, now and then a stopper
        if k == 0:
            v = f'_{rng.randint(300, 320)}'
            stores += [f'int {v} = _{rng.randint(1, 9)};', f'if ({g})', '{', f'    {v} = {rng.randint(0, 9)};', '}', f'int _{rng.randint(321, 340)} = {v};']
            if maybe(0.5): stores[-1] = f'int {n[18]} = {v};'
        elif k == 1:
            items = [f'(*spvDescriptorSet0.cp_s{rng.randint(0, 2)})._m0[{rng.randint(0, 9)}] = ' + ch(f'{rng.randint(0, 9)}u', f'{rng.randint(0, 9)}u', f'{rng.randint(0, 9)}u', f'uint({n[18]})' if 0 in plan else f'{rng.randint(0, 9)}u', f'uint({tmp()})' if maybe(0.1) else '0u', f'{rng.randint(0, 9)}' if maybe(0.1) else '3u') + ';' for _ in range(rng.randint(1, 3))]
            stores += [f'if ({g if maybe(0.95) else "!" + tmp()})', '{'] + ['    ' + i for i in items] + ['}']
        elif k == 2: stores += [f'int _{rng.randint(341, 360)} = {ch("_5 + 1", "_6", "0", "foo(1, 2)", "_5 ? 1 : 2")};']
        else: stores += [ch(f'float _{rng.randint(1, 9)} = 1.0;', 'out.x = 1;', f'if ({g}) x = 1;')]
    body = [sp().join(head) if maybe(0.3) else '\n    '.join(head)] + ['\n    '.join(stores) if maybe(0.4) else sp().join(stores)]
    pre = ['    float _1 = 2.0;'] * rng.randint(0, 2)
    t = PRELUDE + 'kernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint3 gl_WorkGroupID [[threadgroup_position_in_grid]], uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]])\n{\n' + '\n'.join(pre) + '\n    ' + '\n    '.join(body) + '\n    float _99 = 1.0;\n}\n'
    if maybe(0.04): t = t + t.split('kernel')[1]
    return t

# ----------------------------------------------------------------------------------------------------------------------------- tg_spill
@gen('tg_spill')
def g_tgspill(args):
    decls = []
    big = maybe(0.6)
    for i in range(rng.randint(0, 4)):
        elem = ch('uint', 'uint', 'int', 'float', 'float4', 'uint2', 'half', 'half2', 'char', 'uchar', 'ushort', 'uint3', 'foo', 'ulong')
        n = rng.choice([1, 4, 64, 1000, 3200, 8192, 12288, 12288, 16384]) if big else rng.choice([1, 4, 64, 100, 1000])
        form = rng.randint(0, 3)
        ind_ = ch('    ', '    ', '\t', '')
        name = f'_{33 + 2 * i}'
        decls.append(ind_ + ch(f'threadgroup spvUnsafeArray<{elem}, {n}> {name};', f'threadgroup spvUnsafeArray<{elem}, {n}> {name};', f'threadgroup {elem} {name}[{n}];', f'threadgroup {elem} {name};', f'threadgroup spvUnsafeArray<{elem}, {n}> {name}[{rng.randint(1, 3)}];', f'threadgroup spvUnsafeArray<{elem}, {n}> {name}; ', f'threadgroup  uint {name};') + ch('', '', ' ', '\t'))
    sig = ch('constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint3 gl_WorkGroupID [[threadgroup_position_in_grid]], uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]]',
             'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]]', '', 'uint3 g [[threadgroup_position_in_grid]]',
             'constant (*f)(int) a [[buffer(1)]]')
    body = []
    for _ in range(rng.randint(0, 4)):
        body.append(ch('    threadgroup_barrier(mem_flags::mem_threadgroup);', '    threadgroup_barrier(mem_flags::mem_device | mem_flags::mem_threadgroup);', '    threadgroup_barrier(mem_flags::mem_threadgroup | mem_flags::mem_device);',
                       '    uint _5 = atomic_fetch_min_explicit((threadgroup atomic_uint*)&_33[1], _977, memory_order_relaxed);', '    // threadgroup memory',
                       '    threadgroup uint* p = &_33[0];', '    threadgroupx y;', '    x = mem_flags::mem_threadgroup_imageblock;', '    f(mem_flags::mem_threadgroup, mem_flags::mem_threadgroup);', '    a = threadgroup\r', '    float _7 = 1.0;'))
    helper = ''
    if maybe(0.4): helper = ch('static inline void h(threadgroup uint& x)\n{\n    x = 1u;\n}\n\n', 'static void w(threadgroup spvUnsafeArray<uint, 4>& a, threadgroup_barrier(mem_flags::mem_threadgroup)\n{\n}\n\n')
    pre = PRELUDE + ('' if maybe(0.15) else SPV_ARRAY + '\n') + helper
    t = pre + f'kernel void main0({sig})\n{{\n' + '\n'.join(decls[:2] + body + decls[2:]) + '\n    float _9 = 1.0;\n}\n'
    if maybe(0.1): t = t.replace('kernel void main0', 'kernel   void\tmain0 ', 1)
    if maybe(0.08): t = 'threadgroup uint _early;\n' + t
    return t


# ----------------------------------------------------------------------------------------------------------------------------- frag_z_snap
@gen('frag_z_snap')
def g_zsnap(args):
    shared = maybe(0.15)
    fc = ch('float4 gl_FragCoord [[position]]', 'float4 gl_FragCoord [[position]]', 'float4 gl_FragCoord', 'float4  gl_FragCoord [[position]]', 'float3 gl_FragCoord [[position]]', 'xfloat4 gl_FragCoord [[position]]')
    params = ', '.join(rng.sample(['main0_in in [[stage_in]]', fc, 'texture2d<float> t0 [[texture(0)]]', 'depth2d<float> d0 [[texture(1)]]', 'sampler s0 [[sampler(0)]]', 'constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]]'], rng.randint(2, 6)))
    if maybe(0.85) and fc not in params: params += ', ' + fc
    names = iter(range(10, 200))
    body = []; zs = []; xs = []; others = []
    depth = 0
    def new(): return f'_{next(names)}'
    def xdecl():
        v = new(); body.append(ind(4) + f'float {v} = ' + ch('t0.read(uint2(1, 2), 0u).x', 'd0.read(uint2(1, 2)).x', 'd0.sample(s0, float2(0.5, 0.5))', 't0.sample(s0, float2(1.0)).x', 't0.sample(s0, float2(1.0)).x', 't0.sample_compare(s0, float2(1.0), 0.5)', 'in.m_20.x') + ';'); xs.append(v)
    def zdecl():
        v = new(); body.append(ind(4) + f'{ch("float", "float", "float", "const float", "int")} {v} = gl_FragCoord.z;'); zs.append(v)
    def derived():
        if zs and maybe(0.6): v = new(); body.append(ind(4) + f'float {v} = ' + ch('{a} * {b}', '{a} + 1.0', 'fma({a}, 2.0, 1.0)', '-{a}', '{a}').format(a=pick(zs), b=pick(zs + xs + [tmp()])) + ';'); zs.append(v)
        elif xs: v = new(); body.append(ind(4) + f'float {v} = ' + ch('{a} * {b}', '{a} + {b}', '{a}', 'min({a}, {b})').format(a=pick(xs), b=pick(xs + [tmp()])) + ';'); xs.append(v)
    def compare():
        if not (zs and xs): return
        v = new(); a, b = pick(zs), pick(xs)
        if maybe(0.5): a, b = b, a
        body.append(ind(4) + f'{ch("bool", "bool", "bool", "bool", "int")} {v} = {a} {ch("<", ">", "<=", ">=", "==", "!=", "<", ">")} {b};'); others.append(v)
    def opener():
        nonlocal depth
        body.append(ind(4) + 'if (' + (pick(others) if others else '_5') + ')'); body.append(ind(4) + '{'); depth += 1
    def closer():
        nonlocal depth
        if depth: body.append(ind(4) + '}'); depth -= 1
    plan = [xdecl] * rng.randint(0, 2) + [opener] * rng.choice([0, 0, 1]) + [zdecl] * rng.randint(0, 2) + [derived] * rng.randint(0, 3) + [xdecl] * rng.choice([0, 0, 1]) + [closer] * rng.choice([0, 0, 1]) + [compare] * rng.randint(0, 3) + [lambda: body.append(ind(4) + distractor())] * rng.randint(0, 3)
    if maybe(0.25): rng.shuffle(plan)
    for f in plan: f()
    body += [ind(4) + '}'] * depth
    if maybe(0.5): body.append('    gl_FragCoord.z;')
    entry = f'fragment main0_out main0({params})\n{{\n    main0_out out = {{}};\n' + '\n'.join(body) + '\n    return out;\n}\n'
    if shared: entry = f'static __attribute__((always_inline)) main0_out dm_body_main0({params})\n{{\n    main0_out out = {{}};\n' + '\n'.join(body) + '\n    return out;\n}\n\n' + f'fragment main0_out main0({params})\n{{\n    main0_out out = dm_body_main0(in);\n    return out;\n}}\n'
    pre = '' if maybe(0.1) else PRELUDE
    return pre + 'struct main0_out\n{\n    float4 m_color [[color(0)]];\n};\n\n' + entry

# ----------------------------------------------------------------------------------------------------------------------------- pool_arrays
@gen('pool_arrays')
def g_pool(args):
    mem = []; uses = []
    aid = 0
    for i in range(rng.randint(1, 6)):
        kind = rng.randint(0, 7)
        n = rng.choice([1, 2, 8, 32, 32, 33, 33, 64, 1024, 4096])
        if kind == 0: name, ty = ch('fp_t_cb0_40000000', 'fp_t2d_cb0_40000000', 'vp_t3d_cb0_40000000'), ch('texture2d<float>', 'texture2d_array<float>', 'depth2d<float>', 'texture3d<float>')
        elif kind == 1: name, ty = ch('fp_s_cb0_40000000', 'fp_s2_cb0_40000000'), 'sampler'
        elif kind == 2: name, ty = ch('fp_t2d_cb7_2A', 'fp_t2d_cb196614_10570000', 'fp_t_cb7_2A', 'fp_t2d_cb196610_10570000', 'fp_t_cb7_20000002A', 'fp_t2d_cb7_FFFFFFFFF'), 'texture2d<float>'
        elif kind == 3: name, ty = ch('fp_t2d_cb7_2ASmplr', 'fp_t2d_cb196614_10570000Smplr'), 'sampler'
        elif kind == 4: name, ty = ch('fp_t2d_cb2_0', 'fp_t_cb40_1', 'fp_t2d_cb2_80', 'cp_t2d_cb0_40000000'), 'texture2d<float>'
        elif kind == 5: name, ty = f'fp_s{i}', None
        elif kind == 6: name, ty = f'fp_c{i}', None
        else: name, ty = ch('fp_t2d_cb0_40000000', 'fp_t2d_cb7_2A'), ch('texture2d<float>', 'sampler')
        name = name + (f'_{i}' if maybe(0.1) else '')
        if ty is None:
            mem.append(f'    {ch("constant", "device")} _{20 + i}* {name} [[id({aid})]];'); aid += 1; continue
        if maybe(0.15): mem.append(f'    {ty} {name} [[id({aid})]];'); aid += 1; continue
        mem.append(f'{ind(4)}array<{ty}, {n}> {name} [[id({aid})' + ch('', '', ', raster_order_group(0)') + ']];'); aid += n
        if n >= 33 or maybe(0.3): uses.append((name, ty))
    struct = 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(mem) + '\n};\n\n'
    body = []
    for name, ty in uses:
        for _ in range(rng.randint(0, 3)):
            idx = ch('_5', 'uint(_6)', 'int(_7 + 1)', 'min(_8, 3u)', '_9[2]', 'f(a[1], b[2])', '(_5)')
            base = ch('spvDescriptorSet0', 'spvDescriptorSet0', 'x.spvDescriptorSet0', 'a.b')
            if ty == 'sampler' or maybe(0.2): body.append(f'    float4 _{rng.randint(1, 90)} = {base}.{name}[{idx}];')
            else: body.append(f'    float4 _{rng.randint(1, 90)} = {base}.{name}[{idx}].sample(spvDescriptorSet0.s[{idx}], float2(0.5));')
        if maybe(0.07): body.append(f'    auto p = &spvDescriptorSet0.{name}; // other use')
        if maybe(0.04): body.append(f'    float4 _1 = spvDescriptorSet0.{name}[_5;')
        if maybe(0.05): body.append(f'    float4 _2 = spvDescriptorSet0.{name}[spvDescriptorSet0.{name}[1].x];')
        if maybe(0.03): body.append(f'    mark({name}, {name}_x);')
    extras = [ch('    discard_fragment();', '    _1 = _2.write(float4(1), uint2(0));', '    atomic_fetch_add_explicit(&x, 1u, memory_order_relaxed);', '    uint o [[sample_mask]];', '    float d [[ depth(any) ]];', '    float s [[stencil]];', '\n    device uint* p;', 'device float* q;', '    foo_explicit(1);', '    atomic_x_explicit(1);') for _ in range(rng.choice([0, 0, 0, 1, 2]))]
    frag = ch('fragment main0_out main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])', '[[early_fragment_tests]] fragment main0_out main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])', 'kernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])', 'fragment main0_out main0_x(int a)', 'fragment\tmain0_out main0(int)')
    text = PRELUDE + struct + 'struct main0_out\n{\n    float4 c [[color(0)]];\n};\n\n' + frag + '\n{\n' + '\n'.join(body + extras) + '\n}\n'
    if maybe(0.08): text = text.replace('struct spvDescriptorSetBuffer0', 'struct spvDescriptorSetBuffer0  ', 1)
    if maybe(0.08): text += '\n' + frag + '\n{\n}\n'
    return text

# ----------------------------------------------------------------------------------------------------------------------------- wide_loads
@gen('wide_loads')
def g_wide(args):
    bufs = ['fp_s3', 'vp_s0', 'cp_s1', 'fp_s12']
    ro = rng.sample(bufs, rng.randint(0, 3))
    decls = [f'    {ch("const device", "const device", "device", "constant")} _{ch("23", "25")}* {b} [[id({i})]];' for i, b in enumerate(bufs) if b in ro or maybe(0.3)]
    struct = 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(decls) + '\n};\n\n' if maybe(0.97) else ''
    lines = []
    names = iter(range(100, 900))
    def n(): return f'_{next(names)}'
    base = n()
    lines.append(f'    int {base} = int(_5);')
    for blk in range(rng.randint(1, 4)):
        buf = pick(bufs); root = ch(base, base, n())
        for _ in range(rng.randint(1, 8)):
            off = rng.choice([0, 4, 8, 12, 16, 20, 32, 36, 4, 8, -4, -16, 2, 6, 12])
            s = n(); cur = root
            if off or maybe(0.3):
                lines.append(ind(4) + f'int {s} = {cur} + {off};'); cur = s
                if maybe(0.15):
                    s2 = n(); lines.append(ind(4) + f'int {s2} = {cur} + {rng.choice([4, -4, 8])};'); cur = s2
            ix = n(); lines.append(ind(4) + f'uint {ix} = uint(int(uint({cur}) >> uint(2)));')
            if maybe(0.75): lines.append(ind(4) + f'float {n()} = as_type<float>((*spvDescriptorSet0.{buf})._m0[int({ix})]);')
            elif maybe(0.5):
                sg, cond, ins = n(), n(), n()
                lines.append(ind(4) + f'int {sg} = int({ix});')
                lines.append(ind(4) + f'bool {cond} = (uint({sg}) + (*spvDescriptorSet0.cp_c27)._m0[{rng.randint(0, 3)}].{ch("x", "y")}) < (*spvDescriptorSet0.cp_c27)._m0[{rng.randint(0, 3)}].{ch("z", "w")};')
                lines.append(ind(4) + f'float {n()} = as_type<float>(select(0u, (*spvDescriptorSet0.{buf})._m0[{cond} ? {sg} : 0], {cond}));')
            else: lines.append(ind(4) + f'float {n()} = as_type<float>((*spvDescriptorSet0.{buf})._m0[int({ix})]) + as_type<float>((*spvDescriptorSet0.{buf})._m0[int({ix})]);')
            if maybe(0.12): lines.append(ind(4) + ch('(*spvDescriptorSet0.fp_s3)._m0[int(_5)] = 1u;', 'atomic_fetch_add_explicit(&x, 1, y);', 'threadgroup_barrier(mem_flags::mem_device);', '_77 += 1;', 'x = 1;', 'if (_5)', '{', '}', '*p = 3u;', 'img.write(a, b);', 'a[1] <<= 2;', '_5 == 3;'))
    t = PRELUDE + struct + 'kernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])\n{\n' + '\n'.join(lines) + '\n}\n'
    if maybe(0.05): t = t.replace(' >> uint(2)', ' >> uint(3)', 1)
    return t

# ----------------------------------------------------------------------------------------------------------------------------- mul_extended
HELPER_MUL = 'template<typename T, typename U, typename V>\n[[clang::optnone]] T spvMulExtended(V l, V r)\n{\n    return T{U(l * r), U(mulhi(l, r))};\n}\n'
@gen('mul_extended')
def g_mul(args):
    parts = [PRELUDE]
    for _ in range(rng.choice([0, 1, 1, 1, 1, 2])):
        h = HELPER_MUL
        if maybe(0.2): h = h.replace('\n', ch(' ', '\n\n', '\n  ', '\t'), rng.randint(1, 3))
        if maybe(0.1): h = h.rstrip('\n')
        parts.append(h + '\n')
    if maybe(0.1): parts.append('template<typename T, typename U> static inline __attribute__((always_inline)) T spvMulExtended(uint l, uint r) { return T{}; }\n')
    parts.append('kernel void main0()\n{\n' + '\n'.join(f'    uint2 _{i} = spvMulExtended<uint2, uint>(_5, 3u);' for i in range(rng.randint(0, 3))) + '\n}\n')
    return ''.join(parts)

# ----------------------------------------------------------------------------------------------------------------------------- float_add_msl
@gen('float_add_msl')
def g_fadd(args):
    out = [PRELUDE, 'kernel void main0()\n{\n']
    for _ in range(rng.randint(1, 3)):
        ind_ = ' ' * rng.choice([0, 4, 4, 8]); ids = rng.sample(range(10, 99), 9)
        again, old, summ, want, seen, found = [f'_{i}' for i in ids[:6]]
        addr = ch('(*spvDescriptorSet0.cp_s0)._m0[int(_5)]', 'x[1]', '', 'a(b)', 'q);z(')
        addr2 = addr if maybe(0.85) else addr + 'x'
        valtxt = ch('_5', 'fma(a, b, c)', 'x.y', '1.0')
        sumline = ch(f'{old} + {valtxt}', f'{old} + {valtxt}', f'spvFAdd({old}, {valtxt})', f'spvFAdd({old}, {valtxt}))', f'{old} - {valtxt}', f'{old} + ')
        L = [f'bool {again};', 'do', '{', f'    float {old} = as_type<float>({addr});', f'    float {summ} = {sumline};', f'    uint {want} = as_type<uint>({old});', f'    uint {seen};', '    do', '    {', f'        {seen} = {want};',
             f'    }} while (!atomic_compare_exchange_weak_explicit((device atomic_uint*)&{addr2}, &{seen}, as_type<uint>({summ}), memory_order_relaxed, memory_order_relaxed) && {seen} == {want});',
             f'    uint {found} = {seen};', f'    {again} = int({found}) != as_type<int>({old});', f'}} while ({again});']
        if maybe(0.1): del L[rng.randrange(len(L))]
        if maybe(0.1): L[rng.randrange(len(L))] += ' '
        if maybe(0.08): L.insert(rng.randrange(len(L)), '// x')
        out.append('\n'.join(ind_ + l for l in L) + ('\n' if maybe(0.95) else ''))
        out.append(distractor() + '\n')
    out.append('}\n')
    return ''.join(out)

# ----------------------------------------------------------------------------------------------------------------------------- gather_offsets
@gen('gather_offsets')
def g_gather(args):
    arrays = []; calls = []
    quad = [(0, 0), (1, 0), (0, 1), (1, 1)]
    for i in range(rng.randint(0, 3)):
        name = f'_{20 + i}'
        pts = rng.sample(quad, 4) if maybe(0.5) else [(rng.randint(-3, 3), rng.randint(-3, 3)) for _ in range(4)]
        bx, by = rng.randint(-3, 3), rng.randint(-3, 3)
        if maybe(0.6): pts = [(x + bx, y + by) for x, y in pts]
        offs = [f'int2({x}, {y})' if maybe(0.85) else (f'int2({x})' if maybe(0.5) else f'int2( {x} ,{y} )') for x, y in pts]
        if maybe(0.05): offs = offs[:3]
        if maybe(0.05): offs[0] = 'int2(f(1), 2)'
        arrays.append(f'constant spvUnsafeArray<int2, 4> {name} = spvUnsafeArray<int2, 4>({{ ' + ', '.join(offs) + ' });')
    wrappers = ''
    if maybe(0.6): wrappers = f'template<typename T> static inline vec<float, 4> spvGatherConstOffsets(const thread texture2d<float>& t, sampler s, float2 c, int comp, constant T& off)\n{{\n    return t.gather(s, c);\n}}\n' + ('template<typename T> static inline float spvGatherCompareConstOffsets(const thread depth2d<float>& t, sampler s, float2 c, float z, constant T& off)\n{\n    return 0.0;\n}\n' if maybe(0.5) else '')
    for i in range(rng.randint(1, 4)):
        arr = pick([a.split()[3] for a in arrays] + ['_99']) if arrays else '_99'
        cmp = maybe(0.3)
        arr = arr if maybe(0.93) else '_99'
        if cmp: call = f'spvGatherCompareConstOffsets(spvDescriptorSet0.t{i}, spvDescriptorSet0.s{i}, {arr}, {ch("in.uv", "float2(1.0)", "uv", "f(a, b), 2")}, {ch("0.5", "_7")})'
        else: call = f'spvGatherConstOffsets(spvDescriptorSet0.t{i}, spvDescriptorSet0.s{i}, {arr}, {ch("1", "int(_7)", "0")}, {ch("in.uv", "float2(1.0)", "f(a, b)", "in.uv, 2")})'
        calls.append(ch(f'    float4 _{30 + i} = {call};', f'    float4 _{30 + i} = {call}.x;', f'    {call};', f'    float4 _{30 + i} = {call.replace("spvDescriptorSet0", "d", 1)};', f'    float4 _{30 + i} = h({call}, {call});'))
    arrays_txt = '\n'.join(arrays)
    t = (PRELUDE if maybe(0.95) else '') + SPV_ARRAY + wrappers + arrays_txt + '\nkernel void main0()\n{\n' + '\n'.join(calls) + '\n}\n'
    if maybe(0.04): t = t.replace('GatherConstOffsets', 'GatherConstOffsetsx', 1)
    return t

# ----------------------------------------------------------------------------------------------------------------------------- shared_store_msl
@gen('shared_store_msl')
def g_sstore(args):
    parts = [PRELUDE if maybe(0.9) else '']
    for i in range(rng.choice([0, 1, 1, 2, 3])):
        bits = ch('8', '16', '8', '16', '8', '16', '16', '8', '32') if maybe(0.97) else '8'
        space = ch('threadgroup', 'device', 'threadgroup', 'constant') if maybe(0.9) else 'threadgroup'
        good = maybe(0.85)
        p = [ch('thread const uint& a', 'thread const int& a') if good else ch('thread const uint& a', 'thread uint& a', 'const uint& a', 'thread const uint &a'), ch('thread const uint& b', 'thread const int& b'), f'{space} spvUnsafeArray<uint, {ch("64", "1024", "4")}>& m']
        if maybe(0.1): p = p[:2]
        if maybe(0.05): p.append('uint z')
        if maybe(0.05): p.reverse()
        sep = ch(', ', ', ', ',', ' ,')
        sep = sep if good else ch(', ', ',', ' ,')
        parts.append(f'\nvoid dmsh_shared_store{bits}_{rng.randint(0, 40)}({sep.join(p)})\n{ch("{", "{", "{", "{", "{", " {")}\n    uint w = 1;\n    {ch("while (true) { }", "m[0] = 1u;", "if (a) { m[1] = 2u; }")}\n{ch("}", "}", "}", "}", "}", "")}\n')
    parts.append('\nkernel void main0()\n{\n    dmsh_shared_store8_1(1, 2, x);\n}\n')
    return ''.join(parts)

# ----------------------------------------------------------------------------------------------------------------------------- per_vertex
@gen('per_vertex')
def g_pv(args):
    ins = []
    locs = rng.sample(range(0, 5), rng.randint(0, 3))
    for l in locs:
        for v in rng.sample([0, 1, 2], rng.randint(1, 3)):
            loc = 64 + 3 * l + v if maybe(0.95) else 64 + 3 * l + v + 1
            ins.append(f'    float4 dm_pv{l}_{v} [[user(locn{loc}){ch("", ", flat", ", flat")}]];')
    ordinary = [f'    float4 m_{i} [[user(locn{i})]];' for i in rng.sample(range(0, 6), rng.randint(0, 3))]
    members = ordinary + ins
    if maybe(0.15): rng.shuffle(members)
    st = 'struct main0_in\n{\n' + '\n'.join(members) + '\n};\n\n'
    uses = [f'    float4 _{i} = in.dm_pv{l}_{v};' if maybe(0.5) else f'    float _{i} = in.dm_pv{l}_{v}.x + in.dm_pv{l}_{v}.y;' for i, (l, v) in enumerate([(l, v) for l in locs for v in (0, 1, 2)][:6])]
    t = PRELUDE + (st if maybe(0.95) else '') + 'struct main0_out\n{\n    float4 c [[color(0)]];\n};\n\nfragment main0_out main0(main0_in in [[stage_in]])\n{\n' + '\n'.join(uses) + '\n}\n'
    if maybe(0.05): t = t.replace('\nstruct main0_in', '\n// c\nstruct main0_in', 1)
    if maybe(0.08): t = t + '\n' + st
    return t

# ----------------------------------------------------------------------------------------------------------------------------- wide_atomics
@gen('wide_atomics')
def g_watom(args):
    names = rng.sample(['img_rg32ui', 'fp_i_cb7_2A_rg32ui', 'a_rg32ui', 'b_rg32ui'], rng.randint(1, 2)) if maybe(0.9) else ['rg32ui_x', '_rg32ui']
    decls = [f'    {ch("texture2d", "texture2d_array", "texture3d", "texture2d", "texture1d", "texture2d_array")}<uint, {ch("access::read_write", "access::read_write", "access::read", "access::write")}>{ch(" ", " ", "  ", "\t")}{n} [[id({i})]];' for i, n in enumerate(names)]
    body = []
    for n in names:
        for _ in range(rng.choice([0, 1, 1, 2])):
            op = ch('max', 'min')
            coords = ch('uint2(_5, _6)', 'uint3(_5, _6, _7)', 'int2(1), 0', 'x')
            expr = ch(f'spvDescriptorSet0.{n}', f'spvDescriptorSet0.{n}', n, f'a.b.{n}', f'x{n}')
            pre = ch('', '', 'uint _9 = ', 'uint _x = ', 'int _4 = ')
            post = ch('', '', '.x', '.y')
            lo = f'{ind(4)}{pre}{expr}.atomic_fetch_{op}({coords}, {ch("_11", "_12", "uint(_3)")}){post};'
            hi = f'{ind(4)}{pre}{expr}.atomic_fetch_{op if maybe(0.95) else "min"}({coords if maybe(0.92) else "other"}, {ch("_13", "uint(_4)")}){post};'
            if maybe(0.1): lo = lo.rstrip(';') + ';  '
            body += [lo, hi] if maybe(0.93) else [lo]
        if maybe(0.3): body.append(f'    uint _20 = {n}.get_width();')
        if maybe(0.15): body.append(f'    uint _21 = {ch("spvDescriptorSet0.", "")}{n}.get_array_size() + {n}.get_height();')
        if maybe(0.1): body.append(f'    uint2 _22 = {n}.read(uint2(1)).xy;')
        if maybe(0.05): body.append(f'    {n}.write(uint4(1), uint2(1));')
        if maybe(0.05): body.append(f'    {n}.atomic_fetch_add(uint2(1), 1u);')
    t = PRELUDE + 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(decls) + '\n};\n\nkernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])\n{\n' + '\n'.join(body) + '\n}\n'
    if maybe(0.04): t += 'device atomic_uint* x_atomic;\n'
    return t


# ----------------------------------------------------------------------------------------------------------------------------- hmma_msl
HMMA_PARAMS = ['lane', 'blk', 'a0', 'a1', 'a2', 'a3', 'b0', 'b1', 'acc0', 'acc1', 'fl', 'aaddr', 'ao0', 'ao1', 'ao2', 'ao3', 'astr']
@gen('hmma_msl')
def g_hmma(args):
    ret = ch('uint', 'uint', 'int', 'float')
    pnames = [f'p_{n}' for n in HMMA_PARAMS]
    plain = [f'thread const uint& {n}' for n in pnames] + [f'thread {ch("uint", "int", "float")}& p_d1']
    if maybe(0.04): plain.pop(rng.randrange(len(plain)))
    if maybe(0.03): plain[-1] = 'thread const uint& p_d1'
    scr = f'threadgroup spvUnsafeArray<uint, {ch(256, 4096)}>& p_scr'
    params = plain + [scr] if maybe(0.8) else [scr] + plain
    if maybe(0.03): params.append(f'threadgroup spvUnsafeArray<uint, 4>& extra')
    sep = ', '
    pre = ch('', 'static inline __attribute__((always_inline))\n', '')
    helper = f'\n{pre}{ret} dmsh_hmma16816_f16({sep.join(params)})\n{ch("{", "{", "{", " {")}\n    return 0u;\n}}\n'
    if maybe(0.02): helper = helper.replace('\n{', '\n{\n    int x = 1;\n    if (x) {', 1)
    funcs = []
    nfun = rng.choice([1, 1, 1, 2, 3])
    names = iter(range(100, 4000))
    def n(): return f'_{next(names)}'
    arrays_decl = []
    for fi in range(nfun):
        scr_name = f'_scr{fi}'
        lines = [f'    threadgroup spvUnsafeArray<uint, 4096> {scr_name};']
        if maybe(0.6): lines.append(f'    threadgroup spvUnsafeArray<uint, {ch(1024, 2112, 64)}> _sh{fi};')
        if maybe(0.15): lines.append(f'    threadgroup spvUnsafeArray<uint, 32> _sh{fi}b;')
        if maybe(0.1): lines.append('    threadgroup uint foo;')
        lines.append(f'    uint _lane = uint(gl_LocalInvocationID.x);')
        # B values: declared once at the top level, or inside a block, or written twice, or copies
        bs = []
        for _ in range(rng.randint(1, 3)):
            b0, b1 = n(), n()
            how = rng.randint(0, 9)
            if how <= 4: lines += [f'    uint {b0} = as_type<uint>(_src);', f'    uint {b1} = {ch(f"int({b0})", f"uint({b0})", "as_type<uint>(_src2)", "7u", "uint(3)", "_src3")};']
            elif how == 5: lines += [f'    uint {b0} = _x;', f'    {b0} = 3u;', f'    uint {b1} = 2u;']
            elif how == 6: lines += ['    if (_c)', '    {', f'        uint {b0} = 1u;', f'        uint {b1} = 2u;']; lines.append('    }') if maybe(0.9) else None
            elif how == 7: lines += [f'    uint {b0} = as_type<uint>(_src);', f'    uint {b1} = {b0};', f'    foo({b0}, &{b1});']
            elif how == 8: lines += [f'    const uint {b0} = {ch("as_type<uint>(_s)", "_t")};', f'    uint {b1} = {ch("as_type<float>(" + b0 + ")", "float(" + b0 + ")", b0 + " + 1")};']
            else: lines += [f'    uint {b0} = {ch("0", "5", "_k")};', f'    uint {b1} = {b0}{ch("", "++", " += 1")};']
            bs.append((b0, b1))
        for _ in range(rng.randint(0, 5)):
            b0, b1 = pick(bs) if maybe(0.7) else (n(), n())
            d1 = n() if maybe(0.9) else '_d1fixed'
            cargs = [f'{ch("_lane", "_l2")}', ch('_blk', '0u', f'{rng.randint(0, 8)}u'), n(), n(), n(), n(), b0, b1, n(), n(), ch('3', str(rng.randint(0, 255)), '_flags'), ch('_aaddr', '0'), n(), n(), n(), n(), ch('_astr', '32u'), d1]
            scr_arg = scr_name if maybe(0.93) else '_sh0'
            call = f'dmsh_hmma16816_f16({", ".join(cargs)}, {scr_arg})'
            indent = '    ' if maybe(0.85) else '        '
            if indent == '        ': lines += ['    if (_c)', '    {']
            lines += [f'    uint {d1};' if maybe(0.5) else f'    uint {d1}x = 0u;', indent + ch(f'uint {n()} = {call};', f'{call};', f'float {n()} = as_type<float>({call});')]
            if indent == '        ': lines.append('    }')
            if maybe(0.05): lines.append(f'    {n()} = {d1} + 1u;')
        lines.append('}')
        if maybe(0.1): lines[-1] = ''
        funcs.append(f'kernel void main{fi}(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]])\n{{\n' + '\n'.join(lines) + '\n')
    writer = ''
    if maybe(0.4): writer = ch('static void wr(thread uint& x, thread const uint& y)\n{\n    x = y;\n}\n', 'static void wr2(thread spvUnsafeArray<uint, 4>& x)\n{\n}\n', 'void wr3(thread const uint& x) {\n}\n')
    t = PRELUDE + SPV_ARRAY + '\n' + helper + writer + ('\nstruct spvDescriptorSetBuffer0\n{\n    int x [[id(0)]];\n};\n\n') + '\n'.join(funcs)
    if maybe(0.03): t += '\n' + f'{ret} dmsh_hmma16816_f16(foo)\n{{\n}}\n'
    return t


# ----------------------------------------------------------------------------------------------------------------------------- tf_layout
@gen('tf_layout')
def g_tflayout(args):
    ctl = bytearray(rng.choice([0x700, 0x700, 0x700, 0x6e8, 0x6ec, 0x1000, 0x6e4, 0x100]))
    for b in range(4):
        if maybe(0.5): continue
        count = rng.randint(1, 4) if maybe(0.9) else rng.choice([0, 129, 255])
        locs = [rng.randint(0x20, 0x3f) if maybe(0.97) else rng.randint(0, 255) for _ in range(count)]
        sizes = [rng.randint(1, 4) if maybe(0.97) else rng.randint(0, 6) for _ in range(count)]
        stride = 4 * sum(sizes) if maybe(0.93) else rng.randint(0, 100)
        if len(ctl) >= 0x6e4: ctl[0x6dc + 2 * b:0x6dc + 2 * b + 2] = (stride % 65536).to_bytes(2, 'little')
        if len(ctl) > 0x6e8 + b: ctl[0x6e8 + b] = count % 256
        a0 = 0xdc + 0x80 * b; a1 = 0x4dc + 0x80 * b
        for i in range(min(count, 128)):
            if a0 + i < len(ctl): ctl[a0 + i] = locs[i]
            if a1 + i < len(ctl): ctl[a1 + i] = sizes[i]
    return ctl.hex()

# ----------------------------------------------------------------------------------------------------------------------------- pool_arrays: pools handed to functions, buffer texture pools
POOLS = [('fp_t_cb0_40000000', 'texture2d<float>', 1048576), ('fp_s_cb0_40000000', 'sampler', 4096), ('fp_tb_cb0_40000000', 'texture_buffer<float>', 1048576),
         ('fp_t2d_cb7_2A', 'texture2d<float>', 2048), ('fp_tb_cb7_2A', 'texture_buffer<uint>', 2048)]
def g_pool_callable(args):
    pools = rng.sample(POOLS, rng.randint(1, 3))
    mem = [f'    array<{ty}, {n}> {nm} [[id({i})]];' for i, (nm, ty, n) in enumerate(pools)]
    if maybe(0.5): mem.append(f'    const device _12* fp_s0 [[id({len(pools)})]];')
    struct = 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(mem) + '\n};\n\n'
    def use(nm, ty, base, idx):
        if 'texture_buffer' in ty:
            return ch(f'{base}{nm}[{idx}].read(uint({idx}))', f'{base}{nm}[{idx}].get_width()', f'{base}{nm}[{idx}].read(uint({base}{nm}[_3].get_width()))',
                      f'{base}{nm}[{idx}].sample(s, 0.5)' if maybe(0.1) else f'{base}{nm}[ {idx} ].read(uint(f(_1, (_2))))', f'{base}{nm}[{idx}].read(1u' if maybe(0.05) else f'{base}{nm}[{idx}].read(2u)')
        if ty == 'sampler': return f'{base}{nm}[{idx}]'
        return f'{base}{nm}[{idx}].sample(s, float2(0.5))'
    helpers, calls = [], []
    for k in range(rng.randint(0, 3)):
        nm, ty, n = pick(pools)
        pty = ch(f'constant array<{ty}, {n}>& {nm}', f'constant array<{ty}, {n}> & {nm}', f'constant array<{ty},{n}>&{nm}', f'constant array<{ty}, {n + 1}>& {nm}' if maybe(0.1) else f'constant array<{ty}, {n}>& {nm}',
                 f'device array<{ty}, {n}>& {nm}' if maybe(0.05) else f'constant array<{ty}, {n}>& {nm}', f'constant array<float, {n}>& {nm}' if maybe(0.05) else f'constant array<{ty}, {n}>& {nm}')
        params = [pty] + ([ch('uint i', 'thread float2& uv', 'sampler s')] if maybe(0.7) else [])
        rng.shuffle(params)
        body = [f'    auto x{j} = {use(nm, ty, "", ch("i", "0u", "(i + 1u)", "uint(x)"))};' for j in range(rng.randint(1, 3))]
        if maybe(0.1): body.append(f'    auto p = &{nm}[0];')
        if maybe(0.1): body.append(f'    auto q = {nm};')
        if maybe(0.15): body.append(f'    // {nm} in a comment ("{nm}")')
        if maybe(0.2) and helpers: body.append(f'    h{k - 1}({", ".join(nm if "array" in x else ("i" if "uint" in x else ("uv" if "uv" in x else "s")) for x in helpers[-1][1])});')
        name = f'h{k}'
        helpers.append((name, params, body))
        cargs = [('spvDescriptorSet0.' if maybe(0.85) else 'x.') + nm if 'array' in x else ch('_5', '0u', 'uv', 's') for x in params]
        if maybe(0.1): cargs = cargs[:-1]
        calls.append(f'    auto r{k} = {name}({", ".join(cargs)});')
    text = PRELUDE
    pre = maybe(0.05)
    for name, params, body in helpers:
        h = f'static inline __attribute__((always_inline))\nfloat4 {name}({", ".join(params)})\n{{\n' + '\n'.join(body) + '\n    return float4(0.0);\n}\n\n'
        if pre: text += h
    text += struct
    if not pre:
        for name, params, body in helpers: text += f'static inline __attribute__((always_inline))\nfloat4 {name}({", ".join(params)})\n{{\n' + '\n'.join(body) + '\n    return float4(0.0);\n}\n\n'
    main = [f'    auto u{j} = {use(nm, ty, "spvDescriptorSet0.", ch("_5", "uint(_6)", "_7[2]", "f(a[1], b[2])"))};' for j, (nm, ty, n) in enumerate(pools) if maybe(0.8)]
    if maybe(0.05): main.append(f'    auto e = &spvDescriptorSet0.{pools[0][0]}[1];')
    text += ch('fragment main0_out main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])', 'kernel void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])') + '\n{\n' + '\n'.join(calls + main) + '\n}\n'
    if maybe(0.05): text = text.replace('struct spvDescriptorSetBuffer0', '// struct spvDescriptorSetBuffer0', 1)
    return text
_g_pool_plain = GENS['pool_arrays']
GENS['pool_arrays'] = lambda args: g_pool_callable(args) if maybe(0.5) else _g_pool_plain(args)

# ----------------------------------------------------------------------------------------------------------------------------- constant_reads
@gen('constant_reads')
def g_constant_reads(args):
    stage = ch('fp', 'fp', 'vp', 'cp')
    bufs = [f'{stage if maybe(0.85) else ch("fp", "vp", "cp")}_s{i}' for i in range(rng.randint(1, 4))]
    mem = []
    for i, b in enumerate(bufs):
        mem.append(ch(f'    const device _{20 + i}* {b} [[id({i})]];', f'    const device _{20 + i}* {b} [[id({i})]];', f'    device _{20 + i}* {b} [[id({i})]];', f'    const device _{20 + i}*  {b} [[id({i})]];'))
    body = []
    for b in bufs:
        for _ in range(rng.randint(0, 4)):
            k = rng.randint(0, 9)
            if k <= 3: body.append(f'    uint _{rng.randint(1, 99)} = (*spvDescriptorSet0.{b})._m0[{ch("_5", "int(_6)", "0", "_7 ? _8 : 0")}];')
            elif k == 4: body.append(f'    packed_uint{ch(2, 3, 4)} _{rng.randint(1, 99)} = *(const device packed_uint{ch(2, 3, 4)}*)&(*spvDescriptorSet0.{b})._m0[int(_9)];')
            elif k == 5: body.append(f'    uint _{rng.randint(1, 99)} = as_type<uint>(select(0u, (*spvDescriptorSet0.{b})._m0[_10 ? _11 : 0], _10));')
            elif k == 6: body.append(f'    device uint* p = &{ch("", " ", "(", " ( ")}(*spvDescriptorSet0.{b})._m0[1];')
            elif k == 7: body.append(f'    x = spvDescriptorSet0.{b};')
            elif k == 8: body.append(f'    (*spvDescriptorSet0.{b})._m0[2] = 1u;')
            else: body.append(f'    // {b}')
    if maybe(0.1): body.append(ch('    device uint& r = x;', '    f(device uint & r);', '    thread device uint &q;'))
    args['read_only'] = sorted(rng.sample(range(5), rng.randint(0, 4)))
    args['mode'] = ch('fp', 'fp', 'fp', 'all', 'vp,fp', ' cp , fp', '', '0', 'off', 'cp')
    return PRELUDE + 'struct spvDescriptorSetBuffer0\n{\n' + '\n'.join(mem) + '\n};\n\n' + ch('fragment', 'vertex', 'kernel') + ' void main0(constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])\n{\n' + '\n'.join(body) + '\n}\n'

# ----------------------------------------------------------------------------------------------------------------------------- compute_loops
def loop_stmt(depth=0):
    k = rng.randint(0, 9)
    body = ['        x += 1u;'] + ([loop_stmt(depth + 1)] if depth < 2 and maybe(0.3) else []) + (['        threadgroup_barrier(mem_flags::mem_threadgroup);'] if maybe(0.15) else []) + \
           (['        arrive();'] if maybe(0.1) else []) + ([f'        y = helper{rng.randint(0, 2)}(x);'] if maybe(0.3) else []) + (['        // while (1) in a comment'] if maybe(0.1) else [])
    b = '\n'.join(body)
    if k <= 2: return f'    for (uint i = 0u; i < {ch("n", "helper0(n)", "16u", "")}; i++)\n    {{\n{b}\n    }}'
    if k == 3: return f'    for (;;) x++;'
    if k == 4: return f'    while ({ch("x < n", "helper1(x) != 0u", "true", "f(x, (y))")})\n    {{\n{b}\n    }}'
    if k == 5: return f'    do\n    {{\n{b}\n    }} while ({ch("x < n", "helper0(x) < 4u", "false")});'
    if k == 6: return f'    while (x < n) x = x * 2u;'
    if k == 7: return f'    for (int j = 0; j < 4; j++) {{ x += uint(j); }}'
    if k == 8: return f'    do {{ x++; }} while(x < 3u);'
    return f'    for (uint i = 0u; i < 4u; i++)\n    {{\n        spvMat<uint>(x, i);\n    }}'
@gen('compute_loops')
def g_compute_loops(args):
    helpers = []
    for h in range(3):
        body = [loop_stmt(1) if maybe(0.4) else '    x += 2u;'] + ([f'    return helper{h + 1}(x);'] if h < 2 and maybe(0.3) else ['    return x;'])
        helpers.append(f'static inline __attribute__((always_inline))\nuint helper{h}({ch("uint x", "uint x", "thread uint& x", "uint x, device uint* q")})\n{{\n' + '\n'.join(body) + '\n}\n')
    helpers.append('static inline __attribute__((always_inline))\nvoid arrive()\n{\n    threadgroup_barrier(mem_flags::mem_threadgroup);\n}\n')
    helpers.append('template<typename T>\nT spvMat(T a, uint b)\n{\n    for (uint k = 0u; k < b; k++) a += T(k);\n    return a;\n}\n')
    rng.shuffle(helpers)
    main = [loop_stmt() for _ in range(rng.randint(0, 3))] + [f'    uint z = helper{rng.randint(0, 2)}(x);' for _ in range(rng.randint(0, 2))] + ['    uint s = "for (";' if maybe(0.05) else '    a.buf[0] = x;']
    head = ch('kernel void main0(constant Args& a [[buffer(0)]], uint tid [[thread_index_in_threadgroup]])', 'kernel void main0()', 'kernel void main0(device uint* b [[buffer(28)]])' if maybe(0.1) else 'kernel void main0(constant Args& a [[buffer(0)]])',
              'fragment void main0()' if maybe(0.1) else 'kernel  void  main0 (uint x)')
    brace = '\n{\n' if maybe(0.93) else ' {\n'
    ns = 'using namespace metal;\n' if maybe(0.95) else 'using namespace metal; '
    return '#include <metal_stdlib>\n' + ns + 'struct Args { device uint* buf [[id(0)]]; };\n\n' + '\n'.join(helpers) + '\n' + head + brace + '    uint x = 0u, y = 0u, n = 8u;\n' + '\n'.join(main) + '\n}\n'

# ----------------------------------------------------------------------------------------------------------------------------- raw_noinline / mesh_subgroup_noinline
@gen('raw_noinline')
def g_raw_noinline(args):
    out = [PRELUDE]
    for i in range(rng.randint(1, 4)):
        attr = ch('static inline __attribute__((always_inline))', 'static __attribute__((noinline))', 'static inline', 'static  inline __attribute__((always_inline))')
        name = ch('dmsh_raw_find', f'dmsh_raw_find_{i}', 'dmsh_raw_findx', 'dmsh_raw_lookup', 'dm_raw_find')
        out.append(f'{attr}\n{ch("uint", "int2", "void", "spvUnsafeArray<uint, 2>")} {name}({ch("uint a", "")})\n{{\n    return 0u;\n}}\n')
    args['stage'] = ch('cs', 'ms', 'ts', 'vs', 'ps')
    return '\n'.join(out)

@gen('mesh_subgroup_noinline')
def g_mesh_subgroup(args):
    out = [PRELUDE]
    for i in range(rng.randint(1, 4)):
        attr = ch('static inline __attribute__((always_inline))', 'static inline __attribute__((always_inline))', 'static __attribute__((noinline))', '  static inline __attribute__((always_inline))')
        params = ch('thread uint& gl_SubgroupInvocationID, thread uint& gl_SubgroupSize', 'thread const uint& x, thread uint& gl_SubgroupInvocationID', 'uint gl_SubgroupID', 'thread uint4& gl_SubgroupEqMask',
                    'thread uint& x', 'thread uint& gl_Subgroup', 'thread uint& my_gl_SubgroupSize')
        out.append(f'{attr}\n{ch("uint", "void", "uint4")} helper{i}({params})\n{{\n    return {ch("gl_SubgroupInvocationID", "0u", "x")};\n}}\n')
    out.append('kernel void main0(uint gl_SubgroupInvocationID [[thread_index_in_simdgroup]])\n{\n    helper0(gl_SubgroupInvocationID);\n}\n')
    return '\n'.join(out)

# ----------------------------------------------------------------------------------------------------------------------------- driver
def main():
    out = sys.argv[1]
    a = sys.argv[2:]
    seed = int(a[a.index('--seed') + 1]) if '--seed' in a else 1
    n = int(a[a.index('--n') + 1]) if '--n' in a else 500
    only = a[a.index('--steps') + 1].split(',') if '--steps' in a else list(GENS)
    rng.seed(seed)
    os.makedirs(out + '/msl', exist_ok=True)
    cases = []; k = 0
    if os.path.exists(out + '/cases.tsv'): cases = [l for l in open(out + '/cases.tsv').read().split('\n') if l]; k = len(cases)
    res_by_stage = {}
    if '--chain-res' in a:   # also run every document through the whole chain, as a shader of a random stage, with the answer json of a real shader of that stage
        cap = a[a.index('--chain-res') + 1]
        for key in sorted(os.listdir(cap))[::7]:
            if os.path.isfile(f'{cap}/{key}/res.json'): res_by_stage.setdefault(json.load(open(f'{cap}/{key}/job.json'))['stage'], []).append(os.path.abspath(f'{cap}/{key}/res.json'))
    for step in only:
        g = GENS[step]
        for i in range(n):
            args = {}
            text = g(args)
            if maybe(0.35) and step != 'tf_layout': text = mutate(text, 3)
            p = f'{os.path.abspath(out)}/msl/{k}.msl'
            open(p, 'w', newline='').write(text)
            cases.append(f'{k}\t{step}\t{p}\t{json.dumps(args)}'); k += 1
            if res_by_stage:
                st = ch('ps', 'vs', 'cs') if not step.startswith('vs_') else 'vs'
                if step in ('first_store', 'tg_spill', 'hmma_msl', 'shared_store_msl', 'mul_extended', 'float_add_msl', 'wide_atomics'): st = ch('cs', 'cs', 'cs', 'ps')
                if step in ('frag_z_snap', 'pool_arrays', 'per_vertex'): st = ch('ps', 'ps', 'ps', 'cs')
                ca = {'stage': st, 'local': args.get('local') or ([64, 1, 1] if maybe(0.5) else [maybe(0.5) and 8 or 16, 8, 1]), 'tf': args.get('tf') if st == 'vs' and maybe(0.5) else None, 'res': pick(res_by_stage[st]),
                      'dual_want': maybe(0.3), 'partner': (rng.sample(range(0, 8), rng.randint(0, 5)) if maybe(0.5) else None)}
                cases.append(f'{k}\tchain\t{p}\t{json.dumps(ca)}'); k += 1
    open(out + '/cases.tsv', 'w').write('\n'.join(cases) + '\n')
    print(len(cases), 'cases', file=sys.stderr)
if __name__ == '__main__': main()
