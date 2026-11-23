#!/usr/bin/env python3
"""Writes tests/data/vectors-smash-trinity.jsonl.gz: the containers of the synthetic games sync-smash (data.arc) and sync-trinity (data.trpfs, data.trpfd), written by
tools/gen_smash_trinity.py, damaged in many ways, each split end to end by the UPSTREAM engine's shader_split.py. A vector is the damage (a recipe, applied to the committed container
by tests/smash_trinity.rs the same way) and what the Python's split gave: its exit code, its last stderr line, the sha256 of the files it wrote, the count of files it could not read.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_smash_trinity_vectors.py <upstream checkout (the folder that holds dm/)> tests/data

Recipes: {"c": "arc" | "trpfs" | "trpfd", "t": "flip", "at": offset, "x": xor} or {"t": "cut", "n": length}; for data.arc also {"t": "tflip", "at", "x"} and {"t": "tcut", "n", "fix"}:
its file system table decoded, damaged, and written back as one zstd frame of raw blocks (rawframe below; the header's size is the table's new length when "fix", else the old one).
"""
import concurrent.futures as cf, gzip, hashlib, json, os, shutil, struct, subprocess, sys, tempfile
from compression import zstd

engine, data_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
GAME = '0100TESTTEST0000'
PORT_LINE2 = '# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied\n'
FILES = {'arc': ('sync-smash', 'data.arc'), 'trpfs': ('sync-trinity', 'arc/data.trpfs'), 'trpfd': ('sync-trinity', 'arc/data.trpfd')}


def sha(b): return hashlib.sha256(b).hexdigest()


def rawframe(t):
    """one zstd frame: single segment, a 4-byte content size, raw blocks of at most 128 KiB"""
    out = bytearray(bytes.fromhex('28b52ffd')) + bytes([0xa0]) + struct.pack('<I', len(t))
    blocks = [t[i:i + 0x20000] for i in range(0, len(t), 0x20000)] or [b'']
    for i, b in enumerate(blocks):
        h = (len(b) << 3) | (1 if i == len(blocks) - 1 else 0)
        out += struct.pack('<I', h)[:3] + b
    return bytes(out)


def apply(base, v):
    d = bytearray(base)
    if v['t'] == 'flip': d[v['at']] ^= v['x']; return bytes(d)
    if v['t'] == 'cut': return bytes(d[:v['n']])
    fs_off = struct.unpack_from('<Q', d, 0x20)[0]
    size, stored = struct.unpack_from('<II', d, fs_off + 4)
    t = bytearray(zstd.decompress(bytes(d[fs_off + 16:fs_off + 16 + stored])))
    if v['t'] == 'tflip': t[v['at']] ^= v['x']
    else:
        t = t[:v['n']]
        if v['fix']: size = len(t)
    f = rawframe(bytes(t))
    return bytes(d[:fs_off]) + struct.pack('<4I', 0x10, size, len(f), 16 + len(f)) + f


def run(v):
    game, rel = FILES[v['c']]
    src = f'{data_root}/games/{game}'
    g = tempfile.mkdtemp(); shutil.copytree(src + '/RomFS', g + '/RomFS'); shutil.copy(src + '/main', g + '/main'); shutil.copytree(src + '/profile', g + '/profile')
    base = open(f'{src}/RomFS/{rel}', 'rb').read()
    open(f'{g}/RomFS/{rel}', 'wb').write(apply(base, v))
    work = g + '/out'; os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1')
    env.pop('DM_OODLE_HOST', None); env.pop('DM_OODLE_MODULES', None)
    p = subprocess.run([sys.executable, '-I', engine + '/dm/gfx/tools/shader_split.py', g + '/RomFS', g + '/main', work + '/pkg'] + [f'{work}/tables/{t}' for t in tables],
                       env=env, capture_output=True, text=True)
    out = {'rc': p.returncode}
    if p.returncode:
        lines = p.stderr.strip().splitlines()
        out['err'] = (lines[-1] if lines else '').replace(g + '/RomFS', '@ROMFS@')
    else:
        files = {}
        for sub in ('pkg', 'tables'):
            for fn in sorted(os.listdir(f'{work}/{sub}')):
                b = open(f'{work}/{sub}/{fn}', 'rb').read()
                if fn == 'shader_map.tsv':
                    lines = b.decode().splitlines(keepends=True); lines[1] = PORT_LINE2; b = ''.join(lines).encode()
                files[f'{sub}/{fn}'] = sha(b)
        out['files'] = files
        bad = [l for l in p.stdout.splitlines() if l.startswith('[RED] ') and ' file(s) could not be read' in l]
        out['unreadable'] = int(bad[0].split()[1]) if bad else 0
        out['oodle'] = any('DM_OODLE_HOST' in l for l in p.stdout.splitlines())
    shutil.rmtree(g)
    return {'v': v, 'py': out}


def recipes():
    vs = []
    arc = open(f'{data_root}/games/sync-smash/RomFS/data.arc', 'rb').read()
    fs_off = struct.unpack_from('<Q', arc, 0x20)[0]
    t = zstd.decompress(arc[fs_off + 16:fs_off + 16 + struct.unpack_from('<I', arc, fs_off + 8)[0]])
    xs = (0x01, 0x10, 0x80, 0xff)
    for o in range(0x58): vs += [{'c': 'arc', 't': 'tflip', 'at': o, 'x': x} for x in (0x01, 0x80, 0xff)]
    for o in range(0x58, len(t)): vs.append({'c': 'arc', 't': 'tflip', 'at': o, 'x': xs[o % 4]})
    for n in list(range(0, len(t), max(1, len(t) // 40))) + [len(t) - 1]: vs += [{'c': 'arc', 't': 'tcut', 'n': n, 'fix': f} for f in (False, True)]
    for o in range(0x30): vs.append({'c': 'arc', 't': 'flip', 'at': o, 'x': 0xff if o % 2 else 0x01})
    for o in list(range(0x100, fs_off, 0x1f1)) + list(range(fs_off, fs_off + 16)): vs.append({'c': 'arc', 't': 'flip', 'at': o, 'x': xs[o % 4]})
    for n in list(range(0, len(arc), len(arc) // 30)) + [len(arc) - 1]: vs.append({'c': 'arc', 't': 'cut', 'n': n})
    for c in ('trpfs', 'trpfd'):
        d = open(f'{data_root}/games/{FILES[c][0]}/RomFS/{FILES[c][1]}', 'rb').read()
        step = 1 if c == 'trpfd' else 11
        for o in range(0, len(d), step): vs.append({'c': c, 't': 'flip', 'at': o, 'x': xs[(o // step) % 4]})
        for n in list(range(0, len(d), max(1, len(d) // 40))) + [len(d) - 1]: vs.append({'c': c, 't': 'cut', 'n': n})
    return vs


vs = recipes()
print(len(vs), 'vectors', flush=True)
with cf.ThreadPoolExecutor(8) as ex: got = list(ex.map(run, vs))
with gzip.open(f'{data_root}/vectors-smash-trinity.jsonl.gz', 'wt', compresslevel=9) as f:
    for r in got: f.write(json.dumps(r, sort_keys=True, separators=(',', ':')) + '\n')
import collections
print(collections.Counter((r['v']['c'], r['py']['rc'], r['py'].get('unreadable')) for r in got))
