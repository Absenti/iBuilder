# part of gen_vectors.py (exec'd in its namespace): synthetic containers and Unity files, and the engine's answers for them
import random as _r

def sha(b): return hashlib.sha256(b).hexdigest()
def found_json(found): return [[w, st, sha(c), sha(k), len(c), len(k)] for w, st, c, k in found]

def run_blob(d, label='/t'):
    """scan_blob + uniq_keys + var_pairs of the engine, or the error"""
    ss.BAHS_VAR.clear()
    buf = io.StringIO()
    def go():
        found = []
        with contextlib.redirect_stdout(buf): ss.scan_blob(d, label, found)
        r = {'found': found_json(found)}
        try:
            u = ss.uniq_keys(found); r['uniq'] = [[k, v[0]] for k, v in u.items()]
        except (Exception, SystemExit) as e: r['uniq_err'] = True
        r['pairs'] = sorted([list(p) for p in ss.var_pairs(found)])
        return r
    return res(go)

def run_glslc(d, label='/t'):
    """glslc_shaders (what main() and scan_file_in's fallback call) + uniq_keys + var_pairs"""
    ss.BAHS_VAR.clear()
    def go():
        found = ss.glslc_shaders(d, label, need=False)
        r = {'found': found_json(found)}
        try:
            u = ss.uniq_keys(found); r['uniq'] = [[k, v[0]] for k, v in u.items()]
        except (Exception, SystemExit) as e: r['uniq_err'] = True
        r['pairs'] = sorted([list(p) for p in ss.var_pairs(found)])
        return r
    return res(go)

def blob_vec(d, label='/t', **extra):
    if label.startswith('/glslc'): emit('glslc', {'hex': d.hex(), 'label': label, **extra}, run_glslc(d, label))
    else: emit('blob', {'hex': d.hex(), 'label': label, **extra}, run_blob(d, label))

# ---- shaders ------------------------------------------------------------------------------------------------------------------------------
def mk_shader(rng, seed, st, nvn2=False):
    ncode = rng.pick([0x80, 0x100, 0x180])
    if nvn2:
        n = rng.pick([0x48, 0xd8])
        code = lcg_fill(seed, ncode)
        ctl = bytearray(lcg_fill(seed + 1, n)); ctl[:4] = ss.CTL_MAGIC; ctl[4:8] = b'\xc9\0\0\0'; ctl[0x30:0x34] = u32(n); ctl[0x44:0x48] = u32(ncode)
        if len(ctl) > 0x56: ctl[0x56] = {'cs': 5, 'ts': 6, 'ms': 7}.get(st, 0)
        code = bytearray(code); w = {'vs': 1 | (1 << 10), 'ps': 1 | (5 << 10), 'gs': 1 | (4 << 10), 'hs': 1 | (2 << 10), 'ds': 1 | (3 << 10)}.get(st, 0x461)
        code[0:4] = u32(w)   # NVN2 code has no header: the SPH word is read from its first 4 bytes
        return bytes(ctl), bytes(code)
    n = rng.pick([0x760, 0x850, 0x880])
    code = bytearray(lcg_fill(seed, ncode))
    ctl = bytearray(lcg_fill(seed + 1, n)); ctl[:4] = ss.CTL_MAGIC; ctl[0x20:0x24] = u32(n); ctl[0x704:0x708] = u32(ncode)
    # no accidental control magic inside random bytes (the readers would find it): scrub
    for buf in (ctl, code):
        i = bytes(buf).find(ss.CTL_MAGIC, 4 if buf is ctl else 0)
        while i >= 0: buf[i] ^= 0x55; i = bytes(buf).find(ss.CTL_MAGIC, i + 1)
    if st == 'cs':
        code[:4] = ss.CS_MAGIC; ctl[0x748:0x754] = u32(64) + u32(1) + u32(1)
    else:
        code[:4] = ss.GFX_MAGIC; w = {'vs': 0x461, 'ps': 1 | (5 << 10), 'gs': 1 | (4 << 10), 'hs': 1 | (2 << 10), 'ds': 1 | (3 << 10)}[st]; code[0x30:0x34] = u32(w)
    return bytes(ctl), bytes(code)

def mutate(rng, d, n=3):
    b = bytearray(d)
    for _ in range(rng.below(n) + 1):
        if not b: break
        k = rng.below(5)
        if k == 0: b[rng.below(len(b))] ^= 1 + rng.below(255)
        elif k == 1: b[rng.below(len(b)):] = b[rng.below(len(b)):][:rng.below(64)]
        elif k == 2: b = b[:rng.below(len(b) + 1)]
        elif k == 3:
            o = rng.below(len(b)); v = rng.pick([0, 1, 0xffffffff, 0x10, 0x7fffffff, 0x80000000, 0x100000]); b[o:o + 4] = u32(v)[:max(0, min(4, len(b) - o))]
        else: b += bytes([rng.below(256)]) * rng.below(40)
    return bytes(b)

