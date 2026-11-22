import hashlib, json, random, struct, sys
import lz4.block
SEED = int(sys.argv[1]); N = int(sys.argv[2]); rng = random.Random(SEED)
HEXD = '0123456789ABCDEFabcdef'
def rhex(n, d=HEXD): return ''.join(rng.choice(d) for _ in range(n))
src = open(sys.argv[3]).read()
# reuse the generators of gen_cases.py (everything before the case loop), without running it
head = src[:src.index('cases = []')]
head = head.replace("N = int(sys.argv[1]); SEED = int(sys.argv[2]); ROOT = sys.argv[3]", "ROOT = ''")
ns = {'__name__': 'gen'}
exec(head.replace('rng = random.Random(SEED)', 'rng = RNG'), dict(ns, RNG=rng, SEED=SEED) ) if False else None
g = {'RNG': rng, 'SEED': SEED, 'sys': sys}
exec(head.replace('rng = random.Random(SEED)', 'rng = RNG'), g)
ops = []
def add(**k): ops.append(k)
for _ in range(N):
    g['CLEAN'][0] = rng.random() < 0.5
    bad = rng.choices([None, 'cut', 'noend', 'hdr', 'junk'], [60, 10, 10, 10, 10])[0]
    b = g['ips_bytes'](rng.choice([0x100, 0x600]), rng.random() < 0.3, bad)
    if rng.random() < 0.2 and b: b = bytes(x ^ (1 << rng.randrange(8)) if rng.random() < 0.05 else x for x in b)
    add(op='ips', hex=b.hex())
    bids = [rhex(64).upper(), rhex(16).upper()]
    t = g['pch_text'](bids, 0x600)
    if rng.random() < 0.3:
        L = list(t)
        for _ in range(rng.randrange(1, 4)):
            if L: L.insert(rng.randrange(len(L)), rng.choice(['\x1c', '\x85', ' ', '\t', ' ', '\x0b', '"', '/*', '*/', '//', '@', ' ', '\n', '0x', 'x', '　', '\x1f']))
        t = ''.join(L)
    add(op='pch', text=t)
    s = ''.join(rng.choice(['a', 'b', "'", '"', '\\', '\n', '\t', '\x00', '\x7f', '\x80', '\xad', 'é', '​', ' ', '', '\U0001f600', '\U000e0001', '͸', '　', ' ']) for _ in range(rng.randrange(0, 8)))
    add(op='repr', text=s)
    w = rng.choice([rhex(rng.randrange(0, 40)), rhex(16) + '0' * rng.randrange(0, 20), rhex(16) + '\n', rhex(15) + 'g', rhex(32).upper(), ''])
    bid = bytes(rng.randrange(256) for _ in range(32))
    if rng.random() < 0.5: w = bid.hex()[:rng.choice([16, 20, 32, 64])] + rng.choice(['', '0', '00'])
    add(op='bidm', wanted=w, bid=bid.hex())
    add(op='stem', name=rng.choice(['main.ips', rhex(16) + '.ips', 'a/' + rhex(20) + '.IPS', rhex(16) + '\n.ips', '.' + rhex(16), '...' + rhex(16), rhex(16) + '.', 'x.' + rhex(16), rhex(16) + '.a.b', rhex(15) + '.ips', 'é' * 16 + '.ips']))
    add(op='cheatbid', name=rng.choice(['main.txt', rhex(16) + '.txt', 'sub/' + rhex(32) + '.cheats', rhex(15), rhex(16)]))
    add(op='sections', text=g['sections_text']().decode('utf-8', 'replace'))
    # an NSO and patches
    kind = rng.choices(['ok', 'truncated', 'badlz4', 'short', 'nro', 'sizelie'], [70, 5, 8, 4, 3, 5])[0]
    nb = bytes(rng.randrange(256) for _ in range(32))
    data, mem, sizes = g['make_nso'](nb, kind)
    add(op='image', hex=data.hex())
    patches = []
    for _ in range(rng.randrange(0, 4)):
        wr = []
        for _ in range(rng.randrange(0, 4)):
            off = rng.choice([rng.randrange(-8, 4), rng.randrange(0, 0x300), rng.randrange(0xFF0, 0x1100), rng.randrange(0, 1 << 20), -(1 << 90), 1 << 90, rng.choice(mem) + rng.randrange(-4, 6) if mem else 0])
            wr.append([str(off), bytes(rng.randrange(256) for _ in range(rng.randrange(0, 9))).hex()])
        bidp = rng.choice([nb.hex().upper(), nb.hex().upper()[:16], nb.hex().upper()[:16] + '0' * 16, rhex(32).upper()])
        patches.append({'mod': 'm', 'source': 'x.ips', 'name': rng.choice(['ips', 'n']), 'bid': bidp, 'on': rng.random() < 0.85, 'writes': wr})
    add(op='nso', hex=data.hex(), patches=patches)
json.dump(ops, open(sys.argv[4], 'w'))
print(len(ops), 'ops')
