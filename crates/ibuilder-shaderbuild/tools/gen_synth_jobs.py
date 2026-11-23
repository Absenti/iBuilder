"""Differential oracle for `job_for` on records Grizzy does not contain: VertexA/B vertex shaders, transform feedback, early fragment tests, domain shaders, headered compute,
Switch 2 (NVN2) records, and malformed ones. The Python functions are EXTRACTED from the engine's dmsh_build.py (ast), not retyped, run on seeded synthetic records, and
every result (job text, code bytes, c1, or the sys.exit message, and the [RED] lines printed) is written for the Rust test `tests/jobs_synthetic.rs`.
usage: python -I gen_synth_jobs.py <engine tools dir (dm/gfx/tools)> <out dir>"""
import ast, io, contextlib, json, os, random, struct, sys, tempfile, shutil
TOOLS, OUT = sys.argv[1], sys.argv[2]
sys.path.insert(0, TOOLS)
import dmsh_ampere, dmsh_tess, dmsh_meshstage, dmsh_abi as abi
src = open(os.path.join(TOOLS, 'dmsh_build.py')).read()
tree = ast.parse(src)
want = {'vertex_ab', 'early_fragment_tests', 'ps_omap', 'job_for', 'tf_layout'}
nodes = [n for n in tree.body if (isinstance(n, ast.FunctionDef) and n.name in want) or (isinstance(n, ast.Assign) and any(getattr(t, 'id', '') in ('NVN2_STAGE_INDEX', 'TF_SLOT') for t in n.targets))]
assert len([n for n in nodes if isinstance(n, ast.FunctionDef)]) == len(want), 'a function moved'
ns = {'os': os, 'struct': struct, 'sys': sys, 'json': json, 'dmsh_ampere': dmsh_ampere, 'dmsh_tess': dmsh_tess, 'dmsh_meshstage': dmsh_meshstage, 'abi': abi, 'FACTS': {}}
exec(compile(ast.Module(body=nodes, type_ignores=[]), 'dmsh_build_extract', 'exec'), ns)
job_for = ns['job_for']
CTL = bytes.fromhex('34127698'); GFX = bytes.fromhex('78563412'); CS = bytes.fromhex('79563412')
rng = random.Random(20261119)
def rb(n):   # random bytes that cannot hold the control magic
    b = bytearray(rng.randbytes(n))
    while True:
        i = bytes(b).find(CTL)
        if i < 0: return bytes(b)
        b[i] ^= 0x55
def rec_of(key, code, ctl): return b'BTCDBTCD' + bytes.fromhex(key) + bytes(0x28) + code + ctl
cases = []
def add(name, key, stage_name, code, ctl, facts=None, note=''):
    cases.append((f'{len(cases):04d}_{note}', key, stage_name, rec_of(key, code, ctl), facts))
def nvn1_ctl(c1o, c1t, n=0x880):
    ctl = bytearray(rb(n)); ctl[:4] = CTL
    struct.pack_into('<II', ctl, 0x700, c1o, c1t)
    return ctl
def key16(): return rng.randbytes(8).hex()
def sph_word(typ, ver=1): return (typ << 10) | ver
# plain graphics shaders, early fragment tests on / off, SPH word 19 bit 1 (depth write) on / off
for stage in ('ps', 'vs', 'gs', 'hs', 'ds'):
    for flag in (0, 1):
        for depth in (0, 2):
            code = bytearray(rb(0x400)); code[:4] = GFX
            struct.pack_into('<I', code, 0x30, sph_word({'vs': 1, 'hs': 2, 'ds': 3, 'gs': 4, 'ps': 5}[stage]))
            w = struct.unpack_from('<I', code, 0x30 + 19 * 4)[0] & ~2 | depth; struct.pack_into('<I', code, 0x30 + 19 * 4, w)
            ctl = nvn1_ctl(0x300, 0x400); ctl[0x718] = flag
            add('', key16(), f'{stage}_data.bin', bytes(code), bytes(ctl), note=f'{stage}_ef{flag}_d{depth}')
