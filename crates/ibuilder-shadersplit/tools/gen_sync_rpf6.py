#!/usr/bin/env python3
"""Writes tests/data/games/sync-rpf6 and sync-rpf6-setup: synthetic games for the Rockstar RPF6 reader (rpf6.py, rpf_jobs), and tests/data/vectors-rpf6.jsonl.gz, the archives damaged
in many places with what rpf6.py answers for each. Every byte is made here; the AES key is MADE UP (32 distinct bytes), never a game's. The archives are encrypted with it through
CommonCrypto (as dm/tools/tests/test_rpf6.py does); sync-rpf6's main holds the key as a 32-byte window of its data segment (find_key's window search), sync-rpf6-setup's only through a
synthetic key setup function in its code (key_from_setup: the eight key words as 64-bit constants in its read-only segment).

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_sync_rpf6.py <upstream checkout (the folder that holds dm/)> tests/data/games

macOS only (CommonCrypto).
"""
import ctypes, gzip, hashlib, json, os, struct, sys, tempfile, zlib
from compression import zstd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_sync5 as g5   # the builders and the run (its own games are made only when it runs as a script)
from gen_sync5 import Rng, clean, program, glslc

engine = g5.engine
tools = engine + '/dm/gfx/tools'
for sub in ('', '/../../tools'): sys.path.insert(0, os.path.abspath(tools + sub))
import rpf6

KEY = bytes((0x3b * i + 0x11) & 0xff for i in range(32))   # made up: 32 distinct bytes
assert len(set(KEY)) == 32


def aes_encrypt(key, data):
    cc = ctypes.CDLL('/usr/lib/libSystem.B.dylib').CCCrypt
    cc.argtypes = [ctypes.c_uint32, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t,
                   ctypes.c_void_p, ctypes.c_size_t, ctypes.POINTER(ctypes.c_size_t)]
    out, moved = ctypes.create_string_buffer(len(data)), ctypes.c_size_t()
    if cc(0, 0, 2, key, len(key), None, data, len(data), out, len(data), ctypes.byref(moved)): raise ValueError('CCCrypt failed')
    return out.raw


def encrypt(key, data):
    for _ in range(rpf6.ROUNDS): data = aes_encrypt(key, data)
    return data


def rgxd(stages):
    """a RAGE effect: parameter names, then per shader {u32 length, GLSLC magic, u32 code length, u32 control length}, the code area, the control section (packed)"""
    d = bytearray(b'rgxd' + b'\x02\0\0\0gWorld\0gColor\0\x01')
    for ctl, code in stages: d += struct.pack('<4I', 0x2df8, 0x19866891, len(code), len(ctl)) + code + ctl
    return bytes(d)


