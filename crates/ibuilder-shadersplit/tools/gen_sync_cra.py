#!/usr/bin/env python3
"""Writes tests/data/games/sync-cra and sync-cra2021: synthetic games for Camelot's CRA archive (cra.py, cra_jobs, scan_blob's NARC branch), and tests/data/vectors-cra.jsonl.gz.
Every byte is made here. The archive's files are SCRAMBLED here (the inverse of cra.unscramble: a block's word that the CRC is taken of is solved for, the CRC of 4 bytes being a
bijection) and the file table is put in a synthetic NRO whose code holds the instruction patterns cra.index_of / cra.index_2021 look for; upstream's own cra.py reads them back.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_sync_cra.py <upstream checkout (the folder that holds dm/)> tests/data/games
"""
import gzip, hashlib, json, os, struct, sys, tempfile, zlib
import lz4.block
from compression import zstd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_sync5 as g5
from gen_sync5 import Rng, clean, program, shad
from gen_sync_rpf6 import rgxd

tools = g5.engine + '/dm/gfx/tools'
for sub in ('', '/../../tools'): sys.path.insert(0, os.path.abspath(tools + sub))
import cra

M64 = (1 << 64) - 1


# ---- scrambling (the inverse of cra.unscramble) ---------------------------------------------------------------------------------------
def hi_map(tab):
    """the CRC of a 64-bit word whose low half is zero, as a function of its high half: linear over GF(2); its inverse by elimination"""
    def H(h): return cra.crc64(tab, 0, h << 32)
    cols = [H(1 << j) for j in range(32)]
    # solve H(x) = y: rows = output bits; Gaussian elimination on the augmented system, once per y (32 x 32)
    def inv(y):
        rows = [[(cols[j] >> b) & 1 for j in range(32)] + [(y >> b) & 1] for b in range(32)]
        r = 0
        for c in range(32):
            p = next(i for i in range(r, 32) if rows[i][c])
            rows[r], rows[p] = rows[p], rows[r]
            for i in range(32):
                if i != r and rows[i][c]: rows[i] = [a ^ b for a, b in zip(rows[i], rows[r])]
            r += 1
        x = sum(rows[c][32] << c for c in range(32))
        assert H(x) == y
        return x
    return inv
INV = {id(cra.CRC32): hi_map(cra.CRC32), id(cra.CRC32C): hi_map(cra.CRC32C)}


def scramble(path, plain):
    """→ (the stored bytes, the checksum read() wants)"""
    plain = plain + bytes(-len(plain) % 16)
    w = cra.key(path); v = w & 7
    tab = cra.CRC32C if v & 4 else cra.CRC32
    mul = (w & 0xfffffffe ^ cra.VARIANT_MASK[v]) << 32 | 1
    src, oth, a, b = (1, 0, 0, 1) if v & 2 else (0, 1, 0, 1)
    if v & 1: a, b = 1, 0
    q = list(struct.unpack(f'<{len(plain) // 8}Q', plain)); out = [0] * len(q); total = 0
    for i in range(0, len(q), 2):
        A, E = q[i + a], q[i + b]
        crc = ~A & 0xffffffff
        m = crc * mul & M64
        lo = (~(A >> 32) ^ (m >> 32)) & 0xffffffff
        hi = INV[id(tab)](crc ^ cra.crc64(tab, 0, lo ^ w))
        s = hi << 32 | lo
        out[i + src] = s; out[i + oth] = E ^ (m >> 3); total += E
    d = struct.pack(f'<{len(out)}Q', *out)
    assert cra.unscramble(path, d)[0] == plain
    return d, ((total & M64) >> 32 ^ total) & 0xffffffff


# ---- the NRO ------------------------------------------------------------------------------------------------------------------------------
def adrp(pc, rd, target):
    imm = (target >> 12) - (pc >> 12)
    return 0x90000000 | (imm & 3) << 29 | ((imm >> 2) & 0x7ffff) << 5 | rd


def add(rd, imm): return 0x91000000 | (imm & 0xfff) << 10 | rd << 5 | rd


TEXT = 0x2000   # the text segment; the tables follow it


def nro(words_at, ro):
    """an NRO: header, a text segment of zeros with the given instruction words, then ro (the tables)"""
    t = bytearray(TEXT)
    t[0x10:0x14] = b'NRO0'; struct.pack_into('<II', t, 0x18, TEXT + len(ro), 0)
    struct.pack_into('<6I', t, 0x20, 0, TEXT, TEXT, len(ro), TEXT + len(ro), 0)
    for i, w in words_at.items(): struct.pack_into('<I', t, 4 * i, w)
    return bytes(t) + bytes(ro)