# a pixel shader with a code area shorter than the SPH (early_fragment_tests must be False, ps_omap not asked here)
code = bytearray(rb(0x40)); code[:4] = GFX; ctl = nvn1_ctl(0, 0x40); ctl[0x718] = 1
add('', key16(), 'ps_main.bin', bytes(code), bytes(ctl), note='ps_short')
# transform feedback: valid, several streams; invalid layouts (stride mismatch, location out of range, count 0, count 129, size 5)
def tf_ctl(streams, stage_vs=True):
    ctl = nvn1_ctl(0x300, 0x400)
    strides = [0, 0, 0, 0]
    for b, (locs, sizes, stride) in streams.items():
        strides[b] = stride if stride is not None else 4 * sum(sizes)
        ctl[0x6e8 + b] = len(locs)
        for i, (l, s) in enumerate(zip(locs, sizes)):
            ctl[0xdc + 0x80 * b + i] = l; ctl[0x4dc + 0x80 * b + i] = s
    struct.pack_into('<4H', ctl, 0x6dc, *strides)
    return ctl
tfs = {'tf_ok1': {0: ([0x20, 0x21], [4, 4], None)}, 'tf_ok4': {0: ([0x20], [4], None), 1: ([0x22, 0x23, 0x24], [1, 2, 3], None), 3: ([0x3f], [2], None)},
       'tf_stride': {0: ([0x20], [4], 20)}, 'tf_loc_lo': {1: ([0x1f], [4], None)}, 'tf_loc_hi': {2: ([0x40], [4], None)}, 'tf_size0': {0: ([0x20], [0], 0)}, 'tf_size5': {0: ([0x20], [5], None)},
       'tf_count0': {0: ([], [], 16)}, 'tf_many': {0: ([0x20 + i % 32 for i in range(11)], [4] * 11, None)}}
for nm, streams in tfs.items():
    for stage in ('vs', 'gs'):
        code = bytearray(rb(0x400)); code[:4] = GFX; struct.pack_into('<I', code, 0x30, sph_word(1 if stage == 'vs' else 4))
        add('', key16(), f'{stage}_data.bin', bytes(code), bytes(tf_ctl(streams)), note=f'{nm}_{stage}')