# ---- GLSLC outputs --------------------------------------------------------------------------------------------------------------------------
def mk_glslc(rng, seed, stages, nvn2=False, pad=0):
    """one GLSLC output holding the given stages as GPU code sections"""
    first, stride = (0xa8, 0x98) if nvn2 else (0x90, 0x90)
    shs = [mk_shader(rng, seed + 10 * i, st, nvn2) for i, st in enumerate(stages)]
    n = len(shs)
    hdr = first + stride * n
    data, secs = bytearray(), []
    for ctl, code in shs:
        soff = hdr + len(data)
        data += ctl + code
        secs.append((len(ctl) + len(code), soff, len(ctl), len(code)))
    total = hdr + len(data)
    d = bytearray(total)
    d[0:4] = ss.GLSLC_MAGIC if hasattr(ss, 'GLSLC_MAGIC') else b'\x91\x68\x86\x19'
    if nvn2:
        d[0x30:0x34] = u32(0xc9); d[0x44:0x48] = u32(0); d[0x5c:0x60] = u32(total); d[0x64:0x68] = u32(n)
    else:
        d[0x44:0x48] = u32(total); d[0x4c:0x50] = u32(n)
    for i, (ssz, soff, cn, kn) in enumerate(secs):
        s = first + stride * i
        d[s:s + 12] = u32(ssz) + u32(soff) + u32(0)
        d[s + 0x30:s + 0x40] = u32(0) + u32(cn) + u32(kn) + u32(cn)
    d[hdr:] = data
    return bytes(d) + bytes(pad)

# ---- BNSH ----------------------------------------------------------------------------------------------------------------------------------
def mk_bnsh(rng, seed, nvar, stage_sets, junk=0):
    """BNSH at 0: grsc, variation table, programs, stage descriptors, blobs"""
    d = bytearray(0x100)
    d[0:8] = b'BNSH\0\0\0\0'; d[0x60:0x64] = b'grsc'
    pvar = 0x100
    d[0x7c:0x80] = u32(nvar); d[0x80:0x88] = struct.pack('<Q', pvar)
    d += bytes(0x40 * nvar)
    for v in range(nvar):
        prog = len(d); d += bytes(0x50)
        d[pvar + 0x40 * v + 0x10: pvar + 0x40 * v + 0x18] = struct.pack('<Q', prog)
        for si, st in enumerate(ss.STAGES):
            if st not in stage_sets[v % len(stage_sets)]: continue
            ctl, code = mk_shader(rng, seed + 100 * v + si, st)
            sc = len(d); d += bytes(0x20)
            pctl = len(d); d += ctl
            pcode = len(d); d += code
            d[sc + 8: sc + 0x20] = struct.pack('<QQII', pctl, pcode, len(code), len(ctl))
            d[prog + 8 + 8 * si: prog + 16 + 8 * si] = struct.pack('<Q', sc)
    d += bytes(junk)
    d[0x1c:0x20] = u32(len(d))
    return bytes(d)

# ---- BAHS ------------------------------------------------------------------------------------------------------------------------------------
def mk_bahs(rng, seed, kinds, programs):
    """agl BAHS: entries {off, 0, csz, rel} in 16-byte records inside the binary array, control sections after the program array, code at 0x100-aligned offsets in the code region.
    kinds: the kind (0 vertex, 1 fragment, 3 compute) of each binary; programs: the index of the first binary of each program"""
    n = len(kinds)
    shs = [mk_shader(rng, seed + 7 * i, {0: 'vs', 1: 'ps', 3: 'cs'}[k]) for i, k in enumerate(kinds)]
    arr = 0x38
    asz = 8 + 0x20 * n
    pa = arr + asz
    psz = 8 + sum(0x20 for _ in programs)
    ctl0 = pa + psz
    ctls, off = [], 0
    ctl_at, code_at = [], []
    pos = ctl0
    for ctl, code in shs:
        ctl_at.append(pos); pos += len(ctl)
    region = pos
    codes = bytearray()
    for ctl, code in shs:
        code_at.append(len(codes)); codes += code; codes += bytes((-len(codes)) % 0x100)
    d = bytearray(region)
    d[0:4] = b'BAHS'; d[0x18:0x1c] = u32(len(codes)); d[0x1c:0x20] = u32(region)
    d[arr:arr + 8] = u32(asz) + u32(n)
    for i, k in enumerate(kinds):
        o = arr + 8 + 0x20 * i
        d[o:o + 0x10] = u32(0x20) + u32(k) + u32(0) + u32(0)
        a = o + 0x10
        d[a:a + 16] = u32(code_at[i]) + u32(0) + u32(len(shs[i][0])) + struct.pack('<i', ctl_at[i] - a)
    d[pa:pa + 8] = u32(psz) + u32(len(programs))
    for p, base in enumerate(programs):
        o = pa + 8 + 0x20 * p
        d[o:o + 16] = u32(0x20) + u32(0) + u32(0) + u32(base)
    for (ctl, code), c in zip(shs, ctl_at): d[c:c + len(ctl)] = ctl
    return bytes(d) + bytes(codes)

