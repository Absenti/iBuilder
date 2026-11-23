#!/usr/bin/env python3
"""Writes tests/data/vectors-split-rest.jsonl.gz: the containers of tests/data/games/rest-* (tools/gen_split_rest.py) damaged in many places (bytes changed, the file cut), and what the
engine's own shader_split.py / vbf.py (upstream 033793f0) answers for each, reader by reader: sharc_shaders, noire_wad_shaders, rdhs_shaders, fzlc_shaders, ipk_shaders, mtarc_shaders,
ttarch2_shaders, bahs_shaders (with the variations it records), nxcx_shaders, pacx4_parts, vbf.entries / vbf.read; and raw LZMA1 streams (lzma.LZMADecompressor, FORMAT_RAW) cut and
damaged, with the bytes they decode to. tests/split_rest.rs damages the same files the same way and must give the same answers.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_split_rest_vectors.py <upstream tree (the folder that holds dm/)> <crate folder>

Nothing comes from a game.
"""
import sys, os, json, gzip, hashlib, tempfile, lzma, struct
engine, crate = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
prof = tempfile.mkdtemp(); os.makedirs(prof + '/0100TESTTEST0000')
open(prof + '/0100TESTTEST0000/game.tsv', 'w').write('title_id\t0100TESTTEST0000\nshader_groups\t/x/=x\nshader_note\ttest\n')
os.environ.update(DM_GAMES_DIR=prof, DM_GAMES_FIRST='1', DM_GAME='0100TESTTEST0000')
tools = engine + '/dm/gfx/tools'
for sub in ('', '/../../tools', '/../../games'): sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, vbf

M = (1 << 64) - 1
class Rng:
    def __init__(s, seed): s.x = seed & M
    def next(s):
        s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
        return s.x >> 24
    def below(s, n): return s.next() % n

def h16(b): return hashlib.sha256(bytes(b)).hexdigest()[:16]
def text(e): return str(e).removeprefix('[RED] ') if isinstance(e, SystemExit) else f'{type(e).__name__}: {e}'
def shaders(found): return [[w, st, h16(bytes(c) + bytes(k))] for w, st, c, k in found]

def mutate(rng, b, regions):
    muts, cut = [], None
    for _ in range(1 + rng.below(3)):
        lo, hi = regions[rng.below(len(regions))]
        lo, hi = max(0, min(lo, len(b) - 1)), max(1, min(hi, len(b)))
        o = lo + rng.below(max(1, hi - lo))
        muts.append([o, rng.below(256)])
    if rng.below(6) == 0: cut = rng.below(len(b))
    d = bytearray(b)
    for o, v in muts: d[o] = v
    if cut is not None: d = d[:cut]
    return muts, cut, bytes(d)

def run(fn):
    try: return {'ok': fn()}
    except (Exception, SystemExit) as e: return {'err': text(e)}

rng = Rng(0x5e57)
recs = []
G = os.path.join(crate, 'tests/data/games')
def rd(p): return open(os.path.join(G, p), 'rb').read()
tmp = tempfile.mkdtemp()
def at_path(d):
    p = os.path.join(tmp, 'f'); open(p, 'wb').write(d); return p

def head_regions(b, n=0x200): return [(0, min(n, len(b))), (max(0, len(b) - 64), len(b))]
def whole(b): return [(0, len(b))]

def bahs_outcome(d):
    ss.BAHS_VAR.clear()
    r = run(lambda: shaders(ss.bahs_shaders(d, '/x.sharcb')))
    r['var'] = sorted([k, v] for k, v in ss.BAHS_VAR.items())
    return r

def ipk_outcome(d):
    out = []
    def f(): ss.ipk_shaders(at_path(d), 'x.ipk', out); return shaders(out)
    return run(f)

def mtarc_outcome(d):
    out = []
    def f(): ss.mtarc_shaders(d, '/x.arc', out); return shaders(out)
    return run(f)

def nxcx_outcome(d):
    def f():
        with open(at_path(d), 'rb') as fh:
            r = ss.nxcx_shaders(fh, '/x.mrg')
        return None if r is None else shaders(r)
    return run(f)

def vbf_outcome(d):
    p = at_path(d)
    def f():
        es = vbf.entries(p); out = []
        with open(p, 'rb') as fh:
            for e in es:
                m = run(lambda: h16(vbf.read(fh, e)))
                out.append([e[0], e[1], e[2], list(e[3]), m])
        return out
    return run(f)

def rdhs_outcome(d): return run(lambda: shaders(ss.rdhs_shaders(at_path(d), '/c'))) if len(d) >= 16 else {'skip': True}

KINDS = {
    'sharc': (lambda d: run(lambda: shaders(ss.sharc_shaders(d, '/x.sharc'))), ['rest-sharc/RomFS/Data/shared/shader_switch.sharc'], 'head'),
    'wad': (lambda d: run(lambda: shaders(ss.noire_wad_shaders(d, '/out.wad.nx'))), ['rest-wad/RomFS/Data/a/out.wad.nx'], 'whole'),
    'rdhs': (rdhs_outcome, ['rest-redengine/RomFS/content/content0/shadernx.cache'], 'rdhs'),
    'fzlc': (lambda d: run(lambda: shaders(ss.fzlc_shaders(at_path(d), '/c'))), ['rest-redengine/RomFS/content/content0/staticshadernx.cache'], 'fzlc'),
    'ipk': (ipk_outcome, ['rest-ipk/RomFS/Data/bundle_nx.ipk', 'rest-ipk/RomFS/Data/legacy.ipk'], 'ipk'),
    'mtarc': (mtarc_outcome, ['rest-mtarc/RomFS/Data/re/root.arc', 'rest-mtarc/RomFS/Data/sf/root.arc'], 'arc'),
    'tt': (lambda d: run(lambda: shaders(ss.ttarch2_shaders(d, '/x.ttarch2'))), ['rest-telltale/RomFS/Data/SM3_nx_Shaders.ttarch2', 'rest-telltale/RomFS/Data/Patch.ttarch2'], 'tt'),
    'bahs': (bahs_outcome, ['rest-bahs/RomFS/Data/payload.sharcb', 'rest-bahs/RomFS/Data/nvn1.sharcb'], 'bahs'),
    'nxcx': (nxcx_outcome, ['rest-nxcx/RomFS/Data/w_allpac.mrg'], 'whole'),
    'pacx': (lambda d: run(lambda: [[n, h16(p)] for n, p in ss.pacx4_parts(d, '/x.pac')]), ['rest-pacx/RomFS/Data/d.pac', 'rest-pacx/RomFS/Data/c.pac'], 'whole'),
    'vbf': (vbf_outcome, ['rest-vbf-zip-play/RomFS/Data/ffx.vbf'], 'vbf'),
}