# a count of 129 (more than a stream may hold)
ctl = tf_ctl({0: ([0x20], [4], 16)}); ctl[0x6e8] = 129
code = bytearray(rb(0x400)); code[:4] = GFX; add('', key16(), 'vs_data.bin', bytes(code), bytes(ctl), note='tf_count129')
# VertexA + VertexB
def vab(a_instr=48 + 32 * 2, b_instr=64, exits=(8,), bad=None, type0=True, c1_tail=0x100, a_len_adj=0, b_sph_adj=0, no_parts=False):
    # code: 0x30 header, SPH A (0x50), A instructions, B's 0x30 header (at control +0x700's offset), SPH B (0x50), B instructions, c1 tail (Catherine: Full Body's layout)
    a_len = 0x50 + (a_instr // 32) * 32
    b_len = 0x50 + (b_instr // 32) * 32
    code = bytearray(rb(0x30 + a_len + 0x30 + b_len + c1_tail)); code[:4] = GFX
    sa = [rng.getrandbits(32) for _ in range(20)]; sb = [rng.getrandbits(32) for _ in range(20)]
    sa[0] = sph_word(0) | (rng.getrandbits(32) & ~0x3fff & 0xffff0000 & ~(0xf << 10)); sa[0] = (sa[0] & ~0x3fff) | sph_word(0)
    sb[0] = (sb[0] & ~0x3fff) | sph_word(1)
    struct.pack_into('<20I', code, 0x30, *sa)
    b_sph = 0x30 + a_len + 0x30 + b_sph_adj
    struct.pack_into('<20I', code, 0x30 + a_len + 0x30, *sb)
    # EXIT-with-CC-T instructions in part A at offsets not divisible by 32
    aoff = 0x30 + 0x50
    for o in exits:
        ins = (0xe30 << 52) | (rng.getrandbits(32) << 20 & ((1 << 52) - 1)) | (rng.getrandbits(20) & ~0x1f) | 0xf
        struct.pack_into('<Q', code, aoff + o, ins)
    # an EXIT with another condition (must stay)
    struct.pack_into('<Q', code, aoff + 16, (0xe30 << 52) | 0x3)
    ctl = bytearray(rb(0x880)); ctl[:4] = CTL
    c1o = 0x30 + a_len + b_len - 0x30  # relative to the instruction area start (rec + 0x38): part B ends here; the c1 tail follows
    struct.pack_into('<II', ctl, 0x700, (b_sph - 0x30) & 0xffffffff, len(code))
    if no_parts: struct.pack_into('<6I', ctl, 0x748, 0, 0, 0, 0, 0, 0)
    else: struct.pack_into('<6I', ctl, 0x748, 0x30, a_len + a_len_adj, 0, b_sph, b_len, 0)
    return bytes(code), bytes(ctl)
for nm, kw in {'vab_ok': {}, 'vab_ok_big': dict(a_instr=32 * 6, b_instr=32 * 5, exits=(8, 40, 72, 104)), 'vab_no_exit': dict(exits=()), 'vab_no_parts': dict(no_parts=True),
               'vab_bad_a_len': dict(a_len_adj=8), 'vab_bad_b_sph': dict(b_sph_adj=32)}.items():
    code, ctl = vab(**kw)
    add('', key16(), 'vs_main.bin', code, ctl, note=nm)
# VertexA + VertexB with c1 between the parts (Borderlands: control +0x700 gives c1's offset, before B's SPH) and the new checks (+0x700 outside A's end .. B's SPH, a part shorter than its SPH,
# a part B followed by constants of its own)
def vab_split(gap=0x100, c1o_adj=0, tail=b'', a_len=0x50 + 64, b_len=0x50 + 64, b_type=1):
    code = bytearray(rb(0x30 + a_len + gap)); code[:4] = GFX
    sa = [rng.getrandbits(32) for _ in range(20)]; sb = [rng.getrandbits(32) for _ in range(20)]
    sa[0] = (sa[0] & ~0x3fff) | sph_word(0); sb[0] = (sb[0] & ~0x3fff) | sph_word(b_type)
    struct.pack_into('<20I', code, 0x30, *sa)
    if a_len >= 0x60: struct.pack_into('<Q', code, 0x30 + 0x50 + 8, (0xe30 << 52) | (rng.getrandbits(20) & ~0x1f) | 0xf)
    b_sph = len(code)
    part_b = bytearray(rb(max(b_len, 0x50))); struct.pack_into('<20I', part_b, 0, *sb)
    code += part_b[:max(b_len, 0x50)] + tail
    ctl = bytearray(rb(0x880)); ctl[:4] = CTL
    struct.pack_into('<II', ctl, 0x700, 0x30 + a_len + c1o_adj, len(code))
    struct.pack_into('<6I', ctl, 0x748, 0x30, a_len, 0, b_sph, b_len, 0)
    return bytes(code), bytes(ctl)
for nm, kw in {'vab_split_ok': {}, 'vab_split_zero_tail': dict(tail=bytes(0x40)), 'vab_split_dirty_tail': dict(tail=bytes(0x3f) + b'\x01'), 'vab_split_c1o_mid': dict(c1o_adj=0x40),
               'vab_split_c1o_at_b': dict(c1o_adj=0x100), 'vab_split_c1o_in_a': dict(c1o_adj=-0x10), 'vab_split_c1o_past_b': dict(c1o_adj=0x110), 'vab_split_no_gap': dict(gap=0),
               'vab_split_a_short': dict(a_len=0x30), 'vab_split_b_short': dict(b_len=0x30), 'vab_split_b_type0': dict(b_type=0)}.items():
    code, ctl = vab_split(**kw)
    add('', key16(), 'vs_main.bin', code, ctl, note=nm)
# a part A shorter than its SPH in the Catherine layout ((a_len - 0x50) % 32 is 0 for a_len 0x30: only the new length check refuses it)
code, ctl = vab(); ctl = bytearray(ctl); struct.pack_into('<I', ctl, 0x748 + 4, 0x30); add('', key16(), 'vs_main.bin', code, bytes(ctl), note='vab_a_len_0x30')
# a vertex shader of type 1 (the usual) with the same control fields set: untouched
code = bytearray(rb(0x400)); code[:4] = GFX; struct.pack_into('<I', code, 0x30, sph_word(1)); ctl = nvn1_ctl(0x300, 0x400); struct.pack_into('<6I', ctl, 0x748, 0x30, 0x100, 0, 0x130, 0x100, 0)
add('', key16(), 'vs_main.bin', bytes(code), bytes(ctl), note='vs_type1_with_ab_fields')
# compute: header magic with zero padding, header with garbage, no header
code = bytearray(rb(0x500)); code[:4] = CS; code[4:0x100] = bytes(0xfc); add('', key16(), 'cs_data.bin', bytes(code), bytes(nvn1_ctl(0x400, 0x500)), note='cs_hdr_ok')
code = bytearray(rb(0x500)); code[:4] = CS; code[4:0x100] = bytes(0xfc); code[0x80] = 1; add('', key16(), 'cs_data.bin', bytes(code), bytes(nvn1_ctl(0x400, 0x500)), note='cs_hdr_dirty')
code = bytearray(rb(0xc0)); code[:4] = CS; code[4:0xc0] = bytes(0xbc); add('', key16(), 'cs_data.bin', bytes(code), bytes(nvn1_ctl(0x40, 0xc0)), note='cs_hdr_short')
code = bytearray(rb(0x500)); add('', key16(), 'cs_main.bin', bytes(code), bytes(nvn1_ctl(0x400, 0x500)), note='cs_nohdr')
# c1 ranges past the record, inverted, empty
for nm, (a, b) in {'c1_past': (0x300, 0x9000), 'c1_inv': (0x400, 0x200), 'c1_empty': (0x400, 0x400), 'c1_zero': (0, 0)}.items():
    code = bytearray(rb(0x400)); code[:4] = GFX; struct.pack_into('<I', code, 0x30, sph_word(5)); add('', key16(), 'ps_main.bin', bytes(code), bytes(nvn1_ctl(a, b)), note=nm)
# domain shaders with tessellation modes
for nm, (m, s, o) in {'ds_tri': (4, 0x202, 0x900), 'ds_quad_odd_ccw': (7, 0x8E7B, 0x901), 'ds_iso_even': (0x8E7A, 0x8E7C, 0x900), 'ds_bad_mode': (5, 0x202, 0x900), 'ds_bad_spacing': (4, 0, 0x900), 'ds_bad_order': (4, 0x202, 0x902)}.items():
    code = bytearray(rb(0x400)); code[:4] = GFX; struct.pack_into('<I', code, 0x30, sph_word(3)); ctl = nvn1_ctl(0x300, 0x400); struct.pack_into('<II', ctl, 0x738, m, s); struct.pack_into('<I', ctl, 0x744, o)
    add('', key16(), 'ds_data.bin', bytes(code), bytes(ctl), note=nm)
code = bytearray(rb(0x400)); code[:4] = GFX; add('', key16(), 'ds_data.bin', bytes(code), bytes(nvn1_ctl(0x300, 0x400, 0x760 - 0x20)), note='ds_short_ctl')
# the answer table's rows merged into the job (some keys exist already in the job: tex, img, attr, fout; local / shared are new)
for nm, f in {'facts_cs': {'local': [8, 8, 1], 'shared': 2048}, 'facts_tex': {'tex': {'0': 'texture_2d'}, 'attr': {'1': 'uint'}, 'unknown_new': [1, 2, {'x': 'é'}]}, 'facts_float': {'f': 1.5, 'g': 1e-7, 'h': 123456789012345678}}.items():
    code = bytearray(rb(0x400)); code[:4] = GFX; struct.pack_into('<I', code, 0x30, sph_word(5)); k = key16()
    add('', k, 'ps_main.bin', bytes(code), bytes(nvn1_ctl(0x300, 0x400)), facts=f, note=nm)
# NVN2
def nvn2_ctl(inst_len, c1a, c1e, code_len, stage_byte=None, cs=None, n=0x100):
    ctl = bytearray(rb(n)); ctl[:4] = CTL; struct.pack_into('<I', ctl, 4, 0xc9)
    struct.pack_into('<I', ctl, 0x38, inst_len); struct.pack_into('<II', ctl, 0x40, c1a, c1e); struct.pack_into('<I', ctl, 0x44, code_len)
    if stage_byte is not None: ctl[0x56] = stage_byte
    if cs: ctl[0xa9], ctl[0xaa], ctl[0xb1], ctl[0xb2], ctl[0xb3], ctl[0xb4], ctl[0xb5] = cs[:7]; struct.pack_into('<H', ctl, 0xba, cs[7])
    return ctl
for stage in ('vs', 'ps', 'gs', 'hs', 'ds', 'cs'):
    for flag in (0, 1):
        for depth in (0, 2):
            code = bytearray(rb(0x300)); struct.pack_into('<I', code, 19 * 4, struct.unpack_from('<I', code, 19 * 4)[0] & ~2 | depth)
            ctl = nvn2_ctl(0x280, 0x280, 0x300, 0x300); ctl[0x5b] = flag
            add('', key16(), f'{stage}_data.bin', bytes(code), bytes(ctl), note=f'nvn2_{stage}_ef{flag}_d{depth}')
code = rb(0x300); add('', key16(), 'cs_main.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, cs=(3, 5, 7, 9, 11, 13, 2, 0x1234))), note='nvn2_cs_fields')
code = rb(0x300); add('', key16(), 'vs_data.bin', code, bytes(nvn2_ctl(0x200, 0x300, 0x200, 0x300)), note='nvn2_c1_inverted')
code = rb(0x300); add('', key16(), 'vs_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x9000, 0x300)), note='nvn2_c1_past')
code = rb(0x300); add('', key16(), 'vs_data.bin', code, bytes(nvn2_ctl(0x400, 0x200, 0x300, 0x300)), note='nvn2_inst_long')
code = rb(0x300); add('', key16(), 'ds_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, n=0x800)), note='nvn2_ds_big_ctl')
# NVN2 mesh / task (round 3: ported, compared with the Python like the others): the control fields +0x56 (stage), +0x58 threads, +0x5a payload, +0x5e task, +0x5f vertices a primitive,
# +0x60 / +0x62 most vertices / primitives, +0x64 / +0x68 the per-primitive SPH; the code area's SPHs and instructions (some with an LDS / STS / ATOMS opcode: shared memory of its own)
code = rb(0x300); add('', key16(), 'ms_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, stage_byte=7)), note='nvn2_ms')
for i in range(64):
    stage_name = rng.choice(['ts', 'ms', 'ms', 'ms'])
    n = rng.choice([0x300, 0x400, 0x200, 0x90, 0x100])
    code = bytearray(rb(n))
    for off in range(0x34, min(0x80, n), 1):   # an SPH whose output map is sparse
        if rng.random() < 0.7: code[off] = rng.choice([0, 0, 0x0f, 0x30, 0xf0, 0xff, 0x01, 0x11])
    if rng.random() < 0.3 and n >= 0x120:
        o = 0x80 + 16 * rng.randrange(0, (n - 0x80 - 16) // 16); code[o:o + 2] = struct.pack('<H', rng.choice([0x984, 0x388, 0x988, 0x38c, 0x98c, 0xf8c]))
    inst_len = rng.choice([n, n - 0x20, 0x200, 0x80 + 16 * rng.randrange(0, 8)])
    ctl = nvn2_ctl(inst_len, inst_len, n, n, stage_byte=6 if stage_name == 'ts' else 7)
    struct.pack_into('<2H', ctl, 0x58, rng.choice([32, 32, 64, 128, 1]), rng.choice([0, 16, 128, 1408, 256]))
    ctl[0x5e] = rng.choice([0, 1]); ctl[0x5f] = rng.choice([0, 1, 2, 3, 3])
    struct.pack_into('<2H', ctl, 0x60, rng.choice([32, 64, 128, 160, 256, 300]), rng.choice([32, 64, 126, 128, 512]))
    at = rng.choice([0, 0, 0x100, 0x180, 0x80 + 16])
    if at: struct.pack_into('<I', ctl, 0x64, at); ctl[0x68] = 0x80 | rng.randrange(0x80)
    else: ctl[0x68] = rng.randrange(0x80)
    add('', key16(), f'{stage_name}_data.bin', bytes(code), bytes(ctl), note=f'nvn2_{stage_name}_{i}')
# a task / mesh record whose control section is too short for the fields, and one whose stage byte names neither (the Python crashes in both: struct.error / KeyError)
code = rb(0x300); add('', key16(), 'ms_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, stage_byte=7, n=0x60)), note='nvn2_ms_short_ctl')
code = rb(0x300); add('', key16(), 'ms_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, stage_byte=1)), note='nvn2_ms_wrong_byte')
# a ray tracing record (NVN2 stage byte 8) on the ordinary path: the Python's ValueError; an NVN2 control section too short for the stage byte; an NVN1 section with byte 8 there
for stage in ('vs', 'cs', 'ps', 'ms'):
    code = rb(0x300); add('', key16(), f'{stage}_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, stage_byte=8)), note=f'nvn2_rt_stage8_{stage}')
code = rb(0x300); add('', key16(), 'vs_data.bin', code, bytes(nvn2_ctl(0x200, 0x200, 0x300, 0x300, n=0x56)), note='nvn2_vs_ctl_0x56')
code = bytearray(rb(0x400)); code[:4] = GFX; struct.pack_into('<I', code, 0x30, sph_word(5)); ctl = nvn1_ctl(0x300, 0x400); ctl[0x56] = 8
add('', key16(), 'ps_main.bin', bytes(code), bytes(ctl), note='nvn1_byte8')
# task / mesh shaders whose payload exceeds Metal's 16 KB (dmsh_meshstage.payload_spill): a task-fed mesh shader's job gets stage_input_words, a task's stage memory has no input area
for nm, (stage_name, payload, task) in {'pspill_ts_16384': ('ts', 16384, 0), 'pspill_ts_16385': ('ts', 16385, 0), 'pspill_ts_28000': ('ts', 28000, 0), 'pspill_ms_task_16384': ('ms', 16384, 1),
                                        'pspill_ms_task_16385': ('ms', 16385, 1), 'pspill_ms_task_21508': ('ms', 21508, 1), 'pspill_ms_notask_21508': ('ms', 21508, 0),
                                        'pspill_ms_task_65535': ('ms', 65535, 1), 'pspill_ts_small': ('ts', 100, 0)}.items():
    code = bytearray(rb(0x300))
    for off in range(0x34, 0x80): code[off] = rng.choice([0, 0, 0x0f, 0x30, 0xf0, 0xff, 0x01, 0x11])
    ctl = nvn2_ctl(0x300, 0x300, 0x300, 0x300, stage_byte=6 if stage_name == 'ts' else 7)
    struct.pack_into('<2H', ctl, 0x58, 32, payload); ctl[0x5e] = task; ctl[0x5f] = 3
    struct.pack_into('<2H', ctl, 0x60, 64, 126); ctl[0x68] = 0
    add('', key16(), f'{stage_name}_data.bin', bytes(code), bytes(ctl), note=f'nvn2_{nm}')
# malformed: no control section / too short for the fields
add('', key16(), 'ps_main.bin', rb(0x100), b'', note='no_ctl') if False else None
out = os.path.join(OUT)
shutil.rmtree(out, ignore_errors=True); os.makedirs(out)
for nm, key, stage_name, rec, facts in cases:
    d = os.path.join(out, nm); os.makedirs(d)
    open(d + '/rec.bin', 'wb').write(rec); open(d + '/name.txt', 'w').write(stage_name); open(d + '/key.txt', 'w').write(key)
    if facts is not None: open(d + '/facts.json', 'w').write(json.dumps(facts))
    ns['FACTS'].clear()
    if facts is not None: ns['FACTS'][key] = facts
    w = tempfile.mkdtemp(prefix='synth_')
    buf = io.StringIO()
    try:
        with contextlib.redirect_stdout(buf):
            j, c1 = job_for(key, stage_name, rec, w)
        open(d + '/job.json', 'w').write(json.dumps(j)); open(d + '/code.bin', 'wb').write(open(j['code'], 'rb').read()); open(d + '/c1.bin', 'wb').write(c1)
    except SystemExit as e:
        open(d + '/exit.txt', 'w').write(str(e.code))
    except ValueError as e:   # job_for's own ValueError (a ray tracing record on the ordinary path): the build stops
        open(d + '/valueerror.txt', 'w').write(str(e))
    except Exception as e:   # the Python would crash here (a truncated record): the Rust port reports an error
        open(d + '/crash.txt', 'w').write(type(e).__name__ + ': ' + str(e))
    open(d + '/stdout.txt', 'w').write(buf.getvalue())
    shutil.rmtree(w, ignore_errors=True)
print(len(cases), 'cases ->', out)
