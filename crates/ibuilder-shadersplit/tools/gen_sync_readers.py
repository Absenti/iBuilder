#!/usr/bin/env python3
"""Writes tests/data/games/<name>/ for the readers ported from nxnative after a3663659: NXSD (a63ca651), Falcom .pkg / .pka and Phyre RYHP GLSLC outputs (930e9195), Media.Vision MVGL
archives and NXVS / NXFS / NXCS containers (131bd013 + 8cc63cc9). Each game is a RomFS folder, an NSO `main` and a profile, every byte made here (nothing comes from a game), split by
the UPSTREAM engine's own `shader_split.py` (its command line, as build_ipa_from_nsp.py runs it); `expected.json` has the sha256 of what it wrote and its log, in the format of
tests/games.rs, which splits the same folders with this crate at 1, 4 and 8 threads.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_sync_readers.py <upstream checkout (the folder that holds dm/)> tests/data/games

shader_map.tsv: the upstream engine reworded its comment line 2 after the commit this port follows ("names the chosen source of each key" for "records where each key first appears");
the hash recorded is that of the Python's table with line 2 put back to the port's text, every other byte as the Python wrote it. The Python's absolute RomFS path in log lines is
written as @ROMFS@ (tests/games.rs does the same with its own path).
"""
import hashlib, json, os, shutil, struct, subprocess, sys, tempfile
import lz4.block
from compression import zstd

engine, out_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
GAME = '0100TESTTEST0000'
GAME_TSV = f'title_id\t{GAME}\nshader_groups\t/Data/=data /Pack/=pack /aoc/=dlc\nshader_note\tsynthetic game\n'
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


def clean(b):   # no control magic inside random bytes (it would cut the record), no GLSLC magic either
    b = bytearray(b)
    for m in (CTL, GLSLC, GFX, CS):
        i = b.find(m)
        while i >= 0: b[i] ^= 0x55; i = b.find(m, i + 1)
    return bytes(b)


def program(st, seed, extra_ctl=b''):
    """(control section, code area) of one stage: code = 0x30 header + SPH + instructions; the control's +0x20 holds its length, +0x704 the code's"""
    r = Rng(seed * 7919 + 13)
    code = bytearray(CS if st == 'cs' else GFX) + clean(r.bytes(0x2c))
    sph = bytearray(clean(r.bytes(0x50)))
    if st != 'cs':
        struct.pack_into('<I', sph, 0, (SPH[st] << 10) | 1 | (0x40 if st == 'vs' else 0x20))
        if st == 'ps': sph[0x18] = 1   # one generic input: the separable pairs need a cover
    code += sph if st != 'cs' else b''
    code += clean(r.bytes(0x40 + 8 * (seed % 5)))
    ctl = bytearray(clean(r.bytes(0x880))); ctl[:4] = CTL
    struct.pack_into('<I', ctl, 0x20, 0x880); struct.pack_into('<I', ctl, 0x704, len(code))
    ctl[0x748:0x758] = bytes(16)   # no thread group in a graphics control: Phyre's reader would take the bytes before it for headerless compute code
    if st == 'cs': struct.pack_into('<3I', ctl, 0x748, 64, 1, 1); struct.pack_into('<I', ctl, 0x754, 0x100)
    return bytes(ctl) + extra_ctl, bytes(code)


def glslc(stages):
    """A GLSLC output (fuzz_shader_split.glslc's layout): each section's data is its control section with the code area after it"""
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


# ---- NXSD -------------------------------------------------------------------------------------------------------------------------------
def nxsd(progs, order=None, gap=False):
    table = 0x50; controls = table + 24 * len(progs); codes = controls + 0x880 * len(progs)
    d = bytearray(codes); d[:4] = b'NXSD'; offsets = []
    for _, code in progs: offsets.append(len(d) - codes + (8 if gap and offsets else 0)); d += code
    if gap: d += bytes(8)
    struct.pack_into('<7I', d, 4, 0x100, len(d), codes, 0, len(progs), 0x880 * len(progs), len(d) - codes)
    struct.pack_into('<3Q', d, 0x28, table, controls, codes)
    for i, (ctl, _) in enumerate(progs): d[controls + i * 0x880:controls + (i + 1) * 0x880] = ctl
    for i, k in enumerate(range(len(progs)) if order is None else order):
        struct.pack_into('<6I', d, table + i * 24, k * 0x880, 0, offsets[k], 0, 0, 0)
    return bytes(d)