# ---- SARC / Yaz0 -----------------------------------------------------------------------------------------------------------------------------
def mk_sarc(members, name_mode=1):
    n = len(members)
    names = b''; offs = []
    for nm, _ in members:
        offs.append(len(names) // 4); b = nm.encode() + b'\0'; b += bytes((-len(b)) % 4); names += b
    hl = 0x14
    sfat = b'SFAT' + struct.pack('<HHI', 12, n, 0x65)
    data, ents = b'', b''
    for i, (nm, m) in enumerate(members):
        pad = (-len(data)) % 4
        data += bytes(pad); s = len(data); data += m
        attr = (0x01000000 | offs[i]) if name_mode else 0
        ents += struct.pack('<IIII', i, attr, s, s + len(m))
    sfnt = b'SFNT' + struct.pack('<HH', 8, 0) + names
    doff = hl + len(sfat) + len(ents) + len(sfnt)
    doff += (-doff) % 4
    head = b'SARC' + struct.pack('<HHII', hl, 0xfeff, doff + len(data), doff) + struct.pack('<I', 0x100)
    body = head + sfat + ents + sfnt
    body += bytes(doff - len(body))
    return body + data

def yaz0_encode(d, rng, greedy=True):
    out = bytearray(b'Yaz0' + struct.pack('>I', len(d)) + bytes(8))
    i = 0
    while i < len(d):
        hdr_at = len(out); out.append(0); bits = 0
        for k in range(8):
            if i >= len(d): break
            best = (0, 0)
            if greedy and i > 0 and i + 3 <= len(d):
                j = d.rfind(d[i:i + 3], max(0, i - 4096), i + 2)   # the nearest earlier start of the same three bytes
                if 0 <= j < i:
                    dist = i - j; l = 0
                    while i + l < len(d) and l < 0x111 and d[i + l] == d[i - dist + (l % dist)]: l += 1
                    best = (l, dist)
            if best[0] >= 3 and rng.chance(2, 3):
                l, dist = best
                if l <= 17: out += bytes([((l - 2) << 4) | ((dist - 1) >> 8), (dist - 1) & 0xff])
                else: out += bytes([(dist - 1) >> 8, (dist - 1) & 0xff, l - 0x12])
                i += l
            else:
                bits |= 0x80 >> k; out.append(d[i]); i += 1
        out[hdr_at] = bits
    return bytes(out)

# ---- others ----------------------------------------------------------------------------------------------------------------------------------
def mk_mshd(rng, seed, npairs):
    out = bytearray(b'MSHD' + bytes(0x20))
    for i in range(npairs * 2):
        st = 'vs' if i % 2 == 0 else 'ps'
        ctl, code = mk_shader(rng, seed + i, st)
        ctl = bytearray(ctl + bytes(0x880 - len(ctl) if len(ctl) < 0x880 else 0)); ctl[0x20:0x24] = u32(len(ctl))
        out += ctl + bytes(rng.pick([0, 4, 0x10])) + code
    return bytes(out)

def mk_tfxo(rng, seed, n):
    d = bytearray(b'.tfxo file\0' + b'hlsl\0TexUtil\0' * 3)
    for i in range(n):
        st = rng.pick(['vs', 'ps', 'cs'])
        ctl, code = mk_shader(rng, seed + i, st)
        d += ctl + code + bytes(rng.pick([0, 0, 4]))
    return bytes(d)

def mk_pgso(rng, seed, st):
    ctl, code = mk_shader(rng, seed, st)
    magic = {'vs': b'VSO\0', 'ps': b'PSO\0', 'cs': b'CSO\0', 'gs': b'GSO\0'}[st]
    return magic + b'\x01\x00\x09\x00' + u32(len(ctl)) + u32(0x18) + u32(len(code)) + u32(0x18 + len(ctl)) + ctl + code

def mk_ndsr(rng, seed, stages):
    d = bytearray(b'NDSR' + bytes(0x1c)); hdr = 0x10
    blobs = []
    recs = bytearray()
    body_at = 0x10 + 16 * (len(stages) + 1)
    pos = body_at
    for i, st in enumerate(stages):
        ctl, code = mk_shader(rng, seed + i, st, nvn2=True)
        c = pos; pos += len(ctl); k = pos; pos += len(code)
        recs += u32(c) + u32(len(ctl) << 4) + u32(k) + u32(0)
        blobs.append(ctl + code)
    recs += bytes(16)
    return bytes(d[:0x10]) + bytes(recs) + b''.join(blobs)

# ---- vectors ---------------------------------------------------------------------------------------------------------------------------------
for i in range(50):
    seed += 100
    r = rng.pick([1, 1, 2, 3])
    d = mk_glslc(rng, seed, rng.pick([['vs'], ['ps'], ['cs'], ['vs', 'ps'], ['ps', 'vs', 'gs'], ['ps', 'ps']]), nvn2=rng.chance(1, 5), pad=rng.pick([0, 0, 17]))
    if rng.chance(1, 3): d = mutate(rng, d)
    if rng.chance(1, 4): d = lcg_fill(seed, rng.pick([0, 9, 100])) + d + lcg_fill(seed + 5, 40)
    blob_vec(d, '/glslc')
    # a SINGLE and a multi output next to each other, as in a main
    if i % 10 == 0: blob_vec(d + d[:0x100] + mk_glslc(rng, seed + 3, ['ps']), '/glslc2')
for i in range(40):
    seed += 100
    d = mk_bnsh(rng, seed, rng.pick([1, 2, 3]), [{'vs', 'ps'}, {'cs'}, {'vs', 'gs', 'ps'}, {'ps'}], junk=rng.pick([0, 0, 33]))
    if rng.chance(2, 5): d = mutate(rng, d)
    if rng.chance(1, 4): d = lcg_fill(seed, rng.pick([5, 80])) + d + lcg_fill(seed + 5, 40)
    blob_vec(d, '/bnsh')
for i in range(30):
    seed += 100
    kinds = [rng.pick([0, 1, 1, 3]) for _ in range(rng.pick([2, 4, 6]))]
    progs = sorted(set([0] + [rng.below(len(kinds)) for _ in range(rng.pick([0, 1, 2]))]))
    d = mk_bahs(rng, seed, kinds, progs)
    if rng.chance(1, 3): d = mutate(rng, d, 2)
    blob_vec(d, '/bahs')
for i in range(20):
    seed += 100
    sh_files = [(f'f{j}.bnsh', mk_bnsh(rng, seed + j, 1, [{'vs', 'ps'}])) for j in range(rng.pick([1, 2, 3]))] + [(f'n{j}.glslc', mk_glslc(rng, seed + 50 + j, ['ps'])) for j in range(rng.pick([0, 1]))]
    sarc = mk_sarc(sh_files, name_mode=rng.pick([0, 1, 1]))
    top = rng.pick([sarc, yaz0_encode(sarc, rng), mk_sarc([('inner.sarc', sarc), ('inner.szs', yaz0_encode(sarc, rng))]), yaz0_encode(mk_sarc([('x.szs', yaz0_encode(sarc, rng))]), rng)])
    if rng.chance(1, 4): top = mutate(rng, top, 2)
    blob_vec(top, '/sarc')
for i in range(15):
    seed += 100
    blob_vec(mutate(rng, mk_mshd(rng, seed, rng.pick([1, 2, 3])), 1) if rng.chance(1, 3) else mk_mshd(rng, seed, rng.pick([1, 2, 3])), '/mshd')
    blob_vec(mutate(rng, mk_tfxo(rng, seed, rng.pick([1, 3])), 2) if rng.chance(1, 3) else mk_tfxo(rng, seed, rng.pick([1, 3])), '/tfxo')
    blob_vec(mk_pgso(rng, seed, rng.pick(['vs', 'ps', 'cs'])), '/pgso')
    blob_vec(mutate(rng, mk_ndsr(rng, seed, rng.pick([['vs', 'ps'], ['cs'], ['vs', 'ps', 'cs', 'vs', 'ps']])), 2) if rng.chance(1, 3) else mk_ndsr(rng, seed, rng.pick([['vs', 'ps'], ['cs'], ['vs', 'ps', 'cs', 'vs', 'ps']])), '/ndsr')
# yaz0 on its own, with out-of-range back-references
for i in range(60):
    seed += 100
    raw = lcg_fill(seed, rng.pick([10, 100, 1000])) * rng.pick([1, 3])
    enc = yaz0_encode(raw, rng)
    if rng.chance(1, 3): enc = mutate(rng, enc, 2)
    emit('yaz0', {'hex': enc.hex()}, res(lambda: sha(ss.yaz0(enc))))
for i in range(40):
    seed += 100
    sarc = mk_sarc([(f'm{j}', lcg_fill(seed + j, rng.pick([0, 5, 100]))) for j in range(rng.pick([0, 1, 3]))], rng.pick([0, 1]))
    if rng.chance(1, 2): sarc = mutate(rng, sarc, 2)
    emit('sarc', {'hex': sarc.hex()}, res(lambda: [[n, sha(m)] for n, m in ss.sarc_members(sarc)]))

# ---- pixel input cover ----------------------------------------------------------------------------------------------------------------------
for i in range(30):
    seed += 100
    shs = {}
    for j in range(rng.pick([0, 1, 3, 8, 20])):
        code = bytearray(lcg_fill(seed + j, 0x80))
        for w in range(6, 14): code[0x30 + 4 * w: 0x34 + 4 * w] = bytes(rng.pick([0, 0, 0, 1, 2]) for _ in range(4))
        shs['%016x' % (seed * 31 + j * 977)] = bytes(code)
    emit('cover', {'ps': {k: v.hex() for k, v in shs.items()}}, res(lambda: ss.pixel_input_cover(shs)))

# ==== Unity ======================================================================================================================================
import lz4.block, lzma

PRIM = unityfs.PRIM
KNOWN = list(unityfs.KNOWN_TREES.items())   # [((class, hash), (root, layout))]

def rand_value(n, rng, depth=0):
    """a random Python value of the type tree node n (what unityfs._read would return for some bytes)"""
    if n.type in PRIM and not n.kids:
        t = n.type
        if t in ('float', 'double'): return rng.pick([0.0, 0.5, 1.0, 2.25, -3.0])
        if t == 'bool': return bool(rng.below(2))
        bits = struct.calcsize(PRIM[t]) * 8
        return rng.pick([0, 1, 2, 7, 19, 255 & ((1 << bits) - 1)]) if PRIM[t].isupper() or PRIM[t] == '?' else rng.pick([0, 1, -1, 5, 100, -128 if bits == 8 else -20])
    if n.type == 'string': return ''.join(rng.pick('abcXYZ_01') for _ in range(rng.below(7)))
    if n.kids and n.kids[0].type == 'Array':
        arr = n.kids[0]; el = arr.kids[1]
        cnt = 0 if depth > 5 else rng.pick([0, 1, 2, 3])
        if el.type in ('UInt8', 'char', 'SInt8') and not el.kids: return bytes(rng.below(256) for _ in range(cnt))
        return [rand_value(el, rng, depth + 1) for _ in range(cnt)]
    if n.type == 'TypelessData': return bytes(rng.below(256) for _ in range(rng.below(5)))
    return {k.name: rand_value(k, rng, depth + 1) for k in n.kids}

def ser(n, v, out):
    """the bytes of value v of node n, appended to out: the inverse of unityfs._read"""
    def align(): out.extend(bytes((-len(out)) % 4))
    if n.type in PRIM and not n.kids:
        try: out += struct.pack('<' + PRIM[n.type], v)
        except struct.error: raise RuntimeError(f'ser {n.type} {n.name} {v!r}')
    elif n.type == 'string':
        b = v.encode() if isinstance(v, str) else v
        out += struct.pack('<i', len(b)) + b
        if n.kids and n.kids[0].flags & 0x4000: align()
    elif n.kids and n.kids[0].type == 'Array':
        arr = n.kids[0]; el = arr.kids[1]
        out += struct.pack('<i', len(v))
        if el.type in ('UInt8', 'char', 'SInt8') and not el.kids: out += v if el.type != 'SInt8' else bytes(v)
        elif el.type in PRIM and not el.kids:
            try: out += struct.pack('<' + PRIM[el.type] * len(v), *v)
            except struct.error: raise RuntimeError(f'ser array {el.type} {n.name} {v!r}')
        else:
            for x in v: ser(el, x, out)
        if arr.flags & 0x4000: align()
    elif n.type == 'TypelessData': out += struct.pack('<i', len(v)) + v
    else:
        for k in n.kids: ser(k, v[k.name], out)
    if n.flags & 0x4000: align()

def conv(v):
    if isinstance(v, bool): return v
    if isinstance(v, (int, float)): return v
    if isinstance(v, str): return v
    if isinstance(v, bytes): return {'hex': v.hex()}
    if isinstance(v, list): return [conv(x) for x in v]
    if isinstance(v, dict): return {k: conv(x) for k, x in v.items()}
    raise TypeError(type(v))

for idx, ((cid, h), (root, layout)) in enumerate(KNOWN):
    tree = unityfs.known_tree(cid, h)
    for j in range(3):
        seed += 1
        r = Rng(seed * 977)
        v = rand_value(tree, r)
        b = bytearray(); ser(tree, v, b)
        # decode what the Python's own reader makes of these bytes (the same as v, bar tuples), and of a mutated copy
        for data in (bytes(b), mutate(r, bytes(b), 2)):
            rd = unityfs.Reader(data, True)
            emit('unity_obj', {'layout': idx, 'hex': data.hex(), 'le': True}, res(lambda: conv(unityfs._read(rd, tree))))

# ---- serialized files and bundles ----------------------------------------------------------------------------------------------------------------
def mk_tree_bytes(root, ver):
    strs = bytearray(); offs = {}
    def so(s):
        if s not in offs: offs[s] = len(strs); strs.extend(s.encode() + b'\0')
        return offs[s]
    flat = []
    def walk(n, lv):
        flat.append((lv, n)); [walk(k, lv + 1) for k in n.kids]
    walk(root, 0)
    nodes = bytearray()
    for i, (lv, n) in enumerate(flat):
        fmt = '<HBBIIiiiQ' if ver >= 19 else '<HBBIIiii'
        args = (1, lv, 0, so(n.type), so(n.name), n.size, i, n.flags) + ((0,) if ver >= 19 else ())
        nodes += struct.pack(fmt, *args)
    return struct.pack('<ii', len(flat), len(strs)) + bytes(nodes) + bytes(strs)

def mk_serialized(ver, types, objs, little=True):
    """types: [(class ID, hash, root Node or None, with_tree)]; objs: [(path ID, type index, bytes)]"""
    md = bytearray(b'2019.4.0f1\0'); md += struct.pack('<i', 13)
    tt = any(t[3] for t in types)
    md += struct.pack('<?', tt) + struct.pack('<i', len(types))
    for cid, h, root, with_tree in types:
        md += struct.pack('<iBh', cid, 0, -1)
        if cid == 114: md += bytes(16)
        md += h
        if tt:
            if with_tree: md += mk_tree_bytes(root, ver)
            else: md += struct.pack('<ii', 0, 0)
            if ver >= 21: md += struct.pack('<i', 0)
    md += struct.pack('<i', len(objs))
    data = bytearray(); table = []
    base_md = 16 + 4 + (28 if ver >= 22 else 0)
    # object table entries are 4-aligned from the file start: build with explicit padding
    rows = []
    for pid, ti, b in objs:
        data += bytes((-len(data)) % 8)
        rows.append((pid, len(data), len(b), ti)); data += b
    body = bytearray(md)
    pos = base_md + len(body)
    for pid, start, size, ti in rows:
        pad = (-(pos)) % 4; body += bytes(pad); pos += pad
        body += struct.pack('<q', pid) + (struct.pack('<q', start) if ver >= 22 else struct.pack('<I', start)) + struct.pack('<Ii', size, ti); pos += 8 + (8 if ver >= 22 else 4) + 8
    doff = (base_md + len(body) + 15) & ~15
    fsz = doff + len(data)
    hdr = bytearray()
    if ver >= 22:
        hdr += struct.pack('>IIII', 0, 0, ver, 0) + bytes([0 if little else 1, 0, 0, 0]) + struct.pack('>Iqqq', len(body), fsz, doff, 0)
    else:
        hdr += struct.pack('>IIII', len(body), fsz, ver, doff) + bytes([0 if little else 1, 0, 0, 0])
    assert len(hdr) == base_md
    return bytes(hdr) + bytes(body) + bytes(doff - base_md - len(body)) + bytes(data)

def mk_unity_program(rng, seed, stages):
    """a program code of Unity 2019.3 and later: a 0x60 header, then control section + code area of each stage"""
    h = [0] * 24; blocks = bytearray()
    for i in range(6):
        h[6 + i] = -1
    for st in stages:
        ctl, code = mk_shader(rng, seed + len(blocks), st)
        i = ss.NV_STAGES.index(st)
        h[6 + i], h[12 + i], h[18 + i] = len(blocks), len(ctl), len(code)
        blocks += ctl + code
    return struct.pack('<24i', *h) + bytes(blocks)

def mk_subprogram(ver, code, kw=('A', 'B')):
    b = bytearray(struct.pack('<ii', ver, 5) + bytes(12) + (bytes(4) if ver >= 201608170 else b''))
    def strs(l):
        nonlocal b
        b += struct.pack('<i', len(l))
        for s in l: b += struct.pack('<i', len(s)) + s.encode() + bytes((-len(s)) % 4)
    strs(kw)
    if 201806140 <= ver < 202012090: strs(['L'])
    b += struct.pack('<i', len(code)) + code + bytes((-len(code)) % 4)
    b += bytes(rng.below(5))    # whatever follows the code
    return bytes(b)

def mk_shader_object(rng, tree, layout_idx, ver_sub, programs, legacy=False):
    """a random Shader value of `tree` with the platform blob holding the given programs [(stage list)] as subprograms"""
    v = rand_value(tree, rng)
    subs = [mk_subprogram(ver_sub, p) for p in programs]
    head_n = 4 + 12 * len(subs) if not legacy else 4 + 8 * len(subs)
    s0 = bytearray(struct.pack('<I', len(subs)) + bytes(head_n - 4)); body = bytearray()
    for k, sp in enumerate(subs):
        off = head_n + len(body)
        if legacy: s0[4 + 8 * k:12 + 8 * k] = struct.pack('<II', off, len(sp))
        else: s0[4 + 12 * k:16 + 12 * k] = struct.pack('<III', off, len(sp), 0)
        body += sp
    seg = bytes(s0) + bytes(body)
    comp = lz4.block.compress(seg, store_size=False)
    v['platforms'] = [19]
    v['compressedBlob'] = comp
    if legacy:
        v['offsets'], v['compressedLengths'], v['decompressedLengths'] = [0], [len(comp)], [len(seg)]
    else:
        v['offsets'], v['compressedLengths'], v['decompressedLengths'] = [[0]], [[len(comp)]], [[len(seg)]]
    return v

def unity_run(d, label='/t', borrowed=None):
    """the engine's unity_shaders on a file or bundle: found, pairs, and the trees it collected"""
    ss.COMMON = {}
    ss.TREES = dict(borrowed or {})
    trees = {} if not borrowed else dict(borrowed)
    out = []
    buf = io.StringIO()
    def go():
        with contextlib.redirect_stdout(buf):
            pairs = ss.unity_shaders(d, label, out, trees)
        return {'found': found_json(out), 'pairs': sorted([list(p) for p in pairs]), 'trees': sorted([[k[0], k[1].hex(), None if v is None else conv_node(v)] for k, v in trees.items()], key=lambda x: (x[0], x[1])), 'printed': buf.getvalue().splitlines()}
    try: return {'ok': go()}
    except unityfs.NoTree as e: return {'notree': True}
    except (Exception, SystemExit) as e: return {'err': f'{type(e).__name__}: {e}'}

def conv_node(n): return [n.type, n.name, n.flags, [conv_node(k) for k in n.kids]]

# a Shader with its tree in the file (the first pass), with the tree borrowed (the second pass), in a bundle (stored, LZ4, LZMA)
shader_layouts = [i for i, ((cid, h), _) in enumerate(KNOWN) if cid == 48]
def child(n, name): return next(k for k in n.kids if k.name == name)
def elem(n): return n.kids[0].kids[1]
def mk_legacy_program(rng, seed, st):
    """before Unity 2019.3: a 0x10 header {?, ?, control length, code length}, the control section and the code area: one stage"""
    ctl, code = mk_shader(rng, seed, st)
    return bytes(8) + struct.pack('<ii', len(ctl), len(code)) + ctl + code
for rep in range(16):
    seed += 100
    r = Rng(seed)
    li = r.pick(shader_layouts)
    (cid, h), (root, layout) = KNOWN[li]
    tree = unityfs.known_tree(cid, h)
    legacy = elem(child(tree, 'offsets')).type in PRIM      # one segment per platform: offsets are plain numbers
    old_code = r.chance(1, 2) and legacy                       # the 0x10-header program code, its stage from m_ParsedForm
    ver_sub = r.pick([201806140, 202012090, 201608170])
    objs = []
    for j in range(1):
        if old_code:
            progs = [mk_legacy_program(r, seed + 3 * j + k, st) for k, st in enumerate(['vs', 'ps', r.pick(['vs', 'ps'])])]
            v = mk_shader_object(r, tree, li, ver_sub, progs, legacy=True)
            pf = child(tree, 'm_ParsedForm'); sub = elem(child(pf, 'm_SubShaders')); ps_ = elem(child(sub, 'm_Passes'))
            pass_v = rand_value(ps_, r)
            for fld, idxs in (('progVertex', [0, 2]), ('progFragment', [1])):
                sp_node = elem(child(child(ps_, fld), 'm_SubPrograms'))
                lst = []
                for ix in idxs:
                    sv = rand_value(sp_node, r); sv['m_BlobIndex'] = ix; lst.append(sv)
                pass_v[fld]['m_SubPrograms'] = lst
            sub_v = rand_value(sub, r); sub_v['m_Passes'] = [pass_v]
            v['m_ParsedForm']['m_SubShaders'] = [sub_v]
        else:
            progs = [mk_unity_program(r, seed + 11 * j + k, p) for k, p in enumerate([r.pick([['vs', 'ps'], ['vs', 'ps', 'gs'], ['cs'], ['vs']]) for _ in range(r.pick([1, 2]))])]
            v = mk_shader_object(r, tree, li, ver_sub, progs, legacy=legacy)
        b = bytearray(); ser(tree, v, b); objs.append((100 + j, 0, bytes(b)))
    ver = r.pick([17, 19, 21, 22])
    f = mk_serialized(ver, [(48, h, tree, True)], objs)
    ftree_less = mk_serialized(ver, [(48, h, tree, False)], objs)
    emit('unity_file', {'hex': f.hex(), 'label': '/Data/x.assets', 'layout': None}, unity_run(f, '/Data/x.assets'))
    if rep % 4 == 0: emit('unity_file', {'hex': ftree_less.hex(), 'label': '/Data/y.assets', 'layout': None}, unity_run(ftree_less, '/Data/y.assets'))
    emit('unity_file', {'hex': ftree_less.hex(), 'label': '/Data/y.assets', 'layout': li}, unity_run(ftree_less, '/Data/y.assets', borrowed={(cid, h): tree}))
    if rep % 2 == 0:
        mf = mutate(r, f, 2)
        emit('unity_file', {'hex': mf.hex(), 'label': '/Data/z.assets', 'layout': None}, unity_run(mf, '/Data/z.assets'))
    if rep % 5:
        continue
    # a UnityFS bundle holding the file (and a second node that is not a serialized file)
    nodes = [(f, '0.0.assets'), (b'not an asset file', 'x.resS')]
    data = b''.join(n[0] for n in nodes)
    info = bytearray(bytes(16))
    for compress in (0, 2, 1):
        blocks = []
        pos = 0
        stored = bytearray()
        chunk = 8192
        for o in range(0, len(data), chunk):
            raw = data[o:o + chunk]
            if compress == 0: c = raw; fl = 0
            elif compress == 2: c = lz4.block.compress(raw, store_size=False); fl = 2
            else:
                flt = lzma.LZMACompressor(lzma.FORMAT_RAW, filters=[{'id': lzma.FILTER_LZMA1, 'dict_size': 1 << 16, 'lc': 3, 'lp': 0, 'pb': 2}])
                c = bytes([3 + 9 * 0 + 45 * 2]) + struct.pack('<I', 1 << 16) + flt.compress(raw) + flt.flush(); fl = 1
            blocks.append((len(raw), len(c), fl)); stored += c
        info = bytearray(bytes(16)) + struct.pack('>i', len(blocks))
        for u, c, fl in blocks: info += struct.pack('>IIH', u, c, fl)
        info += struct.pack('>i', len(nodes)); off = 0
        for nd, name in nodes: info += struct.pack('>qqI', off, len(nd), 4) + name.encode() + b'\0'; off += len(nd)
        info = bytes(info)
        info_c = lz4.block.compress(info, store_size=False)
        head = b'UnityFS\0' + struct.pack('>I', 6) + b'5.x.x\0' + b'2019.4.0f1\0'
        tail = struct.pack('>qIII', 0, len(info_c), len(info), 2)
        body = head + tail + info_c + bytes(stored)
        bundle = body[:len(head)] + struct.pack('>qIII', len(body), len(info_c), len(info), 2) + body[len(head) + 20:]
        emit('unity_file', {'hex': bundle.hex(), 'label': '/Data/b.unity3d', 'layout': None, 'bundle': True, 'compress': compress}, unity_run(bundle, '/Data/b.unity3d'))

# ==== shader_device.live_package ================================================================================================================
if os.environ.get('SHADER_DEVICE_DIR'):
    sys.path.insert(0, os.environ['SHADER_DEVICE_DIR'])
    import shader_device as sd
    engine_root = os.path.abspath(tools + '/../../..')
    for rep in range(25):
        seed += 100
        entries = []
        for j in range(rng.pick([1, 2, 4, 6])):
            seed += 1
            st = rng.pick(['vs', 'ps', 'cs', 'vs', 'ps'])
            ctl, code = mk_shader(rng, seed, st, nvn2=rng.chance(1, 6))
            if rng.chance(1, 8): ctl = mutate(rng, ctl, 1)
            if rng.chance(1, 10): code = mutate(rng, code, 1)
            if rng.chance(1, 12) and len(ctl) > 0x760: ctl = bytearray(ctl); ctl[0x748:0x754] = bytes(12); ctl = bytes(ctl)   # a thread group of 0
            try: key = ss.skey(ctl, code)
            except Exception: key = '0' * 16
            if rng.chance(1, 8): key = '%016x' % rng.next()
            entries.append((key, ctl, code))
        if rng.chance(1, 5): entries.append((entries[0][0], entries[0][1], entries[0][2]))    # the same key twice
        out_dir = tempfile.mkdtemp()
        def go():
            ok, bad = sd.live_package(entries, prof + '/0100TESTTEST0000', '0100TESTTEST0000', out_dir + '/pkg', out_dir + '/tables', log=lambda l: None, engine=engine_root)
            files = {}
            for sub in ('pkg', 'tables'):
                for n in sorted(os.listdir(f'{out_dir}/{sub}')): files[f'{sub}/{n}'] = sha(open(f'{out_dir}/{sub}/{n}', 'rb').read())
            return {'ok': ok, 'bad': bad, 'files': files}
        r = res(go)
        emit('live', {'entries': [[k, c.hex(), d.hex()] for k, c, d in entries]}, r)
