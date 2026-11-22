#!/usr/bin/env python3
"""Dev-time generator of tests/fixtures/layout/: the Python oracle of `src/layout.rs` (nxnative's dm/tools/mod_layout.py, 17454fc3 + 65dfeb33).

Writes small synthetic modules (no game code: the images of upstream's dm/tools/tests/test_mod_layout.py and variants) and what mod_layout.relayout_module() makes of them:
  base.<name>.bin        a module file (an NRO, or an uncompressed NSO)
  manifest.tsv           case, base, pokes (file offset=u32 word, comma-separated; - for none), then D, the segments, the SHA-256 of the image and the notes,
                         or `error` and mod_layout.Error's message, or `crash` and the exception the Python stops with (a module it cannot read: the Rust returns an error)
The fuzzed cases poke words into an NRO (instructions of the function region, relocation addends, symbol values, dynamic tags) from a fixed seed, so the inputs stay small.

usage: layoutcases.py --tools <nxnative dm/tools at 65dfeb33 or later, with tests/test_mod_layout.py> --out crates/ibuilder-rewrite/tests/fixtures/layout [--fuzz N]
                      [--rewrite-oracle <dm/tools of the pinned engine + engine-patches/0409 + mod_layout.py and its two patch_nso.py hunks>]
       (a Python with Capstone 5: mod_layout reads the text's data blocks with it)
  --rewrite-oracle also runs that patch_nso.py (no DM_GAME, DM_REWRITE_J=1, module name nro0) on a subset of the cases and writes rewrite.tsv: the SHA-256 of every
  file it wrote (not build_src.sha256) and of its stdout, or the [RED] line it stopped with: the whole rewrite of a moved module, for tests/layout.rs.
"""
import argparse, hashlib, os, random, struct, sys

ap = argparse.ArgumentParser()
ap.add_argument('--tools', required=True); ap.add_argument('--out', required=True); ap.add_argument('--fuzz', type=int, default=120); ap.add_argument('--rewrite-oracle')
a = ap.parse_args()
sys.path.insert(0, a.tools); sys.path.insert(0, os.path.join(a.tools, 'tests'))
import mod_layout, nso   # noqa: E402
import test_mod_layout as T   # noqa: E402
from nso_asm import ADRP   # noqa: E402


def eh_variant():
    """test_eh_frame_empty_aug_lsda_lpstart_and_omitted_table's image"""
    d = bytearray(T.math_nro())
    EHH, FN = T.EHH, T.FN
    eh_frame, lsda, cfi_magic = 0x2700, 0x2800, 0xCAFEBABE
    d[EHH:EHH + 4] = bytes((1, 0x1b, 0x0b, 0xff))
    struct.pack_into('<ii', d, EHH + 4, eh_frame - (EHH + 4), 2)
    cie_body = bytearray(b'\x01zPLR\0\x04\x78\x1e')
    aug_data = bytearray([0x1b]) + struct.pack('<i', FN - (eh_frame + 8 + len(cie_body) + 2)) + bytes([0x1b, 0x1b])
    cie_body += bytes([len(aug_data)]) + aug_data
    while (len(cie_body) + 4) % 4: cie_body.append(0)
    struct.pack_into('<II', d, eh_frame, len(cie_body) + 4, 0)
    d[eh_frame + 8:eh_frame + 8 + len(cie_body)] = cie_body
    fde1 = eh_frame + 8 + len(cie_body)
    struct.pack_into('<IIiIBI', d, fde1, 20, fde1 + 4 - eh_frame, FN - (fde1 + 8), 12, 0, cfi_magic)
    fde2 = fde1 + 24
    struct.pack_into('<IIiIBi', d, fde2, 20, fde2 + 4 - eh_frame, FN + 12 - (fde2 + 8), 12, 4, lsda - (fde2 + 17))
    struct.pack_into('<I', d, fde2 + 24, 0)
    d[lsda] = 0x1b
    struct.pack_into('<i', d, lsda + 1, FN - (lsda + 1))
    return bytes(d)


