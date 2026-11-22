#!/usr/bin/env python3
"""Writes tests/data/games/ue-*/: synthetic games holding Unreal Engine paks, and `expected.json` (the tests/games.rs format): what the engine's own `shader_split.py` (upstream
seedds/nxnative origin/main, with `uepak.py`) writes for them. The Rust test (`tests/games.rs`) splits the same folders and must write the same bytes.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_uepak.py <upstream tree (the folder that holds dm/)> <out folder (tests/data/games)>

The Python runs as the NSP script runs it for an Unreal game: `uepak.py job` and `finish` first (the shader code archives of the paks, into a `ue_shaders` folder given by DM_UE_SHADERS, kept
OUTSIDE the game folder so the Rust split reads the archives from the paks itself), then `shader_split.py`, with no Oodle host (DM_OODLE_HOST unset: what the phone has).
Two things of the recorded output are rewritten: the RomFS path in the log becomes @ROMFS@ (tests/games.rs does the same with its own), and line 2 of shader_map.tsv, which upstream reworded
after the port ("column 5 names the chosen source of each key"), is replaced by the port's text before its hash is taken (a pre-existing difference, not one of the reader).

Every byte is a PRNG's; nothing comes from a game.
"""
import sys, os, json, struct, hashlib, tempfile, shutil, subprocess, zlib
from compression import zstd as zstdmod
import lz4.block

engine, out_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
tools = engine + '/dm/gfx/tools'
PY = sys.executable
GID = '0100TESTTEST0000'
GAME_TSV = f'title_id\t{GID}\nshader_groups\t/AT/=at /Game/=game /Proj/=proj /ue_shaders/=ue\nshader_note\tsynthetic Unreal game\n'
PAK_MAGIC = 0x5A6F12E1
CTL_MAGIC, GFX_MAGIC, CS_MAGIC = bytes.fromhex('34127698'), bytes.fromhex('78563412'), bytes.fromhex('79563412')
OUR_MAP_LINE2 = '# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied\n'

M = (1 << 64) - 1
def lcg(seed, n):
    x, out = seed & M, bytearray()
    for _ in range(n):
        x = (x * 6364136223846793005 + 1442695040888963407) & M
        out.append(x >> 56)
    return bytes(out)

BAD = (CTL_MAGIC, b'LSLG', b'\x8c\x06', b'\x8c\x0a', b'\x8c\x0b', b'\x8c\x0c')
def clean(b):
    """the random bytes without any magic a reader looks for (a control section, LSLG, an Oodle stream's start)"""
    b = bytearray(b)
    for m in BAD:
        i = b.find(m)
        while i >= 0:
            b[i] ^= 0x55; i = b.find(m)
    return bytes(b)

def shader(seed, st):
    ncode = 0x100 + 0x40 * (seed % 5)
    ctl = bytearray(clean(lcg(seed, 0x100)) + bytes(0x780))   # compressible: a zlib stream of it is shorter
    ctl[:4] = CTL_MAGIC; ctl[0x20:0x24] = struct.pack('<I', 0x880); ctl[0x704:0x708] = struct.pack('<I', ncode)
    code = bytearray(clean(lcg(seed + 1, 0x80)) + bytes(ncode - 0x80))
    if st == 'cs':
        code[:4] = CS_MAGIC; ctl[0x748:0x754] = struct.pack('<3I', 8, 8, 1)
    else:
        code[:4] = GFX_MAGIC; code[0x30:0x34] = struct.pack('<I', 0x461 if st == 'vs' else 1 | (5 << 10))
    return bytes(ctl), bytes(code)

TAG = {'vs': b'SV', 'ps': b'SP', 'cs': b'SC'}
def lslg(seed, st, tag=None):
    ctl, code = shader(seed, st)
    name = b'Global/FShader/0:/Engine/Private/Synthetic%d.usf|Main\0' % seed
    return b'LSLG' + (tag or TAG[st]) + bytes(10) + struct.pack('<I', len(name)) + name + struct.pack('<I', len(code)) + code + struct.pack('<I', len(ctl)) + ctl + bytes(24)

def filler(seed, n): return clean(lcg(seed, n))

