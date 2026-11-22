#!/usr/bin/env python3
"""Writes the differential test vectors of ibuilder-shadersplit: synthetic inputs (random bytes in the layouts the readers expect, then mutated) and what the ENGINE's own
`shader_split.py` / `unityfs.py` answers for them. The Rust tests (`tests/differential.rs`) read the same inputs and must give the same answers.

  PYTHONDONTWRITEBYTECODE=1 python3 -I tools/gen_vectors.py <engine dm/gfx/tools folder> <skip: shader_key_skip text, e.g. 0+0> <out.jsonl>

Nothing here comes from a game: every byte is a PRNG's. The input "specs" are `{"len", "seed", "set": [[offset, hex], ...]}`: the PRNG (an LCG, below) fills `len` bytes and the sets
overwrite parts of it, so the Rust side materialises the same buffer without the vector file carrying it. Containers are stored as hex (they are small).
"""
import sys, os, json, struct, hashlib, tempfile, importlib, io, contextlib, zlib

tools, skip, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
CTL_ONLY = len(sys.argv) > 4 and sys.argv[4] == '--ctl-only'   # only the vectors that depend on the profile's key rule
prof = tempfile.mkdtemp()
os.makedirs(prof + '/0100TESTTEST0000')
open(prof + '/0100TESTTEST0000/game.tsv', 'w').write('title_id\t0100TESTTEST0000\nshader_groups\t/Data/=data /Pack/=pack\nshader_note\ttest game\n' + (f'shader_key_skip\t{skip}\n' if skip != '0+0' else ''))
os.environ.update(DM_GAMES_DIR=prof, DM_GAMES_FIRST='1', DM_GAME='0100TESTTEST0000')
for sub in ('', '/../../tools', '/../../games'):
    sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, unityfs

M = (1 << 64) - 1
def lcg_fill(seed, n):
    x, out = seed & M, bytearray()
    for _ in range(n):
        x = (x * 6364136223846793005 + 1442695040888963407) & M
        out.append(x >> 56)
    return bytes(out)

def spec(n, seed, sets=()):
    return {'len': n, 'seed': seed, 'set': [[o, bytes(v).hex()] for o, v in sets]}

def materialise(sp):
    b = bytearray(lcg_fill(sp['seed'], sp['len']))
    for o, h in sp['set']:
        v = bytes.fromhex(h)
        b[o:o + len(v)] = v
    return bytes(b)

class Rng:   # a second LCG, for choosing what to build
    def __init__(s, seed): s.x = seed & M
    def next(s):
        s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
        return s.x >> 24
    def below(s, n): return s.next() % n
    def pick(s, seq): return seq[s.below(len(seq))]
    def chance(s, num, den): return s.below(den) < num

def u32(v): return struct.pack('<I', v)

def mk_ctl_spec(rng, seed, kind):
    """a control section spec: kind 'nvn' (0x760..0x890 bytes), 'nvn2', 'short'"""
    if kind == 'nvn2':
        n = rng.pick([0x48, 0xd0, 0xd8, 0x100])
        sets = [(0, ss.CTL_MAGIC), (4, b'\xc9\0\0\0'), (0x30, u32(rng.pick([n, 0, 0xd8]))), (0x56, bytes([rng.pick([0, 1, 5, 6, 7])]))]
    elif kind == 'short':
        n = rng.pick([0, 3, 4, 0x20, 0x24, 0x100, 0x75f])
        sets = [(0, ss.CTL_MAGIC)] if rng.chance(2, 3) else []
    else:
        n = rng.pick([0x760, 0x800, 0x84c, 0x850, 0x880, 0x88c, 0x890, 0x900])
        own = rng.pick([n, n, n, 0x880, 0x850, 0, 0x75f, 0xffffffff, n - 4 if n > 4 else 0])
        sets = [(0, ss.CTL_MAGIC), (0x20, u32(own & 0xffffffff))]
    return spec(n, seed, sets)