def fitting():
    ok = bytearray(0x5000); ok[0x10:0x14] = b'NRO0'
    struct.pack_into('<II', ok, 0x18, 0x5000, 0)
    struct.pack_into('<6I', ok, 0x20, 0, 0x2000, 0x2000, 0x2000, 0x4000, 0x1000)
    struct.pack_into('<I', ok, 4, 0x2040)
    return bytes(ok)


bases = {'math': bytes(T.math_nro()), 'small': T.small_nso()[0], 'eh': eh_variant(), 'fitting': fitting()}


def run(raw):
    try:
        mod = nso.load(raw, 'case')
        out, D, notes = mod_layout.relayout_module(mod)
    except mod_layout.Error as e:
        return ['error', str(e)]
    except (struct.error, KeyError, IndexError, ValueError) as e:   # the Python stops with a traceback (patch_nso.py: no [RED] line); the Rust with an error
        return ['crash', type(e).__name__]
    seg = ' '.join(f'{o:#x}+{s:#x}' for o, s in out.segments)
    return [f'{D:#x}', seg, hashlib.sha256(bytes(out.image)).hexdigest(), '; '.join(notes) or '-']


def poke(raw, pokes):
    b = bytearray(raw)
    for off, w in pokes: struct.pack_into('<I', b, off, w)
    return bytes(b)


rows = []
for name in bases: rows.append((name, name, []))
# hand-made errors and edge cases on Math.nro
M = bases['math']
rows += [
    ('math-dt-rel', 'math', [(T.DYN + 16 * 6, 17), (T.DYN + 16 * 6 + 4, 0)]),                      # a DT_REL tag in place of DT_INIT
    ('math-reloc-type', 'math', [(T.RELA + 8, 1028)]),                                               # an unknown relocation type
    ('math-no-mod0', 'math', [(4, 0x1044)]),
    ('math-abs64-symless', 'math', [(T.RELA + 8, 257), (T.RELA + 16, 0x2400)]),                       # a symbol-less ABS64 whose addend moves
    ('math-abs64-sym', 'math', [(T.RELA + 8, 257), (T.RELA + 12, 1), (T.RELA + 16, 0x2400)]),         # with a symbol: the addend stays
    ('math-shndx-abs', 'math', [(T.SYMTAB + 24 + 4, 0xfff10011)]),                                    # SHN_ABS symbol: stays
    ('math-adr-far', 'math', [(T.FN + 8, 0x707fffe2)]),                                              # adr x2,.+0xfffff: out of range once moved
    ('math-ldr-far', 'math', [(T.FN + 12, 0x587fffe3)]),                                              # a literal load at the edge of its range
    ('math-prfm', 'math', [(T.FN + 12, 0xd8010000 | 3)]),                                             # prfm (literal) into data
    ('math-ldrsw', 'math', [(T.FN + 12, 0x98010003)]),                                                # ldrsw (literal) into data
    ('math-ldr-q', 'math', [(T.FN + 16, 0x9c000004 | ((0x20 - 16) >> 2) << 5)]),                      # ldr q4 (literal) to the pool: 16 bytes of literal words
]
rnd = random.Random(20261123)
FN, SIZE = T.FN, T.SIZE
def rand_target(): return rnd.choice([rnd.randrange(0, 0x1000), rnd.randrange(0x1000, 0x2000), rnd.randrange(0x2000, SIZE), rnd.randrange(SIZE, SIZE + 0x3000)]) & ~3
for k in range(a.fuzz):
    base = rnd.choice(['math', 'math', 'eh'])
    p = []
    for i in range(rnd.randrange(1, 9)):
        pc = FN + 4 * rnd.randrange(0, 10)
        kind = rnd.randrange(6)
        t = rand_target()
        if kind == 0: w = ADRP(rnd.randrange(31), pc, t)
        elif kind == 1: w = T.adr(rnd.randrange(31), pc, t) if -(1 << 20) <= t - pc < (1 << 20) else 0xd503201f
        elif kind == 2: w = T.ldr_lit(rnd.randrange(31), pc, t)
        elif kind == 3: w = (rnd.choice([0x18000000, 0x58000000, 0x98000000, 0xd8000000, 0x1c000000, 0x5c000000, 0x9c000000]) | (((t - pc) >> 2) & 0x7FFFF) << 5 | rnd.randrange(31))
        elif kind == 4: w = rnd.getrandbits(32)
        else: w = T.RET
        p.append((pc, w & 0xFFFFFFFF))
    if rnd.random() < 0.4: p.append((T.RELA + 16 + 24 * rnd.randrange(2), rand_target()))
    if rnd.random() < 0.3: p.append((T.SYMTAB + 24 * rnd.randrange(1, 4) + 8, rand_target()))
    if rnd.random() < 0.2: p.append((T.DYN + 16 * 6 + 8, rand_target()))   # DT_INIT's address
    if rnd.random() < 0.2: p.append((T.MOD0 + 4 + 4 * rnd.randrange(6), (rand_target() - T.MOD0) & 0xFFFFFFFF))
    rows.append((f'fuzz{k:03d}', base, p))

