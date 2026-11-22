#!/usr/bin/env python3
"""ZBIC frames from the `zbic` compressor (the oracle), for the Rust decoder's tests.

  gen_zbic_frames.py OUTDIR COUNT SEED [--from FILE] [--mutate N]

Writes OUTDIR/<name>.zbic and OUTDIR/manifest.tsv (name, plaintext length, SHA-256 of the plaintext). Without --from the plaintexts are synthetic
(random words, skewed byte distributions, counters, records: no game data, so the small set under tests/fixtures/zbic is committable); with
--from the plaintexts are slices of FILE. --mutate N adds N frames with a random byte changed in a frame: their manifest line carries what
the oracle's decompress made of them ('ERR' or the SHA-256 of its output) so the Rust decoder can be checked against it, error for error.
Run with a Python that has the `zbic` package (the reference venv), with -I and PYTHONDONTWRITEBYTECODE=1.
"""
import hashlib, os, random, sys
import zbic


def synthetic(rnd):
    kind = rnd.randrange(6)
    n = rnd.choice([1, 7, 40, 300, 2000, 9000, 40000, 150000])
    if kind == 0:
        return bytes(rnd.randrange(256) for _ in range(n))
    if kind == 1:
        k = rnd.choice([1, 2, 3, 5, 9, 17, 60])
        words = [bytes(rnd.randrange(256) for _ in range(rnd.choice([2, 3, 4, 6, 9, 16]))) for _ in range(k)]
        out = bytearray()
        while len(out) < n:
            out += rnd.choice(words)
        return bytes(out[:n])
    if kind == 2:
        alpha = bytes(rnd.randrange(256) for _ in range(rnd.choice([2, 3, 4, 10, 40])))
        return bytes(rnd.choice(alpha) for _ in range(n))
    if kind == 3:
        out = bytearray()
        i = 0
        while len(out) < n:
            out += i.to_bytes(4, 'little') + bytes([0, 0, rnd.randrange(3), 0])
            i += 1
        return bytes(out[:n])
    if kind == 4:
        out = bytearray()
        while len(out) < n:
            if len(out) > 50 and rnd.random() < 0.4:
                ln = rnd.randrange(3, 80)
                frm = rnd.randrange(0, len(out))
                for k in range(ln):
                    out.append(out[frm + k % (len(out) - frm)])
            else:
                r = rnd.randrange(100)
                out.append(b'etaoin shrdlu'[r % 13] if r < 60 else rnd.randrange(128))
        return bytes(out[:n])
    # long zero runs with sparse values (code-like)
    out = bytearray(n)
    for _ in range(n // 20):
        out[rnd.randrange(n)] = rnd.randrange(256)
    return bytes(out)


def main():
    outdir, count, seed = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    src = None
    mutate = 0
    args = sys.argv[4:]
    while args:
        a = args.pop(0)
        if a == '--from':
            src = open(args.pop(0), 'rb').read()
        elif a == '--mutate':
            mutate = int(args.pop(0))
    os.makedirs(outdir, exist_ok=True)
    rnd = random.Random(seed)
    rows = []
    frames = []
    for i in range(count):
        if src is not None:
            n = rnd.choice([100, 1000, 20000, 130000, 400000])
            off = rnd.randrange(0, max(1, len(src) - n))
            data = src[off:off + n]
        else:
            data = synthetic(rnd)
        level = rnd.choice([1, 2, 3, 5, 9, 12, 15, 19, 22])
        frame = zbic.compress(data, level)
        name = f'f{seed}_{i:05d}'
        open(f'{outdir}/{name}.zbic', 'wb').write(frame)
        rows.append((name, len(data), hashlib.sha256(data).hexdigest()))
        frames.append((name, frame))
    for j in range(mutate):
        name, frame = rnd.choice(frames)
        b = bytearray(frame)
        for _ in range(rnd.choice([1, 1, 2, 4])):
            pos = rnd.randrange(4, len(b))
            b[pos] = (b[pos] ^ (1 << rnd.randrange(8))) if rnd.random() < 0.7 else rnd.randrange(256)
        mname = f'm{seed}_{j:05d}'
        open(f'{outdir}/{mname}.zbic', 'wb').write(bytes(b))
        try:
            out = zbic.decompress(bytes(b))
            rows.append((mname, len(out), hashlib.sha256(out).hexdigest()))
        except Exception:
            rows.append((mname, 0, 'ERR'))
    with open(f'{outdir}/manifest.tsv', 'w') as f:
        for r in rows:
            f.write('\t'.join(map(str, r)) + '\n')
    print(len(rows), 'frames in', outdir)


main()
