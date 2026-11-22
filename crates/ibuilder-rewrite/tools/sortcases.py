"""Writes tests/fixtures/solver/sort_cases.txt: random lists of the jump solver's root tuples and what CPython's sorted() of
[(root, constraint)] does with them (jump_solver.py's State.key): 'ok', or the TypeError's message. The Rust replays the sort
(jump_solver::py_sort_check) and must agree. Run with the reference Python (3.14.1): usage: sortcases.py [count] [out]"""
import random, sys, os
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tests', 'fixtures', 'solver', 'sort_cases.txt')
N = int(sys.argv[1]) if len(sys.argv) > 1 else 400
rng = random.Random(1)
def chain():
    ops = []
    for _ in range(rng.randint(0, 2)):
        k = rng.choice(['w', 'and', 'not', 'add'])
        ops.append(('w',) if k == 'w' else ('and', rng.choice([7, 15, 255])) if k == 'and' else ('not', rng.choice([32, 64])) if k == 'not' else ('add', rng.choice([-1, 1, 2]), rng.choice([32, 64])))
    return tuple(ops)
def root(depth=0):
    k = rng.choice(['seed', 'z', 'd', 'e', 'k', 'p', 'xd', 'xj', 'r'] if depth < 2 else ['seed', 'd', 'e', 'k', 'p', 'xj'])
    a, b = rng.randint(0, 3), rng.randint(0, 3)
    if k == 'seed': return ('seed', a, b, 64)
    if k == 'z': return ('z',)
    if k in ('d', 'e'): return (k, a, b, rng.choice([32, 64]))
    if k == 'k': return ('k', a, b, 64)
    if k == 'p': return ('p', rng.choice([-1, a]), b, rng.choice([32, 64]))
    if k == 'xd': return ('x', root(depth + 1), b, 64)
    if k == 'xj': return ('x', a, b, rng.randint(0, 3), 64)
    return ('r', root(depth + 1), chain(), 64)
lines = []
for _ in range(N):
    n = rng.randint(2, 12); rs = []
    while len(rs) < n:
        r = root()
        if r not in rs: rs.append(r)
    try: sorted([(r, (0,)) for r in rs]); want = 'ok'
    except TypeError as e: want = 'TypeError: ' + str(e)
    lines.append(want + '\t' + ';'.join(repr(r) for r in rs))
open(OUT, 'w').write('\n'.join(lines) + '\n')
print(sum(1 for l in lines if not l.startswith('ok')), 'of', len(lines), 'raise')
