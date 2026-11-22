#!/usr/bin/env python3
"""Writes tests/data/vectors-uepak.jsonl.gz: the synthetic paks of tests/data/games/ue-* (tools/gen_uepak.py), damaged in many places (bytes changed, the file cut), and what the engine's
own `uepak.py` / `shader_split.py` (upstream origin/main) answers for each: the footer, the route (`pak_shader_sources`), every file's bytes or the error reading it (`Pak.read` with the
game's decoder `oodle`, no Mac host), and the shaders `ue_inline_shaders` finds in each or its error. tests/uepak.rs damages the same files the same way and must give the same answers.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_uepak_vectors.py <upstream tree (the folder that holds dm/)> <crate folder>

The pak's path in the messages is written as P. Nothing comes from a game.
"""
import sys, os, json, gzip, hashlib, tempfile, struct
engine, crate = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
prof = tempfile.mkdtemp(); os.makedirs(prof + '/0100TESTTEST0000')
open(prof + '/0100TESTTEST0000/game.tsv', 'w').write('title_id\t0100TESTTEST0000\nshader_groups\t/x/=x\nshader_note\ttest\n')
os.environ.update(DM_GAMES_DIR=prof, DM_GAMES_FIRST='1', DM_GAME='0100TESTTEST0000')
os.environ.pop('DM_OODLE_HOST', None)
tools = engine + '/dm/gfx/tools'
for sub in ('', '/../../tools', '/../../games'): sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, uepak

M = (1 << 64) - 1
class Rng:
    def __init__(s, seed): s.x = seed & M
    def next(s):
        s.x = (s.x * 6364136223846793005 + 1442695040888963407) & M
        return s.x >> 24
    def below(s, n): return s.next() % n

BASES = ['ue-v8-kakarot/RomFS/AT/Content/Paks/pakchunk0-Switch.pak', 'ue-v8-kakarot/RomFS/AT/Content/Paks/pakchunk1-Switch.pak',
         'ue-v8short-v9/RomFS/Game/Content/Paks/a-Switch.pak', 'ue-v8short-v9/RomFS/Game/Content/Paks/b-Switch.pak', 'ue-v8short-v9/RomFS/Game/Content/Paks/c-Switch.pak',
         'ue-v11/RomFS/Proj/Content/Paks/Proj-Switch.pak', 'ue-v11/RomFS/Proj/Content/Paks/Shaders-Switch.pak']

def text(e, path):
    s = str(e).removeprefix('[RED] ') if isinstance(e, SystemExit) else f'{type(e).__name__}: {e}'
    return s.replace(path, 'P')

def outcome(path):
    out = {}
    try:
        with open(path, 'rb') as f: enc, ioff, isz, methods, ver = uepak.footer(f)
        out['footer'] = {'ok': [bool(enc), ioff, isz, methods, ver]}
    except (Exception, SystemExit) as e:
        out['footer'] = {'err': text(e, path)}; return out
    try: out['sources'] = {'ok': list(uepak.pak_shader_sources(path))}
    except (Exception, SystemExit) as e: out['sources'] = {'err': text(e, path)}
    try: pk = uepak.Pak(path)
    except (Exception, SystemExit) as e: out['pak'] = {'err': text(e, path)}; return out
    out['pak'] = {'ok': [pk.mount, sorted(pk.files)]}
    files = []
    for name in sorted(pk.files):
        try: d = pk.read(name, decode=ss.oodle)
        except (Exception, SystemExit) as e: files.append([name, {'err': text(e, path)}]); continue
        found = []
        try:
            ss.ue_inline_shaders(d, '/x#' + name, found)
            sh = {'ok': [[w, st, hashlib.sha256(bytes(c) + bytes(k)).hexdigest()[:16]] for w, st, c, k in found]}
        except (Exception, SystemExit) as e: sh = {'err': text(e, path)}
        files.append([name, {'ok': hashlib.sha256(d).hexdigest()[:16], 'shaders': sh}])
    pk.f.close()
    out['files'] = files
    return out

rng = Rng(0x5eed)
recs = []
tmp = tempfile.mkdtemp() + '/P.pak'
for base in BASES:
    b = open(os.path.join(crate, 'tests/data/games', base), 'rb').read()
    for k in range(70):
        muts, cut = [], None
        kind = rng.below(5)
        for _ in range(1 + rng.below(3)):
            if kind == 0: o = len(b) - 1 - rng.below(min(len(b), 0x120))   # the footer
            elif kind == 1: o = rng.below(len(b))                           # anywhere
            elif kind == 2: o = rng.below(min(len(b), 0x200))                # the first entries' headers and data
            else: o = len(b) - 1 - rng.below(min(len(b), 0x1200))            # the index and the directory
            muts.append([o, rng.below(256)])
        if rng.below(8) == 0: cut = rng.below(len(b))
        d = bytearray(b)
        for o, v in muts: d[o] = v
        if cut is not None: d = d[:cut]
        open(tmp, 'wb').write(d)
        recs.append({'base': base, 'muts': muts, 'cut': cut, 'out': outcome(tmp)})
    open(tmp, 'wb').write(b)
    recs.append({'base': base, 'muts': [], 'cut': None, 'out': outcome(tmp)})
with gzip.open(os.path.join(crate, 'tests/data/vectors-uepak.jsonl.gz'), 'wt') as f:
    for r in recs: f.write(json.dumps(r, sort_keys=True) + '\n')
print(len(recs), 'vectors')
