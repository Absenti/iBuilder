#!/usr/bin/env python3
"""Writes the fixtures of the CRI CPK reader (cpk.py, cpk_jobs) and Diablo III's package reader (d3cpk.py, d3cpk_jobs) of ibuilder-shadersplit (src/cpk.rs):

  * tests/data/games/sync-cpk and tests/data/games/sync-d3cpk: synthetic games (a RomFS folder, an NSO `main`, a profile) split by the UPSTREAM engine's own `shader_split.py`
    (its command line); `expected.json` has the sha256 of what it wrote and its log, in the format of tests/games.rs, which splits the same folders with this crate at 1, 4 and 8 threads;
  * tests/data/vectors-cpk.jsonl.gz: the games' archives damaged in many places (bytes changed, the file cut), and what the upstream readers answer for each: `cpk_jobs` (its jobs, its
    log, or the exception that ends the split), then every entry read by `cpk.read` as `cpk_shaders` reads it (its bytes, or the error that skips it or ends its job); `d3cpk_jobs` and
    `d3cpk_shaders` over the job's ranges and over ranges cut anywhere (the shaders, the log). tests/cpk.rs damages the same files the same way and must give the same answers.

  PYTHONDONTWRITEBYTECODE=1 python3 -I tools/gen_cpk.py <upstream checkout (the folder that holds dm/)> <crate folder>

Every byte is made here (a PRNG's, or zeros); nothing comes from a game. CRILAYLA is compressed by the encoder below, and every compressed entry is checked to decode, with the upstream
decoder, to its bytes. shader_map.tsv: line 2 is recorded with the port's text, as tools/gen_sync_readers.py does. The archive's path in the vectors' messages is written as P.
"""
import contextlib, gzip, hashlib, io, json, os, shutil, struct, subprocess, sys, tempfile, zlib

engine, crate = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
out_root = os.path.join(crate, 'tests/data/games')
GAME = '0100TESTTEST0000'
GAME_TSV = f'title_id\t{GAME}\nshader_groups\t/Data/=data /CPKs/=pack /aoc/=dlc\nshader_note\tsynthetic game\n'
PORT_LINE2 = '# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied\n'
CTL, GFX, CS, GLSLC = bytes.fromhex('34127698'), bytes.fromhex('78563412'), bytes.fromhex('79563412'), bytes.fromhex('91688619')
SPH = {'vs': 1, 'hs': 2, 'ds': 3, 'gs': 4, 'ps': 5}
STAGES = ['vs', 'hs', 'ds', 'gs', 'ps', 'cs']

prof = tempfile.mkdtemp(); os.makedirs(prof + '/' + GAME); open(f'{prof}/{GAME}/game.tsv', 'w').write(GAME_TSV)
os.environ.update(DM_GAMES_DIR=prof, DM_GAMES_FIRST='1', DM_GAME=GAME)
tools = engine + '/dm/gfx/tools'
for sub in ('', '/../../tools', '/../../games'): sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, cpk, d3cpk


def sha(b): return hashlib.sha256(b).hexdigest()
def sha16(b): return hashlib.sha256(bytes(b)).hexdigest()[:16]


class Rng:   # xorshift64*, so the bytes do not depend on Python's random module
    def __init__(self, s): self.s = s or 1
    def u64(self):
        x = self.s; x ^= x >> 12; x ^= (x << 25) & (2**64 - 1); x ^= x >> 27; self.s = x
        return (x * 0x2545F4914F6CDD1D) & (2**64 - 1)
    def bytes(self, n): return bytes(self.u64() >> 56 for _ in range(n))
    def below(self, n): return (self.u64() >> 11) % n


def clean(b):   # no control magic inside random bytes (it would cut the record), no GLSLC / BNSH / zlib-looking magic either
    b = bytearray(b)
    for m in (CTL, GLSLC, GFX, CS, b'BNSH', b'CRILAYLA', b'@UTF'):
        i = b.find(m)
        while i >= 0: b[i] ^= 0x55; i = b.find(m, i + 1)
    return bytes(b)


