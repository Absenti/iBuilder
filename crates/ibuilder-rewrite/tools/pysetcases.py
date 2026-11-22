"""Writes tests/fixtures/solver/pyset_cases.txt: random sequences of set operations on ints (add, |, -, |=, set()) and the
iteration order CPython gives after them; src/pyset.rs replays them and must give the same orders. Run with the reference
Python (3.14.1): usage: pysetcases.py [count] [out]. One case per line: the operations, then '=>' and the expected lines."""
import random, sys, os
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tests', 'fixtures', 'solver', 'pyset_cases.txt')
N = int(sys.argv[1]) if len(sys.argv) > 1 else 30
out = []
for seed in range(N):
    rng = random.Random(seed); names = 'ABCD'; T = {n: set() for n in names}; ops = []; want = []
    def val():
        r = rng.random()
        return rng.randrange(0, 64) if r < 0.4 else rng.randrange(0, 5000) if r < 0.7 else rng.randrange(0x10000, 0x8000000, 4)
    for _ in range(rng.randrange(5, 30)):
        r = rng.random(); a, b, c = rng.choice(names), rng.choice(names), rng.choice(names)
        if r < 0.6:
            for _ in range(rng.randrange(1, 16)):
                v = val(); ops.append(f'add {a} {v}'); T[a].add(v)
        elif r < 0.7 and a != b: ops.append(f'or {c} {a} {b}'); T[c] = T[a] | T[b]
        elif r < 0.8: ops.append(f'sub {c} {a} {b}'); T[c] = T[a] - T[b]
        elif r < 0.9 and a != b: ops.append(f'ior {a} {b}'); T[a] |= T[b]
        else: ops.append(f'copy {c} {a}'); T[c] = set(T[a])
    for n in names: want.append(n + ':' + ','.join(str(x) for x in T[n]))
    out.append(';'.join(ops) + '=>' + ' '.join(want))
open(OUT, 'w').write('\n'.join(out) + '\n')