def index_today(entries, suffixes):
    """entries: [(stored name with '$', suffix index or None, size, offset, checksum)], record 0 empty; → (NRO bytes)"""
    count = len(entries) + 1
    table = TEXT
    sfx_at = None
    recs, own = [bytes(0xe)], ['']   # record 0: size word 0 → size 0 after the xor? no: 0 ^ 0xff00ff00; written below
    body = []
    for i, (name, sfx, size, off, _) in enumerate(entries, 1):
        prev = own[i - 1]
        p = 0
        while p < min(len(name), len(prev)) and name[p] == prev[p]: p += 1
        if i == 1: p = 0
        own.append(name)
        chars = name[p:]
        enc = bytes((i - ord(c)) & 0x7f for c in chars)
        assert 0 not in enc, (i, name)
        r = struct.pack('<HBb', 1 if p else 0, p, -1 if sfx is None else sfx) + struct.pack('<I', size ^ 0xff00ff00) + ((off ^ 0xff00ff00ff) & 0xffffffffff).to_bytes(5, 'little') + enc + b'\0'
        recs.append(r)
    recs[0] = struct.pack('<HBb', 0, 0, -1) + struct.pack('<I', 0xff00ff00) + bytes(5) + b'\0'
    ro = bytearray(2 * count)
    pos = 2 * count
    for i, r in enumerate(recs):
        struct.pack_into('<h', ro, 2 * i, pos - (2 * count + 18 * i))
        ro += r; pos += len(r)
    ro += bytes(-len(ro) % 8)
    sums_at = table + len(ro)
    ro += struct.pack(f'<{count}I', 0, *[e[4] for e in entries])
    ro += bytes(-len(ro) % 8)
    strs = []
    for k, s in enumerate(suffixes):
        strs.append(bytes((k - ord(c)) & 0x7f for c in s) + b'\0')
        assert 0 not in strs[-1][:-1]
    names_at = table + len(ro)
    ro += bytes(8 * len(suffixes))
    for k, s in enumerate(strs):
        struct.pack_into('<Q', ro, names_at - table + 8 * k, table + len(ro)); ro += s
    w = {}
    i = 0x100   # the table read: adrp + add x9; mov w1, #0x12; ldrsh; madd; mov w3, #(2 × count)
    w.update({i - 3: adrp(4 * (i - 3), 9, table), i - 2: add(9, table & 0xfff), i - 1: 0x52800241, i: 0x78e0d800 | 9 << 5 | 2, i + 1: 0x1b020c20, i + 2: 0x52800000 | (2 * count) << 5 | 3})
    j = 0x200   # the checksums: adrp + add x10; mov x8, #'UseU'; movk
    w.update({j - 2: adrp(4 * (j - 2), 10, sums_at), j - 1: add(10, sums_at & 0xfff), j: 0xd28e6aa8, j + 1: 0xf2aaaca8})
    k = 0x300   # the suffixes: ldrsb w2, [x1, #3]; tbnz; and; adrp + add x11; ldr x3, [x11, x2, lsl #3]
    w.update({k: 0x39c00c22, k + 1: 0x37000000, k + 2: 0x92401c42, k + 3: adrp(4 * (k + 3), 11, names_at), k + 4: add(11, names_at & 0xfff), k + 5: 0xf8607800 | 11 << 5 | 3})
    return nro(w, ro)


REV8 = cra.REV8


def index_2021(entries):
    """entries: [(name, size, offset, checksum)], record 0 empty"""
    count = len(entries) + 1
    table = TEXT
    recs, own = [bytes(0x12)], ['']
    for i, (name, size, off, ck) in enumerate(entries, 1):
        prev = own[i - 1]; p = 0
        while p < min(len(name), len(prev)) and name[p] == prev[p]: p += 1
        if i == 1: p = 0
        own.append(name)
        k = (i & 15) + 1
        enc = bytes(REV8[ord(c) ^ k] for c in name[p:])
        assert 0 not in enc
        recs.append(struct.pack('<HBB', 1 if p else 0, p, 0) + struct.pack('<II', size ^ cra.SIZE_2021, ck) + ((off ^ cra.OFF_2021) & 0xffffffffff).to_bytes(5, 'little') + enc + b'\0')
    ro = bytearray(4 * count); pos = 4 * count
    for i, r in enumerate(recs):
        struct.pack_into('<i', ro, 4 * i, -pos); ro += r; pos += len(r)
    ro += bytes(16)
    w = {}
    i = 0x180   # adrp + add x12, table; ldrsw [x12, x?, lsl #2]; ...; mov w5, #0x2eca; movk w5, #0xce46, lsl #16
    w.update({i - 6: adrp(4 * (i - 6), 12, table), i - 5: add(12, table & 0xfff), i - 4: 0xb8a07800 | 12 << 5 | 4, i: 0x52800000 | 0x2eca << 5 | 5, i + 1: 0x72a00000 | 0xce46 << 5 | 5})
    return nro(w, ro)