def regions(kind, b):
    if kind == 'whole': return whole(b)
    if kind == 'head': return head_regions(b, 0x120)
    if kind == 'rdhs': return [(0, 17), (len(b) - 40, len(b))]   # the first record's header, the index and footer (not inside a zstd frame)
    if kind == 'fzlc': return [(0, 12 + 12 * struct.unpack_from('<I', b, 4)[0])]   # the header and the chunk table
    if kind == 'ipk': return [(0, struct.unpack_from('>I', b, 12)[0])]   # the header and the entry table
    if kind == 'arc': return [(0, 0x200), (0x8000, 0x8004)]
    if kind == 'tt': return [(0, 0x60)]   # the block headers (a damaged deflate chunk is tested by the games)
    if kind == 'bahs': return [(0, 0x400)]
    if kind == 'vbf': return [(0, struct.unpack_from('<I', b, 4)[0])]
    return whole(b)

for kind, (fn, paths, reg) in KINDS.items():
    for fi, p in enumerate(paths):
        b = rd(p)
        recs.append({'kind': kind, 'file': fi, 'muts': [], 'cut': None, 'want': fn(b)})
        for _ in range(140):
            muts, cut, d = mutate(rng, b, regions(reg, b))
            recs.append({'kind': kind, 'file': fi, 'muts': muts, 'cut': cut, 'want': fn(d)})
        for cut in sorted({rng.below(len(b)) for _ in range(30)} | {0, 1, 4, 15, 16, 0x2b, 0x2c, len(b) - 1}):
            if 0 <= cut < len(b): recs.append({'kind': kind, 'file': fi, 'muts': [], 'cut': cut, 'want': fn(b[:cut])})

# ---- raw LZMA1: streams of the SHARC game's members and of random data, cut and damaged, decoded with several properties and output limits ---------------------------------------
def lz(data, lc, lp, pb, ds):
    return lzma.compress(data, format=lzma.FORMAT_RAW, filters=[{'id': lzma.FILTER_LZMA1, 'dict_size': ds, 'lc': lc, 'lp': lp, 'pb': pb}])

def lz_outcome(d, lc, lp, pb, ds, mx):
    try:
        dec = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=[{'id': lzma.FILTER_LZMA1, 'dict_size': ds, 'lc': lc, 'lp': lp, 'pb': pb}])
        out = dec.decompress(d, max_length=mx)
        return {'ok': [len(out), h16(out), dec.eof]}
    except (lzma.LZMAError, ValueError) as e: return {'err': str(e)}

samples = []
r2 = Rng(77)
text_like = b''.join(b'shader %d uses texture %d;\n' % (i, i * 7 % 13) for i in range(400))
noise = bytes(r2.below(256) for _ in range(3000))
mixed = text_like[:2000] + noise[:500] + text_like[1000:3000] + bytes(700) + noise[500:900]
for data in (text_like, noise, mixed, rd('rest-sharc/RomFS/Data/shared/shader_switch.sharc')):
    for lc, lp, pb in ((3, 0, 2), (0, 2, 0), (4, 0, 4), (1, 3, 1), (8, 0, 2), (2, 3, 2)):
        samples.append((data, lc, lp, pb))
for si, (data, lc, lp, pb) in enumerate(samples):
    ds = [1 << 20, 4096, 100, 1 << 16][si % 4]
    try: stream = lz(data, lc, lp, pb, max(ds, 4096))
    except lzma.LZMAError: stream = lz(data, 3, 0, 2, 1 << 20)
    cases = [(stream, len(data) + 1), (stream[:-1], len(data) + 1), (stream, len(data) // 3), (stream[:5], 10), (b'', 10)]
    for _ in range(25): cases.append((stream[:r2.below(len(stream))], len(data) + 1))
    for _ in range(25):
        d = bytearray(stream)
        for _ in range(1 + r2.below(3)): d[r2.below(len(d))] = r2.below(256)
        cases.append((bytes(d), len(data) + 1))
    for d, mx in cases:
        recs.append({'kind': 'lzma', 'sample': si, 'lc': lc, 'lp': lp, 'pb': pb, 'ds': ds, 'max': mx, 'data': d.hex(), 'want': lz_outcome(d, lc, lp, pb, ds, mx)})

out = os.path.join(crate, 'tests/data/vectors-split-rest.jsonl.gz')
with gzip.GzipFile(out, 'wb', mtime=0) as f:
    for r in recs: f.write((json.dumps(r, sort_keys=True) + '\n').encode())
kinds = {}
for r in recs: kinds[r['kind']] = kinds.get(r['kind'], 0) + 1
print(len(recs), 'vectors', kinds)
