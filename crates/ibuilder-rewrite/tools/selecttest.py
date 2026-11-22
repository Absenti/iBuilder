#!/usr/bin/env python3
"""Differential fuzz of the stack-free conditional-select rewrite (engine patch 0409: `select_x18`, `select_scratch`, `site_select` of `patch_nso.py`; src/select.rs).

Synthetic modules (stubsynth.py: proper unwind entries, which the Python's `function_ends` needs) whose functions are random sequences of CSEL / CSINC / CSINV / CSNEG with x18 as
Rn / Rm / both / neither, as Rd, with wzr / xzr, in 32- and 64-bit forms, every condition, followed by the instructions the liveness proof reads: writes of x9..x17 (mov, add, ldr
through itself, orr), reads (str, cmp), branches both ways, calls, `ret`, unknown instructions, and enough instructions to exhaust the search budget. Each is run through the
real `patch_nso.py` (the engine tools with 0409 applied) and the Rust CLI; every output file and what was printed must be identical, or both must stop with the same message.

usage: selecttest.py --tools <engine dm/tools with 0409> --python <venv python> --rust <ibuilder-rewrite> [--first N] [--count N] [--jobs N] [--work DIR] [--export DIR --export-count N]
--export keeps the first N modules that took the new path at least once (and the Python's SHA-256 of every file) in DIR as the fixtures of tests/select.rs.
"""
import argparse, hashlib, os, random, re, shutil, struct, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stubsynth

TEMPS = list(range(9, 18))
CONDS = list(range(16))


def csel_family(rng):
    """one conditional select with x18 in some role (or none)"""
    sf = rng.randrange(2)
    base = rng.choice([0x1A800000, 0x1A800000, 0x1A800000, 0x1A800400, 0x5A800000, 0x5A800400])   # csel (most), csinc, csinv, csneg
    role = rng.choice(['rn', 'rn', 'rm', 'rm', 'both', 'none', 'rd'])
    rd = rng.choice(TEMPS + [0, 1, 8, 19, 28, 29, 30, 31])
    rn, rm = rng.choice(TEMPS + [0, 8, 31]), rng.choice(TEMPS + [0, 8, 31])
    if role == 'rn': rn = 18
    elif role == 'rm': rm = 18
    elif role == 'both': rn = rm = 18
    elif role == 'rd': rd = 18
    cond = rng.choice(CONDS)
    return base | sf << 31 | rm << 16 | cond << 12 | rn << 5 | rd


def filler(rng, pos, n):
    """one instruction of the liveness proof's vocabulary; branches stay near (and sometimes leave the function)"""
    t = rng.choice(TEMPS)
    u = rng.choice(TEMPS + [0, 1, 31])
    k = rng.randrange(24)
    if k == 0: return 0xD2800000 | rng.randrange(0x100) << 5 | t                        # mov xT,#imm
    if k == 1: return 0x52800000 | rng.randrange(0x100) << 5 | t                        # mov wT,#imm
    if k == 2: return 0x91000400 | u << 5 | t                                           # add xT,xU,#1
    if k == 3: return 0xF9400000 | t << 5 | t                                           # ldr xT,[xT]   (reads itself)
    if k == 4: return 0xF9400000 | u << 5 | t                                           # ldr xT,[xU]
    if k == 5: return 0xAA0003E0 | u << 16 | t                                          # mov xT,xU
    if k == 6: return 0xF90003E0 | t                                                    # str xT,[sp]   (a read)
    if k == 7: return 0xF100001F | 1 << 10 | t << 5                                     # cmp xT,#1
    if k == 8: return 0xD503201F                                                        # nop
    if k == 9: return 0xD65F03C0                                                        # ret
    if k == 10: return 0x94000000 | (rng.randrange(-8, 8) & 0x3FFFFFF)                  # bl (nearby): no proof
    if k == 11: return 0xD63F0000 | t << 5                                              # blr xT
    if k == 12: return 0x14000000 | (rng.randrange(-6, 12) & 0x3FFFFFF)                  # b
    if k == 13: return 0x54000000 | (rng.randrange(-6, 12) & 0x7FFFF) << 5 | rng.randrange(14)   # b.cond
    if k == 14: return 0xB4000000 | (rng.randrange(-6, 12) & 0x7FFFF) << 5 | t          # cbz xT
    if k == 15: return 0x35000000 | (rng.randrange(-6, 12) & 0x7FFFF) << 5 | t          # cbnz wT
    if k == 16: return 0x36000000 | rng.randrange(32) << 19 | (rng.randrange(-6, 12) & 0x3FFF) << 5 | t   # tbz
    if k == 17: return 0xAA000000 | u << 16 | 0x3E0 | t                                 # orr xT,xzr,xU
    if k == 18: return 0xB9400000 | u << 5 | t                                          # ldr wT,[xU]
    if k == 19: return 0xF8400400 | u << 5 | t                                          # ldr xT,[xU],#0 (post-index writeback)
    if k == 20: return 0xA9400000 | u << 5 | t | 1 << 10                                # ldp xT,x1,[xU] (a writer the proof does not know)
    if k == 21: return 0x8B000000 | u << 16 | t << 5 | rng.choice(TEMPS)                # add xD,xT,xU
    if k == 22: return 0x91000400 | 18 << 5 | rng.choice([0, 1, 8])                     # add xN,x18,#1  (an ordinary x18 site)
    return 0xD2800000 | t                                                                # mov xT,#0


