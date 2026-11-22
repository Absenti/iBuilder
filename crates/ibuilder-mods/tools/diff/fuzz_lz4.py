import json, random, sys, lz4.block
rng = random.Random(int(sys.argv[2]) if len(sys.argv) > 2 else 1)
N = int(sys.argv[1])
ops = []
def data_for():
    k = rng.randrange(5)
    n = rng.choice([0, 1, 5, 12, 13, 16, 17, 40, 64, 100, 255, 256, 300, 1000, 5000, 70000])
    if k == 0: return bytes(rng.randrange(256) for _ in range(n))
    if k == 1: return bytes([rng.randrange(256)]) * n
    if k == 2: return bytes(rng.randrange(4) for _ in range(n))
    if k == 3: return (bytes(range(16)) * (n // 16 + 1))[:n]
    return b''.join(bytes([rng.randrange(256)]) * rng.randrange(1, 40) for _ in range(n // 20 + 1))[:n]
while len(ops) < N:
    d = data_for()
    comp = lz4.block.compress(d, store_size=False, mode=rng.choice(['default', 'high_compression']))
    if rng.random() < 0.15 and d:
        comp = b'\x00' * 0 + comp
    sizes = [len(d), len(d) + 1, max(0, len(d) - 1), len(d) + 100, len(d) // 2]
    variants = [comp]
    for _ in range(3):
        b = bytearray(comp)
        t = rng.randrange(4)
        if t == 0 and b:
            for _ in range(rng.randrange(1, 4)): b[rng.randrange(len(b))] = rng.randrange(256)
        elif t == 1 and b: b = b[:rng.randrange(len(b))]
        elif t == 2: b += bytes(rng.randrange(256) for _ in range(rng.randrange(1, 8)))
        else:
            b = bytearray(rng.randrange(256) for _ in range(rng.randrange(0, 40)))
        variants.append(bytes(b))
    for v in variants:
        ops.append({'op': 'lz4', 'hex': v.hex(), 'size': rng.choice(sizes)})
json.dump(ops, open(sys.argv[3] if len(sys.argv) > 3 else 'lz4_ops.json', 'w'))