# ---- Falcom -----------------------------------------------------------------------------------------------------------------------------
def pack(raw, flags, stored=None):
    if stored is not None: return stored
    if flags == 1: return lz4.block.compress(raw, store_size=False)
    if flags == 0x10: return zstd.compress(raw)
    return raw


def falcom_pkg(entries):
    """[(name, raw, flags, stored or None)]"""
    n = len(entries); at = 8 + 0x50 * n; table, data = bytearray(), bytearray()
    for name, raw, flags, stored in entries:
        p = pack(raw, flags, stored)
        table += name.encode().ljust(0x40, b'\0') + struct.pack('<4I', len(raw), len(p), at + len(data) if p else 0, flags); data += p
    return struct.pack('<II', 0, n) + bytes(table) + bytes(data)


def falcom_pka(packages, blobs):
    out = bytearray(bytes.fromhex('0dcff77f') + struct.pack('<I', len(packages)))
    for pnm, entries in packages:
        out += pnm.encode().ljust(32, b'\0') + struct.pack('<I', len(entries))
        for enm, digest in entries: out += enm.encode().ljust(64, b'\0') + digest
    out += struct.pack('<I', len(blobs)); start = len(out) + 52 * len(blobs); table, data = bytearray(), bytearray()
    for digest, raw, flags, stored in blobs:
        p = pack(raw, flags, stored); table += digest + struct.pack('<QIII', start + len(data), len(p), len(raw), flags); data += p
    return bytes(out + table + data)


def ryhp(stages): return b'RYHP' + bytes(0x3c) + b''.join(glslc([s]) for s in stages)


# ---- MVGL -------------------------------------------------------------------------------------------------------------------------------
def nxsh(magic, pair, tail=b''):
    ctl, code = pair; coff = 0x40; koff = coff + len(ctl); total = koff + len(code) + len(tail)
    h = bytearray(coff); h[:4] = magic; struct.pack_into('<6I', h, 4, total, 0, coff, len(ctl), koff, len(code))
    return bytes(h) + ctl + code + tail


def doboz_compress(raw):
    """A doboz version 0 stream (4-byte matches, type 7: offset < 2^21, length 3..258), greedy, the last 8 bytes literals"""
    items, i, n = [], 0, len(raw)
    while i < n:
        best = (0, 0)
        if i + 8 < n:
            for off in range(1, min(i, 64) + 1):
                k = 0
                while k < 258 and i + k < n - 8 and raw[i + k] == raw[i + k - off]: k += 1
                if k > best[1]: best = (off, k)
        if best[1] >= 6: items.append(best); i += best[1]
        else: items.append(raw[i]); i += 1
    body = bytearray()
    for g in range(0, len(items), 31):
        grp = items[g:g + 31]; ctl = 1 << len(grp)
        for b, it in enumerate(grp):
            if isinstance(it, tuple): ctl |= 1 << b
        body += struct.pack('<I', ctl)
        for it in grp: body += struct.pack('<I', (it[0] << 11) | ((it[1] - 3) << 3) | 7) if isinstance(it, tuple) else bytes([it])
    return bytes([3 << 3]) + struct.pack('<II', n, 9 + len(body)) + bytes(body)