def make(seed):
    rng = random.Random(seed)
    funcs, pokes, pos = [], {}, 0x100
    for _ in range(rng.randint(1, 4)):
        n = rng.choice([rng.randint(8, 40), rng.randint(8, 40), rng.randint(70, 140)])   # long ones exhaust the search budget
        words = []
        for i in range(n - 1):
            words.append(csel_family(rng) if rng.random() < 0.18 else filler(rng, i, n))
        words.append(0xD65F03C0 if rng.random() < 0.85 else filler(rng, n - 1, n))
        for i, w in enumerate(words): pokes[pos + 4 * i] = w
        funcs.append((pos, 4 * n))
        pos += 4 * n + 0x40 * rng.randrange(0, 2)
    return stubsynth.build(funcs, pokes, (pos + 0x2000) & ~0xfff, bytes(range(1, 17)), bss=0x1000)[0]


def run(args, seed, work):
    d = f'{work}/s{seed}'
    shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    open(f'{d}/in.nso', 'wb').write(make(seed))
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1'); env.pop('DM_GAME', None); env.pop('DM_GAMES_DIR', None)
    p = subprocess.run([args.python, '-B', 'patch_nso.py', f'{d}/in.nso', f'{d}/py', 'm'], capture_output=True, text=True, env=env, cwd=args.tools, timeout=900)
    r = subprocess.run([args.rust, '--threads', '1', f'{d}/in.nso', f'{d}/rs', 'm'], capture_output=True, text=True, timeout=900)
    selects = 0
    m = re.search(r"'select': (\d+)", p.stdout)
    if m: selects = int(m.group(1))
    stats = re.search(r"\{.*\}", p.stdout)
    pyerr = (p.stderr.strip().splitlines() or [''])[-1]
    if p.returncode and r.returncode:
        res = 'BOTHFAIL' if pyerr.startswith(('AssertionError', 'RuntimeError', 'KeyError')) or pyerr in r.stderr else f'BOTHFAIL-DIFFMSG py={pyerr[:80]!r} rs={r.stderr.strip()[:80]!r}'
    elif p.returncode or r.returncode:
        res = f'MISMATCH py={p.returncode} rs={r.returncode} {pyerr[:100]!r} | {r.stderr.strip()[:100]!r}'
    else:
        bad = [f for f in sorted(os.listdir(f'{d}/py')) if f != 'build_src.sha256' and open(f'{d}/py/{f}', 'rb').read() != open(f'{d}/rs/{f}', 'rb').read()]
        if p.stdout != r.stdout: bad.append('stdout')
        if set(os.listdir(f'{d}/py')) - {'build_src.sha256'} != set(os.listdir(f'{d}/rs')) - {'build_src.sha256'}: bad.append('file set')
        res = 'SAME' if not bad else 'DIFF ' + ','.join(bad)
        if res == 'SAME' and selects:
            hashes = {'stdout': hashlib.sha256(p.stdout.encode()).hexdigest()}
            for f in os.listdir(f'{d}/py'):
                if f != 'build_src.sha256': hashes[f.split('.', 1)[1]] = hashlib.sha256(open(f'{d}/py/{f}', 'rb').read()).hexdigest()
            return seed, res, selects, hashes
    if res in ('SAME', 'BOTHFAIL'): shutil.rmtree(d, ignore_errors=True)
    return seed, res, selects, None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--tools', required=True); ap.add_argument('--python', required=True); ap.add_argument('--rust', required=True)
    ap.add_argument('--first', type=int, default=0); ap.add_argument('--count', type=int, default=200); ap.add_argument('--jobs', type=int, default=4)
    ap.add_argument('--work', required=True, help='scratch folder (copies of the engine tools are made in it)'); ap.add_argument('--export'); ap.add_argument('--export-count', type=int, default=24)
    args = ap.parse_args()
    os.makedirs(args.work, exist_ok=True)
    tally, total_selects, with_selects = {}, 0, 0
    kept = []
    with ThreadPoolExecutor(args.jobs) as ex:
        for seed, res, sel, hashes in ex.map(lambda s: run(args, s, args.work), range(args.first, args.first + args.count)):
            if args.export and hashes and len(kept) < args.export_count: kept.append((seed, hashes))
            key = res.split(' ')[0]
            tally[key] = tally.get(key, 0) + 1
            total_selects += sel; with_selects += sel > 0
            if res not in ('SAME', 'BOTHFAIL'): print(seed, res, flush=True)
    if args.export:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        import stubtest
        os.makedirs(args.export, exist_ok=True)
        with open(f'{args.export}/manifest.tsv', 'w') as m:
            m.write('# case\tmodule\tfile\tsha256 of what the real patch_nso.py (engine patch 0409) wrote for it (stdout = what it printed)\n')
            for seed, hashes in kept:
                open(f'{args.export}/sel_{seed}.nso', 'wb').write(stubtest.compress(make(seed)))
                for f, h in sorted(hashes.items()): m.write(f'sel_{seed}\tm\t{f}\t{h}\n')
    print('TALLY', tally, f'{total_selects} stack-free select sites in {with_selects} modules')
    sys.exit(1 if any(k not in ('SAME', 'BOTHFAIL') for k in tally) else 0)


if __name__ == '__main__':
    main()
