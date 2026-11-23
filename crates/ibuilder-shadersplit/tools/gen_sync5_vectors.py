#!/usr/bin/env python3
"""Writes tests/data/vectors-sync5.jsonl.gz: the containers of tests/data/games/sync-lspk, sync-casc and sync-pdat (tools/gen_sync5.py) damaged in many places (bytes changed, the file
cut), and what the engine's own lspk.py / casc.py / shader_split.py (upstream) answers for each:
  lspk:  lspk.package() of the damaged package (its entries, or its error), lspk.read() of every member, bshd_shaders() of every .bshd member that reads; also damaged BSHD records;
  casc:  casc.files() of the folder with a damaged build manifest, or casc.read() of every current file from a damaged archive;
  pdat:  pdat_shaders() of the damaged archive over its whole length and over byte ranges that start and end inside it.
tests/sync5.rs damages the same files the same way and must give the same answers.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_sync5_vectors.py <upstream tree (the folder that holds dm/)> <crate folder>

Paths in the messages are written as P (the package) and F (the CASC folder). Nothing comes from a game.
"""
import sys, os, json, gzip, hashlib, tempfile, shutil
engine, crate = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
prof = tempfile.mkdtemp(); os.makedirs(prof + '/0100TESTTEST0000')
open(prof + '/0100TESTTEST0000/game.tsv', 'w').write('title_id\t0100TESTTEST0000\nshader_groups\t/x/=x\nshader_note\ttest\n')
os.environ.update(DM_GAMES_DIR=prof, DM_GAMES_FIRST='1', DM_GAME='0100TESTTEST0000')
tools = engine + '/dm/gfx/tools'
for sub in ('', '/../../tools', '/../../games'): sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, lspk, casc, zlib

M = (1 << 64) - 1
class Rng:
    def __init__(s, seed): s.x = seed & M
    def next(s):
        s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
        return s.x >> 24
    def below(s, n): return s.next() % n

def h16(b): return hashlib.sha256(bytes(b)).hexdigest()[:16]
def text(e):
    return str(e).removeprefix('[RED] ') if isinstance(e, SystemExit) else f'{type(e).__name__}: {e}'
def shaders(found): return [[w, st, h16(bytes(c) + bytes(k))] for w, st, c, k in found]

def mutate(rng, b, regions, ascii_=False):
    muts, cut = [], None
    for _ in range(1 + rng.below(3)):
        lo, hi = regions[rng.below(len(regions))]
        lo, hi = max(0, min(lo, len(b) - 1)), max(1, min(hi, len(b)))
        o = lo + rng.below(max(1, hi - lo))
        v = b'|\n0123456789z#= x-_:abcdef'[rng.below(26)] if ascii_ and rng.below(4) else rng.below(256)
        muts.append([o, v])
    if rng.below(8) == 0: cut = rng.below(len(b))
    d = bytearray(b)
    for o, v in muts: d[o] = v
    if cut is not None: d = d[:cut]
    return muts, cut, bytes(d)

rng = Rng(0x5ec5)
recs = []
G = os.path.join(crate, 'tests/data/games')
tmp = tempfile.mkdtemp()

# ---- lspk ------------------------------------------------------------------------------------------------------------------------------
def lspk_outcome(d0, d1):
    p = os.path.join(tmp, 'P.pak'); open(p, 'wb').write(d0); open(os.path.join(tmp, 'P_1.pak'), 'wb').write(d1)
    try: pk = lspk.package(p)
    except (ValueError, OSError) as e: return {'package': {'err': str(e).replace(p, 'P').replace(tmp, 'T')}}
    except Exception as e: return {'package': {'crash': text(e)}}
    if pk is None: return {'package': {'none': True}}
    out = {'package': {'ok': [[e[0], int(e[1] != p), e[2], e[3], e[4], e[5]] for e in pk[1]]}, 'members': []}
    for e in pk[1]:
        try: data = lspk.read(e)
        except ValueError as x: out['members'].append({'err': str(x)}); continue
        m = {'ok': h16(data)}
        if e[0].lower().endswith('.bshd'):
            try: m['shaders'] = {'ok': shaders(ss.bshd_shaders(data, '/x/' + e[0]))}
            except (Exception, SystemExit) as x: m['shaders'] = {'err': text(x)}
        out['members'].append(m)
    return out

main0, main1 = (open(f'{G}/sync-lspk/RomFS/Data/{n}', 'rb').read() for n in ('Main.pak', 'Main_1.pak'))
n0 = len(main0)
for k in range(260):
    which = rng.below(3)
    base = main1 if which == 2 else main0
    regions = [(n0 - 40, n0), (n0 - 400, n0 - 40), (0, n0)] if which != 2 else [(0, len(main1))]
    muts, cut, d = mutate(rng, base, regions)
    recs.append({'kind': 'lspk', 'part': int(which == 2), 'muts': muts, 'cut': cut,
                 'out': lspk_outcome(d if which != 2 else main0, d if which == 2 else main1)})
