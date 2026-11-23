#!/usr/bin/env python3
"""Writes tests/data/vectors-ttdat-frostbite.jsonl.gz: the containers of tests/data/games/sync-ttdat and sync-frostbite (tools/gen_ttdat_frostbite.py), damaged in many places (bytes
changed, the file cut), and what the engine's own `ttdat.py` / `frostbite.py` / `shader_split.py` (upstream, no Mac host) answers for each:
  * a TT DAT: `ttdat.entries` (the members or its error), each member's `ttdat.read` (with the game's decoder `oodle`) and `ttdat_programs` (the shaders or the error);
  * a Frostbite file (layout.toc, a superbundle .toc or a cas file, put back in a copy of the RomFS): `frostbite.resources` (the CompiledBytecode resources, or only that it raises: the
    Python's planner then ends the split), and each resource as `frostbite_shaders` reads it (`read` + `nvn_shader` + the stage and `check`).
tests/ttdat_frostbite.rs damages the same files the same way and must give the same answers.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14> -I tools/gen_ttdat_frostbite_vectors.py <upstream tree (the folder that holds dm/)> <crate folder>

A DAT's path in the messages is written as P. Nothing comes from a game.
"""
import sys, os, json, gzip, hashlib, tempfile, shutil, struct
engine, crate = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
prof = tempfile.mkdtemp(); os.makedirs(prof + '/0100TESTTEST0000')
open(prof + '/0100TESTTEST0000/game.tsv', 'w').write('title_id\t0100TESTTEST0000\nshader_groups\t/x/=x\nshader_note\ttest\n')
os.environ.update(DM_GAMES_DIR=prof, DM_GAMES_FIRST='1', DM_GAME='0100TESTTEST0000')
os.environ.pop('DM_OODLE_HOST', None)
tools = engine + '/dm/gfx/tools'
for sub in ('', '/../../tools', '/../../games'): sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, ttdat, frostbite

M = (1 << 64) - 1
class Rng:
    def __init__(s, seed): s.x = seed & M
    def next(s):
        s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
        return s.x >> 24
    def below(s, n): return s.next() % n

def text(e, path=None):
    if isinstance(e, FileNotFoundError): return 'FileNotFoundError'
    s = str(e).removeprefix('[RED] ') if isinstance(e, SystemExit) else f'{type(e).__name__}: {e}'
    if 'DM_OODLE_HOST' in s: return 'OODLE'   # no Mac host: the Python's red line; this port refuses the container
    return s.replace(path, 'P') if path else s

def sha16(b): return hashlib.sha256(b).hexdigest()[:16]

def tt_outcome(path):
    try: es = ttdat.entries(path)
    except (ValueError, OSError, struct.error) as e: return {'entries': {'err': str(e).replace(path, 'P')}}   # ttdat_jobs keeps str(e)
    if es is None: return {'entries': None}
    out = {'entries': {'ok': [list(e) for e in es]}, 'members': []}
    with open(path, 'rb') as f:
        for e in es:
            label = f'/x/{e[0]}'
            try: d = ttdat.read(f, e, ss.oodle)
            except (Exception, SystemExit) as x: out['members'].append({'err': text(x)}); continue
            try: found = ss.ttdat_programs(d, label)
            except (Exception, SystemExit) as x: out['members'].append({'read': sha16(d), 'err': text(x)}); continue
            out['members'].append({'read': sha16(d), 'ok': [[w, st, sha16(bytes(c) + bytes(k))] for w, st, c, k in found]})
    return out

def fb_outcome(root):
    try: res = frostbite.resources(root, frostbite.COMPILED_BYTECODE)
    except (Exception, SystemExit): return {'resources': 'err'}
    out = {'resources': sorted([os.path.relpath(c, root), o, s, z, n] for (c, o, s), (z, n) in res.items()), 'items': []}
    for (cas, off, stored), (size, name) in sorted(res.items()):
        try:
            with open(cas, 'rb') as f: d = f.read()
            sh = frostbite.nvn_shader(frostbite.read(d, off, stored, size, ss.oodle))
        except (Exception, SystemExit) as x: out['items'].append({'err': text(x)}); continue
        if sh is None: out['items'].append({'red': 'no VNVN record'}); continue
        code, ctl = sh; st = ss.glslc_stage(ctl, code)
        why = ss.check('x', st, ctl, code) if st else 'no stage in the code area header'
        out['items'].append({'red': why} if why else {'ok': [st, sha16(bytes(ctl) + bytes(code))]})
    return out

