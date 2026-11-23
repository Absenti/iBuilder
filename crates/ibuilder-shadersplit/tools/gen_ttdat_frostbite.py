#!/usr/bin/env python3
"""Writes tests/data/games/sync-ttdat and sync-frostbite: synthetic games for the readers ported from nxnative 9f834e41 + 5cd38d39 (TT Games' CC4 DAT archives, the add-on's DFLT members,
main's loose control sections: a code copy built by address, a unique adjacent code area) and 682fdd26 (Frostbite's superbundles: layout.toc, .toc tables, cas files). Every byte is made
here (nothing comes from a game) and split by the UPSTREAM engine's own `shader_split.py` (its command line); `expected.json` has the sha256 of what it wrote and its log, in the format of
tests/games.rs. The builders follow upstream's own tests (dm/tools/tests/test_split_ttdat.py, test_split_adjacent.py, dm/gfx/tests/test_frostbite.py).

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_ttdat_frostbite.py <upstream checkout (the folder that holds dm/)> tests/data/games

No member is Oodle-compressed: the Python decodes Oodle with the game's own decoder in the Mac host, which the phone has not; this port refuses such a member (tests/ttdat_frostbite.rs).
The Python's absolute RomFS path in log lines is written as @ROMFS@.
"""
import hashlib, json, os, shutil, struct, subprocess, sys, tempfile, zlib

here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, 'gen_sync_readers.py')).read()
exec(src[:src.index('# ---- NXSD')])   # engine, out_root, Rng, clean, program, glslc, nso, PORT_LINE2, GAME
GAME_TSV = f'title_id\t{GAME}\nshader_groups\t/Data/=data /Pack/=pack /Patch/=patch /aoc/=dlc\nshader_note\tsynthetic game\n'
S = lambda st, seed, extra=b'': program(st, seed, extra)


