"""Writes the small synthetic NSO fixtures (LZ4-compressed segments) of crates/ibuilder-rewrite/tests/fixtures/synth/ and their manifest. No game code goes into them:
jump-table cases are hand-built (tools/jtcases.py), the others are random instruction-template functions the shipped Python rewrites, and two registered per-game stub builds.

The manifest is written by tools/synth_manifest.py (write(), the one writer of its format: the shipped engine's hashes, or its message where it stops, and the
`#! shipped-engine` header), so this needs the same environment as that tool (IBR_PY, IBR_PATCH_NSO, IBR_ENGINE_STAMP, IBR_WORK; see its docstring) plus what
tools/difftest.py reads (IBR_SCRATCH, IBR_GRIZZY_SDK or IOSBUILDER).

usage: export_fixtures.py [out dir]      (default: the repository's tests/fixtures/synth; give a scratch folder to check that a regeneration reproduces the fixtures)"""
import os, shutil, struct, subprocess, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import synth_manifest
PY, PATCH, _, WORK = synth_manifest.config()
import difftest, jtcases, stubtest_lib as st
import lz4.block
OUT = sys.argv[1] if len(sys.argv) > 1 else synth_manifest.FX
os.makedirs(OUT, exist_ok=True)

def compress(nso):
    h = bytearray(nso[:0x100])
    segs = []
    for i in range(3):
        fo, mo, sz = struct.unpack_from('<III', h, 0x10 + 0x10 * i)
        segs.append((mo, sz, nso[fo:fo + sz]))
    out = bytearray(0x100); body = b''
    flags = 0
    for i, (mo, sz, raw) in enumerate(segs):
        c = lz4.block.compress(raw, store_size=False)
        fo = 0x100 + len(body); body += c
        struct.pack_into('<III', h, 0x10 + 0x10 * i, fo, mo, sz)
        struct.pack_into('<I', h, 0x60 + 4 * i, len(c))
        flags |= 1 << i
    struct.pack_into('<I', h, 0xc, flags)
    return bytes(h) + body

def rewrites(nso, name):
    """whether the shipped engine's patch_nso.py (IBR_PATCH_NSO) rewrites the module (the template cases keep only those)"""
    d = f'{WORK}/fx'; shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    open(f'{d}/in.nso', 'wb').write(nso)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1'); env.pop('DM_GAME', None); env.pop('DM_GAMES_DIR', None)
    return subprocess.run([PY, PATCH, f'{d}/in.nso', f'{d}/py', name], capture_output=True, text=True, env=env, cwd=WORK).returncode == 0

cases = []
seen = set()
for seed in range(60):
    nso, variant = jtcases.make(seed)
    if variant in seen: continue
    seen.add(variant); cases.append((f'jt_{variant}', nso, 'm'))
n = 0
for seed in range(400):
    if n >= 10: break
    nso = difftest.build_nso(difftest.make_tcase(seed))[0]
    if rewrites(nso, 'm'): cases.append((f'tmpl_{seed}', nso, 'm')); n += 1
cases += [('stub_ori', st.ori(), 'main'), ('stub_ttyd', st.ttyd(), 'main')]
for name, nso, mod in cases:
    c = compress(nso)
    open(f'{OUT}/{name}.nso', 'wb').write(c)
    print(name, len(c), 'bytes')
synth_manifest.write([(name, mod) for name, _, mod in cases], OUT)