rng = Rng(0x77d47)
recs = []
games = os.path.join(crate, 'tests/data/games')
TT = [('sync-ttdat/RomFS/Data/GAME.DAT', 130), ('sync-ttdat/dlc/1/DLC.DAT', 90), ('sync-ttdat/RomFS/Pack/amb.dat', 30)]
tmp = tempfile.mkdtemp() + '/P.dat'
for base, n in TT:
    b = open(os.path.join(games, base), 'rb').read()
    off = struct.unpack_from('<I', b)[0]
    if off & 0x80000000: off = ((off ^ 0xffffffff) + 1) << 8
    open(tmp, 'wb').write(b); es = ttdat.entries(tmp)
    heads = [o for _, at, _, _ in es for o in range(at, at + 0x30)]
    for k in range(n):
        muts, cut = [], None
        kind = rng.below(4)
        for _ in range(1 + rng.below(2)):
            if kind == 0: o = off + rng.below(len(b) - off)          # the index
            elif kind == 1: o = rng.below(len(b))                    # anywhere
            elif kind == 2: o = heads[rng.below(len(heads))]          # a member's first bytes (its chunk headers)
            else: o = rng.below(8)                                    # the index pointer
            muts.append([o, rng.below(256)])
        if rng.below(8) == 0: cut = rng.below(len(b))
        d = bytearray(b)
        for o, v in muts: d[o] = v
        if cut is not None: d = d[:cut]
        open(tmp, 'wb').write(d)
        recs.append({'kind': 'tt', 'base': base, 'muts': muts, 'cut': cut, 'out': tt_outcome(tmp)})
    open(tmp, 'wb').write(b)
    recs.append({'kind': 'tt', 'base': base, 'muts': [], 'cut': None, 'out': tt_outcome(tmp)})

FB = [('Data/layout.toc', 40), ('Data/NX/game.toc', 70), ('Data/NX/sub/more.toc', 30), ('Data/NX/superbundlelayout/installpackage_00/cas_01.cas', 70),
      ('Data/NX/superbundlelayout/installpackage_00/cas_02.cas', 40), ('Data/NX/superbundlelayout/installpackage_12/cas_12.cas', 40)]
src_root = os.path.join(games, 'sync-frostbite/RomFS')
work = tempfile.mkdtemp() + '/RomFS'
for rel, n in FB:
    b = open(os.path.join(src_root, rel), 'rb').read()
    for k in range(n + 1):
        muts, cut = [], None
        if k < n:
            kind = rng.below(3)
            for _ in range(1 + rng.below(2)):
                if kind == 0: o = rng.below(len(b))
                elif kind == 1 and rel.endswith('.toc'): o = 0x22c + rng.below(max(1, len(b) - 0x22c))
                else: o = rng.below(min(len(b), 0x200))
                muts.append([o, rng.below(256)])
            if rng.below(8) == 0: cut = rng.below(len(b))
        d = bytearray(b)
        for o, v in muts: d[o] = v
        if cut is not None: d = d[:cut]
        shutil.rmtree(work, ignore_errors=True); shutil.copytree(src_root, work)
        open(os.path.join(work, rel), 'wb').write(d)
        recs.append({'kind': 'fb', 'base': rel, 'muts': muts, 'cut': cut, 'out': fb_outcome(work + '/')})
with gzip.open(os.path.join(crate, 'tests/data/vectors-ttdat-frostbite.jsonl.gz'), 'wt') as f:
    for r in recs: f.write(json.dumps(r, sort_keys=True) + '\n')
print(len(recs), 'vectors')
