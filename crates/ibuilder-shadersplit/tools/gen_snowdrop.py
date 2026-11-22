#!/usr/bin/env python3
"""Fixtures of the Snowdrop bare-zlib reader (`paches/engine/0104`, `shader_split.snowdrop_zlib`, marker `#SDFZLIB@`), built with the engine's own functions and Python's zlib:

  SHADER_DEVICE_DIR=<folder with shader_device.py> PYTHONDONTWRITEBYTECODE=1 python3 -I tools/gen_snowdrop.py <engine root (the folder that holds dm/)> <games out folder> <vectors out .jsonl.gz>

* `<games out>/snowdrop*/`: synthetic games (a RomFS folder of BERG data slices, an NSO `main`, a profile, a DLC folder) and `expected.json`, as `tools/gen_games.py` writes them
  (the sha256 of every file the engine's split writes, its `shaders:` / `[RED]` lines); `tests/games.rs` splits the same folders and must write the same bytes.
* `vectors-snowdrop.jsonl.gz`: (a) `zlib` vectors: a byte string and what `zlib.decompressobj().decompress(data[, max_length])` did with it (output, `eof`, bytes used, or the error text):
  valid streams of every kind (stored, fixed, dynamic blocks; windows 9-15), truncated, bit-flipped, with a preset-dictionary header; (b) `berg` vectors: a BERG data slice (c5 / c6 records, SDF
  zstd chunks, bare zlib caches, other assets) cut, flipped and rewritten in every place a reader looks at, and what `shader_split.snowdrop_shaders` found in it, or the `[RED]` text it exited with.
  `tests/snowdrop.rs` reads them and the Rust reader must give the same answers.

Nothing here comes from a game: every byte is a PRNG's or zeros.
"""
import sys, os, json, struct, hashlib, zlib, gzip, base64
here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, 'gen_games.py')).read()
games_out, vec_out = os.path.abspath(sys.argv[2]), os.path.abspath(sys.argv[3])
exec(src[:src.index("rng = Rng(0xfeed)")])   # engine, out_root = argv[1], argv[2]: loads the engine (`ss`), shader_device (`sd`), `make_game`, the PRNGs (`Rng`, `lcg_fill`)

M64 = (1 << 64) - 1
def u32(v): return struct.pack('<I', v)

def shader(stage, var=0):
    """a GLSLC control section + code area as the engine's own test builds them (0x880 + 0x100 bytes, mostly zeros: they compress), `var` makes the key distinct"""
    ctl, code = bytearray(0x880), bytearray(0x100)
    ctl[:4] = ss.CTL_MAGIC
    struct.pack_into('<I', ctl, 0x20, len(ctl)); struct.pack_into('<I', ctl, 0x704, len(code))
    code[:4] = ss.CS_MAGIC if stage == 'cs' else ss.GFX_MAGIC
    if stage == 'cs': struct.pack_into('<3I', ctl, 0x748, 8, 8, 1)   # a compute shader's thread group (the split's facts refuse 0)
    if stage != 'cs': struct.pack_into('<I', code, 0x30, (ss.STAGES.index(stage) + 1) << 10 | 1)
    if stage == 'ps': struct.pack_into('<I', code, 0x48, 1)
    struct.pack_into('<I', code, 0x80, var)
    return bytes(ctl), bytes(code)

def S(stage, var=0): return b''.join(shader(stage, var))

def record(raw, flags=0x48, legacy=False):
    packed = zlib.compress(raw) if legacy else zstd.compress(raw)
    return struct.pack('<5I', 0xabcd00c5 if legacy else 0xabcd00c6, flags, 0, len(raw), len(packed)) + packed

def archive(*records): return b'BERG' + struct.pack('<I', 22) + bytes(48) + b''.join(records)

def cache(raw, kind=0x13c, n=None, m=None):
    """a bare .mcache file: its 0xabcd00d4 head (kind, 0, twice the length that follows), then the body; n / m change the two length fields"""
    return struct.pack('<5I', 0xabcd00d4, kind, 0, len(raw) if n is None else n, len(raw) if m is None else m) + raw

def zlib_blocks(data, wbits=10, level=9):
    """as the SDF data slices store a file: one zlib stream per 64 KB, a small window (zlib header 0x28 0x15)"""
    out = b''
    for i in range(0, max(len(data), 1), 0x10000):
        c = zlib.compressobj(level, zlib.DEFLATED, wbits); out += c.compress(data[i:i + 0x10000]) + c.flush()
    return out

