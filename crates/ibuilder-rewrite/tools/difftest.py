"""Differential fuzz: synthetic modules built from real Grizzy code chunks (x18-ified) run through the Python oracle
(patch_nso.py, scratch copy) and the Rust CLI; outputs must be identical, or both must fail.
usage: difftest.py [jt | tfuzz | jsfuzz] <first seed> <count> [jobs]   (jt: tools/jtcases.py's hand-built tables; tfuzz: instruction
templates; jsfuzz: tools/jsfuzz.py's random dispatch code for the jump solver; default: chunks of Grizzy's sdk)"""
import os, random, struct, subprocess, sys, shutil, hashlib
from concurrent.futures import ThreadPoolExecutor
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from synth import build_nso

# the oracle and the Rust binary are the ones of the environment: IBR_PY (the reference venv's python), IBR_PATCH_NSO (the engine's patch_nso.py, run from its own folder),
# IBR_RUST (the ibuilder-rewrite binary), IBR_WORK (scratch), IBR_GRIZZY_SDK (a real module whose code chunks the fuzz modules are made of)
S = os.environ.get('IBR_SCRATCH') or os.environ.get('IBUILDER_SCRATCH_OLD') or sys.exit('set IBR_SCRATCH (or IBUILDER_SCRATCH_OLD): the scratch folder with ref/venv, ref/engine and the Rust build; see scripts/local.env.example')
PY = os.environ.get('IBR_PY', S + '/ref/venv/bin/python')
PATCH = os.environ.get('IBR_PATCH_NSO', S + '/ref/engine/dm/tools/patch_nso.py')
RUST = os.environ.get('IBR_RUST', S + '/target-rewrite/release/ibuilder-rewrite')
WORK = os.environ.get('IBR_WORK', S + '/rw-fuzz')
GRIZZY_SDK = os.environ.get('IBR_GRIZZY_SDK') or (os.environ.get('IOSBUILDER') or sys.exit('set IBR_GRIZZY_SDK or IOSBUILDER (the original iOSBuilder checkout); see scripts/local.env.example')) + '/out/ref-out-grizzy/ExeFS/sdk'
os.makedirs(WORK, exist_ok=True)

# real code to chunk: the oracle's own loader on Grizzy's sdk
sys.path.insert(0, os.path.dirname(PATCH))
import nso
_m = nso.read(GRIZZY_SDK)
TEXT = bytes(_m.image[:_m.text_end])
NW = len(TEXT) // 4

def words_at(i, n): return list(struct.unpack_from('<%dI' % n, TEXT, 4 * i))

def tmpl(rng):
    x = rng.randrange(0, 16) ; y = rng.choice([0, 1, 2, 3, 4, 9, 16, 17])
    c = [
        0xD53BD052, 0xD53BD072, 0xD51BD052, 0xD53BE032, 0xD53B0032, 0xD40003E1, 0xD53BE020 | y, 0xD53BD040 | y, 0xD53BD060 | y, 0xD51BD040 | y,
        0xC85F7C12, 0xC8017C12, 0x885F7C00 | (y << 0), 0x88017C12, 0x085FFC12, 0x08017C20,   # exclusive loads / stores (x18 forms and not)
        0x48207C00 | (1 << 22) | (1 << 15) | (18 << 16) | (16 << 5) | 4,                       # caspal x18, x19, x4, x5, [x16]
        0x48207C00 | (18 << 16) | (0 << 5) | 4, 0x48207C00 | (2 << 16) | (0 << 5) | 18,
        0x10000052, 0xB4000052, 0x36180052, 0xB5000052, 0x37180052, 0xD63F0240,
        0xCB2563FF, 0xD10263FF, 0x910263FF, 0xAA1203E0 | x, 0xAA0003F2 | (x << 16), 0xF9400240 | x, 0xF9000240 | x, 0xB9400240 | x, 0xAA0003E0 | (18 << 16),
        0x8B120000 | (x << 5) | y, 0x91000252, 0x91002240, 0xF8400E52, 0xF8000E52, 0xA9400A52, 0xA9000A52, 0x3940024A, 0x39000252, 0xB8727800 | (x << 5), 0x9B127C00,
        0x5400004C, 0x540000C1, 0x34000040 | y, 0xB5000040 | y, 0x14000004, 0xD65F03C0, 0xD503201F, 0xD4200000,
        0xC8DFFC00 | (x << 5) | y, 0x88DFFC00 | (x << 5) | y, 0xC89FFC00 | (x << 5) | y, 0x88007C00 | (x << 5) | y | (2 << 16),
    ]
    return rng.choice(c)