# ---- NARC and the members' packing ----------------------------------------------------------------------------------------------------
def narc(members):
    data = bytearray(); fat = bytearray()
    for m in members:
        data += bytes(-len(data) % 4); a = len(data); data += m; fat += struct.pack('<II', a, len(data))
    btaf = b'BTAF' + struct.pack('<II', 12 + len(fat), len(members)) + fat
    btnf = b'BTNF' + struct.pack('<I', 16) + bytes(8)
    gmif = b'GMIF' + struct.pack('<I', 8 + len(data)) + data
    return b'NARC' + struct.pack('<HHIHH', 0xfffe, 0x100, 16 + len(btaf) + len(btnf) + len(gmif), 16, 3) + btaf + btnf + gmif


def lz_pack(d, head):
    """Camelot's LZ (kinds 0x50 and 0x60): greedy matches of 3..15 bytes (and long ones), literals, a distance-0 end"""
    toks, i = [], 0
    while i < len(d):
        best = (0, 0)
        for dist in range(1, min(i, 4095) + 1):
            n = 0
            while i + n < len(d) and n < 300 and d[i + n - dist] == d[i + n]: n += 1
            if n > best[1]: best = (dist, n)
        if best[1] >= 3: toks.append(best); i += best[1]
        else: toks.append(d[i]); i += 1
    toks.append((0, 0))
    out = bytearray(head)
    for g in range(0, len(toks), 8):
        grp = toks[g:g + 8]
        f = 0
        for bit, t in enumerate(grp):
            if isinstance(t, tuple): f |= 1 << (7 - bit)
        if f == 0 and len(grp) == 8:
            out += b'\0' + bytes(grp); continue
        out.append(-f & 0xff)
        for t in grp:
            if not isinstance(t, tuple): out.append(t); continue
            dist, n = t
            if dist == 0: out += b'\0\0'; break
            if 2 <= n <= 15: out += bytes([(dist & 0xf) << 4 | n, dist >> 4])
            elif n <= 0x10f: out += bytes([(dist & 0xf) << 4, dist >> 4, n - 0x10])
            else:
                x = n - 0x11 - 0xff; out += bytes([(dist & 0xf) << 4 | 1, dist >> 4, x & 0xff, x >> 8])
        if isinstance(grp[-1], tuple) and grp[-1][0] == 0: break
    return bytes(out)


def pack(kind, d):
    if kind == 'raw': return d
    if kind == 'lz4': return b'\x94\0\0\0' + struct.pack('<I', len(d)) + lz4.block.compress(d, store_size=False)
    if kind == 'zstd': return b'Z\0\0\0' + struct.pack('<I', len(d)) + zstd.compress(d)
    if kind == 'lz50': return lz_pack(d, b'\x50' + bytes(7))
    if kind == 'lz60': return lz_pack(d, b'\x60' + bytes(3))
    if kind == 'k70': return b'\x70' + bytes(15) + d
    if kind == 'zlib': return b'\x00' + bytes(15) + zlib.compress(d)
    if kind == 'gzip': return b'\x10' + bytes(15) + gzip.compress(d, mtime=0)
    if kind == 'deflate': o = zlib.compressobj(9, zlib.DEFLATED, -15); return b'\x30' + bytes(15) + o.compress(d) + o.flush()
    if kind == 'notzlib': return b'\x20' + bytes(15) + d   # a member that only starts with such a byte: itself
    raise ValueError(kind)


def mmmm(parts):
    n = len(parts); out = bytearray(b'MMMM' + struct.pack('<II', sum(len(p) for p in parts), n) + b''.join(struct.pack('<I', len(p)) for p in parts))
    out += bytes(-len(out) % 16)
    for p in parts: out += p + bytes(-len(p) % 16)
    return bytes(out)