def make_game(name, files, main_text, dlc=None):
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    dlcs = []
    for dn, dfiles in (dlc or {}).items():
        for p, b in dfiles.items():
            os.makedirs(os.path.dirname(f'{g}/dlc/{dn}/{p}'), exist_ok=True); open(f'{g}/dlc/{dn}/{p}', 'wb').write(b)
        dlcs.append(dn)
    open(g + '/main', 'wb').write(nso(main_text))
    os.makedirs(f'{g}/profile/{GAME}'); open(f'{g}/profile/{GAME}/game.tsv', 'w').write(GAME_TSV)
    work = tempfile.mkdtemp(); os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1')
    env.pop('DM_OODLE_HOST', None)
    p = subprocess.run([sys.executable, '-I', engine + '/dm/gfx/tools/shader_split.py', g + '/RomFS', g + '/main', work + '/pkg'] + [f'{work}/tables/{t}' for t in tables]
                       + [','.join(f'{g}/dlc/{d}' for d in dlcs)], env=env, capture_output=True, text=True)
    if p.returncode: sys.exit(f'{name}: the Python split failed ({p.returncode}): {p.stderr[-3000:]}')
    log = [l.replace(g + '/RomFS', '@ROMFS@') for l in p.stdout.splitlines() if not l.startswith('progress')]
    expected = {}
    for sub in ('pkg', 'tables'):
        for fn in sorted(os.listdir(f'{work}/{sub}')):
            b = open(f'{work}/{sub}/{fn}', 'rb').read()
            if fn == 'shader_map.tsv':
                lines = b.decode().splitlines(keepends=True); lines[1] = PORT_LINE2; b = ''.join(lines).encode()
            expected[f'{sub}/{fn}'] = sha(b)
    n = sum(1 for l in open(f'{work}/tables/shader_map.tsv') if not l.startswith(('#', 'package\t')))
    json.dump({'shaders': n, 'files': expected, 'dlc': dlcs, 'updates': [], 'log': log}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    shutil.rmtree(work)
    print(name, n, 'shaders', len(expected), 'files;', *log, sep='\n  ')


def compute(seed, exit_at=8):
    """A headerless compute pair (test_split_adjacent.compute): a 0x100-byte code area whose first word is a scheduling word, an EXIT in an instruction slot"""
    ctl = bytearray(program('vs', seed)[0]); code = bytearray(0x100)
    struct.pack_into('<I', ctl, 0x704, len(code)); struct.pack_into('<3I', ctl, 0x748, 64, 1, 1); struct.pack_into('<I', ctl, 0x700, 0)
    struct.pack_into('<Q', code, 0, 1)
    if exit_at is not None: struct.pack_into('<Q', code, exit_at, 0xe30000000007000f)
    return bytes(ctl), bytes(code)


# ---- TT Games CC4 DAT -------------------------------------------------------------------------------------------------------------------
def tt_hash(name):
    v = 0xcbf29ce484222325
    for b in name.replace('/', '\\').lstrip('\\').upper().encode(): v = ((v ^ b) * 1099511628211) & 0xffffffffffffffff
    return v


def ttdat(members, compact=False, version=2, dup_hash=False):
    """members: [(full name, stored bytes, decoded length or None (= stored length))]: folders from the names, the file records in reverse name order, every member at 16-byte alignment"""
    strings = bytearray(); rows = [struct.pack('>I4H', 0xffffffff, 0, 0, 0, 0)]; folders = {'': 0}
    def sid(s):
        o = len(strings); strings.extend(s.encode() + b'\0'); return o
    files = []
    for full, payload, raw in members:
        parts = full.split('/'); parent, acc = 0, ''
        for d in parts[:-1]:
            acc += d + '/'
            if acc not in folders: rows.append(struct.pack('>I4H', sid(d), parent, 0, 0, 0)); folders[acc] = len(rows) - 1
            parent = folders[acc]
        files.append((full, payload, len(payload) if raw is None else raw))
        rows.append(struct.pack('>I4H', sid(parts[-1]), parent, 7, 0, len(files)))
    data = bytearray(b'data' + bytes(0x100 - 12)); at = {}
    for full, payload, _ in files:
        while (8 + len(data)) % 16: data.append(0)
        at[full] = 8 + len(data); data += payload
    order = list(reversed(files))
    records = b''.join(struct.pack('>QII', at[f] | (9 << 56), len(p), r | (0x80000000 if i % 2 else 0)) for i, (f, p, r) in enumerate(order))
    hashes = [tt_hash(f) for f, _, _ in order]
    if dup_hash: hashes[-1] = hashes[0]
    body = bytes(strings) + bytes(4) + b''.join(rows) + struct.pack('>iI', -13, len(order)) + records + b''.join(struct.pack('>Q', h) for h in hashes)
    table = struct.pack('>I8si4I', 28 + len(body), b'.CC40TAD', -13, version, len(order), len(rows), len(strings)) + body
    off = (8 + len(data) + 255) & ~255
    pointer = (0xffffffff ^ ((off >> 8) - 1)) if compact else off
    return struct.pack('<II', pointer, len(table)) + bytes(data) + bytes(off - 8 - len(data)) + table


def chunks(tag, raw, parts, pack=lambda b: b):
    """{tag, u32 packed, u32 raw} chunks of `raw` cut in `parts` pieces"""
    out, n = b'', len(raw)
    for i in range(parts):
        piece = raw[i * n // parts:(i + 1) * n // parts]; p = pack(piece)
        out += struct.pack('<4sII', tag, len(p), len(piece)) + p
    return out


def dflt_fixed(b):
    e = zlib.compressobj(wbits=-15, strategy=zlib.Z_FIXED); p = e.compress(b) + e.flush()
    assert p[0] & 7 == 3; return p


def dflt_dynamic(b):
    e = zlib.compressobj(9, wbits=-15); p = e.compress(b) + e.flush()
    assert p[0] & 7 == 5, p[0]; return bytes([p[0] & ~4]) + p[1:]   # TT writes the dynamic type as 0


vs1, ps2, vs3, ps4, cs5, vs6, vs7, ps8, ps9 = S('vs', 101), S('ps', 102), S('vs', 103), S('ps', 104), S('cs', 105), S('vs', 106), S('vs', 107), S('ps', 108), S('ps', 109)
ccs = compute(110)
Z = bytes(0x400)
mem_a = glslc([vs1, ps2]) + vs3[0] + vs3[1]                                    # a two-stage GLSLC output, then a loose program with its code right after its control
mem_b = ps4[0] + bytes(0x5a8) + ps4[1] + glslc([cs5])                          # an older program: reflection data between its control and its code, then a GLSLC output
mem_c = ccs[0] + ccs[1] + vs6[0] + vs6[1]                                      # a headerless compute program and a vertex shader (OODL chunks stored as they are)
mem_d = vs7[0] + vs7[1] + ps8[0] + ps8[1] + Z * 8                                    # DFLT, fixed Huffman
mem_g = ps9[0] + ps9[1] + Z                                                    # DFLT, dynamic Huffman written as type 0 (the add-on)
mem_h = S('vs', 111)[0] + S('vs', 111)[1] + Z
amb_c, amb_k = S('ps', 112)
bad_h = dflt_dynamic(mem_h)
game_dat = ttdat([
    ('shaders/fx/a.nnx_shaders', mem_a, None),
    ('shaders/old/b.nnx_shaders', mem_b, None),
    ('shaders/C.NNX_SHADERS', chunks(b'OODL', mem_c, 2), len(mem_c)),
    ('shaders/fx/d.nnx_shaders', chunks(b'DFLT', mem_d, 1, dflt_fixed), len(mem_d)),
    ('textures/t.bin', Rng(5).bytes(0x180), None),
])
dlc_dat = ttdat([
    ('shaders/g.nnx_shaders', chunks(b'DFLT', mem_g, 1, dflt_dynamic), len(mem_g)),
    ('shaders/h.nnx_shaders', struct.pack('<4sII', b'DFLT', len(bad_h) - 1, len(mem_h)) + bad_h[:-1], len(mem_h)),   # the stream cut short: the member is red
], compact=True)
cut = ttdat([('shaders/x.nnx_shaders', mem_a, None)])
cut = cut[:struct.unpack_from('<I', cut)[0] + 20]                               # the index header cut short
# main: a vertex shader whose control section carries its source, copied from a code area the code builds the address of (test_split_adjacent.test_code_copy_takes_precedence_over_adjacent_code),
# then a headerless compute program right after its control section, which nothing references
cctl, ccode = S('vs', 120)
other = ccode[:-1] + bytes([ccode[-1] ^ 1])
control = bytearray(cctl + b'!!NVvp5.0'); struct.pack_into('<I', control, 0x14, len(cctl)); struct.pack_into('<I', control, 0x20, len(control))
image = bytearray(0x2000) + control + other
image[0x1000:0x1000 + len(ccode)] = ccode
struct.pack_into('<2I', image, 0x70, 0xd0000008, 0x91000108)
struct.pack_into('<4I', image, 0x80, 0xb0000001, 0x91000021, 0x52800002 | len(ccode) << 5, 0x94000000)
while len(image) % 16: image.append(0)
image += bytes(0x40)
mctl, mcode = compute(121)
image += mctl + mcode + bytes(0x100)

make_game('sync-ttdat', {
    'Data/GAME.DAT': game_dat,
    'Data/plain.dat': Rng(6).bytes(0x300),
    'Data/glslc.dat': glslc([S('vs', 113)]),
    'Pack/BROKEN.DAT': ttdat([('shaders/x.nnx_shaders', mem_a, None)], version=3),
    'Pack/dup.dat': ttdat([('shaders/x.nnx_shaders', mem_a, None), ('y.bin', b'y' * 16, None)], dup_hash=True),
    'Pack/cut.dat': cut,
    'Pack/amb.dat': ttdat([('shaders/amb.nnx_shaders', amb_c + bytes(0x5a8) + amb_k + amb_k, None)]),
}, bytes(image), dlc={'1': {'DLC.DAT': dlc_dat}})


# ---- Frostbite -------------------------------------------------------------------------------------------------------------------------
TOC_MAGIC, TOC_BODY, BUNDLE_MAGIC = bytes.fromhex('00d1ce01'), 0x22c, 0x9D798ED6
def fb_hash(name):
    h = 5381
    for c in name.lower().encode(): h = (h * 33 ^ c) & 0xffffffff
    return h
CB, TEX, MESH = fb_hash('CompiledBytecode'), fb_hash('Texture'), fb_hash('MeshSet')


def leb(n):
    out = bytearray()
    while True:
        b = n & 0x7f; n >>= 7; out.append(b | (0x80 if n else 0))
        if not n: return bytes(out)
def dbo(kind, name, body): return bytes([kind | (0x80 if name is None else 0)]) + (b'' if name is None else name.encode() + b'\0') + body
def dbo_obj(name, items, kind=2): body = b''.join(items) + b'\0'; return dbo(kind, name, leb(len(body)) + body)
def dbo_str(name, s): b = s.encode() + b'\0'; return dbo(7, name, leb(len(b)) + b)
def dbo_i32(name, v): return dbo(8, name, struct.pack('<i', v))
def dbo_bool(name, v): return dbo(6, name, bytes([v]))
def toc_file(body): return TOC_MAGIC + bytes(TOC_BODY - 4) + body


def block(data, codec=0, stored=None, mark=0x70):
    s = data if stored is None else stored
    return struct.pack('>IBBH', len(data), codec, mark | len(s) >> 16, len(s) & 0xffff) + s


def vnvn(pair, stage, ver=9):
    ctl, code = pair
    head = b'VNVN' + struct.pack('<IHHI', 0, ver, 0, 0) + (b'\0' if ver >= 3 else b'') + struct.pack('<H', 1) + bytes(4) + struct.pack('<HHHH', 0, 0, 0, 0)
    co = len(head) + 24; ko = co + len(code)
    return bytes([5, stage, 8, 0]) + bytes(0x1c) + head + struct.pack('<4IQ', co, len(code), ko, len(ctl), 0) + code + ctl


def manifest(ebx, res, chs):
    """ebx [(name, size)], res [(name, size, type)], chs [(guid, logical offset, size)] → the manifest as the cas file stores it"""
    names, offs = b'', []
    for n in [e[0] for e in ebx] + [r[0] for r in res]: offs.append(len(names)); names += n.encode() + b'\0'
    ne, nr, nc = len(ebx), len(res), len(chs); tot = ne + nr + nc
    so = 0x20 + 20 * tot + 8 * ne + 8 * nr + 4 * nr + 24 * nr + 24 * nc
    hdr = struct.pack('<8I', BUNDLE_MAGIC, tot, ne, nr, nc, so, 0, 0) + bytes(20 * tot)
    hdr += b''.join(struct.pack('<II', o, s) for o, (_, s) in zip(offs, ebx))
    hdr += b''.join(struct.pack('<II', o, s) for o, (_, s, _) in zip(offs[ne:], res))
    hdr += b''.join(struct.pack('<I', t) for _, _, t in res) + bytes(24 * nr)
    hdr += b''.join(struct.pack('<16sII', g, lo, ls) for g, lo, ls in chs) + names
    return struct.pack('>I', len(hdr)) + hdr


class Cas:
    def __init__(self, cid): self.cid, self.d = cid, bytearray(Rng(cid[-1] + 40).bytes(0x30))
    def put(self, b):
        o = len(self.d); self.d += b + bytes(3); return (self.cid, o, len(b))


def bundle_data(ents):
    """[(cas id, offset, stored size)] → the bundle's data: header, entries (a cas id before each one that changes it), one flag byte each"""
    body, flags, prev = b'', b'', None
    for cid, off, size in ents:
        if cid != prev: body += cid; flags += b'\x80'; prev = cid
        else: flags += b'\x00'
        body += struct.pack('>II', off, size)
    return struct.pack('>9I', 0, 0, 0x24 + len(body), len(ents), 0x24, 0x24, 0x24, 0, len(ents)) + body + flags


def toc(bundles):
    """[bundle data] → a superbundle .toc"""
    head = struct.pack('>24I', 0, 0x60, len(bundles), *([0] * 21))
    table, data, at = b'', b'', 0x60 + 16 * len(bundles)
    for b in bundles:
        table += struct.pack('>IIQ', 0, 0x40000000 | len(b), at + len(data)); data += b + bytes(5)
    return toc_file(head + table + data)


def bundle(man_cas, items):
    """items: [(kind 'ebx' | 'res' | 'chunk', name or GUID, bytes, type, cas, stored or None)] → (the manifest's toc entry, the items' toc entries) after writing them"""
    ebx = [(n, len(b)) for k, n, b, t, c, s in items if k == 'ebx']
    res = [(n, len(b), t) for k, n, b, t, c, s in items if k == 'res']
    chs = [(n, 0, len(b)) for k, n, b, t, c, s in items if k == 'chunk']
    m = manifest(ebx, res, chs); me = man_cas.put(m)
    ents = []
    for kind in ('ebx', 'res', 'chunk'):
        for k, n, b, t, c, s in items:
            if k == kind: ents.append(c.put(block(b) if s is None else s) if not isinstance(s, tuple) else s)
    return bundle_data([me] + ents)


CH0 = 0xA3A00DE1
cid = lambda patch, pi, n: bytes([0, patch]) + struct.pack('>IH', pi, n)
c00, c01, c12, p00 = Cas(cid(0, CH0, 1)), Cas(cid(0, CH0, 2)), Cas(cid(0, 7, 12)), Cas(cid(1, CH0, 1))
fvs, fps, fcs, fvs2, fps2, fbad = S('vs', 201), S('ps', 202), S('cs', 203), S('vs', 204), S('ps', 205), S('vs', 206)
mismatch = (fbad[0][:0x704] + struct.pack('<I', 0x10) + fbad[0][0x708:], fbad[1])
r_vs = vnvn(fvs, 0)
two_blocks = vnvn(fps, 1)
two_blocks = block(two_blocks[:0x400]) + block(two_blocks[0x400:])
shared = c00.put(block(r_vs))                                                    # one stored copy two bundles name
b1 = bundle(c00, [('ebx', 'ebx/level', b'ebx data' * 4, 0, c00, None), ('res', 'shaders/bytecode/0001', r_vs, CB, c00, shared),
                  ('res', 'shaders/bytecode/0002', vnvn(fps, 1), CB, c00, two_blocks), ('res', 'textures/a', b'texture data', TEX, c00, None),
                  ('res', 'shaders/bytecode/0003', vnvn(fcs, 5), CB, c12, None), ('chunk', bytes(range(16)), b'chunk data', 0, c00, None)])
b2 = bundle(c01, [('res', 'shaders/bytecode/0004', Rng(7).bytes(0x60), CB, c01, None), ('res', 'shaders/bytecode/0005', vnvn(mismatch, 0), CB, c01, None),
                  ('res', 'shaders/bytecode/0001b', r_vs, CB, c00, shared), ('res', 'meshes/m', b'mesh' * 8, MESH, c01, None)])
dmg = block(vnvn(fvs2, 0)); dmg = dmg[:5] + b'\x60' + dmg[6:]                  # the 0x70 mark broken: the job is red
b3 = bundle(c12, [('res', 'shaders/bytecode/0006', vnvn(fvs2, 0), CB, c12, dmg), ('res', 'shaders/bytecode/0007', vnvn(fps2, 1, ver=2), CB, c12, None)])
b4 = bundle(p00, [('res', 'shaders/bytecode/0008', vnvn(S('ps', 207), 1), CB, p00, None)])
chunks_list = dbo_obj('installChunks', [
    dbo_obj(None, [dbo_str('name', 'superbundlelayout/installpackage_00'), dbo_str('installBundle', ''), dbo_i32('persistentIndex', CH0 - (1 << 32))]),
    dbo_obj(None, [dbo_str('name', 'superbundlelayout/installpackage_12'), dbo_bool('optional', 1), dbo_i32('persistentIndex', 7)])], kind=1)
layout = toc_file(dbo_obj(None, [dbo_str('version', '2025.1.2'), dbo_obj('installManifest', [dbo_i32('count', 2), chunks_list])]))
make_game('sync-frostbite', {
    'Data/layout.toc': layout,
    'Data/initfs_NX': Rng(8).bytes(0x200),
    'Data/NX/game.toc': toc([b1, b2]),
    'Data/NX/empty.toc': TOC_MAGIC + bytes(0x40),
    'Data/NX/sub/more.toc': toc([b3]),
    'Data/NX/superbundlelayout/installpackage_00/cas_01.cas': bytes(c00.d),
    'Data/NX/superbundlelayout/installpackage_00/cas_02.cas': bytes(c01.d),
    'Data/NX/superbundlelayout/installpackage_12/cas_12.cas': bytes(c12.d),
    'Patch/NX/patch.toc': toc([b4]),
    'Patch/NX/superbundlelayout/installpackage_00/cas_01.cas': bytes(p00.d),
    'Pack/loose.glslc': glslc([S('ps', 208)]),
}, bytes(0x10) + clean(Rng(9).bytes(0x1000)))