def mvgl(entries, wide=True):
    """[(name, ext, raw, stored)]: stored None = compressed (LZ4 when wide, doboz when legacy), 'raw' = stored as it is, else those bytes"""
    nt = nn = len(entries) + 1; nd = len(entries)
    hs, ts, ns, ds = (0x20, 16, 0x80, 16) if wide else (0x14, 8, 0x40, 12)
    start = hs + nt * ts + nn * ns + nd * ds
    tree, names, data, body = bytearray(nt * ts), bytearray(nn * ns), bytearray(nd * ds), bytearray()
    struct.pack_into('<4I' if wide else '<4H', tree, 0, *((0xffffffff,) * 2 if wide else (0xffff,) * 2), 0, 1)
    for i, (name, ext, raw, stored) in enumerate(entries):
        k = i + 1
        struct.pack_into('<4I' if wide else '<4H', tree, k * ts, 0, i, 0, k)
        names[k * ns:k * ns + 4] = ext.encode('latin1').ljust(4, b' ')[:4]; nb = name.encode('latin1'); names[k * ns + 4:k * ns + 4 + len(nb)] = nb
        s = raw if stored == 'raw' else stored if stored is not None else (lz4.block.compress(raw, store_size=False) if wide else doboz_compress(raw))
        off = len(body); body += s
        struct.pack_into('<QII' if wide else '<III', data, i * ds, off, len(raw), len(s))
    total = start + len(body)
    hdr = struct.pack('<4sIIIQQ', b'MDB1', nt, nn, nd, start, total) if wide else struct.pack('<4sHHIII', b'MDB1', nt, nn, nd, start, total)
    return hdr + bytes(tree) + bytes(names) + bytes(data) + bytes(body)