def program(st, seed, ctl_len=0x880):
    """(control section, code area) of one stage: code = 0x30 header + SPH + instructions; the control's +0x20 holds its length, +0x704 the code's"""
    r = Rng(seed * 7919 + 13)
    code = bytearray(CS if st == 'cs' else GFX) + clean(r.bytes(0x2c))
    sph = bytearray(clean(r.bytes(0x50)))
    if st != 'cs':
        struct.pack_into('<I', sph, 0, (SPH[st] << 10) | 1 | (0x40 if st == 'vs' else 0x20))
        if st == 'ps': sph[0x18] = 1   # one generic input: the separable pairs need a cover
    code += sph if st != 'cs' else b''
    code += clean(r.bytes(0x40 + 8 * (seed % 5)))
    ctl = bytearray(clean(r.bytes(ctl_len))); ctl[:4] = CTL
    struct.pack_into('<I', ctl, 0x20, ctl_len); struct.pack_into('<I', ctl, 0x704, len(code))
    ctl[0x748:0x758] = bytes(16)
    if st == 'cs': struct.pack_into('<3I', ctl, 0x748, 64, 1, 1); struct.pack_into('<I', ctl, 0x754, 0x100)
    return bytes(ctl), bytes(code)


def glslc(stages):
    d = bytearray(0x90 * (len(stages) + 1)); d[:4] = GLSLC; struct.pack_into('<I', d, 0x4c, len(stages))
    for i, (ctl, code) in enumerate(stages):
        at = len(d); d += ctl + code
        h = 0x90 + 0x90 * i; struct.pack_into('<III', d, h, len(ctl) + len(code), at, 0); struct.pack_into('<4I', d, h + 0x30, 0, len(ctl), len(code), len(ctl))
    struct.pack_into('<I', d, 0x44, len(d))
    return bytes(d)


def ryhp(stages): return b'RYHP' + bytes(0x3c) + b''.join(glslc([s]) for s in stages)


def bnsh(variations):
    """A BNSH at 0 (grsc, variation table, programs, stage descriptors): variations = [[(stage, (ctl, code)), ...], ...]"""
    d = bytearray(0x100); d[0:8] = b'BNSH\0\0\0\0'; d[0x60:0x64] = b'grsc'
    pvar = 0x100; d[0x7c:0x80] = struct.pack('<I', len(variations)); d[0x80:0x88] = struct.pack('<Q', pvar)
    d += bytes(0x40 * len(variations))
    for v, stages in enumerate(variations):
        prog = len(d); d += bytes(0x50)
        d[pvar + 0x40 * v + 0x10:pvar + 0x40 * v + 0x18] = struct.pack('<Q', prog)
        for st, (ctl, code) in stages:
            si = STAGES.index(st)
            sc = len(d); d += bytes(0x20)
            pctl = len(d); d += ctl
            pcode = len(d); d += code
            d[sc + 8:sc + 0x20] = struct.pack('<QQII', pctl, pcode, len(code), len(ctl))
            d[prog + 8 + 8 * si:prog + 16 + 8 * si] = struct.pack('<Q', sc)
    d[0x1c:0x20] = struct.pack('<I', len(d))
    return bytes(d)


def pgdat(members):
    """A PlatinumGames DAT archive: 'DAT\\0', count, the offsets of the offset / extension / name / size / hash tables, members 0x10-aligned"""
    n = len(members); w = max(len(nm) for nm, _ in members) + 1
    ot = 0x20; et = ot + 4 * n; nt = et + 4 * n; st = nt + 4 + w * n; st += -st % 4; ht = st + 4 * n
    d = bytearray(ht + 0x10); d[:4] = b'DAT\0'; struct.pack_into('<6I', d, 4, n, ot, et, nt, st, ht)
    struct.pack_into('<I', d, nt, w)
    for i, (nm, b) in enumerate(members):
        d += bytes(-len(d) % 0x10); struct.pack_into('<I', d, ot + 4 * i, len(d)); struct.pack_into('<I', d, st + 4 * i, len(b))
        d[et + 4 * i:et + 4 * i + 4] = nm.rsplit('.', 1)[-1].encode()[:3].ljust(4, b'\0')
        d[nt + 4 + w * i:nt + 4 + w * i + len(nm)] = nm.encode()
        d += b
    return bytes(d)


