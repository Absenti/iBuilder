#!/usr/bin/env python3
"""Writes tests/data/games/sync-lspk, sync-casc, sync-pdat: synthetic games for the readers ported from nxnative's lspk.py (Larian LSPK v13 + BSHD), casc.py (Blizzard CASC) and
pdat_jobs (phx PDAT). Each game is a RomFS folder, an NSO `main` and a profile, every byte made here (nothing comes from a game), split by the UPSTREAM engine's own `shader_split.py`
(its command line); `expected.json` has the sha256 of what it wrote and its log, in the format of tests/games.rs, which splits the same folders with this crate at 1, 4 and 8 threads.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_sync5.py <upstream checkout (the folder that holds dm/)> tests/data/games

shader_map.tsv line 2 and the @ROMFS@ path: as tools/gen_sync_readers.py.
"""
import hashlib, json, os, shutil, struct, subprocess, sys, tempfile, zlib
import lz4.block
from compression import zstd

engine, out_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
GAME = '0100TESTTEST0000'
GAME_TSV = f'title_id\t{GAME}\nshader_groups\t/Data/=data /data/=casc /Pack/=pack /aoc/=dlc\nshader_note\tsynthetic game\n'
PORT_LINE2 = '# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied\n'
CTL, GFX, CS, GLSLC = bytes.fromhex('34127698'), bytes.fromhex('78563412'), bytes.fromhex('79563412'), bytes.fromhex('91688619')
SPH = {'vs': 1, 'hs': 2, 'ds': 3, 'gs': 4, 'ps': 5}


def sha(b): return hashlib.sha256(b).hexdigest()


class Rng:   # xorshift64*, so the bytes do not depend on Python's random module
    def __init__(self, s): self.s = s or 1
    def u64(self):
        x = self.s; x ^= x >> 12; x ^= (x << 25) & (2**64 - 1); x ^= x >> 27; self.s = x
        return (x * 0x2545F4914F6CDD1D) & (2**64 - 1)
    def bytes(self, n): return bytes(self.u64() >> 56 for _ in range(n))


def clean(b):   # no control magic, GLSLC magic, code headers or zstd magic inside random bytes
    b = bytearray(b)
    for m in (CTL, GLSLC, GFX, CS, b'\x28\xb5\x2f\xfd', b'SHAD', b'BTCD'):
        i = b.find(m)
        while i >= 0: b[i] ^= 0x55; i = b.find(m, i + 1)
    return bytes(b)


def program(st, seed):
    """(control section, code area) of one NVN stage (as tools/gen_sync_readers.py)"""
    r = Rng(seed * 7919 + 13)
    code = bytearray(CS if st == 'cs' else GFX) + clean(r.bytes(0x2c))
    sph = bytearray(clean(r.bytes(0x50)))
    if st != 'cs':
        struct.pack_into('<I', sph, 0, (SPH[st] << 10) | 1 | (0x40 if st == 'vs' else 0x20))
        if st == 'ps': sph[0x18] = 1
    code += sph if st != 'cs' else b''
    code += clean(r.bytes(0x40 + 8 * (seed % 5)))
    ctl = bytearray(clean(r.bytes(0x880))); ctl[:4] = CTL
    struct.pack_into('<I', ctl, 0x20, 0x880); struct.pack_into('<I', ctl, 0x704, len(code))
    ctl[0x748:0x758] = bytes(16)
    if st == 'cs': struct.pack_into('<3I', ctl, 0x748, 64, 1, 1); struct.pack_into('<I', ctl, 0x754, 0x100)
    return bytes(ctl), bytes(code)


def nvn2(st, seed, nctl=0xe0):
    """(control section, code area) of one NVN2 stage: version 0xc9, its length at +0x30, the code's at +0x44, the stage byte at +0x56 (5 compute), the SPH word at the code's start"""
    r = Rng(seed * 104729 + 7)
    code = bytearray(clean(r.bytes(0x80 + 8 * (seed % 7))))
    if st != 'cs': struct.pack_into('<I', code, 0, (SPH[st] << 10) | 1)
    ctl = bytearray(clean(r.bytes(nctl))); ctl[:8] = CTL + b'\xc9\0\0\0'
    struct.pack_into('<I', ctl, 0x30, nctl); struct.pack_into('<I', ctl, 0x44, len(code)); ctl[0x56] = 5 if st == 'cs' else 0
    return bytes(ctl), bytes(code)


