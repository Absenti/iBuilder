#!/usr/bin/env python3
"""Writes tests/data/games/sync-registered/: the registered shader map of nxnative c9b12271 + f41613f7 (`DM_SPLIT_REGISTERED_MAP`, read_registered_sources, uniq_keys and merge_keys
keeping the source the map names when the same shader is found in several places). Every byte is made here (the builders of gen_sync_readers.py); the UPSTREAM engine's own
`shader_split.py` splits the game with the map in its environment and a DLC folder, and `expected.json` (tests/games.rs's format, its "registered" field naming the map) has the sha256 of
what it wrote and its log.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_registered.py <upstream checkout (the folder that holds dm/)> tests/data/games

Cases: a key in two RomFS files whose map row names the later one (taken); a key in main twice, the map naming the second copy (taken inside main's uniq_keys); a key in the game and in
the DLC whose row names the DLC copy (the game keeps it: the add-on side never wins); a row with another stage (ignored); a row naming a source where the key is not (ignored); a key
only in the DLC whose row names it.
shader_map.tsv line 2: as gen_sync_readers.py, the hash is that of the Python's table with line 2 put back to the port's text.
"""
import json, os, shutil, struct, subprocess, sys, tempfile
here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, 'gen_sync_readers.py')).read()
exec(src[:src.index('# ---- NXSD')])
engine, out_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
S = lambda st, seed: program(st, seed)


def main_text(extra):
    r = Rng(9).bytes(0x1000), Rng(10).bytes(0x200)
    return clean(r[0]) + extra + clean(r[1])


def run(name, files, dlc, main_extra, rows):
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for root, fs in [(g + '/RomFS', files)] + [(f'{g}/dlc/{d}', f) for d, f in dlc.items()]:
        for p, b in fs.items():
            os.makedirs(os.path.dirname(f'{root}/{p}'), exist_ok=True); open(f'{root}/{p}', 'wb').write(b)
    open(g + '/main', 'wb').write(nso(main_text(main_extra)))
    os.makedirs(f'{g}/profile/{GAME}'); open(f'{g}/profile/{GAME}/game.tsv', 'w').write(GAME_TSV)
    open(g + '/registered_map.tsv', 'w').write('# kind: evidence  registered\npackage\tindex\tstage\tdm_ctl_sha16\tsource\n' + ''.join(f'x.bin\t{i}\t{st}\t{k}\t{w}\n' for i, (st, k, w) in enumerate(rows)) + 'a short row\n')
    work = tempfile.mkdtemp(); os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1', DM_SPLIT_REGISTERED_MAP=g + '/registered_map.tsv')
    p = subprocess.run([sys.executable, '-I', engine + '/dm/gfx/tools/shader_split.py', g + '/RomFS', g + '/main', work + '/pkg'] + [f'{work}/tables/{t}' for t in tables]
                       + [','.join(f'{g}/dlc/{d}' for d in dlc)], env=env, capture_output=True, text=True)
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
    chosen = [l.rstrip('\n').split('\t')[3:] for l in open(f'{work}/tables/shader_map.tsv') if not l.startswith(('#', 'package\t'))]
    json.dump({'shaders': n, 'files': expected, 'dlc': sorted(dlc), 'updates': [], 'registered': 'registered_map.tsv', 'log': log}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    shutil.rmtree(work)
    print(name, n, 'shaders', len(expected), 'files;', *log, *('  ' + '\t'.join(c) for c in chosen), sep='\n  ')


sys.path.insert(0, engine + '/dm/gfx/tools'); sys.path.insert(0, engine + '/dm/tools')
os.environ.update(DM_GAMES_DIR=tempfile.mkdtemp(), DM_GAME=GAME)
os.makedirs(os.environ['DM_GAMES_DIR'] + '/' + GAME); open(os.environ['DM_GAMES_DIR'] + f'/{GAME}/game.tsv', 'w').write(GAME_TSV)
import shader_split as ss
key = lambda prog: ss.skey(*prog)
A, B, C, D, E, F = S('vs', 101), S('ps', 102), S('vs', 103), S('ps', 104), S('cs', 105), S('vs', 106)
one = lambda prog: glslc([prog])
pad = bytes(0x40)
mainx = one(F) + pad + one(F)
second = 0x1000 + len(one(F)) + len(pad)
files = {
    'Data/a1.glslc': one(A), 'Pack/a2.glslc': one(A),       # A: the map names the second file
    'Data/c.glslc': one(C),                                  # C: also in the DLC, the map names the DLC's copy: the game keeps it
    'Data/e1.glslc': one(E), 'Pack/e2.glslc': one(E),        # E: the map's row has another stage: the first copy stays
    'Data/b1.glslc': one(B), 'Data/b2.glslc': one(B),        # B: the map names a source where B is not: the first copy stays
}
dlc = {'1': {'Data/c.glslc': one(C), 'Data/d.glslc': one(D), 'Pack/b3.glslc': one(B)}}
rows = [('vertex', key(A), '/Pack/a2.glslc#GLSLC@0x0'), ('vertex', key(C), '/aoc/1/Data/c.glslc#GLSLC@0x0'), ('vertex', key(E), '/Pack/e2.glslc#GLSLC@0x0'),
        ('pixel', key(B), '/Data/b9.glslc#GLSLC@0x0'), ('vertex', key(F), f'main#GLSLC@{second:#x}'), ('pixel', key(D), '/aoc/1/Data/d.glslc#GLSLC@0x0')]
run('sync-registered', files, dlc, mainx, rows)