os.makedirs(a.out, exist_ok=True)
for name, raw in bases.items():
    open(os.path.join(a.out, f'base.{name}.bin'), 'wb').write(raw)
with open(os.path.join(a.out, 'manifest.tsv'), 'w', encoding='utf-8') as f:
    f.write('# case\tbase\tpokes (file offset=word)\tD or error\tsegments or message\timage sha256\tnotes  (generated by tools/layoutcases.py from nxnative mod_layout.py; do not edit)\n')
    for name, base, p in rows:
        res = run(poke(bases[base], p))
        pk = ','.join(f'{o:#x}={w:#010x}' for o, w in p) or '-'
        f.write('\t'.join([name, base, pk] + res) + '\n')
res = [r.split('\t')[3] for r in open(os.path.join(a.out, 'manifest.tsv')) if not r.startswith('#')]
print(f'{len(rows)} cases: {res.count("error")} errors, {res.count("crash")} crashes, {sum(1 for r in res if r == "0x0")} unchanged, {sum(1 for r in res if r.startswith("0x") and r != "0x0")} moved')

if a.rewrite_oracle:
    import subprocess, tempfile
    # every case until 30 rewrites have finished, and every [RED] stop; of the cases the Python stops on with a traceback (a later stage, e.g. the eh variant's
    # synthetic LSDA: KeyError) only the first two, where the Rust must stop too
    done = crashed = 0
    with open(os.path.join(a.out, 'rewrite.tsv'), 'w', encoding='utf-8') as f, tempfile.TemporaryDirectory() as tmp:
        f.write('# case\tfile (stdout = what it printed; error = the [RED] line it stopped with)\tSHA-256 or the line  (tools/layoutcases.py --rewrite-oracle: patch_nso.py of the pinned engine + 0409 + mod_layout)\n')
        for name, base, p in rows:
            if done >= 30 and not name.startswith('math'): break
            mod = os.path.join(tmp, name + '.bin'); out = os.path.join(tmp, name)
            open(mod, 'wb').write(poke(bases[base], p))
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1'); env.pop('DM_GAME', None); env.pop('DM_GAMES_DIR', None)
            r = subprocess.run([sys.executable, '-B', 'patch_nso.py', mod, out, 'nro0'], capture_output=True, text=True, env=env, cwd=a.rewrite_oracle)
            if r.returncode:
                line = (r.stderr.strip().splitlines() or [""])[-1]
                if not line.startswith('[RED]'):
                    crashed += 1
                    if crashed > 2: continue
                f.write(f'{name}\terror\t{line}\n'); continue
            done += 1
            f.write(f'{name}\tstdout\t{hashlib.sha256(r.stdout.encode()).hexdigest()}\n')
            for fn in sorted(os.listdir(out)):
                if fn.endswith('build_src.sha256'): continue
                f.write(f'{name}\t{fn.split(".", 1)[1]}\t{hashlib.sha256(open(os.path.join(out, fn), "rb").read()).hexdigest()}\n')
    print('rewrite.tsv written')