def glslc(stages):
    d = bytearray(0x90 * (len(stages) + 1)); d[:4] = GLSLC; struct.pack_into('<I', d, 0x4c, len(stages))
    for i, (ctl, code) in enumerate(stages):
        at = len(d); d += ctl + code
        h = 0x90 + 0x90 * i; struct.pack_into('<III', d, h, len(ctl) + len(code), at, 0); struct.pack_into('<4I', d, h + 0x30, 0, len(ctl), len(code), len(ctl))
    struct.pack_into('<I', d, 0x44, len(d))
    return bytes(d)


def nso(text):
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, 0), (0x100 + len(text), end, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text


# ---- LSPK (dm/tools/tests/test_split_lspk.py's builders) ------------------------------------------------------------------------------
def bshd(sections, compressed=True):
    data = bytearray(b'\xff\xff\xff\xff\x01' + bytes([len(sections)]))
    at = 6 + len(sections) * 9
    for stage, body in sections: data += struct.pack('<BII', stage, at, len(body)); at += len(body)
    data += b''.join(body for _, body in sections)
    return struct.pack('>I', len(data) | (0x80000000 if compressed else 0)) + (zlib.compress(data) if compressed else data)


def lspk(members, parts=1, version=13):
    """members: [(name, body, method, part)] → {part index: bytes}"""
    areas = [bytearray() for _ in range(parts)]; table = bytearray()
    for name, body, method, part in members:
        packed = body if method == 0 else zlib.compress(body) if method == 1 else lz4.block.compress(body, store_size=False)
        table += name.encode().ljust(256, b'\0') + struct.pack('<6I', len(areas[part]), len(packed), len(body) if method else 0, part, method, 0)
        areas[part] += packed
    offset = len(areas[0]); stored = struct.pack('<I', len(members)) + lz4.block.compress(bytes(table), store_size=False)
    areas[0] += stored + struct.pack('<IIIHBB16sI4s', version, offset, len(stored), parts, 2, 0, bytes(16), 40, b'LSPK')
    return [bytes(a) for a in areas]


# ---- CASC (dm/tools/tests/test_split_casc.py's builders) ------------------------------------------------------------------------------
HEADER = 'FileId!STRING:0|KeyLayout!DEC:4|Chunk!DEC:8|Archive!DEC:8|CKey!HEX:16|CSize!DEC:4|EKey!HEX:16|ESize!DEC:4|Encoding!STRING:0|Path!STRING:0|Flags!STRING:0'
KEY = '079013ef272cc109bebfc8b62fbd2a16'


def shad_record(ctl, code): return struct.pack('<5I', 5, len(ctl), len(code) + 0xc, len(code), 4) + code + ctl


def chunk(tag, body, cnt=0, tag2=None, table=b''):
    hl = 0x1c
    return struct.pack('<4s4sIIIII', tag, tag2 or tag, cnt, 0xffffffff, hl, hl + len(table), hl + len(table) + len(body)) + table + body


def btcd(ctl, code):
    body = struct.pack('<2I', 0, 0) + shad_record(ctl, code)
    return struct.pack('<4s4sIIIII', b'BTCD', b'BTCD', 1, len(body) - 8, 0x24, 0x24, 0x1c + len(body)) + body


def shad(*stages):
    progs = [btcd(*s) for s in stages]
    table, at = b'', 0
    for p in progs: table += struct.pack('<I', at); at += len(p)
    prm = chunk(b'PRM[', b''.join(progs), len(progs), table=table)
    return chunk(b'SHAD', chunk(b'TXT[', b'') + chunk(b'SYM[', b'') + chunk(b'SHMD', prm, 1, tag2=b'/\0\0\0'), 3)


def blte(data, size=0x40, z=False):
    parts = []
    for i in range(0, len(data), size):
        p = data[i:i + size]
        parts.append(((b'Z' + zlib.compress(p)) if z and i % (2 * size) == 0 else b'N' + p, len(p)))
    table = b''.join(struct.pack('>II', len(p), n) + bytes(16) for p, n in parts)
    return b'BLTE' + struct.pack('>I', 12 + len(table)) + b'\x0f' + len(parts).to_bytes(3, 'big') + table + b''.join(p for p, _ in parts)


def storage(files, key=KEY, want=KEY):
    """files: [(file ID, bytes, encoding, archive, stored or None)] → {name: bytes} of one CASC folder"""
    archs, lines = {}, ['## config-key = ' + key, HEADER]
    for fid, data, enc, arch, stored in files:
        if stored is None: stored = zlib.compress(data) if enc == 'z' else blte(data, z=True) if enc.startswith('b:') else data
        a = archs.setdefault(arch, bytearray(b'\0' * 0x20)); off = len(a); a += stored
        ekey = bytes(9) + bytes([0, arch]) + off.to_bytes(5, 'big')
        lines.append(f'{fid}|0|0|{arch}|{"0" * 32}|{len(data)}|{ekey.hex()}|{len(stored)}|{enc}|{fid}|table')
    out = {f'00-{a:08d}.data': bytes(b) for a, b in archs.items()}
    out['build_manifest.psv'] = ('\n'.join(lines) + '\n').encode()
    out['archive_manifest.psv'] = b'#Archives\nName!STRING:0|Size!DEC:8\n00-00000001.data|0\n'
    out['build_key.txt'] = want.encode() + b'\n'
    return out


# ---- PDAT -------------------------------------------------------------------------------------------------------------------------------
def pmap(stages, pad=0x10):
    """a PMAP record: a flatbuffer-like table (root offset, tag PMAP at +4), each stage's control section 8-aligned, its code area right after"""
    d = bytearray(struct.pack('<I', 0x10) + b'PMAP' + bytes(8)) + clean(Rng(len(stages) + pad).bytes(pad))
    for ctl, code in stages:
        d += bytes(-len(d) % 8); d += ctl + code
    d += bytes(-len(d) % 8) + b'tail'
    return bytes(d)


def pdat(records):
    """records: [bytes or ('stored', bytes)]: each zstd-framed but the stored ones"""
    d = bytearray(b'PDAT' + struct.pack('<III', 1, len(records), 0))
    for r in records:
        d += bytes(-len(d) % 16)
        d += r[1] if isinstance(r, tuple) else zstd.compress(r)
    return bytes(d)


# ---- the run ----------------------------------------------------------------------------------------------------------------------------
def make_game(name, files, main_extra=b'', main=None, nro=None):
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    text = Rng(9).bytes(0x1000) + bytes(main_extra) + Rng(10).bytes(0x200)
    open(g + '/main', 'wb').write(main if main is not None else nso(clean(text[:0x1000]) + bytes(main_extra) + clean(text[0x1000:])))
    os.makedirs(f'{g}/profile/{GAME}'); open(f'{g}/profile/{GAME}/game.tsv', 'w').write(GAME_TSV)
    work = tempfile.mkdtemp(); os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1')
    env.pop('DM_NRO_DIR', None)
    if nro:   # DM_NRO_DIR: the NROs the game decodes at run time
        os.makedirs(g + '/nro')
        for n, b in nro.items(): open(f'{g}/nro/{n}', 'wb').write(b)
        env['DM_NRO_DIR'] = g + '/nro'
    p = subprocess.run([sys.executable, '-I', engine + '/dm/gfx/tools/shader_split.py', g + '/RomFS', g + '/main', work + '/pkg'] + [f'{work}/tables/{t}' for t in tables],
                       env=env, capture_output=True, text=True)
    if p.returncode: sys.exit(f'{name}: the Python split failed ({p.returncode}): {p.stderr[-2000:]}')
    log = [l.replace(g + '/RomFS', '@ROMFS@') for l in p.stdout.splitlines() if not l.startswith('progress')]
    expected = {}
    for sub in ('pkg', 'tables'):
        for fn in sorted(os.listdir(f'{work}/{sub}')):
            b = open(f'{work}/{sub}/{fn}', 'rb').read()
            if fn == 'shader_map.tsv':
                lines = b.decode().splitlines(keepends=True); lines[1] = PORT_LINE2; b = ''.join(lines).encode()
            expected[f'{sub}/{fn}'] = sha(b)
    n = sum(1 for l in open(f'{work}/tables/shader_map.tsv') if not l.startswith(('#', 'package\t')))
    log = [l.replace(g + '/nro', '@NRO@') for l in log]
    exp = {'shaders': n, 'files': expected, 'dlc': [], 'updates': [], 'log': log}
    if nro: exp['nro_dir'] = 'nro'
    json.dump(exp, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    shutil.rmtree(work)
    print(name, n, 'shaders', len(expected), 'files;', *log, sep='\n  ')


S, N = program, nvn2

if __name__ == '__main__':
    # LSPK: a package in two parts (BSHD records zlib-wrapped or not, members stored / zlib / LZ4, in either part), other platforms' programs, a sound member never read, an
    # upper-case .BSHD name, a stage that disagrees with its program (its job red), a member whose zlib stream is damaged (its job red); a package of another version (an error job),
    # one whose second part is missing (an error job), a .pak of another format (a loose file)
    a, a1 = lspk([('sound.wem', Rng(1).bytes(0x300), 0, 1),
                  ('Shaders/NX/a.bshd', bshd([(1, glslc([S('vs', 1)])), (3, glslc([S('ps', 2)]))]), 2, 1),
                  ('Shaders/NX/b.BSHD', bshd([(4, glslc([S('cs', 3)]))], compressed=False), 1, 0),
                  ('Shaders/NX/c.bshd', bshd([(1, glslc([S('vs', 4)])), (3, glslc([S('ps', 5)])), (9, b'UA5 another GPU')]), 0, 0),
                  ('Shaders/PS5/d.bshd', bshd([(1, b'PSSL program'), (3, b'PSSL pixel')]), 1, 1),
                  ('Shaders/NX/bad.bshd', bshd([(3, glslc([S('vs', 6)]))]), 2, 0),
                  ('Shaders/NX/broken.bshd', b'\0\0\0\x10' + b'not a record', 0, 1),
                  ('Shaders/NX/z.bshd', bshd([(1, glslc([S('vs', 7)]))])[:-9], 1, 0)], parts=2)
    bad, = lspk([('x.bshd', bshd([(1, glslc([S('vs', 8)]))]), 0, 0)], version=14)
    miss = lspk([('y.bshd', bshd([(1, glslc([S('vs', 9)]))]), 0, 1)], parts=2)[0]
    make_game('sync-lspk', {'Data/Main.pak': a, 'Data/Main_1.pak': a1, 'Data/Old.pak': bad, 'Data/Lost.pak': miss, 'Data/Other.pak': Rng(2).bytes(0x400),
                            'Data/Loose.bshd': bshd([(1, glslc([S('vs', 10)]))])})

    # CASC: one folder whose manifest is current (two archives; SHAD files zlib, stored and BLTE-wrapped with N and Z chunks; a music file; a damaged zlib entry and a length that
    # does not add up: logged, not scanned), and one whose build key names no manifest (logged; its archive is scanned as a loose file, the SHAD in it found there)
    cur = storage([('data:data/hd/shaders/vs_ui.bin', shad(S('vs', 20)), 'z', 1, None), ('data:data/hd/shaders/ps_ui.bin', shad(S('ps', 21), S('ps', 21)), 'n', 1, None),
                   ('data:data/hd/shaders/cs_lut.bin', shad(S('cs', 22)), 'b:64K*=n', 2, None), ('data:data/global/music.opus', b'opus' * 100, 'b:64K*=n', 1, None),
                   ('data:data/hd/shaders/bad.bin', shad(S('vs', 23)), 'z', 2, b'\x78\x9c' + b'\xff' * 40),
                   ('data:data/hd/shaders/short.bin', shad(S('vs', 24)), 'n', 1, shad(S('vs', 24))[:-4]),
                   ('data:data/hd/shaders/vs_more.bin', shad(S('vs', 25), S('ps', 26)), 'z', 2, None)])
    old = storage([('a', shad(S('ps', 27)), 'n', 1, None)], want='00' * 16)
    files = {f'data/{n}': b for n, b in cur.items()}
    files.update({f'Pack/old/{n}': b for n, b in old.items()})
    make_game('sync-casc', files)

    # PDAT: PMAP frames (vertex + pixel, vertex + pixel + geometry, compute), a frame of another tag, a stored compute record outside the frames; a second archive whose PMAP record
    # names a code area past its end (its job red)
    # (a frame's extent is taken as p + 1 MiB - what follows it in the 1 MiB read: in a small archive every frame reaches its end, so a stored record after one is not read)
    recs = [('stored', pmap([N('cs', 36)])), pmap([N('vs', 30), N('ps', 31)]), b'\x08\0\0\0MESH' + Rng(3).bytes(0x200), pmap([N('vs', 32), N('ps', 33), N('gs', 34)], pad=0x18),
            pmap([N('cs', 35, 0x128)]), ('stored', pmap([N('cs', 39)])), pmap([N('vs', 30), N('ps', 31)])]
    c, k = N('ps', 37)
    bad = pmap([(c[:0x44] + struct.pack('<I', 0x10000) + c[0x48:], k)])
    make_game('sync-pdat', {'Data/a.dat': pdat(recs), 'Data/b.dat': pdat([pmap([N('vs', 38)]), bad]), 'Data/c.dat': b'PDA' + bytes(8)})
