"""regen_cases.py <engine dir (holding dm/)> <tests/data/tessmesh_cases.json>: recompute every case's Python result ('py': `status same len crc32 meta`, 'msg') with the engine's own
functions (tools/difftest/stepdiff.py's tessmesh steps), and (re)add the synthetic payload-spill cases: task / mesh shaders whose payload exceeds Metal's 16 KB (dmsh_meshstage.payload_spill),
with and without the mesh shader's stage-input array (the translator's dm_stage_in), each of to_stage's ValueErrors, and memory's layouts of a task shader and a spilled task-fed mesh shader.
The inputs of the other cases are kept as they are; prints how many results changed.
usage: PYTHONDONTWRITEBYTECODE=1 python3 -I regen_cases.py <engine> crates/ibuilder-shader/tests/data/tessmesh_cases.json"""
import json, os, random, struct, sys, tempfile
eng, path = sys.argv[1], sys.argv[2]
sys.argv = [sys.argv[0], eng]
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'difftest'))
import stepdiff as sd  # noqa: E402
dms = sd.dms
NOTE = 'payload spill'


def result(step, text, args, tmp, n):
    a = dict(args)
    if 'spv' in a:
        p = os.path.join(tmp, f'{n}.spv'); open(p, 'wb').write(bytes.fromhex(a['spv'])); a['spv'] = p
    if 'gs' in a:
        p = os.path.join(tmp, f'{n}.gs.metal'); open(p, 'w', encoding='utf-8', newline='').write(a['gs']); a['gs'] = p
    line, _ = sd.one((str(n), step, '', a), text)
    c = line.split('\t')
    return '\t'.join(c[1:6]), c[6]


HEAD = '''#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

template<typename T, size_t Num>
struct spvUnsafeArray
{
    T elements[Num ? Num : 1];

    thread T& operator [] (size_t pos) thread { return elements[pos]; }
    constexpr const thread T& operator [] (size_t pos) const thread { return elements[pos]; }

    device T& operator [] (size_t pos) device { return elements[pos]; }
    constexpr const device T& operator [] (size_t pos) const device { return elements[pos]; }

    threadgroup T& operator [] (size_t pos) threadgroup { return elements[pos]; }
    constexpr const threadgroup T& operator [] (size_t pos) const threadgroup { return elements[pos]; }
};

struct B { uint _m0[1]; };

'''


def kernel(words, stage_in=None, body=None, helper=''):
    sin = f'    threadgroup spvUnsafeArray<uint, {stage_in[0]}> {stage_in[1]};\n' if stage_in else ''
    body = body if body is not None else ('    uint v = _24[1u] + _24[0u] + _24[gl_LocalInvocationID.x + 2u];\n    _23[20u] = v;\n' if stage_in else '    _23[20u] = 1u;\n')
    return (HEAD + helper + 'kernel void main0(device B& buf [[buffer(3)]], uint3 gl_LocalInvocationID [[thread_position_in_threadgroup]], uint3 gl_WorkGroupID [[threadgroup_position_in_grid]])\n{\n'
            f'    threadgroup spvUnsafeArray<uint, {words}> _23;\n' + sin + body +
            '    _23[4u] = 3u;\n    _23[5u] = buf._m0[0];\n    if (gl_LocalInvocationID.x != 0u)\n    {\n        return;\n    }\n    _23[6u] = gl_WorkGroupID.x;\n}\n\n')


def ms_info(payload, task, threads=32, max_v=64, max_p=126):
    return {'kind': 'ms', 'threads': threads, 'payload': payload, 'task': task, 'verts': 3, 'max_v': max_v, 'max_p': max_p,
            'vertex': [[None, 0], [None, 1], [None, 2], [None, 3], [3, 0], [3, 1]], 'primitive': [], 'primitive_system': 0, 'primitive_builtins': []}


def words_of(info):
    return dms.memory(sd._tuples(info))[1] // 4