recs.append({'kind': 'lspk', 'part': 0, 'muts': [], 'cut': None, 'out': lspk_outcome(main0, main1)})
# BSHD records alone (the loose one)
sys.path.insert(0, os.path.join(crate, 'tools'))
import gen_sync5 as g5   # its builders (its main part runs only as a script)
plain = g5.bshd([(1, g5.glslc([g5.program('vs', 50)])), (3, g5.glslc([g5.program('ps', 51)])), (7, b'other GPU')], compressed=False)
for k in range(320):
    rec0 = open(f'{G}/sync-lspk/RomFS/Data/Loose.bshd', 'rb').read() if k % 4 == 0 else plain
    muts, cut, d = mutate(rng, rec0, [(0, 0x60), (0, 0x120), (0, len(rec0))])
    try: o = {'ok': shaders(ss.bshd_shaders(d, '/x'))}
    except (Exception, SystemExit) as x: o = {'err': text(x)}
    recs.append({'kind': 'bshd', 'base': 'loose' if k % 4 == 0 else 'plain', 'muts': muts, 'cut': cut, 'out': o})

# ---- casc ------------------------------------------------------------------------------------------------------------------------------
C = f'{G}/sync-casc/RomFS/data'
def casc_files_outcome(folder):
    try: es = casc.files(folder)
    except casc.Error as e: return {'casc': str(e).replace(folder, 'F')}
    except Exception as e: return {'other': text(e).replace(folder, 'F')}
    return {'ok': [list(e) for e in es]}

good = casc.files(C)
manifest0 = open(f'{C}/build_manifest.psv', 'rb').read()
for k in range(220):
    muts, cut, d = mutate(rng, manifest0, [(0, 60), (60, len(manifest0)), (0, len(manifest0))], ascii_=True)
    f = os.path.join(tmp, 'F'); shutil.rmtree(f, ignore_errors=True); shutil.copytree(C, f)
    open(f'{f}/build_manifest.psv', 'wb').write(d)
    recs.append({'kind': 'casc-manifest', 'muts': muts, 'cut': cut, 'out': casc_files_outcome(f)})
for arch in ('00-00000001.data', '00-00000002.data'):
    a0 = open(f'{C}/{arch}', 'rb').read()
    ents = [e for e in good if e[1] == arch]
    regions = [(e[2], e[2] + min(e[3], 0x40)) for e in ents] + [(0, len(a0))]
    for k in range(120):
        muts, cut, d = mutate(rng, a0, regions)
        p = os.path.join(tmp, 'A.data'); open(p, 'wb').write(d)
        res = []
        with open(p, 'rb') as fh:
            for e in ents:
                try: res.append({'ok': h16(casc.read(fh, e))})
                except (casc.Error, zlib.error) as x: res.append({'skip': str(x)})
                except Exception as x: res.append({'job': text(x)})
        recs.append({'kind': 'casc-archive', 'archive': arch, 'muts': muts, 'cut': cut, 'out': res})

# ---- pdat ------------------------------------------------------------------------------------------------------------------------------
def pdat_outcome(d):
    p = os.path.join(tmp, 'P.dat'); open(p, 'wb').write(d)
    n, out = len(d), []
    for start, end in [(0, n), (0, n // 3), (n // 3, n), (n // 4, n // 2), (0x30, n - 0x30)]:
        if end <= start: continue
        found = []
        try: ss.pdat_shaders(p, 'x.dat', start, end, found); out.append([start, end, {'ok': shaders(found)}])
        except (Exception, SystemExit) as x: out.append([start, end, {'err': text(x)}])
    return out

for name in ('a.dat', 'b.dat'):
    b = open(f'{G}/sync-pdat/RomFS/Data/{name}', 'rb').read()
    for k in range(130):
        muts, cut, d = mutate(rng, b, [(0, len(b))])
        recs.append({'kind': 'pdat', 'base': name, 'muts': muts, 'cut': cut, 'out': pdat_outcome(d)})
    recs.append({'kind': 'pdat', 'base': name, 'muts': [], 'cut': None, 'out': pdat_outcome(b)})

with gzip.open(os.path.join(crate, 'tests/data/vectors-sync5.jsonl.gz'), 'wt') as f:
    for r in recs: f.write(json.dumps(r, sort_keys=True) + '\n')
print(len(recs), 'vectors')