def mk_code_spec(rng, seed, kind, ncode):
    sets = []
    if kind in ('gfx', 'gfx-vs0'):
        sets.append((0, ss.GFX_MAGIC))
        w = rng.pick([0x461, 0x61, 0x1 | (5 << 10), 0x1 | (3 << 10), 0x1 | (2 << 10), 0x1 | (4 << 10), 0x1 | (7 << 10), 0, 0x400])
        sets.append((0x30, u32(w)))
    elif kind == 'cs':
        sets.append((0, ss.CS_MAGIC))
    elif kind == 'bad':
        sets.append((0, rng.pick([ss.CS_MAGIC, ss.GFX_MAGIC, b'\0\0\0\0', b'\xde\xad\xbe\xef'])))
    return spec(ncode, seed, sets)

recs = []
def emit(kind, inp, out): recs.append({'kind': kind, 'in': inp, 'out': out})

def res(fn):
    """('ok', value) or ('err', text): every exception counts as one outcome"""
    try: return {'ok': fn()}
    except (Exception, SystemExit) as e: return {'err': f'{type(e).__name__}: {e}'}

def stage_str(st): return st

rng = Rng(0x5eed)
seed = 1
# ---- (a) control sections, codes, keys, stages ----------------------------------------------------------------------------------------
for i in range(800):
    seed += 1
    ck = rng.pick(['nvn', 'nvn', 'nvn', 'nvn2', 'short'])
    ctl_sp = mk_ctl_spec(rng, seed, ck)
    ncode = rng.pick([0, 2, 4, 0x34, 0x40, 0x80, 0x200, 0x1000])
    code_kind = rng.pick(['gfx', 'gfx', 'cs', 'bad', 'none'])
    code_sp = mk_code_spec(rng, seed * 7, code_kind, ncode)
    ctl, code = materialise(ctl_sp), materialise(code_sp)
    # make the lengths agree most of the time, so check() is exercised past its first tests
    if ck == 'nvn' and len(ctl) >= 0x708 and rng.chance(3, 4):
        ctl_sp['set'].append([0x704, u32(len(code)).hex()]); ctl = materialise(ctl_sp)
    if ck == 'nvn2' and len(ctl) >= 0x48 and rng.chance(3, 4):
        ctl_sp['set'].append([0x44, u32(len(code)).hex()]); ctl = materialise(ctl_sp)
    if ck == 'nvn' and rng.chance(1, 3) and len(ctl) >= 0x760:   # a compute thread group, so glslc_stage's headerless rule is reached
        ctl_sp['set'].append([0x748, (u32(rng.pick([0, 1, 4, 64])) + u32(rng.pick([0, 1, 2])) + u32(rng.pick([0, 1, 3]))).hex()]); ctl = materialise(ctl_sp)
    inp = {'ctl': ctl_sp, 'code': code_sp}
    emit('ctl', inp, {
        'nvn2_ctl': ss.nvn2_ctl(ctl),
        'ctl_len': res(lambda: ss.ctl_len(ctl)),
        'key_ctl': res(lambda: hashlib.sha256(ss.key_ctl(ctl)).hexdigest()),
        'skey': res(lambda: ss.skey(ctl, code)),
        'sph_stage': ss.sph_stage(code),
        'nvn2_stage': ss.nvn2_stage(ctl, code),
        'glslc_stage': res(lambda: ss.glslc_stage(ctl, code)),
        'check': {st: res(lambda st=st: ss.check('x', st, ctl, code)) for st in ('vs', 'hs', 'ds', 'gs', 'ps', 'cs')},
    })
# ---- same_shader: pairs differing in the skipped range, past the own length, in the code --------------------------------------------------
for i in range(300):
    seed += 1
    n = rng.pick([0x760, 0x850, 0x880, 0x890])
    base = spec(n, seed, [(0, ss.CTL_MAGIC), (0x20, u32(rng.pick([n, 0x850, 0x880])))])
    a = materialise(base)
    cb = bytearray(a)
    for _ in range(rng.below(4)):
        o = rng.pick([0x770, 0x778, 0x790, 0x7d0, 0x100, n - 1, 0x20, 0x4]) % max(len(cb), 1)
        cb[o] ^= 1 + rng.below(255)
    cut = len(cb)
    if rng.chance(1, 4): cut = rng.pick([0x24, 0x760, n - 8])
    ext = b''
    if rng.chance(1, 4): ext = b'\xaa' * rng.pick([1, 12, 0x40])
    diffs = [(o, bytes([cb[o]])) for o in range(len(a)) if cb[o] != a[o]]
    cb = bytes(cb[:cut]) + ext
    ka = lcg_fill(seed + 1, 0x40); kb = ka if rng.chance(3, 4) else lcg_fill(seed + 2, 0x40)
    emit('same', {'a': base, 'b': {'sets': [[o, v.hex()] for o, v in diffs], 'cut': cut, 'ext': ext.hex()}, 'ka': ka.hex(), 'kb': kb.hex()}, res(lambda: ss.same_shader((a, ka), (cb, kb))))
