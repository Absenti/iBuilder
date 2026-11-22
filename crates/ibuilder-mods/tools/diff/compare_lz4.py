"""Compares the LZ4 fuzz results. A stream with an offset of 0 makes the reference decoder copy bytes of its own output buffer that were never written
(undefined: zeros in practice); this port calls such a stream corrupt, so those cases are counted apart, not as mismatches."""
import json, sys
def has_zero_offset(raw):
    ip, n = 0, len(raw)
    try:
        while ip < n:
            t = raw[ip]; ip += 1; lit = t >> 4
            if lit == 15:
                while True:
                    b = raw[ip]; ip += 1; lit += b
                    if b != 255: break
            ip += lit
            if ip >= n: return False
            off = raw[ip] | raw[ip + 1] << 8; ip += 2
            if off == 0: return True
            ml = t & 15
            if ml == 15:
                while True:
                    b = raw[ip]; ip += 1
                    if b != 255: break
    except IndexError:
        return False
    return False
ops = json.load(open(sys.argv[1])); py = json.load(open(sys.argv[2])); rs = json.load(open(sys.argv[3]))
same = zero = bad = ok = 0
for o, a, b in zip(ops, py, rs):
    if a == b:
        same += 1; ok += 'ok' in a
    elif has_zero_offset(bytes.fromhex(o['hex'])): zero += 1
    else:
        bad += 1
        if bad <= 5: print('MISMATCH', o['size'], o['hex'][:80], str(a)[:60], str(b)[:60])
print('lz4: %d cases, %d identical (%d of them decoded), %d offset-0 streams (reference reads unwritten bytes), %d MISMATCHES' % (len(ops), same, ok, zero, bad))
sys.exit(1 if bad else 0)