def nso(text):
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, 0), (0x100 + len(text), end, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text


# ---- CRILAYLA ---------------------------------------------------------------------------------------------------------------------------
def crilayla(data, chain=16):
    """CRILAYLA of data (at least 0x100 bytes): the first 0x100 bytes stored after the bits; the rest coded from its end backwards, as cpk.crilayla decodes it (a match copies from
    back = 3..0x2002 bytes above the write position). Greedy, the matches found by the 3 bytes they end with"""
    assert len(data) >= 0x100
    bits, heads, w, top = [], {}, len(data) - 1, len(data)
    def add(j):   # position j (a match source's top byte) may now be used: back = j - w >= 3
        if j - 2 >= 0x100: heads.setdefault(data[j - 2:j + 1], []).append(j)
    nxt = len(data) - 1   # positions >= nxt + 1 were added
    while w >= 0x100:
        while nxt >= w + 3:
            add(nxt); nxt -= 1
        best, bj = 0, 0
        if w - 2 >= 0x100:
            for j in reversed(heads.get(data[w - 2:w + 1], [])[-chain:]):
                back = j - w
                if back > 0x2002: break
                k = 0
                while w - k >= 0x100 and data[w - k] == data[j - k]: k += 1   # j - k > w - k: the source is decoded already, also when it overlaps the copy
                if k > best: best, bj = k, j
        if best >= 3:
            bits.append('1' + format(bj - w - 3, '013b')); L = best - 3
            if L < 3: bits.append(format(L, '02b'))
            else:
                bits.append('11'); L -= 3
                if L < 7: bits.append(format(L, '03b'))
                else:
                    bits.append('111'); L -= 7
                    if L < 31: bits.append(format(L, '05b'))
                    else:
                        bits.append('11111'); L -= 31
                        while L >= 255: bits.append('11111111'); L -= 255
                        bits.append(format(L, '08b'))
            w -= best
        else:
            bits.append('0' + format(data[w], '08b')); w -= 1
    s = ''.join(bits); s += '0' * (-len(s) % 8)
    packed = bytes(int(s[i:i + 8], 2) for i in range(0, len(s), 8))
    c = b'CRILAYLA' + struct.pack('<II', len(data) - 0x100, len(packed)) + packed[::-1] + data[:0x100]
    assert cpk.crilayla(c) == data, 'the CRILAYLA encoder disagrees with cpk.crilayla'
    return c


# ---- CRI @UTF tables and CPK archives -----------------------------------------------------------------------------------------------
FMT = {0: '>B', 1: '>b', 2: '>H', 3: '>h', 4: '>I', 5: '>i', 6: '>Q', 7: '>q', 8: '>f'}


def utf_table(name, cols, rows):
    """cols: [(name, type, storage, constant)], storage 'r' (per row), 'c' (constant), 'z' (zero); rows: one dict per row"""
    strings, data = bytearray(b'<NULL>\0'), bytearray()
    def string(s):
        o = len(strings); strings.extend(s.encode() + b'\0'); return o
    def value(t, v):
        if t == 0xa: return struct.pack('>I', string(v))
        if t == 0xb:
            o = len(data); data.extend(v); return struct.pack('>II', o, len(v))
        return struct.pack(FMT[t], v) if t in FMT else bytes(4)   # a type no reader knows: 4 bytes of something
    name_at = string(name)
    colb = bytearray()
    for c, t, st, k in cols:
        colb += bytes([{'r': 0x50, 'c': 0x30, 'z': 0x10}[st] | t]) + struct.pack('>I', string(c))
        if st == 'c': colb += value(t, k)
    body = bytearray()
    for r in rows:
        for c, t, st, _ in cols:
            if st == 'r': body += value(t, r[c])
    width = len(body) // len(rows) if rows else 0
    ro = 0x18 + len(colb); so = ro + len(body); do = so + len(strings)
    head = struct.pack('>HHIIIHHI', 1, ro, so, do, name_at, len(cols), width, len(rows))
    t = head + bytes(colb) + bytes(body) + bytes(strings) + bytes(data)
    return b'@UTF' + struct.pack('>I', len(t)) + t


def chunk(magic, table, masked=False):
    return magic + b'\xff\0\0\0' + struct.pack('<I', len(table)) + bytes(4) + (cpk.unmask(table) if masked else table)


TOC_COLS = [('DirName', 0xa, 'r', None), ('FileName', 0xa, 'r', None), ('FileSize', 4, 'r', None), ('ExtractSize', 4, 'r', None), ('FileOffset', 6, 'r', None),
            ('ID', 4, 'r', None), ('UserString', 0xa, 'c', '<NULL>')]


def cpk_file(files, toc_at_end=False, masked=False, toc_cols=TOC_COLS, align=0x800, content=None, header_extra=(), patch=None, rowfix=None, hfix=None):
    """files: [(dir, name, stored bytes, extract size or None (= its length))]; the data 0x800-aligned from ContentOffset; the TOC right after the header or after the data"""
    content = content or (0x1800 if toc_at_end else 0x2000)
    data, at = bytearray(), []
    for _, _, b, _ in files:
        at.append(content + len(data)); data += b + bytes(-len(b) % align)
    toc = content + len(data) if toc_at_end else 0x800
    base = min(content, min(toc, 0x800))
    rows = [{'DirName': d, 'FileName': nm, 'FileSize': len(b), 'ExtractSize': len(b) if x is None else x, 'FileOffset': a - base, 'ID': i}
            for i, ((d, nm, b, x), a) in enumerate(zip(files, at))]
    if rowfix: rowfix(rows)
    toc_chunk = chunk(b'TOC ', utf_table('CpkTocInfo', toc_cols, rows), masked)
    hcols = [('UpdateDateTime', 6, 'r', None), ('ContentOffset', 6, 'r', None), ('ContentSize', 6, 'r', None), ('TocOffset', 6, 'r', None), ('TocSize', 6, 'r', None),
             ('EtocOffset', 6, 'z', None), ('ItocOffset', 6, 'z', None), ('Files', 4, 'r', None), ('Version', 2, 'c', 7), ('Revision', 2, 'c', 14), ('Align', 2, 'r', None),
             ('Comment', 0xa, 'r', None), ('Tvers', 0xa, 'c', 'CRI_CPK_SYNTH'), ('Codec', 4, 'r', None)] + list(header_extra)
    hrow = {'UpdateDateTime': 1, 'ContentOffset': content, 'ContentSize': len(data), 'TocOffset': toc, 'TocSize': len(toc_chunk), 'Files': len(files), 'Align': align,
            'Comment': 'synthetic', 'Codec': 0}
    hrow.update({c: k for c, _, st, k in header_extra if st == 'r'}); hrow.update(patch or {})
    hrows = [hrow]
    if hfix: hcols, hrows = hfix(hcols, hrows)
    head = chunk(b'CPK ', utf_table('CpkHeader', hcols, hrows), masked)
    out = bytearray(head.ljust(0x800, b'\0'))
    if not toc_at_end: out += toc_chunk
    out = out.ljust(content, b'\0') + data
    if toc_at_end: out += toc_chunk
    return bytes(out)


# ---- Diablo III packages ------------------------------------------------------------------------------------------------------------
D3_CTL = 0x850   # 2.7.7's control sections
def sno(*stages, pad=0x24):
    body = bytearray(struct.pack('<4I', 0xdeadbeef, 37, 1, 0) + bytes(pad))
    for ctl, code in stages: body += ctl + code + bytes(0x10)
    return bytes(body)


def record(data, decoded_len=None):
    z = zlib.compress(data, 9)
    assert len(z) <= 0xffff
    return struct.pack('<IH', len(data) if decoded_len is None else decoded_len, len(z)) + z


def package(*parts, version=7):
    head = bytearray(0x48 if version == 7 else 0x40); head[:4] = d3cpk.MAGIC; struct.pack_into('<I', head, 4, version)
    return bytes(head) + b''.join(parts)


# ---- the games ----------------------------------------------------------------------------------------------------------------------
def make_game(name, files):
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    text = Rng(9).bytes(0x1000) + Rng(10).bytes(0x200)
    open(g + '/main', 'wb').write(nso(clean(text)))
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


S = program
pad = lambda b, n: b + bytes(n)   # zeros: something for CRILAYLA to find
def cri(raw): return crilayla(raw), len(raw)
dat_a = pgdat([('a.vso', bnsh([[('vs', S('vs', 1)), ('ps', S('ps', 2))]])), ('pad.bin', bytes(0x600)), ('b.pso', bnsh([[('ps', S('ps', 3))], [('vs', S('vs', 4))]]))])
dat_b = pgdat([('e.vso', bnsh([[('vs', S('vs', 5)), ('ps', S('ps', 6))]]))])
runs = b''.join(bytes([i]) * (40 + 97 * i) for i in range(12)) + clean(Rng(3).bytes(0x300)) + bytes(0x3000)   # long runs: every length field of a match
good, runs_c = crilayla(dat_a), crilayla(runs)
raw_t = pad(bnsh([[('cs', S('cs', 7))]]), 0x400); trunc = crilayla(raw_t)
trunc = trunc[:8] + struct.pack('<II', len(raw_t) - 0x100, struct.unpack_from('<I', trunc, 12)[0] - 40) + trunc[0x10 + 40:]   # the bits run out: "string index out of range"
raw_s = pad(bnsh([[('vs', S('vs', 8))]]), 0x200)
data000 = cpk_file([
    ('shader', 'shader.dat', good, len(dat_a)),                                      # CRILAYLA (stored < extract)
    ('shader', 'shadereff.dat', dat_b, None),                                        # stored
    ('tex', 'a.bntx', clean(Rng(4).bytes(0x900)), None),
    ('tex', 'runs.bin', runs_c, len(runs)),
    ('fx', 'bare.glslc', glslc([S('vs', 9)]), None),                                  # a GLSLC output alone: scan_blob does not read it
    ('fx', 'eff.phyre', ryhp([S('vs', 10), S('ps', 11)]), None),
    ('bad', 'trunc.dat', trunc, len(raw_t)),                                          # skipped, the split goes on
    ('bad', 'size.dat', crilayla(raw_s), len(raw_s) + 5),                             # decodes, but not to the TOC's size
    ('bad', 'plain.bin', clean(Rng(5).bytes(0x80)), 0x90),                            # stored 0x80 of 0x90: "not CRILAYLA"
    ('', 'root.bin', bnsh([[('vs', S('vs', 12))]]), None),                            # no directory
])
data001 = cpk_file([   # masked tables (Unicorn Overlord), the TOC after the data (LUNAR), DirName a constant, no ExtractSize column (the stored size is the size)
    ('', 'x.dat', pad(bnsh([[('vs', S('vs', 1)), ('ps', S('ps', 2))]]), 0x300), None),   # the same shaders as data000's first: data000's copy is the first
    ('', 'y.dat', crilayla(pad(bnsh([[('gs', S('gs', 13))]]), 0x300)), None),           # CRILAYLA by its magic alone: more bytes than the stored size the TOC says
    ('', 'z.bin', clean(Rng(6).bytes(0x400)), None),
], toc_at_end=True, masked=True,
   toc_cols=[('DirName', 0xa, 'c', 'pl'), ('FileName', 0xa, 'r', None), ('FileSize', 4, 'r', None), ('ExtractSize', 4, 'z', None), ('FileOffset', 6, 'r', None)])
data002 = cpk_file([('m', 'q.dat', *cri(pad(bnsh([[('ps', S('ps', 14))]]), 0x500)))], toc_at_end=True)
broken = cpk_file([('a', 'b.bin', bnsh([[('vs', S('vs', 15))]]), None)], patch={'TocOffset': 0})   # "no TOC": scanned as one file, which finds the BNSH in it
jobfail = cpk_file([('a', 'cut.bin', b'CRILAYLA' + bytes(4), 0x20), ('a', 'v.dat', bnsh([[('ps', S('ps', 16))]]), None)])   # struct.error: the job's shaders are left out
make_game('sync-cpk', {
    'Data/data000.cpk': data000,
    'Data/data001.cpk': data001,
    'Data/DATA002.CPK': data002,
    'Data/broken.cpk': broken,
    'Data/jobfail.cpk': jobfail,
    'Data/notcpk.cpk': b'CPX ' + clean(Rng(7).bytes(0x100)) + bnsh([[('ps', S('ps', 17))]]),
})

VS, PS = S('vs', 30, D3_CTL), S('ps', 31, D3_CTL)
CS3 = S('cs', 32, D3_CTL)
fake = b'\x00' * 6 + b'\x78\xda' + bytes(range(256)) * 4   # stored data with a zlib header in it: not a stream
cut = struct.pack('<4I', 0xdeadbeef, 37, 1, 0) + VS[0] + VS[1][:0x40]
client = package(fake, record(sno(VS, PS)), bytes(0x3000), record(b'texture' * 400), record(sno(PS, pad=0x200)), record(cut),
                 record(sno(CS3), decoded_len=len(sno(CS3)) + 1), record(sno(S('vs', 33, D3_CTL))), record(sno(S('ps', 34, 0x800))))
core = package(record(sno(S('vs', 35, D3_CTL), S('ps', 36, D3_CTL))), bytes(0x10), record(sno(S('cs', 37, 0x760))), version=6)
make_game('sync-d3cpk', {
    'CPKs/ClientCommon.cpk': client,
    'CPKs/Update/CoreCommon.CPK': core,
    'CPKs/v8.cpk': d3cpk.MAGIC + struct.pack('<I', 8) + bnsh([[('vs', S('vs', 38))]]),   # version 8: not a package, scanned as one file
    'CPKs/swapped.cpk': struct.pack('>I', 0xa1b2c3d4) + struct.pack('<I', 7) + bytes(0x40) + record(sno(S('vs', 39, D3_CTL))),   # the byte-swapped magic is not taken
})


# ---- vectors ------------------------------------------------------------------------------------------------------------------------
M = (1 << 64) - 1
class Lcg:
    def __init__(s, seed): s.x = seed & M
    def next(s):
        s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
        return s.x >> 24
    def below(s, n): return s.next() % n


def val(v):
    """a Python value as the vectors carry it"""
    if v is None: return None
    if isinstance(v, bool): raise TypeError(v)
    if isinstance(v, int): return {'i': str(v)}
    if isinstance(v, float): return {'f': repr(v)}
    if isinstance(v, str): return {'s': v}
    if isinstance(v, (bytes, bytearray)): return {'b': bytes(v).hex()}
    raise TypeError(type(v))


def text(e, path): return f'{type(e).__name__}: {e}'.replace(path, 'P')


def cpk_outcome(path):
    log = io.StringIO()
    with contextlib.redirect_stdout(log):
        try: jobs, taken = ss.cpk_jobs([(path, 'x.cpk')])
        except Exception as e: return {'crash': text(e, path), 'log': log.getvalue().replace(path, 'P').splitlines()}
    out = {'log': log.getvalue().replace(path, 'P').splitlines(), 'taken': sorted(taken) == [path],
           'jobs': [[[e[0], val(e[1]), val(e[2]), val(e[3])] for e in j[2]] for j in jobs]}
    reads = []
    for j in jobs:
        for e in j[2]:
            try: reads.append({'ok': sha16(cpk.read(path, e))})
            except (ValueError, IndexError) as x: reads.append({'skip': str(x).replace(path, 'P')})
            except Exception as x: reads.append({'job': text(x, path)})
    out['reads'] = reads
    return out


def d3_outcome(path, size, ranges):
    out = {}
    with contextlib.redirect_stdout(io.StringIO()):
        jobs, taken = ss.d3cpk_jobs([(path, 'x.cpk')])
    out['jobs'] = [[a, b] for _, _, (_, a, b) in jobs]
    res = []
    for a, b in [tuple(x) for x in out['jobs']] + ranges:
        found, log = [], io.StringIO()
        try:
            with contextlib.redirect_stdout(log): ss.d3cpk_shaders(path, 'x.cpk', a, b, found)
            res.append({'range': [a, b], 'found': [[w, st, sha16(c + k)] for w, st, c, k in found], 'log': log.getvalue().splitlines()})
        except Exception as e: res.append({'range': [a, b], 'err': text(e, path)})
    out['ranges'] = res
    return out


rng = Lcg(0xc9c)
recs = []
tmp = tempfile.mkdtemp() + '/P.cpk'
CPKS = ['sync-cpk/RomFS/Data/data000.cpk', 'sync-cpk/RomFS/Data/data001.cpk', 'sync-cpk/RomFS/Data/DATA002.CPK', 'sync-cpk/RomFS/Data/jobfail.cpk']
for base in CPKS:
    b = open(os.path.join(out_root, base), 'rb').read()
    toc = b.find(b'TOC ')
    for k in range(90):
        muts, cut = [], None
        kind = rng.below(6)
        for _ in range(1 + rng.below(3)):
            if kind == 0: o = rng.below(0x200)                                   # the header's @UTF table
            elif kind == 1: o = toc + rng.below(min(len(b) - toc, 0x200))        # the TOC
            elif kind == 2: o = rng.below(len(b))                                # anywhere
            elif kind == 3: o = 0x10 + rng.below(0x30)                            # the header table's own header
            elif kind == 4: o = toc + 0x10 + rng.below(0x60)                      # the TOC table's header and columns
            else: o = 0x1800 + rng.below(max(1, len(b) - 0x1800))                 # the entries' data (CRILAYLA bits)
            v = rng.below(256) if rng.below(3) else b[min(o, len(b) - 1)] ^ (1 << rng.below(8))
            muts.append([min(o, len(b) - 1), v])
        if rng.below(7) == 0: cut = rng.below(len(b))
        d = bytearray(b)
        for o, v in muts: d[o] = v
        if cut is not None: d = d[:cut]
        if any(i + 12 <= len(d) and struct.unpack_from('<I', d, i + 8)[0] > 1 << 26 for i in range(len(d) - 7) if d[i:i + 8] == b'CRILAYLA'): continue   # no 4 GB buffer here
        open(tmp, 'wb').write(d)
        recs.append({'kind': 'cpk', 'base': base, 'muts': muts, 'cut': cut, 'out': cpk_outcome(tmp)})
    open(tmp, 'wb').write(b)
    recs.append({'kind': 'cpk', 'base': base, 'muts': [], 'cut': None, 'out': cpk_outcome(tmp)})
# values of other types where the reader expects numbers and strings (a damaged column type): what the Python does with each, archive by archive (carried as hex)
def col(cols, name, t, st='r', k=None): return [(c, t, st, k) if c == name else (c, t0, st0, k0) for c, t0, st0, k0 in cols]
def setrows(**kv):
    def f(rows):
        for r in rows: r.update({k: (v(r) if callable(v) else v) for k, v in kv.items()})
    return f
def hdr(name, t, v, st='r'):
    def f(hcols, hrows):
        for r in hrows: r[name] = v
        return col(hcols, name, t, st, v if st == 'c' else None), hrows
    return f
small = [('d', 'a.bin', bnsh([[('vs', S('vs', 50))]]), None), ('d', 'b.bin', clean(Rng(8).bytes(0x300)), None)]
TYPED = {
    'filesize-str': dict(toc_cols=col(TOC_COLS, 'FileSize', 0xa), rowfix=setrows(FileSize='12')),
    'extract-float-equal': dict(toc_cols=col(TOC_COLS, 'ExtractSize', 8), rowfix=setrows(ExtractSize=lambda r: float(r['FileSize']))),
    'extract-float-frac': dict(toc_cols=col(TOC_COLS, 'ExtractSize', 8), rowfix=setrows(ExtractSize=lambda r: r['FileSize'] + 0.5)),
    'dirname-bytes': dict(toc_cols=col(TOC_COLS, 'DirName', 0xb), rowfix=setrows(DirName=b'dd')),
    'dirname-bytes-empty': dict(toc_cols=col(TOC_COLS, 'DirName', 0xb), rowfix=setrows(DirName=b'')),
    'dirname-int': dict(toc_cols=col(TOC_COLS, 'DirName', 4), rowfix=setrows(DirName=3)),
    'dirname-zero': dict(toc_cols=col(TOC_COLS, 'DirName', 4), rowfix=setrows(DirName=0)),
    'filename-missing': dict(toc_cols=[c for c in TOC_COLS if c[0] != 'FileName']),
    'filename-none': dict(toc_cols=col(TOC_COLS, 'FileName', 0xa, 'z')),
    'fileoffset-negative': dict(toc_cols=col(TOC_COLS, 'FileOffset', 7), rowfix=setrows(FileOffset=-0x900)),
    'fileoffset-huge': dict(rowfix=setrows(FileOffset=(1 << 63) + 5)),
    'fileoffset-float': dict(toc_cols=col(TOC_COLS, 'FileOffset', 8), rowfix=setrows(FileOffset=lambda r: float(r['FileOffset']))),
    'filesize-minus-one': dict(toc_cols=col(TOC_COLS, 'FileSize', 5), rowfix=setrows(FileSize=-1)),
    'filesize-minus-two': dict(toc_cols=col(TOC_COLS, 'FileSize', 5), rowfix=setrows(FileSize=-2)),
    'filesize-huge': dict(toc_cols=col(TOC_COLS, 'FileSize', 6), rowfix=setrows(FileSize=1 << 63)),
    'filesize-none': dict(toc_cols=col(TOC_COLS, 'FileSize', 4, 'z')),
    'sizes-none': dict(toc_cols=col(col(TOC_COLS, 'FileSize', 4, 'z'), 'ExtractSize', 4, 'z')),
    'extract-str': dict(toc_cols=col(TOC_COLS, 'ExtractSize', 0xa), rowfix=setrows(ExtractSize='x')),
    'column-type-c': dict(toc_cols=col(TOC_COLS, 'ID', 0xc)),
    'const-type-c': dict(toc_cols=col(TOC_COLS, 'UserString', 0xc, 'c', '')),
    'content-str': dict(hfix=hdr('ContentOffset', 0xa, 'x')),
    'content-zero': dict(hfix=hdr('ContentOffset', 6, 0)),
    'content-float-small': dict(hfix=hdr('ContentOffset', 8, 0x400 + 0.25)),
    'content-float-equal': dict(hfix=hdr('ContentOffset', 8, 2048.0)),
    'toc-str': dict(hfix=hdr('TocOffset', 0xa, 'x')),
    'toc-float': dict(hfix=hdr('TocOffset', 8, 2048.0)),
    'toc-negative': dict(hfix=hdr('TocOffset', 7, -5)),
    'toc-huge': dict(hfix=hdr('TocOffset', 6, 1 << 63)),
    'toc-past-end': dict(hfix=hdr('TocOffset', 6, 1 << 40)),
    'toc-const': dict(hfix=hdr('TocOffset', 6, 0x800, 'c')),
    'head-two-rows': dict(hfix=lambda c, r: (c, r + r)),
    'head-no-rows': dict(hfix=lambda c, r: (c, [])),
}
for name, kw in TYPED.items():
    d = cpk_file(small, **kw)
    open(tmp, 'wb').write(d)
    recs.append({'kind': 'cpk-hex', 'base': name, 'hex': d.hex(), 'muts': [], 'cut': None, 'out': cpk_outcome(tmp)})
# the CRILAYLA decoder alone, on damaged streams (the stored bytes of entries of data000.cpk, read by their TOC rows): the bits, the stored prefix, the sizes in its header
d000 = open(os.path.join(out_root, CPKS[0]), 'rb').read()
for name in ['shader/shader.dat', 'tex/runs.bin', 'bad/trunc.dat', 'bad/size.dat']:
    e = next(e for e in cpk.files(os.path.join(out_root, CPKS[0])) if e[0] == name)
    c = d000[e[1]:e[1] + e[2]]
    for k in range(60):
        d = bytearray(c); muts = []
        for _ in range(1 + rng.below(4)):
            o = rng.below(len(d)) if rng.below(2) else [8, 9, 12, 13, 14, 15][rng.below(6)]   # not the top bytes of the size: no 4 GB buffer
            v = rng.below(256); d[o] = v; muts.append([o, v])
        cut = rng.below(len(d)) if rng.below(6) == 0 else None
        if cut is not None: d = d[:cut]
        if len(d) >= 12 and struct.unpack_from('<I', d, 8)[0] > 1 << 26: continue
        try: x = cpk.crilayla(bytes(d)); r = {'ok': sha16(x), 'len': len(x)}
        except (ValueError, IndexError) as x: r = {'skip': str(x)}
        except Exception as x: r = {'job': f'{type(x).__name__}: {x}'}
        recs.append({'kind': 'crilayla', 'base': CPKS[0] + '#' + name, 'muts': muts, 'cut': cut, 'out': r})
D3 = 'sync-d3cpk/RomFS/CPKs/ClientCommon.cpk'
b = open(os.path.join(out_root, D3), 'rb').read()
for k in range(80):
    muts, cut = [], None
    for _ in range(1 + rng.below(3)):
        o = rng.below(len(b)) if rng.below(2) else rng.below(0x60)
        muts.append([o, rng.below(256)])
    if rng.below(6) == 0: cut = rng.below(len(b))
    d = bytearray(b)
    for o, v in muts: d[o] = v
    if cut is not None: d = d[:cut]
    open(tmp, 'wb').write(d)
    ranges = []
    for _ in range(2):
        c1 = rng.below(len(d) + 1); ranges.append([0, c1]); ranges.append([c1, len(d)])
    recs.append({'kind': 'd3', 'base': D3, 'muts': muts, 'cut': cut, 'out': d3_outcome(tmp, len(d), ranges)})
open(tmp, 'wb').write(b)
recs.append({'kind': 'd3', 'base': D3, 'muts': [], 'cut': None, 'out': d3_outcome(tmp, len(b), [[0, c], [c, len(b)]] if False else [[0, x] for x in range(0, len(b), 97)] + [[x, len(b)] for x in range(0, len(b), 97)])})
with gzip.open(os.path.join(crate, 'tests/data/vectors-cpk.jsonl.gz'), 'wt') as f:
    for r in recs: f.write(json.dumps(r, sort_keys=True) + '\n')
print(len(recs), 'vectors')