def archive(tree, key=KEY, encrypted=True, extra_names=()):
    """tree: [(name, node)], node = [(name, node)] (a directory) or (data, kind): kind 'plain', 'zstd', 'zlib', 'deflate', 'rsc5', 'rsc85', 'script', 'unnamed', 'badzstd'"""
    entries, blobs, names = [(0, 0, 0x80000001, len(tree), 0)], [], [b'']
    def add(items, at):
        q = []
        for name, node in items:
            i = len(entries); entries.append(None); q.append((i, name, node))
        for i, name, node in q:
            h = rpf6.joaat(name)
            if not (isinstance(node, tuple) and node[1] == 'unnamed'): names.append(name.encode())
            if isinstance(node, list):
                first = len(entries)
                entries[i] = (h, 0, 0x80000000 | first, len(node), 0)
                add(node, first)
                continue
            data, kind = node
            if kind == 'plain': blob, c, rf = data, len(data), None
            elif kind in ('zstd', 'unnamed'): blob, c, rf = zstd.compress(data), 0x40000000 | len(data), None
            elif kind == 'badzstd': blob, c, rf = zstd.compress(data)[:-7], 0x40000000 | len(data), None
            elif kind == 'garbage': blob, c, rf = b'\x00\x01' + data[2:], 0x40000000 | len(data), None
            elif kind == 'zlib': blob, c, rf = zlib.compress(data), 0x40000000 | len(data), None
            elif kind == 'deflate': o = zlib.compressobj(9, zlib.DEFLATED, -15); blob, c, rf = o.compress(data) + o.flush(), 0x40000000 | len(data), None
            elif kind == 'rsc5':
                rf = 0x80000000 | 0x40000000 | 0x123; blob, c = rpf6.RSC5 + struct.pack('<II', 0x1e, rf) + zlib.compress(data), rf
            elif kind == 'rsc85':
                rf = 0x80000000; blob, c = rpf6.RSC85 + struct.pack('<III', 2, rf, 0) + data, rf
            elif kind == 'script':
                z = zstd.compress(data); n = len(z) // 16 * 16
                blob, c, rf = (encrypt(key, z[:n]) if encrypted else z[:n]) + z[n:], 0x40000000 | len(data), None
            blobs.append((i, h, blob, c, rf))
    add(tree, 1)
    names += [n.encode() for n in extra_names]
    table_n = len(entries) * 20
    names_at = (0x10 + table_n + 15) // 16 * 16
    name_tab = b'\0\0\0\0' + b'\0'.join(names) + b'\0'
    name_tab += bytes(-len(name_tab) % 16)
    data_at = (names_at + len(name_tab) + 0x7ff) // 0x800 * 0x800
    d = bytearray(data_at); pos = data_at
    for i, h, blob, c, rf in blobs:
        pos = (pos + 0x7ff) // 0x800 * 0x800
        off_word = (pos // 8) | (0x1e if rf is not None else 0)
        entries[i] = (h, len(blob), off_word, c, 0)
        d += bytes(pos - len(d)) + blob; pos += len(blob)
    table = b''.join(struct.pack('>5I', *e) for e in entries); table += bytes(-len(table) % 16)
    if encrypted: table, name_tab = encrypt(key, table), encrypt(key, name_tab)
    d[:16] = rpf6.MAGIC + struct.pack('>3I', len(entries), names_at // 8, rpf6.ENCRYPTED if encrypted else 0)
    d[0x10:0x10 + len(table)] = table
    d[names_at:names_at + len(name_tab)] = name_tab
    return bytes(d)


def nso3(text, ro, data):
    """an uncompressed NSO with three segments at page-aligned memory offsets"""
    pg = lambda n: (n + 0xfff) & ~0xfff
    h = bytearray(0x100); h[:4] = b'NSO0'
    segs = [(0x100, 0, len(text)), (0x100 + len(text), pg(len(text)), len(ro)), (0x100 + len(text) + len(ro), pg(len(text)) + pg(len(ro)), len(data))]
    for i, s in enumerate(segs): struct.pack_into('<III', h, 0x10 + 0x10 * i, *s)
    return bytes(h) + text + ro + data


def adrp(a, rd, target):
    imm = ((target & ~0xfff) - (a & ~0xfff)) >> 12
    return 0x90000000 | (imm & 3) << 29 | ((imm >> 2) & 0x7ffff) << 5 | rd


def setup_text(ro_at, consts_off, decoy=True):
    """code with the key setup's first instructions, b.ne, adrp x9; ldr q0-q3 from the constants; add x8, x0, #8; stp q0, q1, [x8]; stp q2, q3, [x8, #32]; b (as test_rpf6.setup_image);
    a decoy setup first that stores other words (wider than 32 bits: no candidate)"""
    t = bytearray(clean(Rng(40).bytes(0x1000)))
    ldr_q = lambda rt, rn, off: 0x3dc00000 | (off // 16) << 10 | rn << 5 | rt
    stp_q = lambda rt, rt2, rn, off: 0xad000000 | ((off // 16) & 0x7f) << 15 | rt2 << 10 | rn << 5 | rt
    add_x = lambda rd, rn, imm: 0x91000000 | imm << 10 | rn << 5 | rd
    for n, base in enumerate([consts_off + 64, consts_off] if decoy else [consts_off]):
        at = 0x200 + n * 0x100
        t[at:at + 20] = rpf6.SETUP
        a = at + 20
        code = [0x54000001, adrp(a + 4, 9, ro_at + base)] + [ldr_q(i, 9, ((ro_at + base) & 0xfff) // 16 * 16 + i * 16) for i in range(4)]
        code += [add_x(8, 0, 8), stp_q(0, 1, 8, 0), stp_q(2, 3, 8, 32), 0x14000000]
        for i, w in enumerate(code): struct.pack_into('<I', t, a + 4 * i, w)
    return bytes(t)


def tree(key=KEY):
    vs, ps, cs = program('vs', 60), program('ps', 61), program('cs', 62)
    return [
        ('shaders', [('fx_basic.nvn', (rgxd([vs, ps]), 'zstd')), ('fx_cs.nvn', (rgxd([cs]), 'zlib')), ('fx_more.nvn', (rgxd([program('vs', 63), program('ps', 64)]), 'zstd')),
                     ('sub', [('fx_deep.nvn', (rgxd([program('ps', 65)]), 'deflate'))])]),
        ('readme.txt', (b'plain text, nothing to read', 'plain')),
        ('res', [('r.wtx', (rgxd([program('vs', 66)]), 'rsc5')), ('g.wgd', (b'stored resource data ' * 4, 'rsc85'))]),
        ('scripts', [('main.wsc', (rgxd([program('ps', 67)]), 'script'))]),
        ('nameless.bin', (rgxd([program('vs', 68)]), 'unnamed')),
        ('broken.nvn', (rgxd([program('vs', 69)]), 'garbage')),
    ]


def main_with_window():
    data = bytearray(clean(Rng(41).bytes(0x800)))
    data[0x100:0x120] = bytes(range(0x40, 0x60))   # a decoy window (another key)
    data[0x400:0x420] = KEY
    return nso3(clean(Rng(42).bytes(0x1000)), clean(Rng(43).bytes(0x400)), bytes(data))


def main_with_setup():
    ro = bytearray(clean(Rng(44).bytes(0x400)))
    words = struct.unpack('>8I', KEY)
    ro[0x100:0x140] = struct.pack('<8Q', *words)
    ro[0x140:0x180] = struct.pack('<8Q', *[(1 << 32) | w for w in words])   # the decoy's: wider than 32 bits
    return nso3(setup_text(0x1000, 0x100), bytes(ro), clean(Rng(45).bytes(0x200)))


if __name__ == '__main__':
    out_root = g5.out_root
    t = tree()
    files = {'Data/shaders.rpf': archive(t), 'Data/other.rpf': archive([('a.nvn', (rgxd([program('vs', 70), program('ps', 71)]), 'zstd'))], extra_names=('unused_name', 'x' * 300))}
    g5.make_game('sync-rpf6', files, main=main_with_window())
    g5.make_game('sync-rpf6-setup', {'Data/shaders.rpf': archive(t)}, main=main_with_setup())

    # ---- the damaged vectors (tests/rpf6.rs) ----
    import nso
    M = (1 << 64) - 1

    class Lcg:
        def __init__(s, seed): s.x = seed & M
        def below(s, n):
            s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
            return (s.x >> 24) % n
    rng = Lcg(0x4bf6)
    h16 = lambda b: hashlib.sha256(bytes(b)).hexdigest()[:16]

    def text(e, p): return (str(e).removeprefix('[RED] ') if isinstance(e, SystemExit) else f'{type(e).__name__}: {e}').replace(p, 'P')

    def mutate(b, regions, cuts=True):
        muts, cut = [], None
        for _ in range(1 + rng.below(3)):
            lo, hi = regions[rng.below(len(regions))]
            hi = min(hi, len(b)); lo = min(lo, hi - 1)
            muts.append([lo + rng.below(max(1, hi - lo)), rng.below(256)])
        if cuts and rng.below(8) == 0: cut = rng.below(len(b))
        d = bytearray(b)
        for o, v in muts: d[o] = v
        return muts, cut, bytes(d if cut is None else d[:cut])
    tmp = tempfile.mkdtemp(); P = os.path.join(tmp, 'x.rpf')

    def outcome(d):
        open(P, 'wb').write(d)
        try: r = rpf6.Rpf(P, KEY)
        except Exception as e: return {'err': text(e, P)}
        files = []
        with open(P, 'rb') as f:
            for e in r.files:
                try: files.append([list(e), {'ok': h16(r.read(f, e, zstd))}])
                except Exception as x: files.append([list(e), {'err': text(x, P)}])
        return {'ok': files}
    recs = []
    for k in range(8):   # AES-256 against CommonCrypto: random keys and blocks
        key, pt = Rng(100 + k).bytes(32), Rng(200 + k).bytes(48)
        recs.append({'kind': 'aes', 'key': key.hex(), 'ct': aes_encrypt(key, pt).hex(), 'pt': pt.hex()})
    base = archive(t)
    with_bad = archive(t[:-1] + [('broken.nvn', (rgxd([program('vs', 69)]), 'badzstd'))])
    recs.append({'kind': 'rpf-base', 'name': 'with_bad', 'hex': with_bad.hex()})
    names_at = struct.unpack_from('>I', base, 8)[0] * 8
    for k in range(400):
        b, name = (base, 'shaders') if k % 5 else (with_bad, 'with_bad')
        muts, cut, d = mutate(b, [(0, 16), (16, names_at), (names_at, names_at + 0x200), (names_at, len(b)), (0, len(b))])
        recs.append({'kind': 'rpf', 'base': name, 'muts': muts, 'cut': cut, 'out': outcome(d)})
    recs.append({'kind': 'rpf', 'base': 'shaders', 'muts': [], 'cut': None, 'out': outcome(base)})
    m = nso.load(main_with_setup(), 'm'); img = bytes(m.image); text_seg = m.segments[0]
    recs.append({'kind': 'setup-base', 'hex': img.hex(), 'text': list(text_seg)})
    for k in range(200):
        muts, _, d = mutate(img, [(0x200, 0x260), (0x300, 0x360), (0x1100, 0x1180)], cuts=False)
        try: o = {'ok': [c.hex() for c in rpf6.key_from_setup(d, text_seg)]}
        except Exception as e: o = {'err': f'{type(e).__name__}: {e}'}
        recs.append({'kind': 'setup', 'muts': muts, 'cut': None, 'out': o})
    m = nso.load(main_with_window(), 'm'); img = bytes(m.image)
    recs.append({'kind': 'window-base', 'hex': img.hex(), 'segments': [list(s) for s in m.segments]})
    blk = base[16:32]
    for k in range(40):
        muts, _, d = mutate(img, [(m.segments[2][0] + 0x3f0, m.segments[2][0] + 0x430)], cuts=False)
        # find_key's search on this image (find_key itself loads the NSO from a path; the same two loops)
        found = None
        for k2 in rpf6.key_from_setup(d, m.segments[0]):
            if rpf6.is_root(rpf6.decrypt(k2, blk)): found = k2; break
        if found is None:
            for mo, sz in m.segments[1:]:
                for a in range(mo, mo + sz - 32, 4):
                    w = d[a:a + 32]
                    if len(set(w)) >= 24 and rpf6.is_root(rpf6.decrypt(w, blk)): found = w; break
                if found: break
        recs.append({'kind': 'window', 'muts': muts, 'cut': None, 'out': found.hex() if found else None})
    with gzip.open(os.path.join(out_root, '../vectors-rpf6.jsonl.gz'), 'wt') as f:
        for r in recs: f.write(json.dumps(r, sort_keys=True) + '\n')
    print(len(recs), 'vectors')