R, SH = rgxd, shad
P = lambda st, s: program(st, s)


def content(seed):
    """NARC containers with members of every packing, rgxd effects and SHAD containers in them"""
    return {
        'shader/basic.narc': narc([pack('lz4', R([P('vs', seed), P('ps', seed + 1)])), pack('zstd', SH(P('cs', seed + 2))), pack('raw', R([P('vs', seed + 3)])),
                                   pack('zlib', SH(P('ps', seed + 4))), pack('notzlib', b'plain member bytes' * 3)]),
        'shader/font/$.narc': narc([pack('lz50', R([P('vs', seed + 5), P('ps', seed + 6)])), pack('lz60', SH(P('vs', seed + 7))), pack('k70', R([P('ps', seed + 8)])),
                                    pack('deflate', SH(P('cs', seed + 9))), pack('gzip', R([P('vs', seed + 10)]))]),
        'shader/chunks.narc': narc([mmmm([pack('lz4', R([P('ps', seed + 11)])), pack('raw', b'x' * 40), pack('zstd', R([P('vs', seed + 12)]))]),
                                    pack('raw', narc([pack('lz4', SH(P('ps', seed + 13)))]))]),
        'sound/bgm_title.bfstm': b'FSTM' + bytes(60),
        'movie/op.webm': b'\x1aE\xdf\xa3' + bytes(60),
        'model/kart.bfres': b'FRES' + clean(Rng(seed).bytes(200)),
        'shader/bad.narc': narc([pack('lz50', R([P('vs', seed + 14)]))[:-6] + b'\x01\xf0']),
        'shader/sum.narc': narc([pack('raw', R([P('vs', seed + 15)]))]),
    }


def archive_today(files, bad_sum=('shader/sum.narc',)):
    """→ (rom:/42 bytes, NRO bytes); paths with '$' and a suffix from the suffix table"""
    suffixes = ['.narc', '.bfres']
    arch = bytearray(b'CRA.' + struct.pack('<HHII', 0x200, 0x100, 0x2a0, 0) + bytes(0x30))
    ents = []
    for path, data in files.items():
        stored, sfx = path, None
        for k, s in enumerate(suffixes):
            if path.endswith(s): stored, sfx = path[:-len(s)], k
        full = path.replace('$', path.split('/')[-2]) if '$' in path else path
        arch += bytes(-len(arch) % 16)
        d, ck = scramble(full, data)
        if path in bad_sum: ck ^= 1
        ents.append((stored, sfx, len(data), len(arch), ck)); arch += d
    return bytes(arch), index_today(ents, suffixes)


def archive_2021(files):
    arch = bytearray(b'CRA.' + struct.pack('<HHII', 0x200, 0x100, 0x2a0, 0) + bytes(0x30))
    ents = []
    for path, data in files.items():
        n = len(data)
        ks = cra.keys_2021(path)
        stored = bytes(REV8[x] ^ ks[i % 16] for i, x in enumerate(data))
        assert cra.plain_2021(path, stored) == data
        ck = zlib.crc32(data) ^ cra.SUM_2021
        if path == 'shader/sum.narc': ck ^= 1
        arch += bytes(-len(arch) % 16); ents.append((path, n, len(arch), ck)); arch += stored
    return bytes(arch), index_2021(ents)