# ---- the run ----------------------------------------------------------------------------------------------------------------------------
def make_game(name, files, main_extra=b''):
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    text = Rng(9).bytes(0x1000) + bytes(main_extra) + Rng(10).bytes(0x200)
    open(g + '/main', 'wb').write(nso(clean(text[:0x1000]) + bytes(main_extra) + clean(text[0x1000:])))
    os.makedirs(f'{g}/profile/{GAME}'); open(f'{g}/profile/{GAME}/game.tsv', 'w').write(GAME_TSV)
    work = tempfile.mkdtemp(); os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1')
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
    json.dump({'shaders': n, 'files': expected, 'dlc': [], 'updates': [], 'log': log}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    shutil.rmtree(work)
    print(name, n, 'shaders', len(expected), 'files;', *log, sep='\n  ')


S = lambda st, seed, extra=b'': program(st, seed, extra)

# NXSD: three stages in descriptor order and permuted, by magic beside a .dict and under a .data name; one with a gap in its code region (red); a file named like an archive with no magic
make_game('sync-nxsd', {
    'Data/shaders.binary': nxsd([S('vs', 1), S('ps', 2), S('cs', 3)]),
    'Data/permuted.data': nxsd([S('vs', 4), S('ps', 5)], order=[1, 0]),
    'Data/permuted.dict': b'',
    'Data/gap.bin': nxsd([S('vs', 6), S('ps', 7)], gap=True),
    'Data/badver.bin': (lambda d: d[:4] + struct.pack('<I', 0x101) + d[8:])(nxsd([S('vs', 8)])),
    'Pack/other.nxsd': Rng(11).bytes(0x200),
})

# Falcom: a .pkg with stored / LZ4 / zstd entries, skipped textures, an unknown flag ([WARN]), an empty entry; a damaged .pkg (red); a file that starts with a u32 0 and is no .pkg;
# a .pka with two packages naming one blob (read once), skipped names, an unknown flag and enough blobs for several shards; a .pka whose index is damaged (red); a loose RYHP effect
fx = lambda *st: ryhp(list(st))
blobs = [(bytes([i]) * 32, fx(S('vs', 40 + 2 * i), S('ps', 41 + 2 * i)), [0, 1, 0x10][i % 3], None) for i in range(6)]
blobs += [(b'\x77' * 32, b'texture', 0, None), (b'\x78' * 32, b'external', 9, None), (b'\x79' * 32, glslc([S('cs', 60)]), 0x10, None)]
make_game('sync-falcom', {
    'Data/fx.pkg': falcom_pkg([('shader_a.fx.phyre', fx(S('vs', 20), S('ps', 21)), 0x10, None), ('shader_b.fx.phyre', fx(S('vs', 22), S('ps', 23)), 1, None),
                               ('plain.glslc', glslc([S('cs', 24)]), 0, None), ('tex.dds.phyre', fx(S('vs', 25)), 0, None), ('mesh.DAE.phyre', b'mesh', 1, None),
                               ('ext.bin', b'outside', 9, None), ('empty.bin', b'', 0, None), ('bnsh_free.bin', Rng(5).bytes(64), 0, None)]),
    'Data/bad.pkg': falcom_pkg([('broken.fx.phyre', fx(S('vs', 26)), 0x10, zstd.compress(fx(S('vs', 26)))[:-6])]),
    'Data/bad_lz4.pkg': falcom_pkg([('big.fx.phyre', fx(S('vs', 27)), 1, b'\xff\xff\xff\xff')]),
    'Data/zero.bin': struct.pack('<II', 0, 3) + Rng(6).bytes(0x200),
    'Pack/assets.pka': falcom_pka([('pkg_one', [('a.fx.phyre', bytes([0]) * 32), ('b.fx.phyre', bytes([1]) * 32), ('t.dds.phyre', b'\x77' * 32), ('x.bin', b'\x78' * 32)]),
                                   ('pkg_two', [('a_again.fx.phyre', bytes([0]) * 32)] + [(f'c{i}.fx.phyre', bytes([i]) * 32) for i in range(2, 6)] + [('cs.glslc', b'\x79' * 32)])],
                                  blobs),
    'Pack/broken.pka': bytes.fromhex('0dcff77f') + struct.pack('<I', 1000) + bytes(64),
    'Pack/loose.fx.phyre': fx(S('vs', 30), S('ps', 31)),
})

# MVGL: a wide archive (LZ4 and stored .shad entries holding NXVS / NXFS / NXCS, a .SHAD name, other entries not read), a legacy one (doboz and stored), one whose .shad does not decode,
# one whose header fits no layout; NXVS / NXFS containers in main (one with compiler options after its control section) and a loose .shad no job takes
vsx, psx, csx = S('vs', 70), S('ps', 71), S('cs', 72)
Z = bytes(0x400)   # a tail of zeros, so that a compressed entry is shorter than its length
make_game('sync-mvgl', {
    'Data/app_0.nx64.mvgl': mvgl([('shader\\a', 'shad', nxsh(b'NXVS', vsx, Z), None), ('shader\\b', 'shad', nxsh(b'NXFS', psx), 'raw'), ('shader\\c', 'SHAD', nxsh(b'NXCS', csx, Z), None),
                                  ('model\\m', 'mdl', Rng(3).bytes(300), 'raw'), ('shader\\g', 'shad', glslc([S('vs', 73)]) + Z, None)]),
    'Data/legacy.nx64.mvgl': mvgl([('sh\\x', 'shad', nxsh(b'NXVS', S('vs', 74), tail=bytes(0x800)), None), ('sh\\y', 'shad', nxsh(b'NXFS', S('ps', 75)), 'raw'), ('tx\\t', 'img ', b'tex' * 40, None), ('sh\\z', 'shad', nxsh(b'NXCS', S('cs', 80), tail=bytes(0x800)), None)], wide=False),
    'Pack/bad.mvgl': mvgl([('s\\bad', 'shad', nxsh(b'NXVS', S('vs', 76)), b'\xff' * 4)]),
    'Pack/weird.mvgl': b'MDB1' + bytes(0x30),
    'Pack/loose.shad': nxsh(b'NXFS', S('ps', 77)),
}, main_extra=nxsh(b'NXVS', S('vs', 78)) + bytes(16) + nxsh(b'NXFS', S('ps', 79, extra=bytes(0x20))) + b'NXFS' + bytes(12))