# ---- the record of one shader (the lines of shader_split.main and live_package), and the facts of a compute shader --------------------------------
def py_record(key, ctl, code):
    rec = bytearray(b'BTCDBTCD' + bytes.fromhex(key) + bytes(0x28)); struct.pack_into('<2I', rec, 0x28, len(ctl), len(code)); rec += code + ctl
    if rec.find(ss.CTL_MAGIC, 0x38) != 0x38 + len(code): raise SystemExit('magic in code')
    if rec.count(b'BTCDBTCD') != 1: raise SystemExit('separator in record')
    return bytes(rec)
def py_facts(ctl):
    at = (0x58, 0x64) if ss.nvn2_ctl(ctl) else (0x748, 0x754)
    x, y, z = struct.unpack_from('<3I', ctl, at[0]); sh = struct.unpack_from('<I', ctl, at[1])[0]
    if not x * y * z: raise SystemExit('thread group 0')
    return {'local': [x, y, z], 'shared': (sh + 15) // 16 * 16}
for i in range(300):
    seed += 1
    ck = rng.pick(['nvn', 'nvn', 'nvn2'])
    ctl_sp = mk_ctl_spec(rng, seed, ck)
    ncode = rng.pick([0x40, 0x80, 0x100, 0x300])
    code_sp = mk_code_spec(rng, seed * 3, rng.pick(['gfx', 'cs']), ncode)
    sets = []
    if rng.chance(1, 3): sets.append((rng.pick([0x40, 0x50, 0x80, 0xa0, 0x38]), ss.CTL_MAGIC))        # the control magic inside the code area
    if rng.chance(1, 3): sets.append((rng.pick([0x10, 0x40, 0xa0]), b'BTCDBTCD'))                       # the record separator inside
    if rng.chance(1, 5): sets.append((ncode - rng.pick([1, 2, 3]), ss.CTL_MAGIC[:rng.pick([1, 2, 3])]))  # the start of the magic at the end of the code area
    if rng.chance(1, 5): sets.append((rng.pick([0x0, 0x10]), b'BTCDBTCD'[:rng.pick([4, 8])]))
    code_sp['set'] += [[o, v.hex()] for o, v in sets]
    if ck == 'nvn' and len(materialise(ctl_sp)) >= 0x760:
        ctl_sp['set'].append([0x748, (u32(rng.pick([0, 32, 64])) + u32(rng.pick([0, 1])) + u32(1) + u32(0) + u32(0) + u32(rng.pick([0, 100, 4096]))).hex()])
    if ck == 'nvn2' and len(materialise(ctl_sp)) >= 0x68:
        ctl_sp['set'].append([0x58, (u32(rng.pick([0, 32])) + u32(1) + u32(1) + u32(0) + u32(rng.pick([0, 17, 4096]))).hex()])
    ctl, code = materialise(ctl_sp), materialise(code_sp)
    key = ss.skey(ctl, code) if len(ctl) >= 0x24 or ss.nvn2_ctl(ctl) else '0' * 16
    emit('record', {'ctl': ctl_sp, 'code': code_sp, 'key': key}, {'record': res(lambda: hashlib.sha256(py_record(key, ctl, code)).hexdigest()), 'facts': res(lambda: py_facts(ctl))})

json.dump  # (the containers follow in gen_containers.py's part of this file)
if not CTL_ONLY: exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'gen_vectors_containers.py')).read())
import gzip
text = ''.join(json.dumps(r, sort_keys=True, separators=(',', ':')) + '\n' for r in recs).encode()
with open(out_path, 'wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', compresslevel=9, mtime=0, filename='') as f: f.write(text)   # mtime 0: the same bytes every run
print(f'{len(recs)} vectors -> {out_path} ({os.path.getsize(out_path)} bytes)')