def make_case(seed):
    rng = random.Random(seed)
    funcs = []
    for _ in range(rng.randint(1, 6)):
        n = rng.randint(20, 300)
        i = rng.randrange(0x40, NW - n)
        w = words_at(i, n)
        rate = rng.choice([0.0, 0.01, 0.03, 0.08])
        for k in range(n):
            r = rng.random()
            if r < rate:
                w[k] = tmpl(rng)
            elif r < rate * 1.5:
                pos = rng.choice([0, 5, 10, 16]); nw = (w[k] & ~(31 << pos)) | (18 << pos)
                if not (nw & 0xFFFFFC1F in (0xD61F0000, 0xD65F0000) and (nw >> 5) & 31 == 18): w[k] = nw   # no br x18: its jump table would stop the whole module
        funcs.append(w)
    return funcs

def make_tcase(seed):
    """functions made only of instruction templates, nops and nearby branches (no game code)"""
    rng = random.Random(seed)
    funcs = []
    for _ in range(rng.randint(1, 5)):
        n = rng.randint(20, 120)
        w = []
        for k in range(n):
            r = rng.random()
            if r < 0.45: w.append(tmpl(rng))
            elif r < 0.60: w.append(NOP_)
            elif r < 0.75:   # b.cond / cbz / tbz / b to a nearby word
                d = rng.randint(-12, 12) or 3
                kind = rng.choice([0x54000000, 0x34000000, 0xB5000000, 0x36000000, 0x14000000])
                if kind == 0x14000000: w.append(0x14000000 | (d & 0x3FFFFFF))
                elif kind == 0x36000000: w.append(0x36000000 | ((d & 0x3FFF) << 5) | rng.choice([0, 1, 9, 18]))
                else: w.append(kind | ((d & 0x7FFFF) << 5) | rng.choice([0, 1, 9, 18, 16]))
            elif r < 0.85: w.append(rng.choice([0xD65F03C0, 0xD503201F, 0xA9BF7BFD, 0xA8C17BFD, 0x910003FD]))
            else: w.append(0xB9400000 | (rng.randrange(0, 31) << 5) | rng.randrange(0, 31) | (rng.randrange(0, 40) << 10))
        w.append(0xD65F03C0)
        funcs.append(w)
    return funcs
NOP_ = 0xD503201F

MODE = 'fuzz'
def run_case(seed):
    d = f'{WORK}/{MODE}{seed}'
    shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    if MODE == 'jt':
        import jtcases
        nsof, variant = jtcases.make(seed)
        open(f'{d}/variant', 'w').write(variant)
    elif MODE == 'jsfuzz':
        import jsfuzz
        nsof = jsfuzz.make(seed)
    elif MODE == 'tfuzz':
        nsof, _, _, _ = build_nso(make_tcase(seed), bss=int(os.environ.get('SYNTH_BSS', '4096'), 0))
    else:
        nsof, _, _, _ = build_nso(make_case(seed))
    open(f'{d}/in.nso', 'wb').write(nsof)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1')
    env.pop('DM_GAME', None)
    p = subprocess.run([PY, '-B', PATCH, f'{d}/in.nso', f'{d}/py', 'm'], capture_output=True, text=True, env=env, timeout=600, cwd=os.path.dirname(PATCH))
    r = subprocess.run([RUST, '--threads', '2', f'{d}/in.nso', f'{d}/rs', 'm'], capture_output=True, text=True, timeout=600)
    pyok, rsok = p.returncode == 0, r.returncode == 0
    if pyok and rsok:
        bad = []
        for f in sorted(os.listdir(f'{d}/py')):
            a, b = f'{d}/py/{f}', f'{d}/rs/{f}'
            if f == 'build_src.sha256': continue
            if not os.path.exists(b) or open(a, 'rb').read() != open(b, 'rb').read(): bad.append(f)
        if p.stdout != r.stdout: bad.append('stdout')
        res = ('SAME' if not bad else 'DIFF ' + ','.join(bad), '')
    elif not pyok and not rsok:
        pyerr = (p.stderr.strip().splitlines() or ['?'])[-1]
        res = ('BOTHFAIL', f'py: {pyerr[:110]} | rs: {r.stderr.strip()[:110]}')
    else:
        res = ('MISMATCH py=%s rs=%s' % (pyok, rsok), (p.stderr.strip().splitlines() or ['?'])[-1][:120] + ' | ' + r.stderr.strip()[:120])
    if res[0] in ('SAME',) or res[0] == 'BOTHFAIL':
        shutil.rmtree(d, ignore_errors=True)
    return seed, res

if __name__ == '__main__':
    if sys.argv[1] in ('jt', 'tfuzz', 'jsfuzz'): MODE = sys.argv[1]; sys.argv.pop(1)
    first, count = int(sys.argv[1]), int(sys.argv[2]); jobs = int(sys.argv[3]) if len(sys.argv) > 3 else 4
    tally = {}
    with ThreadPoolExecutor(jobs) as ex:
        for seed, (kind, note) in ex.map(run_case, range(first, first + count)):
            tally[kind.split(' ')[0]] = tally.get(kind.split(' ')[0], 0) + 1
            if kind not in ('SAME',):
                print(seed, kind, note, flush=True)
    print('TALLY', tally)