if __name__ == '__main__':
    out_root = g5.out_root
    files = content(80)
    a42, nro_b = archive_today(files)
    loose = narc([pack('lz4', R([P('vs', 120), P('ps', 121)]))])
    other = nro({}, bytes(0x40))   # an NRO without the table: skipped
    g5.make_game('sync-cra', {'42': a42, 'Data/loose.narc': loose}, nro={'a_other.nro': other, 'b_main.nro': nro_b})
    a2021, n2021 = archive_2021(content(140))
    g5.make_game('sync-cra2021', {'42': a2021, 'Data/loose.narc': loose}, nro={'a_other.nro': other, 'b_main.nro': n2021})

    # ---- the damaged vectors (tests/cra.rs) ----
    class Lcg:
        def __init__(s, seed): s.x = seed
        def below(s, n):
            s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M64
            return (s.x >> 24) % n
    rng = Lcg(0xc4a)
    h16 = lambda b: hashlib.sha256(bytes(b)).hexdigest()[:16]
    text = lambda e: f'{type(e).__name__}: {e}'
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
    recs = []
    idx_t = cra.index(nro_b).files()
    idx_2 = cra.index(n2021).files()
    recs.append({'kind': 'bases', 'nro': nro_b.hex(), 'nro2021': n2021.hex(), 'a2021': a2021.hex()})
    import signal
    def alarm(*_): raise TimeoutError('the prefix chain does not end')
    signal.signal(signal.SIGALRM, alarm)
    def files_of(n):
        signal.alarm(3)
        try: return files_of_in(n)
        except TimeoutError: return {'hang': True}
        finally: signal.alarm(0)
    def files_of_in(n):
        try: idx = cra.index(n)
        except cra.Error as e: return {'skip': str(e)}
        except Exception as e: return {'crash': type(e).__name__}
        try: return {'ok': [list(e) for e in idx.files()]}
        except Exception as e: return {'crash': type(e).__name__}
    # the instruction patterns and the table heads of the NROs (the records' prefix chains are left whole: a damaged one can make the Python loop for ever)
    for base, regions in [(nro_b, [(0x10, 0x28), (4 * 0xfd, 4 * 0x103), (4 * 0x1fe, 4 * 0x202), (4 * 0x300, 4 * 0x306), (TEXT, TEXT + 2 * (len(idx_t) + 1))]),
                          (n2021, [(0x10, 0x28), (4 * 0x17a, 4 * 0x182), (TEXT, TEXT + 4)])]:
        for k in range(120):
            muts, _, d = mutate(base, regions, cuts=False)
            recs.append({'kind': 'index', 'base': 'nro' if base is nro_b else 'nro2021', 'muts': muts, 'cut': None, 'out': files_of(d)})
    # read() / head() of every file from a damaged archive
    import io
    for base, ents, name in [(a42, idx_t, '42'), (a2021, idx_2, 'a2021')]:
        regions = [(e[3], e[3] + min(e[2], 0x40)) for e in ents] + [(0, len(base))]
        for k in range(100):
            muts, cut, d = mutate(base, regions)
            f = io.BytesIO(d); res = []
            for e in ents:
                try: hd = h16(cra.head(f, e))
                except Exception as x: hd = text(x)
                try: res.append([hd, {'ok': h16(cra.read(f, e))}])
                except Exception as x: res.append([hd, {'err': text(x)}])
            recs.append({'kind': 'read', 'base': name, 'muts': muts, 'cut': cut, 'out': res})
    # narc_members + undo of damaged containers
    for path in ('shader/basic.narc', 'shader/font/$.narc', 'shader/chunks.narc'):
        b = files[path]
        for k in range(90):
            muts, cut, d = mutate(b, [(0, 0x40), (0, len(b))])
            try: ms = cra.narc_members(d, 'N')
            except cra.Error as e: recs.append({'kind': 'narc', 'base': path, 'muts': muts, 'cut': cut, 'out': {'err': str(e)}}); continue
            res = []
            for m in ms:
                try: res.append({'ok': h16(cra.undo(m))})
                except Exception as x: res.append({'err': text(x)})
            recs.append({'kind': 'narc', 'base': path, 'muts': muts, 'cut': cut, 'out': {'ok': res}})
    # undo() of damaged members of every packing (the base members travel in the vector file)
    body = R([P('vs', 160), P('ps', 161)]) + bytes(200) + R([P('vs', 160)])
    for kind in ('lz4', 'zstd', 'lz50', 'lz60', 'zlib', 'gzip', 'deflate', 'k70', 'mmmm'):
        m = mmmm([pack('lz50', body[:900]), pack('lz4', body[900:]), b'\x20' + bytes(20)]) if kind == 'mmmm' else pack(kind, body)
        recs.append({'kind': 'member-base', 'base': kind, 'hex': m.hex()})
        for k in range(60):
            muts, cut, d = mutate(m, [(0, 0x20), (0, len(m)), (len(m) - 0x20, len(m))])
            try: o = {'ok': h16(cra.undo(d))}
            except Exception as x: o = {'err': text(x)}
            recs.append({'kind': 'member', 'base': kind, 'muts': muts, 'cut': cut, 'out': o})
    with gzip.open(os.path.join(out_root, '../vectors-cra.jsonl.gz'), 'wt') as f:
        for r in recs: f.write(json.dumps(r, sort_keys=True) + '\n')
    print(len(recs), 'vectors')