def noise(seed, n): return lcg_fill(seed, n)
def fill(n): return (b'snow-filler-text ' * (n // 17 + 1))[:n]   # compressible stand-in for assets

# ---- the games ---------------------------------------------------------------------------------------------------------------------------------
small = cache(b'reflection\0' + S('vs', 1))
big = cache(noise(1, 0x12000) + S('ps', 2) + S('cs', 3), kind=0x65)
three = cache(fill(0x2c000) + S('hs', 4) + fill(0x1000) + S('ds', 5) + S('gs', 6), kind=0x13c)   # 3 blocks, three shaders, two of them a few bytes apart
exact = cache(fill(0x10000 - 20 - 0x980) + S('vs', 7), kind=0x13c)                                       # the decoded size is exactly 0x10000
exact1 = cache(fill(0x10001 - 20 - 0x980) + S('ps', 8), kind=0x13c)                                    # one byte over a block: a second, one-byte block
dup = cache(b'again\0' + S('vs', 1))                                                                      # the same shader as `small`: a duplicate key
wrapped = zstd.compress(record(S('ps', 9), legacy=True))

files = {
    # bare caches between other assets, in the order of the engine's own test; the file's shaders: vs ps cs, then the rest of the caches
    'Data/bare.slice': archive(zlib_blocks(small), zlib_blocks(b'other asset' * 100), b'\x28\x15 not a stream', zlib_blocks(big), zlib_blocks(b'x' * 50),
                               zlib_blocks(three), zlib_blocks(exact), zlib_blocks(exact1), zlib_blocks(dup)),
    # every encoding side by side: the bare caches are read first, then the c5 / c6 records in file order, then the zstd chunks
    'Data/mixed.slice': archive(record(S('vs', 20)), zlib_blocks(cache(S('vs', 21) + S('ps', 22))), record(S('ps', 23), legacy=True), wrapped, zlib_blocks(cache(b'x' + S('cs', 24))),
                                record(b'x' + S('cs', 25), flags=0x404)),
    # windows of 9 to 15 bits (the zlib header's high nibble), levels 0 (stored blocks), 1 and 9, a cache that is one stream of stored blocks
    'Pack/windows.slice': archive(*[zlib_blocks(cache(noise(30 + w, 0x300) + S('vs', 30 + w)), wbits=w, level=lv) for w, lv in ((9, 9), (11, 1), (12, 0), (13, 6), (14, 9), (15, 9))]),
    # a file with no cache at all (zlib streams of other assets and the cache head in the wrong place), and one that is only a head
    'Pack/none.slice': archive(zlib_blocks(b'plain asset' * 50), zlib_blocks(struct.pack('<5I', 0xabcd00d4, 1, 5, 4, 4) + S('vs', 40)), zlib_blocks(struct.pack('<5I', 0xabcd00d4, 1, 0, 4, 5) + S('vs', 41)),
                               zlib_blocks(b'\xd4\x00\xcd\xab' + bytes(12)), b'\xd4\x00\xcd\xab' + bytes(40), zlib_blocks(S('ps', 42))),
}
# each of these is a defect the reader reports (`[RED] <file>: ... (its shaders are left out)`): the whole file is left out, the split goes on
bigz = zlib_blocks(cache(fill(0x18000) + S('vs', 50)))
first = zlib.decompressobj(); first.decompress(bigz); cut1 = len(bigz) - len(first.unused_data)    # the first 64 KB stream only
files['Pack/cut_after_first_block.slice'] = archive(bigz[:cut1])
files['Pack/cut_in_second_block.slice'] = archive(bigz[:cut1 + 40])
files['Pack/cut_in_first_block.slice'] = archive(bigz[:cut1 - 40], b'x' * 300)
bad = bytearray(bigz); bad[cut1 + 30] ^= 0x55
files['Pack/damaged_second_block.slice'] = archive(bytes(bad))
bad = bytearray(bigz); bad[cut1 - 1] ^= 1
files['Pack/damaged_first_block_check.slice'] = archive(bytes(bad), zlib_blocks(cache(S('ps', 51))))
files['Pack/head_longer_than_stream.slice'] = archive(zlib_blocks(cache(S('vs', 52), n=0x10000 + 20, m=0x10000 + 20)))
files['Pack/head_shorter_than_stream.slice'] = archive(zlib_blocks(cache(bytes(0x3000) + S('vs', 53), n=0x100, m=0x100)))   # decodes inside the input window, to more than the head says
two = zlib_blocks(cache(fill(0x10000 - 20 + 8)))                                    # 0x10008 bytes: a 64 KB stream and an 8-byte one
f2 = zlib.decompressobj(); f2.decompress(two); cut2 = len(two) - len(f2.unused_data)
files['Pack/empty_second_block.slice'] = archive(two[:cut2] + zlib.compress(b''))
dlc = {'7': {'Data/dlc.slice': archive(zlib_blocks(cache(b'dlc\0' + S('ps', 60))), zlib_blocks(small))}}   # one shader of its own, one the game has too
rng = Rng(0xfeed)
os.makedirs(games_out, exist_ok=True)
make_game('snowdrop', files, main_shaders=[mk_glslc(rng, 61000, ['ps', 'vs'])], dlc=dlc)


# ---- the vectors --------------------------------------------------------------------------------------------------------------------------------
vecs = []
def sha(b): return hashlib.sha256(b).hexdigest()
def b64(b): return base64.b64encode(b).decode()
vr = Rng(0x5d0f)

def zres(data, lim):
    d = zlib.decompressobj()
    try: out = d.decompress(data, lim) if lim else d.decompress(data)
    except zlib.error as e: return {'err': str(e)}
    r = {'len': len(out), 'sha': sha(out), 'eof': d.eof}
    if d.eof: r['used'] = len(data) - len(d.unused_data)
    return {'ok': r}

WORDS = [b'vertex', b'pixel', b'shader', b'cache', b'reflection', b'uniform', b'\0\0\0\0', b'\xff' * 6, b'snow', b'drop', b'0123456789', b'texture', b' ', b'\n']
def sample(kind, n):
    if kind == 'rand': return lcg_fill(vr.next(), n)
    if kind == 'text': return b''.join(vr.pick(WORDS) for _ in range(n // 5 + 1))[:n]
    if kind == 'runs':
        out = b''
        while len(out) < n: out += bytes([vr.below(256)]) * (1 + vr.below(300))
        return out[:n]
    if kind == 'mixed': return sample('text', n // 2) + lcg_fill(vr.next(), n // 4) + sample('runs', n // 4)
    return bytes(n)

def mutate(c):
    """a few kinds of damage, half of them near the start of the stream (the header, the block header, the code tables)"""
    c = bytearray(c); k = vr.below(12)
    near = lambda: vr.below(min(len(c), 48)) if vr.chance(1, 2) else vr.below(max(len(c), 1))
    if not c: return bytes(c)
    if k == 0: return bytes(c)
    if k <= 3:
        for _ in range(1 + vr.below(3)): c[near() % len(c)] ^= 1 << vr.below(8)
    elif k == 4: c[near() % len(c)] = vr.below(256)
    elif k == 5: return bytes(c[:vr.below(len(c) + 1)])
    elif k == 6: del c[near() % len(c)]
    elif k == 7: c.insert(near() % (len(c) + 1), vr.below(256))
    elif k == 8: return bytes(c[:max(len(c) - 1 - vr.below(6), 0)])                     # the Adler-32 cut or damaged
    elif k == 9: c += lcg_fill(vr.next(), 1 + vr.below(8))                              # bytes after the stream
    elif k == 10 and len(c) >= 2:                                                       # a header of another kind: window bits, preset dictionary (FDICT), a wrong check
        cmf = vr.pick([0x08, 0x18, 0x28, 0x38, 0x48, 0x58, 0x68, 0x78, 0x88, 0x79, 0x07, 0x78])
        flg = vr.below(256)
        if vr.chance(2, 3): flg = (flg & 0xe0) | (31 - ((cmf << 8 | (flg & 0xe0)) % 31)) % 31      # a valid FCHECK, FDICT as it comes
        c[0], c[1] = cmf, flg
    else:
        c[near() % len(c)] ^= 0xff
    return bytes(c)

n_zlib = 0
for i in range(2400):
    kind = vr.pick(['rand', 'text', 'text', 'runs', 'mixed', 'zeros'])
    n = vr.pick([0, 1, 3, 20, 21, 40, 100, 300, 700, 1500]) if kind != 'rand' else vr.pick([1, 5, 19, 20, 21, 64, 200, 300])
    data = sample(kind, n)
    level, wbits = vr.pick([0, 1, 3, 6, 9, 9]), vr.pick([9, 10, 12, 15, 15])
    c = zlib.compressobj(level, zlib.DEFLATED, wbits, vr.pick([1, 4, 8, 9]), vr.pick([0, 0, 0, 1, 2, 3, 4]))
    stream = c.compress(data) + c.flush()
    if vr.chance(1, 3): stream = zlib.compress(data, level) if vr.chance(1, 2) else stream
    if vr.chance(1, 4) and len(data) > 5:   # two blocks in one stream: a flush in the middle (a stored empty block with its marker)
        c = zlib.compressobj(level, zlib.DEFLATED, wbits); stream = c.compress(data[:len(data) // 2]) + c.flush(zlib.Z_FULL_FLUSH) + c.compress(data[len(data) // 2:]) + c.flush()
    stream = mutate(stream)
    vecs.append({'kind': 'zlib', 'in': {'b64': b64(stream)}, 'out': {'full': zres(stream, 0), 'lim': {str(l): zres(stream, l) for l in (1, 7, 20, 100)}}})

def berg_res(data):
    try: found = ss.snowdrop_shaders(data, '/v')
    except (Exception, SystemExit) as e: return {'err': str(e)}
    return {'ok': [[w, st, sha(ctl), sha(code), len(ctl), len(code)] for w, st, ctl, code in found]}

def parts_of(name):
    """the parts a slice is made of (each is one stored asset): the vectors damage them one at a time"""
    return {
        'small': [zlib_blocks(small), zlib_blocks(b'other asset' * 100), b'\x28\x15 not a stream', zlib_blocks(b'x' * 50)],
        'two': [zlib_blocks(cache(fill(0x12000) + S('ps', 2) + S('cs', 3), kind=0x65)), zlib_blocks(small)],
        'three': [zlib_blocks(cache(fill(0x2c000) + S('hs', 4) + fill(0x100) + S('ds', 5) + S('gs', 6)))],
        'exact': [zlib_blocks(cache(fill(0x10000 - 20 - 0x980) + S('vs', 7))), zlib_blocks(cache(fill(0x10001 - 20 - 0x980) + S('ps', 8)))],
        'recs': [record(S('vs', 20)), record(S('ps', 23), legacy=True), record(b'x' + S('cs', 25), flags=0x404), zlib_blocks(cache(S('vs', 21)))],
        'wrapped': [wrapped, record(S('vs', 20)), zlib_blocks(cache(S('ps', 22))), zstd.compress(b'other asset')],
        'heads': [zlib_blocks(cache(S('vs', 40), n=5)), zlib_blocks(cache(S('vs', 41), n=0x2000, m=0x2000)), zlib_blocks(cache(S('ps', 42), n=len(S('ps', 42)) - 3, m=len(S('ps', 42)) - 3)),
                  zlib_blocks(b'\xd4\x00\xcd\xab' + bytes(12)), zlib_blocks(cache(S('ps', 43), kind=0x7777))],
    }[name]

n_berg = 0
names = ['small', 'two', 'three', 'exact', 'recs', 'wrapped', 'heads']
for name in names:
    parts = parts_of(name)
    clean = archive(*parts)
    vecs.append({'kind': 'berg', 'in': {'b64': b64(clean)}, 'out': berg_res(clean)}); n_berg += 1
    starts, o = [], 56
    for p in parts: starts.append(o); o += len(p)
    for j in range(150):
        d = bytearray(clean); k = vr.below(9)
        st = vr.pick(starts)
        if k == 0: d[56 + vr.below(len(d) - 56)] ^= 1 << vr.below(8)
        elif k == 1: d[st + vr.below(min(40, len(d) - st))] ^= 1 << vr.below(8)             # at the start of a part: the header, the first block
        elif k == 2: d = d[:56 + vr.below(len(d) - 55)]
        elif k == 3: d = d[:st + vr.below(min(300, len(d) - st))]                           # cut inside or just after a part
        elif k == 4: d[st + 1] = vr.below(256)                                              # FLG / second byte of a record
        elif k == 5: del d[st + vr.below(min(60, len(d) - st))]
        elif k == 6: d[st + vr.below(min(24, len(d) - st))] = vr.below(256)
        elif k == 7:
            e = st + vr.below(min(len(d) - st, 400)); d[e:e] = lcg_fill(vr.next(), 1 + vr.below(5))
        else:
            for _ in range(3): d[56 + vr.below(len(d) - 56)] ^= 1 << vr.below(8)
        d = bytes(d)
        vecs.append({'kind': 'berg', 'in': {'b64': b64(d)}, 'out': berg_res(d)}); n_berg += 1

# the engine's own two cases (test_snowdrop.py) too
vecs.append({'kind': 'berg', 'in': {'b64': b64(files['Data/bare.slice'])}, 'out': berg_res(files['Data/bare.slice'])})
os.makedirs(os.path.dirname(vec_out) or '.', exist_ok=True)
with gzip.GzipFile(vec_out, 'wb', mtime=0) as f:
    f.write(('\n'.join(json.dumps(v, sort_keys=True, separators=(',', ':')) for v in vecs) + '\n').encode())
print('vectors:', len(vecs), 'zlib', sum(v['kind'] == 'zlib' for v in vecs), 'berg', n_berg, os.path.getsize(vec_out) // 1024, 'KiB gz')