def spill_cases():
    out = []
    def add(note, info, text, reads=None):
        out.append({'step': 'mesh_to_stage', 'in': text, 'args': {'info': info, 'reads': reads}, 'note': f'{NOTE}: {note}'})
    for p in (16384, 16385, 28000, 65535):   # a task shader: its payload in a slice of the spill buffer past 16 KB
        i = {'kind': 'ts', 'threads': 32, 'payload': p}
        add(f'ts {p}', i, kernel(words_of(i)))
    i = {'kind': 'ts', 'threads': 64, 'payload': 21508}; add('ts 21508, 64 threads', i, kernel(words_of(i)))
    for p in (16384, 16385, 21508, 65535):   # a task-fed mesh shader reading its input through dm_stage_in
        i = ms_info(p, True); sw = dms.payload_words(sd._tuples(i))
        add(f'ms task {p} with dm_stage_in', i, kernel(words_of(i), (sw, '_24') if p > 16384 else None))
    i = ms_info(21508, True); sw = dms.payload_words(sd._tuples(i)); w = words_of(i)
    add('ms task without dm_stage_in', i, kernel(w))
    add('ms task, dm_stage_in of the wrong size', i, kernel(w, (sw + 4, '_24')))
    add('ms task, dm_stage_in passed to a helper', i, kernel(w, (sw, '_24'), helper='static inline __attribute__((always_inline)) uint f(threadgroup spvUnsafeArray<uint, 5380>& a, uint k) { return a[k]; }\n\n',
                                                         body='    _23[20u] = f(_24, 1u);\n'))
    add('ms task, dm_stage_in passed with a space', i, kernel(w, (sw, '_24'), body='    _23[20u] = g(1u, _24 );\n'))
    add('ms task, an element of dm_stage_in passed', i, kernel(w, (sw, '_24'), body='    _23[20u] = g(_24[1u], 2u) + h(_24 [3u]);\n'))
    add('ms task, a member and a longer name passed', i, kernel(w, (sw, '_24'), body='    _23[20u] = g(s._24, _245) + _24[0u];\n'))
    add('ms task, dm_stage_in not after the stage memory', i, kernel(w, None, body=f'    uint pre = 0u;\n    threadgroup spvUnsafeArray<uint, {sw}> _24;\n    _23[20u] = _24[1u] + pre;\n'))
    add('ms without a task, its payload past 16 KB', ms_info(21508, False), kernel(words_of(ms_info(21508, False)), (5380, '_24')))
    i = ms_info(21508, True, threads=32, max_v=256, max_p=256); sw = dms.payload_words(sd._tuples(i))
    add('ms task with a device stage slice', i, kernel(words_of(i), (sw, '_24')))
    i = ms_info(30000, True, threads=64); sw = dms.payload_words(sd._tuples(i))
    add('ms task 30000, 64 threads, reads', i, kernel(words_of(i), (sw, '_24')), reads={'3': 15, '7': 3})
    return out


def info_cases():   # memory()'s layouts: a task shader has no input area, a spilled task-fed mesh shader's outputs follow word 0's block too
    rng, out = random.Random(20261123), []
    for kind, payload, task in (('ts', 0, 0), ('ts', 100, 0), ('ts', 16384, 0), ('ts', 16385, 0), ('ts', 40000, 0), ('ms', 16384, 1), ('ms', 16385, 1), ('ms', 21508, 1), ('ms', 21508, 0), ('ms', 65535, 1)):
        ctl = bytearray(rng.randbytes(0x100)); ctl[0x56] = 6 if kind == 'ts' else 7
        struct.pack_into('<2H', ctl, 0x58, 32, payload); ctl[0x5e] = task; ctl[0x5f] = 3; struct.pack_into('<2H', ctl, 0x60, 64, 126); ctl[0x68] = 0
        area = bytearray(rng.randbytes(0x200))
        for o in range(0x34, 0x80): area[o] = rng.choice([0, 0, 0x0f, 0x30, 0xf0, 0x01])
        area[0x80:0x200] = bytes(0x180)
        out.append({'step': 'mesh_info', 'in': '', 'args': {'ctl': ctl.hex(), 'area': area.hex(), 'code': bytes(area[0x80:]).hex()}, 'note': f'{NOTE}: info {kind} {payload} task {task}'})
    return out


cases = [c for c in json.load(open(path, encoding='utf-8')) if not str(c.get('note', '')).startswith(NOTE)]
cases += spill_cases() + info_cases()
changed = 0
with tempfile.TemporaryDirectory() as tmp:
    for n, c in enumerate(cases):
        py, msg = result(c['step'], c['in'], c['args'], tmp, n)
        if (c.get('py'), c.get('msg')) != (py, msg): changed += 1
        c['py'], c['msg'] = py, msg
json.dump(cases, open(path, 'w', encoding='utf-8'), ensure_ascii=False, separators=(',', ':'))
print(f'{len(cases)} cases, {changed} results new or changed', file=sys.stderr)
for c in cases:
    if str(c.get('note', '')).startswith(NOTE): print(c['note'], '->', c['py'][:3], c['msg'][:90], file=sys.stderr)