def zarr(rec): z = zlib.compress(rec); return struct.pack('<i', len(z)) + z   # a byte array holding one zlib stream
def sarr(rec): return struct.pack('<i', len(rec)) + rec                        # a record stored as it is
def lz4rec(rec, freq): c = lz4.block.compress(rec, store_size=False); return struct.pack('<i', len(c)) + c + struct.pack('<iB', len(rec), freq)   # an FShaderMapResourceCode entry

def nso(text):
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, 0), (0x100 + len(text), end, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text

# ---- paks ------------------------------------------------------------------------------------------------------------------------
def fstr(s): b = s.encode() + b'\0'; return struct.pack('<i', len(b)) + b

def legacy_entry(off, size, raw, meth, blocks, bsz, short=False, v4=False):
    b = struct.pack('<qqqB' if short else '<qqqi', off, size, raw, meth) + bytes(20)
    if meth: b += struct.pack('<i', len(blocks)) + b''.join(struct.pack('<qq', s, e) for s, e in blocks)
    return b + struct.pack('<BI', 0, bsz)

def chunks(data, bsz): return [data[i:i + bsz] for i in range(0, len(data), bsz)] or [b'']

def legacy_pak(files, *, version=8, short=False, methods=('Zlib',), codec=None):
    """files: [(name, data, method index (0 stored), block size)] → version 8 (ordinary or short) or 9 (serialized index); codec: {method index: compress function}"""
    body, index = bytearray(), b''
    for name, data, meth, bsz in files:
        off = len(body)
        if meth:
            comp = (codec or {}).get(meth, zlib.compress)
            parts = [comp(c) for c in chunks(data, bsz)]
            head = len(legacy_entry(0, 0, 0, meth, [(0, 0)] * len(parts), 0, short))
            blocks, cur = [], head
            for p in parts: blocks.append((cur, cur + len(p))); cur += len(p)
            e = legacy_entry(off, sum(map(len, parts)), len(data), meth, blocks, bsz, short)
            body += e + b''.join(parts)
        else:
            e = legacy_entry(off, len(data), len(data), 0, [], 0, short)
            body += e + data
        index += fstr(name) + e
    index = fstr('../../../') + struct.pack('<i', len(files)) + index
    slots = 4 if short else 5
    names = b''.join(m.encode().ljust(32, b'\0') for m in (list(methods) + [''] * slots)[:slots])
    foot = bytes(16) + b'\0' + struct.pack('<IIQQ', PAK_MAGIC, version, len(body), len(index)) + bytes(20) + (b'\0' if version == 9 else b'') + names
    return bytes(body) + index + foot

def frozen_pak(files, methods=('Zlib',)):
    """a version 9 pak with a frozen index: files [(dir, name, data, method, block size)]"""
    body, ents = bytearray(), []
    for dn, fn, data, meth, bsz in files:
        off = len(body)
        if meth:
            parts = [zlib.compress(c) for c in chunks(data, bsz)]
            head = 53 + 4 + 16 * len(parts)
            blocks, cur = [], head
            for p in parts: blocks.append((cur, cur + len(p))); cur += len(p)
            body += bytes(head) + b''.join(parts); ents.append((off, cur - head, len(data), blocks, bsz, meth))
        else:
            body += bytes(53) + data; ents.append((off, len(data), len(data), [], 0, 0))
    heap = bytearray(0x60)
    def alloc(b):
        at = len(heap); heap.extend(b); heap.extend(bytes(-len(heap) % 8)); return at
    def ptr(at, target): struct.pack_into('<q', heap, at, ((target - at) << 1) | 1)
    def put_array(at, target, n): ptr(at, target); struct.pack_into('<ii', heap, at + 8, n, n)
    def put_string(at, s):
        u = (s + '\0').encode('utf-16-le'); put_array(at, alloc(u), len(u) // 2)
    put_string(0, '../../../')
    ent_at = alloc(bytes(80 * len(ents)))
    put_array(16, ent_at, len(ents))
    for i, (off, size, raw, blocks, bsz, meth) in enumerate(ents):
        a = ent_at + 80 * i
        struct.pack_into('<qqq', heap, a, off, size, raw)
        if blocks: put_array(a + 48, alloc(b''.join(struct.pack('<qq', s, e) for s, e in blocks)), len(blocks))
        struct.pack_into('<IiB', heap, a + 64, bsz, meth, 0)
    dirs = {}
    for i, (dn, fn, *_r) in enumerate(files): dirs.setdefault(dn, []).append((fn, i))
    dir_at = alloc(bytes(80 * len(dirs)))
    def sparse(at, start, n):   # array + allocation flags (all set) + bits
        put_array(at, start, n); ptr(at + 16, alloc(bytes([0xff]) * (((n + 31) // 32) * 4))); struct.pack_into('<ii', heap, at + 24, n, n)
    sparse(32, dir_at, len(dirs))
    for k, (dn, items) in enumerate(dirs.items()):
        d = dir_at + 80 * k
        put_string(d, dn)
        it = alloc(bytes(32 * len(items)))
        sparse(d + 16, it, len(items))
        for j, (fn, idx) in enumerate(items):
            put_string(it + 32 * j, fn); struct.pack_into('<i', heap, it + 32 * j + 16, idx)
    index = bytes(heap)
    names = b''.join(m.encode().ljust(32, b'\0') for m in (list(methods) + [''] * 5)[:5])
    foot = bytes(16) + b'\0' + struct.pack('<IIQQ', PAK_MAGIC, 9, len(body), len(index)) + bytes(20) + b'\1' + names
    return bytes(body) + index + foot

def pak11(files, methods=('Zlib', 'Zstd', 'LZ4')):
    """version 11: files [(name, raw, method index, bsz bits, explicit bsz, compressed blocks)]"""
    data, enc, dirs = bytearray(), bytearray(), []
    for name, raw, meth, bits, explicit, parts in files:
        at = len(data); nblk = len(parts)
        head = 53 + (4 + 16 * nblk if meth else 0)
        data.extend(bytes(head) + (b''.join(parts) if meth else raw))
        v = (bits & 0x3f) | (nblk << 6) | (meth << 23) | (1 << 29) | (1 << 30) | (1 << 31)
        dirs.append((name, len(enc)))
        enc += struct.pack('<I', v)
        if bits & 0x3f == 0x3f: enc += struct.pack('<I', explicit)
        enc += struct.pack('<II', at, len(raw))
        if meth: enc += struct.pack('<I', sum(map(len, parts)))
        if nblk != 1:
            for p in parts: enc += struct.pack('<I', len(p))
    ioff = len(data)
    bydir = {}
    for name, o in dirs:
        d, _, f = name.rpartition('/'); bydir.setdefault('/' + d + '/' if d else '/', []).append((f, o))
    dirindex = struct.pack('<i', len(bydir)) + b''.join(fstr(d) + struct.pack('<i', len(fs)) + b''.join(fstr(f) + struct.pack('<i', o) for f, o in fs) for d, fs in bydir.items())
    index = fstr('../../../') + struct.pack('<i', len(files)) + struct.pack('<Q', 0) + struct.pack('<i', 0) + struct.pack('<i', 1)
    doff = ioff + len(index) + 8 + 8 + 20 + 4 + len(enc) + 4
    index += struct.pack('<QQ', doff, len(dirindex)) + bytes(20) + struct.pack('<i', len(enc)) + bytes(enc) + struct.pack('<i', 0)
    names = b''.join(m.encode().ljust(32, b'\0') for m in (list(methods) + [''] * 5)[:5])
    foot = bytes(16) + b'\0' + struct.pack('<IIQQ', PAK_MAGIC, 11, ioff, len(index)) + bytes(20) + names
    return bytes(data) + index + dirindex + foot

def v1_archive(records):
    """records: [(data stored or zlib, raw size, frequency)]"""
    entries, code = bytearray(), bytearray()
    for i, (data, raw, freq) in enumerate(records):
        entries += lcg(900 + i, 20) + struct.pack('<QIIB', len(code), len(data), raw, freq); code += data
    return struct.pack('<II', 1, len(records)) + entries + code

def v2_archive(records):
    """records: [(stored bytes, raw size, frequency)] (LZ4 when stored is shorter)"""
    n = len(records)
    out = struct.pack('<I', 2) + struct.pack('<I', 1) + lcg(1, 20) + struct.pack('<I', n) + b''.join(lcg(50 + i, 20) for i in range(n))
    out += struct.pack('<I', 1) + struct.pack('<IIII', 0, n, 0, 0)
    out += struct.pack('<I', n); code = bytearray()
    for data, raw, freq in records: out += struct.pack('<QIIB', len(code), len(data), raw, freq); code += data
    out += struct.pack('<I', 1) + bytes(16) + struct.pack('<I', n) + b''.join(struct.pack('<I', i) for i in range(n))
    return out + code

def lz4c(b): return lz4.block.compress(b, store_size=False)

# ---- running the engine's split ------------------------------------------------------------------------------------------------------
def sha(b): return hashlib.sha256(b).hexdigest()

def make_game(name, files, update=None):
    """files: {path in RomFS: bytes}; update: {path in the update's RomFS: bytes} (a version beside the base game, update/v1)"""
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True)
    def put(root, fs):
        for p, b in fs.items():
            os.makedirs(os.path.dirname(f'{root}/{p}'), exist_ok=True); open(f'{root}/{p}', 'wb').write(b)
    os.makedirs(g + '/RomFS'); put(g + '/RomFS', files)
    open(g + '/main', 'wb').write(nso(lcg(9, 0x1000) + b'AABB\0'))
    versions, updates = [g], []
    if update:
        put(g + '/update/v1/RomFS', update); open(g + '/update/v1/main', 'wb').write(nso(lcg(19, 0x800) + b'AABB\0'))
        versions.append(g + '/update/v1'); updates.append('update/v1')
    os.makedirs(g + '/profile/' + GID); open(f'{g}/profile/{GID}/game.tsv', 'w').write(GAME_TSV)
    # the Python runs on a copy, so that the archives the NSP script decodes (<version dir>/ue_shaders, one folder per version) stay out of the game folder the Rust split reads
    work = tempfile.mkdtemp(); c = work + '/game'; shutil.copytree(g, c)
    env = dict(os.environ, DM_GAMES_DIR=c + '/profile', DM_GAMES_FIRST='1', DM_GAME=GID, J='1', PYTHONDONTWRITEBYTECODE='1')
    for k in ('DM_OODLE_HOST', 'DM_OODLE_MODULES', 'DM_UE_SHADERS'): env.pop(k, None)
    run = lambda *a: subprocess.run([PY, '-I', *a], env=env, capture_output=True, text=True, cwd=work)
    for v in versions:
        v = c + v[len(g):]
        # the NSP script's decode_ue_shaders: the archives of the paks, decoded (no Oodle host: archives with LZ4 or v1 shaders only)
        r = run(tools + '/uepak.py', 'job', v + '/RomFS', v + '/main', v + '/uejob')
        if r.returncode: sys.exit(f'{name}: uepak.py job failed:\n{r.stdout}{r.stderr}')
        r = run(tools + '/uepak.py', 'finish', v + '/uejob', v + '/ue_shaders')
        if r.returncode: sys.exit(f'{name}: uepak.py finish failed:\n{r.stdout}{r.stderr}')
    os.makedirs(work + '/pkg'); os.makedirs(work + '/tables')
    t = lambda n: work + '/tables/' + n
    roms = ','.join(c + v[len(g):] + '/RomFS' for v in versions); mains = ','.join(c + v[len(g):] + '/main' for v in versions)
    r = run(tools + '/shader_split.py', roms, mains, work + '/pkg', t('shader_map.tsv'), t('shader_facts.tsv'), t('shader_pairs.tsv'), t('pso_pairs.tsv'))
    if r.returncode: sys.exit(f'{name}: shader_split.py failed:\n{r.stdout}{r.stderr}')
    log = [l.replace(c + '/RomFS', '@ROMFS@') for l in r.stdout.splitlines() if not l.startswith('progress')]
    lines = open(t('shader_map.tsv')).readlines(); lines[1] = OUR_MAP_LINE2; open(t('shader_map.tsv'), 'w').writelines(lines)
    expected = {}
    for sub in ('pkg', 'tables'):
        for fn in sorted(os.listdir(f'{work}/{sub}')): expected[f'{sub}/{fn}'] = sha(open(f'{work}/{sub}/{fn}', 'rb').read())
    n = sum(1 for _ in open(t('shader_map.tsv'))) - 3
    json.dump({'shaders': n, 'files': expected, 'dlc': [], 'updates': updates, 'log': log}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    print(name, n, 'shaders;', '\n  '.join(log))
    shutil.rmtree(work)

# ---- the games -------------------------------------------------------------------------------------------------------------------------
# 1. DRAGON BALL Z: KAKAROT's layout: ordinary version 8 paks (Zlib), inline shaders, a patch pak, damaged and Oodle entries
rock = filler(11, 0x9000) + zarr(lslg(101, 'vs')) + filler(12, 0x8000) + zarr(lslg(102, 'ps')) + filler(13, 0x300) + sarr(lslg(103, 'cs')) + filler(14, 0x200)
tree = filler(21, 0x400) + lz4rec(lslg(104, 'vs'), 0) + filler(22, 0x80) + lz4rec(lslg(105, 'ps'), 3) + filler(23, 0x40)
tree_p = filler(24, 0x200) + lz4rec(lslg(106, 'ps'), 3) + filler(25, 0x40)
gsc = sarr(lslg(107, 'vs')) + sarr(lslg(108, 'ps')) + zarr(lslg(109, 'cs')) + filler(26, 0x30)
oodle = filler(28, 0x500)
badtag = filler(29, 0x40) + sarr(lslg(111, 'vs', tag=b'SX'))
pak0 = legacy_pak([
    ('AT/Content/Materials/M_Rock.uexp', rock, 1, 0x10000),
    ('AT/Content/Materials/M_Tree.uexp', tree, 0, 0),
    ('Engine/GlobalShaderCache-GLSL_SWITCH.bin', gsc, 1, 0x10000),
    ('AT/Config/DefaultGame.ini', b'[/Script/Engine]\nx=1\n', 0, 0),
    ('AT/Content/BadTag.uexp', badtag, 1, 0x10000),
    ('AT/Content/Oodled.uexp', oodle, 2, 0x10000),
], methods=('Zlib', 'Oodle'), codec={2: lambda b: b'\x8c\x0a' + b})
# a damaged block, beside a package that reads
def damaged_pak():
    raw = filler(27, 0x2000) + zarr(lslg(110, 'vs'))
    z = bytearray(zlib.compress(raw)); z[20:28] = b'\xff' * 8
    head = len(legacy_entry(0, 0, 0, 1, [(0, 0)], 0))
    e = legacy_entry(0, len(z), len(raw), 1, [(head, head + len(z))], 0x10000)
    body = e + bytes(z)
    index = fstr('../../../') + struct.pack('<i', 2) + fstr('AT/Content/Broken.uexp') + e
    ok = sarr(lslg(112, 'ps'))
    e2 = legacy_entry(len(body), len(ok), len(ok), 0, [], 0)
    body += e2 + ok; index += fstr('AT/Content/Fine.uexp') + e2
    names = b'Zlib'.ljust(32, b'\0') + bytes(32 * 4)
    return body + index + bytes(16) + b'\0' + struct.pack('<IIQQ', PAK_MAGIC, 8, len(body), len(index)) + bytes(20) + names
patch = legacy_pak([('AT/Content/Materials/M_Tree.uexp', tree_p, 1, 0x10000), ('AT/Content/Materials/M_New.uexp', zarr(lslg(113, 'vs')), 1, 0x10000)])
# a job of many small packages that all fail: at most 8 lines for the job, every one counted
def bad_block(i): z = bytearray(zlib.compress(filler(60 + i, 0x80))); z[2] ^= 0xff; return bytes(z)
many = legacy_pak([('AT/Content/Big.uexp', filler(59, 0x40000) + sarr(lslg(114, 'ps')), 0, 0)] + [(f'AT/Content/Z/Bad{i:02}.uexp', filler(60 + i, 0x80), 1, 0x10000) for i in range(12)],
                  codec={1: lambda b: bad_block(b[0] % 7)})
make_game('ue-v8-kakarot', {
    'AT/Content/Paks/pakchunk2-Switch.pak': many,
    'AT/Content/Paks/pakchunk0-Switch.pak': pak0,
    'AT/Content/Paks/pakchunk1-Switch.pak': damaged_pak(),
    'AT/Content/Paks/pakchunk0-Switch_0_P.pak': patch,
    'AT/Content/Movies/intro.bin': filler(30, 0x800),
})

# 2. short version 8 (mixed: a .uexp and a version 1 archive), version 9 serialized (a global shader cache and a version 2 archive with LZ4 shaders) and frozen
s1, s2 = lslg(201, 'vs'), lslg(202, 'ps')
arch1 = v1_archive([(zlib.compress(s1), len(s1), 0), (s2, len(s2), 3)])
short = legacy_pak([('Game/Content/M_Short.uexp', zarr(lslg(203, 'cs')) + filler(31, 0x100), 1, 0x10000),
                    ('Game/Content/ShaderArchive-Global-SF_GLSL_SWITCH.ushaderbytecode', arch1, 0, 0)], short=True)
a1, a2, a3 = lslg(204, 'vs'), lslg(205, 'ps'), lslg(206, 'cs')
arch2 = v2_archive([(lz4c(a1), len(a1), 0), (a2, len(a2), 3), (lz4c(a3), len(a3), 5)])
v9 = legacy_pak([('Engine/GlobalShaderCache-SF_GLSL_SWITCH.bin', sarr(lslg(207, 'vs')) + sarr(lslg(208, 'ps')), 0, 0),
                 ('Game/Content/ShaderArchive-Game-SF_GLSL_SWITCH.ushaderbytecode', arch2, 1, 0x4000)], version=9)
frozen = frozen_pak([('/Game/Content/', 'M_Frozen.uexp', filler(32, 0x4100) + zarr(lslg(209, 'vs')) + zarr(lslg(210, 'ps')), 1, 0x4000),
                     ('/Game/Config/', 'Default.ini', b'[x]\n', 0, 0)])
make_game('ue-v8short-v9', {
    'Game/Content/Paks/a-Switch.pak': short,
    'Game/Content/Paks/b-Switch.pak': v9,
    'Game/Content/Paks/c-Switch.pak': frozen,
})

# 3. version 11: single-block entries with the truncated 6-bit block size (b3e0d8c5), several blocks, Zstd (22222648), LZ4, stored; and a pak of shader code archives only
one = filler(41, 0x3b00) + zarr(lslg(301, 'vs'))
multi = filler(42, 0x10000) + zarr(lslg(302, 'ps')) + filler(43, 0x500)
zs = filler(44, 0x200) + sarr(lslg(303, 'cs'))
l4 = filler(45, 0x100) + zarr(lslg(304, 'ps'))
st = sarr(lslg(305, 'vs'))
def zstd_nosize(b): c = zstdmod.ZstdCompressor(); return c.compress(b) + c.flush()
v11 = pak11([
    ('Proj/Content/M_One.uexp', one, 1, len(one) >> 11, 0, [zlib.compress(one)]),
    ('Proj/Content/M_Multi.uexp', multi, 1, 0x10000 >> 11, 0, [zlib.compress(c) for c in chunks(multi, 0x10000)]),
    ('Proj/Content/M_Zstd.uexp', zs, 2, 0, 0, [zstd_nosize(zs)]),
    ('Proj/Content/M_LZ4.uexp', l4, 3, 0x3f, 0x800, [lz4c(c) for c in chunks(l4, 0x800)]),
    ('Proj/Content/M_Stored.uexp', st, 0, 0, 0, []),
    ('Proj/Config/DefaultEngine.ini', b'[Core]\n', 1, 0, 0, [zlib.compress(b'[Core]\n')]),
])
b1, b2 = lslg(306, 'vs'), lslg(307, 'ps')
arch3 = v2_archive([(lz4c(b1), len(b1), 0), (lz4c(b2), len(b2), 3)])
v11a = pak11([('Proj/Content/ShaderArchive-Proj-SF_GLSL_SWITCH.ushaderbytecode', arch3, 1, 0x3f, 0x10000, [zlib.compress(c) for c in chunks(arch3, 0x10000)]),
              ('Proj/Content/Readme.txt', b'nothing\n', 0, 0, 0, [])])
c1 = lslg(308, 'vs')
upd = pak11([('Proj/Content/M_Update.uexp', zarr(c1) + zarr(lslg(301, 'vs')), 1, 0, 0, [zlib.compress(zarr(c1) + zarr(lslg(301, 'vs')))])])
c2 = lslg(309, 'ps')
upda = pak11([('Proj/Content/ShaderArchive-Proj-SF_GLSL_SWITCH.ushaderbytecode', v2_archive([(lz4c(c2), len(c2), 3), (lz4c(b1), len(b1), 0)]), 0, 0, 0, [])])
make_game('ue-v11', {
    'Proj/Content/Paks/Proj-Switch.pak': v11,
    'Proj/Content/Paks/Shaders-Switch.pak': v11a,
}, update={'Proj/Content/Paks/Proj-Switch_1_P.pak': upd, 'Proj/Content/Paks/Shaders-Switch_1_P.pak': upda})
