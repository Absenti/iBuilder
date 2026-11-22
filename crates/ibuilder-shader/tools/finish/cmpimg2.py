import sys, struct, mmap
def idx(path):
    f = open(path, 'rb'); m = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
    ioff, isz, count = struct.unpack_from('<QQQ', m, 8)
    ix = m[ioff:ioff+isz]; pool = count*24; out = {}
    for k in range(1, count):
        a, b, no, nl, t = struct.unpack_from('<QQIHB', ix, k*24)
        out[ix[pool+no:pool+no+nl].decode()] = (a, b)
    return m, out
ma, fa = idx(sys.argv[1]); mb, fb = idx(sys.argv[2])
bad = [n for n in fa if n not in fb or ma[fa[n][0]:fa[n][0]+fa[n][1]] != mb[fb[n][0]:fb[n][0]+fb[n][1]]]
print('files a', len(fa), 'b', len(fb), 'differing/missing', len(bad), bad[:10])
