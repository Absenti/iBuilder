#!/usr/bin/env python3
"""Fixtures for what the rebased engine (nxnative a3663659 + the NX2Apple patches) changed in `shader_split.py` since the Rust port was written (`docs/shaders/SHADERSPLIT-RUST.md` section 10):

  SHADER_DEVICE_DIR=<folder with shader_device.py> PYTHONDONTWRITEBYTECODE=1 python3 -I tools/gen_rebase.py <engine root (the folder that holds dm/)> <games out folder> <vectors out .jsonl.gz>

* NVN2 ray tracing (`'rt'` stage: control +0x56 = 8; `check` parses the module metadata with `dmsh_rt.parse_module`; `same_shader`'s RT rule; the BNSH program's 7th pointer, +0x38);
* Civilization VI's CIVBSF revision 0x1118 (platform descriptors, NVBC) and the revision check; Burnout Paradise Remastered's bnd2 version 2 (three blocks per resource);
* Phyre's loose programs and effects (`glslc_stage`, Unravel TWO's headerless compute, the RYHP fall-through of `scan_blob`); Unravel TWO's KIT archives (`kit.py`, `kit_jobs`);
* `main`'s `uniq_keys` (the first copy of a key wins; a copy that differs is `red()`).

`<games out>/rebase-*/` are games as `tools/gen_games.py` writes them (`expected.json` = the engine's `sha256` of every file it writes); `<games out>/err-*/` end in an error, whose text is in
`expected.json`. `vectors-rebase.jsonl.gz` holds one function call each (`rt_check`, `rt_same`, `fn_*`: the input bytes and the engine's value or its error text, exactly).
Nothing here comes from a game: every byte is a PRNG's or zeros.
"""
import sys, os, json, struct, hashlib, zlib, gzip, base64, io, contextlib
here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, 'gen_games.py')).read()
games_out, vec_out = os.path.abspath(sys.argv[2]), os.path.abspath(sys.argv[3])
exec(src[:src.index("rng = Rng(0xfeed)")])   # engine, out_root = argv[1], argv[2]: the engine (`ss`), shader_device (`sd`), `make_game`, `mk_glslc`, `mk_bnsh`, the PRNGs
import kit as kitmod, dmsh_rt
import lz4.block

def u32(v): return struct.pack('<I', v)
def u64(v): return struct.pack('<Q', v)
def b64(b): return base64.b64encode(b).decode()
def sha(b): return hashlib.sha256(b).hexdigest()
def res(fn):
    """('ok', value) or ('err', text): the engine's own text for a red() / exception"""
    try: return {'ok': fn()}
    except (Exception, SystemExit) as e: return {'err': str(e) if isinstance(e, SystemExit) else f'{type(e).__name__}: {e}'}

R = Rng(0x7e57)

# ---- RTCore module metadata (dmsh_rt.parse_module's own layout, written the way it reads) --------------------------------------------------------
class W:
    """the serializer's writer: positions are absolute in the metadata, as `_Reader.align` counts them"""
    def __init__(s, buf): s.b = buf
    def align(s, a): s.b += bytes((-len(s.b)) & (a - 1))
    def u8(s, v): s.b.append(v & 255)
    def u32(s, v): s.b += u32(v)
    def u64(s, v): s.b += u64(v)
    def take(s, raw): s.b += raw
    def count(s, n): s.align(8); s.b += u64(n)
    def vector(s, raw, stride): s.count(len(raw) // stride); s.b += raw
    def name(s, text): s.vector(text.encode(), 1)

def mk_region(w, name, rr):
    w.name(name); w.align(4); w.take(b''.join(u32(rr.below(5)) for _ in range(17))); w.align(8)
    for _ in range(8): w.b += u64(rr.below(100))
    n = rr.pick([0, 0, 1]); w.count(n)
    for i in range(n): w.name(f'access{i}'); w.align(4); w.take(lcg_fill(rr.next(), 64))
    n = rr.pick([0, 1]); w.count(n)
    for _ in range(n): w.align(4); w.u32(7); w.align(8); w.b += u64(9)
    w.u8(1); w.align(4); w.u32(3); w.vector(lcg_fill(rr.next(), 8 * rr.pick([0, 1])), 8); w.take(b'\x01\x02')

def mk_entry(w, name, regions, rr):
    w.name(name); w.align(4)
    w.take(u32(len(regions)) + b''.join(u32(rr.below(50)) for _ in range(8)))
    w.take(bytes(9)); w.align(4); w.u32(1)
    w.count(len(regions))
    for rn in regions: mk_region(w, rn, rr)
    w.align(4); w.u32(2); w.vector(lcg_fill(rr.next(), 12 * rr.pick([0, 1])), 12); w.align(4); w.take(bytes(6)); w.align(4); w.u32(3)

def mk_resource(w, name, rr):
    w.name(name); w.align(4); w.u32(5); w.align(8)
    al = rr.pick([1, 4, 16, 64]); w.b += u64(rr.pick([0, 32, 100])) + u64(al) + u64(al * rr.below(5))
    w.vector(lcg_fill(rr.next(), rr.pick([0, 7])), 1); w.align(4); w.u32(1); w.align(8); w.b += u64(3) + u32(4)

def mk_rt(seed, nentries=None):
    """(control section, code area) of a valid NVN2 ray-tracing shader: dmsh_rt.parse_module accepts its metadata"""
    rr = Rng(seed)
    counts = [rr.pick([1, 2]) for _ in range(nentries or rr.pick([1, 2]))]
    nreg = sum(counts)
    names = [f'region{i}' for i in range(nreg)]
    sizes = [16 * rr.pick([2, 3, 4]) for _ in range(nreg)]
    starts, o = [], 0
    for s in sizes: starts.append(o); o += s
    code = lcg_fill(seed + 5, o)
    relocs = []
    for _ in range(rr.pick([0, 1, 2])):
        i = rr.below(nreg); kind = rr.pick([-56, -56, 3])
        relocs.append(struct.pack('<QQQiIII', 16 * rr.below(sizes[i] // 16), rr.below(9), rr.below(9), kind, starts[i], rr.below(nreg) if kind < 0 else rr.below(len(code)), 0))
    b = bytearray(56)
    def put_u32s(vals):
        at = len(b); b.extend(b''.join(u32(v) for v in vals)); return at
    starts_off, sizes_off = put_u32s(starts), put_u32s(sizes)
    names_off = len(b); b.extend(bytes(4 * nreg))
    sel_off = put_u32s([rr.pick([0, 1, 0xffffffc8]) for _ in range(nreg)])
    ec_off = put_u32s(counts)
    for i, n in enumerate(names):
        struct.pack_into('<I', b, names_off + 4 * i, len(b)); b.extend(n.encode() + b'\0')
    while len(b) % 8: b.append(0)
    reloc_off = len(b); b.extend(b''.join(relocs))
    while len(b) % 8: b.append(0)
    inner_off = len(b)
    w = W(b)
    w.take(lcg_fill(seed + 6, 16)); w.vector(b'fp', 1); w.align(8); w.take(lcg_fill(seed + 7, 0xc5)); w.vector(b'opts', 1); w.take(b'abc'); w.align(4); w.u32(1)
    w.count(0); w.align(4); w.u32(2); w.name('module'); w.align(8); w.take(lcg_fill(seed + 8, 0x24)); w.align(4); w.u32(3)
    n = rr.pick([0, 1]); w.count(n)
    for _ in range(n): w.align(4); w.take(bytes(8)); w.vector(bytes(8), 8); w.vector(bytes(4), 4); w.align(4); w.take(bytes(9)); w.vector(bytes(24), 24); w.vector(bytes(4), 4)
    w.count(len(counts)); k = 0
    for i, c in enumerate(counts): mk_entry(w, f'entry{i}', names[k:k + c], rr); k += c
    n = rr.pick([0, 1]); w.count(n)
    for i in range(n): w.name(f'opaque{i}'); w.align(4); w.take(bytes(12))
    w.u8(0)
    for _ in range(2):
        n = rr.pick([0, 1]); w.count(n)
        for i in range(n): w.name(f'k{i}'); w.align(8); w.b.extend(u64(5))
    n = rr.pick([0, 1]); w.count(n)
    for i in range(n):
        w.name(f'set{i}'); m = rr.pick([0, 1, 2]); w.count(m)
        for j in range(m): w.name(f'v{j}')
    w.align(4); w.u32(7); w.u32(8)
    n = rr.pick([0, 1, 2]); w.count(n)
    for i in range(n): mk_resource(w, f'res{i}', rr)
    w.u8(1); w.align(4); w.u32(9); w.vector(b'end', 1)
    struct.pack_into('<14I', b, 0, 0, 0, len(b) - inner_off, inner_off, nreg, starts_off, sizes_off, names_off, sel_off, len(relocs), reloc_off, len(counts), ec_off, 0)
    return rt_ctl(bytes(b), code), code

def rt_ctl(meta, code, pmeta=0x68, ncode=None, stage=8):
    ctl = bytearray(pmeta) + meta
    ctl[:4] = ss.CTL_MAGIC; ctl[4:8] = b'\xc9\0\0\0'; ctl[0x30:0x34] = u32(len(ctl)); ctl[0x44:0x48] = u32(len(code)); ctl[0x56] = stage
    ctl[0x5c:0x68] = u32(len(meta)) + u32(pmeta) + u32(len(code) if ncode is None else ncode)
    return bytes(ctl)

# ---- containers -------------------------------------------------------------------------------------------------------------------------------
def glslc_pairs(pairs):
    """one GLSLC output (NVN, not NVN2) holding the given (control, code) pairs as GPU code sections"""
    first, stride = 0x90, 0x90
    n = len(pairs); hdr = first + stride * n
    data, secs = bytearray(), []
    for ctl, code in pairs:
        secs.append((len(ctl) + len(code), hdr + len(data), len(ctl), len(code))); data += ctl + code
    d = bytearray(hdr + len(data))
    d[0:4] = b'\x91\x68\x86\x19'; d[0x44:0x48] = u32(len(d)); d[0x4c:0x50] = u32(n)
    for i, (ssz, soff, cn, kn) in enumerate(secs):
        s = first + stride * i
        d[s:s + 12] = u32(ssz) + u32(soff) + u32(0); d[s + 0x30:s + 0x40] = u32(0) + u32(cn) + u32(kn) + u32(cn)
    d[hdr:] = data
    return bytes(d)

def shd(seed, st): return mk_shader(Rng(seed), seed, st)   # the code length is the seed's own, so two calls give one shader

def mk_civ(version, shaders, bad_kind=None):
    """Firaxis's CIVBSF: 0x90-byte header, the shader table, then per shader its descriptors and data. Revision 0x1118: 0x48-byte entries -> four {u64 offset, u32 size, u32 platform} descriptors, platform 3 =
    NVBC {tag, u64 output length, u64 reflection length} + GLSLC output + reflection; revision 0x1218: 0x50-byte entries -> {u64 offset, u64 length} -> {0xff00ff00, code length, control length, 0}, code, control"""
    stride = {0x1118: 0x48, 0x1218: 0x50}.get(version, 0x50)
    n = len(shaders); table = 0x90
    body = bytearray(); fix = []
    d = bytearray(table + stride * n)
    for i, (ctl, code) in enumerate(shaders):
        entry = table + stride * i
        if version == 0x1118:
            out = glslc_pairs([(ctl, code)]); refl = lcg_fill(i + 77, 20)
            nvbc = b'NVBC\0\0\0\0' + u64(len(out)) + u64(len(refl)) + bytes(0x28 - 0x18) + out + refl
            desc_at = len(d) + len(body); body += bytes(0x40)
            payloads = [lcg_fill(i + 90 + p, 10 + p) for p in range(3)] + [nvbc]
            for p, pl in enumerate(payloads):
                struct.pack_into('<QII', body, desc_at - len(d) + 16 * p, len(d) + len(body), len(pl), p); body += pl
            d[entry + 8:entry + 16] = u64(desc_at)
        else:
            data = struct.pack('<4I', 0xff00ff00, len(code), len(ctl), 0) + code + ctl
            desc_at = len(d) + len(body); body += u64(desc_at + 16) + u64(len(data)); body += data
            d[entry + 8:entry + 16] = u64(desc_at)
    d += body
    d[0:8] = ss.CIVBSF; d[8:12] = u32(version); d[0xc:0x10] = u32(len(d)); d[0x20:0x28] = u64(table); d[0x68:0x6c] = u32(n)
    return bytes(d)

def mk_bnd2(version, resources, compress=True, junk_blocks=True):
    """Criterion's Bundle 2: `resources` = [(id, type, [block bytes per block])]; version 2 has 3 blocks per resource and 0x40-byte entries, version 3 has 4 and 0x50"""
    B, esz = {2: (3, 0x40), 3: (4, 0x50)}[version]
    n = len(resources); eoff = 0x30
    data_base = eoff + n * esz
    d = bytearray(data_base); pos = 0; blob = bytearray()
    d[4:8] = u32(version); d[0x10:0x14] = u32(n); d[0x14:0x18] = u32(eoff)
    for k in range(B): d[0x18 + 4 * k:0x1c + 4 * k] = u32(data_base)
    d[0x18 + 4 * B:0x1c + 4 * B] = u32(1 if compress else 0)
    for i, (rid, typ, blocks) in enumerate(resources):
        e = eoff + i * esz
        d[e:e + 8] = u64(rid)
        for k in range(B):
            raw = blocks[k] if k < len(blocks) else b''
            stored = zlib.compress(raw) if compress and raw else raw
            struct.pack_into('<I', d, e + 0x10 + 4 * k, len(raw)); struct.pack_into('<I', d, e + 0x10 + 4 * B + 4 * k, len(stored) | (0x30000000 if stored else 0))
            struct.pack_into('<I', d, e + 0x10 + 8 * B + 4 * k, len(blob)); blob += stored
        struct.pack_into('<I', d, e + 0x14 + 12 * B, typ)
    return bytes(d) + bytes(blob)

def phyre_code_ctl(seed, st):
    ctl, code = shd(seed, st)
    return ctl, code

def mk_headerless_cs(seed, with_ctl_magic=False):
    ctl, code = shd(seed, 'cs')
    code = bytearray(code); code[:4] = b'\x11\x07\x20\xe2'   # no 0x12345679 header: the instructions start at +0
    if with_ctl_magic: code[0x40:0x44] = ss.CTL_MAGIC
    return bytes(ctl), bytes(code)

def mk_ryhp(pairs, junk=64, lead=b'RYHP'):
    """a Phyre effect: the head, then for each pair its code area immediately followed by its control section"""
    d = bytearray(lead + lcg_fill(len(pairs) + 3, junk - len(lead)))
    for ctl, code in pairs: d += code + ctl + lcg_fill(len(d), 5)
    return bytes(d)

def mk_phyre_loose(pairs):
    """loose Phyre programs (FFX / X-2): two lengths, then the code (a graphics header at +8), the control section"""
    ctl, code = pairs[0]
    return u32(len(code)) + u32(len(ctl)) + code + ctl

def mk_kit(members, parts=2, name='x.kit'):
    """(index bytes, {part: bytes}); members = [(decoded bytes, compressed)]: compressed ones are chained LZ4 blocks of 64 KB with the previous 64 KB as the dictionary"""
    pbuf = [bytearray() for _ in range(parts)]
    recs = bytearray()
    for i, (data, comp) in enumerate(members):
        part = i % parts
        if comp:
            out = bytearray()
            for o in range(0, len(data), 0x10000):
                chunk = data[o:o + 0x10000]
                blk = lz4.block.compress(chunk, store_size=False, mode='high_compression', dict=bytes(data[max(0, o - 0x10000):o])) if o else lz4.block.compress(chunk, store_size=False, mode='high_compression')
                out += u32(len(blk)) + blk
            stored = bytes(out)
        else: stored = data
        off = len(pbuf[part]); pbuf[part] += stored
        recs += struct.pack('<IHIHQQ20sII', 8 if comp else 0, part, off, 0, len(stored), len(data), hashlib.sha1(data).digest(), 0, 0)
    idx = bytearray(0x25); idx[:4] = b'kit!'; idx[0x10:0x18] = u64(0); idx[0x18:0x1c] = u32(len(members))
    return bytes(idx + recs), {p: bytes(b) for p, b in enumerate(pbuf)}

def write_kit(files, rel, index, parts):
    files[rel] = index
    for p, b in parts.items(): files[f'{rel}.{p}'] = b

def mk_bnsh_rt(seed, rts, stage_sets=({'vs', 'ps'},)):
    """a BNSH of NVN2 shaders: per variation graphics stages as mk_bnsh writes them, and a ray-tracing stage (the program's 7th pointer, +0x38) when rts[v] is (control, code)"""
    nvar = len(rts)
    d = bytearray(0x100); d[0:8] = b'BNSH\0\0\0\0'; d[0x60:0x64] = b'grsc'
    pvar = 0x100; d[0x7c:0x80] = u32(nvar); d[0x80:0x88] = u64(pvar); d += bytes(0x40 * nvar)
    def stage(ctl, code):
        sc = len(d); d.extend(bytes(0x20)); pctl = len(d); d.extend(ctl); pcode = len(d); d.extend(code)
        d[sc + 8:sc + 0x20] = struct.pack('<QQII', pctl, pcode, len(code), len(ctl)); return sc
    for v in range(nvar):
        prog = len(d); d.extend(bytes(0x50)); d[pvar + 0x40 * v + 0x10:pvar + 0x40 * v + 0x18] = u64(prog)
        for si, st in enumerate(ss.STAGES):
            if st not in stage_sets[v % len(stage_sets)]: continue
            ctl, code = mk_shader(R, seed + 100 * v + si, st, True)
            d[prog + 8 + 8 * si:prog + 16 + 8 * si] = u64(stage(ctl, code))
        if rts[v] is not None: d[prog + 0x38:prog + 0x40] = u64(stage(*rts[v]))
    d[0x1c:0x20] = u32(len(d))
    return bytes(d)

def found_json(found): return [[w, st, sha(c), sha(k), len(c), len(k)] for w, st, c, k in found]

# ---- games --------------------------------------------------------------------------------------------------------------------------------------
ERR = []
class _Stop(Exception): pass
_split = sd.split
def _wrapped(*a, **k):
    try: return _split(*a, **k)
    except RuntimeError as e: ERR.append(str(e)); raise _Stop()
sd.split = _wrapped

def game(name, files, **kw):
    """make_game; paths in what the engine logged (a KIT member's message names its file by the path the engine opened) are written as @ROMFS@; a split that ends in an error writes `{"error": text}`"""
    g = f'{games_out}/{name}'
    ERR.clear()
    try: make_game(name, files, **kw)
    except _Stop:
        json.dump({'error': ERR[0].replace(g + '/RomFS', '@ROMFS@')}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
        print(name, 'error:', ERR[0].replace(g, '')[:100]); return
    e = json.load(open(g + '/expected.json'))
    e['log'] = [l.replace(g + '/RomFS', '@ROMFS@') for l in e['log']]
    json.dump(e, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)

rng = Rng(0xfeed)   # make_game's padding between the GLSLC outputs in main
os.makedirs(games_out, exist_ok=True)
def ctl_tail(seed, tail, own=0x880, n=0x890, st='vs'):
    """a control section of n bytes whose own length (+0x20) says `own`, with `tail` as its last bytes: two of them with different tails are the same key, and the same shader only up to `own`"""
    ctl, code = shd(seed, st)
    c = bytearray(lcg_fill(seed + 9, n)); c[:4] = ss.CTL_MAGIC; c[0x20:0x24] = u32(own); c[0x704:0x708] = u32(len(code))
    for i in range(4, n):
        if bytes(c[i:i + 4]) == ss.CTL_MAGIC: c[i] ^= 0x55
    c[n - len(tail):] = tail
    return bytes(c), code

rt1, rt2 = mk_rt(171), mk_rt(172, 2)
fx_pairs = [shd(140, 'vs'), shd(141, 'ps'), mk_headerless_cs(142)]
nofx = b'RYHP' + lcg_fill(7, 60) + mk_bnsh(R, 160, 1, [{'vs', 'ps'}])
files = {
    'Data/civ6.bsf': mk_civ(0x1118, [shd(100, 'vs'), shd(101, 'ps'), shd(102, 'cs')]),
    'Data/civ7.bsf': mk_civ(0x1218, [shd(110, 'vs'), shd(111, 'ps')]),
    'Data/burn.bndl': mk_bnd2(2, [(1, 8, [b'', glslc_pairs([shd(120, 'vs'), shd(121, 'ps')])]), (2, 3, [lcg_fill(4, 100), b'', lcg_fill(5, 50)])]),
    'Data/nfs.bndl': mk_bnd2(3, [(7, 8, [b'', glslc_pairs([shd(130, 'ps')]), b'', b''])], compress=False),
    'Data/fx.ryhp': mk_ryhp(fx_pairs),
    'Data/loose.bin': mk_phyre_loose([shd(150, 'ps')]),
    'Data/nofx.ryhp': nofx,
    'Pack/rt.bnsh': mk_bnsh_rt(170, [rt1]),
    'Pack/rt2.bnsh': mk_bnsh_rt(175, [rt2, None, rt1], stage_sets=({'vs', 'ps'}, {'ps'})),
}
dupA, dupB = ctl_tail(200, b'AAAAAAAA'), ctl_tail(200, b'BBBBBBBB')
game('rebase-misc', files, main_shaders=[glslc_pairs([dupA]), glslc_pairs([shd(210, 'ps')]), glslc_pairs([dupB])])
dA, dB = ctl_tail(220, b'AAAAAAAA', own=0x890), ctl_tail(220, b'BBBBBBBB', own=0x890)
game('err-main-dup', {'Data/a.bnsh': mk_bnsh(R, 230, 1, [{'vs', 'ps'}])}, main_shaders=[glslc_pairs([dA]), glslc_pairs([dB])])

# KIT: an archive of two parts and one of one, a file named .kit that is no archive, members compressed (several 64 KB blocks) and stored
ry = mk_ryhp(fx_pairs[:2])
members = [(ry, True), (b'asset text ' * 14000, True), (mk_bnsh(R, 300, 1, [{'vs', 'ps'}]), False), (mk_ryhp([fx_pairs[2]]), True), (lcg_fill(9, 500), False)]
files = {}
write_kit(files, 'Data/x.kit', *mk_kit(members, 2))
write_kit(files, 'Pack/y.kit', *mk_kit([(mk_ryhp([shd(310, 'ps')]), False), (mk_ryhp([shd(311, 'vs')]), True)], 1))
files['Data/fake.kit'] = mk_bnsh(R, 320, 1, [{'vs', 'ps'}])
files['Data/plain.bnsh'] = mk_bnsh(R, 330, 1, [{'ps'}])
game('rebase-kit', files)

# members that cannot be read leave their job's shaders out ([RED] ... (its shaders are left out)): one defect per archive
def kit_with(mut_index=None, mut_part=None, comp=True, data=None):
    members = [(data or mk_ryhp([shd(400, 'vs')]), comp), (mk_ryhp([shd(401, 'ps')]), False)]
    idx, parts = mk_kit(members, 1)
    idx, p0 = bytearray(idx), bytearray(parts[0])
    if mut_index: mut_index(idx)
    if mut_part: mut_part(p0)
    return bytes(idx), {0: bytes(p0)}
files = {}
write_kit(files, 'Data/ok.kit', *kit_with())
write_kit(files, 'Data/sha.kit', *kit_with(mut_index=lambda i: i.__setitem__(slice(0x25 + 28, 0x25 + 29), bytes([i[0x25 + 28] ^ 1]))))
write_kit(files, 'Data/lz4len.kit', *kit_with(mut_part=lambda p: p.__setitem__(slice(0, 4), u32(0x7fff))))
write_kit(files, 'Data/lz4zero.kit', *kit_with(mut_part=lambda p: p.__setitem__(slice(0, 4), u32(0))))
game('rebase-kit-bad', files)

def kit_idx_err(name, mut, parts=None):
    idx, pp = mk_kit([(mk_ryhp([shd(410, 'vs')]), True), (mk_ryhp([shd(411, 'ps')]), False)], 1)
    idx = bytearray(idx); mut(idx)
    files = {'Data/a.kit': bytes(idx), 'Data/b.bnsh': mk_bnsh(R, 420, 1, [{'ps'}])}
    for p, b in pp.items():
        if parts is None or p in parts: files[f'Data/a.kit.{p}'] = b
    game(name, files)
kit_idx_err('err-kit-dir', lambda i: i.__setitem__(slice(0x18, 0x1c), u32(1000)))
kit_idx_err('err-kit-high', lambda i: i.__setitem__(slice(0x25 + 10, 0x25 + 12), b'\x01\x00'))
kit_idx_err('err-kit-nopart', lambda i: i.__setitem__(slice(0x25 + 4, 0x25 + 6), b'\x05\x00'))
kit_idx_err('err-kit-outside', lambda i: i.__setitem__(slice(0x25 + 12, 0x25 + 20), u64(10 ** 6)))
kit_idx_err('err-kit-header', lambda i: i.__delitem__(slice(0x20, len(i))))

# ---- vectors ------------------------------------------------------------------------------------------------------------------------------------
vecs = []
V = Rng(0x7ec7)

def damage(d, rng, fix=None, head=48):
    """one kind of damage: a bit flip (half of them in the first `head` bytes), a byte set, a cut, a deleted or inserted byte, a run of bytes; `fix` repairs a length field some of the time"""
    d = bytearray(d); k = rng.below(9)
    if not d: return bytes(d)
    near = lambda: rng.below(min(len(d), head)) if rng.chance(1, 2) else rng.below(len(d))
    if k <= 2:
        for _ in range(1 + rng.below(2)): d[near()] ^= 1 << rng.below(8)
    elif k == 3: d[near()] = rng.below(256)
    elif k == 4: d = d[:rng.below(len(d) + 1)]
    elif k == 5: del d[near()]
    elif k == 6: d.insert(near(), rng.below(256))
    elif k == 7:
        i = near(); d[i:i + 4] = lcg_fill(rng.next(), 4)
    else: d[near()] = rng.pick([0, 0xff, 0x7f, 0x80, 1])
    if fix and rng.chance(1, 2): d = bytearray(fix(d))
    return bytes(d)

# rt: `check` of a ray-tracing control section, `nvn2_stage`, and `same_shader`'s RT rule
def mutate_rt(ctl, code, rng):
    ctl = bytearray(ctl); code = bytearray(code); k = rng.below(10)
    if k == 0: return bytes(ctl), bytes(code)
    if k == 1: ctl[0x68 + rng.below(56)] ^= 1 << rng.below(8)                                  # the outer header
    elif k == 2: ctl[0x68 + rng.below(len(ctl) - 0x68)] ^= 1 << rng.below(8)                     # anywhere in the module
    elif k == 3: ctl[0x56] = rng.pick([0, 5, 6, 7, 8, 9])
    elif k == 4: ctl[0x5c:0x60] = u32(rng.pick([0, 1, 56, len(ctl) - 0x68 - 1, len(ctl), 0x7fffffff]))
    elif k == 5: ctl[0x60:0x64] = u32(rng.pick([0, 8, 0x60, 0x68, 0x6c, 0x70, len(ctl), 0xffffffff]))
    elif k == 6: ctl[0x64:0x68] = u32(rng.pick([0, len(code) - 16, len(code) + 1, len(code)]))
    elif k == 7: code = code[:rng.pick([0, 8, len(code) - 1, len(code) - 16, 16])]
    elif k == 8: ctl = ctl[:rng.pick([0x48, 0x56, 0x57, 0x67, 0x68, 0x70, len(ctl) - 1, len(ctl) - 40])]
    else:
        for _ in range(3): ctl[0x68 + rng.below(len(ctl) - 0x68)] = rng.below(256)
    return bytes(ctl), bytes(code)

for i in range(420):
    seed = 1 + i % 37
    ctl, code = mk_rt(seed)
    if i >= 40: ctl, code = mutate_rt(ctl, code, V)
    out = {'check': res(lambda: ss.check('t', 'rt', ctl, code)), 'stage': ss.nvn2_stage(ctl, code), 'glslc_stage': res(lambda: ss.glslc_stage(ctl, code)), 'skey': res(lambda: ss.skey(ctl, code)),
           'nvn2': ss.nvn2_ctl(ctl)}
    out['check_other'] = {st: res(lambda: ss.check('t', st, ctl, code)) for st in ('vs', 'cs')}
    vecs.append({'kind': 'rt_check', 'in': {'ctl': b64(ctl), 'code': b64(code)}, 'out': out})

for i in range(160):
    seed = 1 + i % 23
    a, ca = mk_rt(seed)
    b, cb = bytearray(a), ca
    k = V.below(8)
    if k == 0: pass
    elif k == 1: b[0x30:0x34] = u32(V.below(2 ** 32))                                              # the declared length: masked
    elif k == 2: b[0x68 + V.below(len(b) - 0x68)] ^= 1                                              # RT metadata: never discarded
    elif k == 3: b += b'\0'                                                                          # longer
    elif k == 4: b[0x30] ^= 1; b[0x34] ^= 1                                                          # masked and not masked
    elif k == 5: b[0x56] = 5                                                                         # not RT any more: the older rule
    elif k == 6: b[0x4a] ^= 1                                                                        # inside the key's first 0x48? (0x4a is outside)
    else: b = bytearray(a); b[0x10] ^= 1
    b = bytes(b)
    if V.chance(1, 6): cb = bytes(ca[:-1]) + bytes([ca[-1] ^ 1])
    vecs.append({'kind': 'rt_same', 'in': {'a': b64(a), 'ca': b64(ca), 'b': b64(b), 'cb': b64(cb)},
                 'out': {'same': res(lambda: ss.same_shader((a, ca), (b, cb))), 'same_ba': res(lambda: ss.same_shader((b, cb), (a, ca))), 'ka': ss.skey(a, ca), 'kb': res(lambda: ss.skey(b, cb))}})
# the non-RT rules (an NVN2 control without RT, an NVN control) keep working through the new first test
for i in range(60):
    a, ca = mk_shader(V, 500 + i, V.pick(['vs', 'ps', 'cs']), nvn2=bool(i % 2))
    b = bytearray(a); 
    if V.chance(1, 2): b[V.below(len(b))] ^= 1 << V.below(8)
    b = bytes(b)
    vecs.append({'kind': 'rt_same', 'in': {'a': b64(a), 'ca': b64(ca), 'b': b64(b), 'cb': b64(ca)},
                 'out': {'same': res(lambda: ss.same_shader((a, ca), (b, ca))), 'same_ba': res(lambda: ss.same_shader((b, ca), (a, ca))), 'ka': ss.skey(a, ca), 'kb': res(lambda: ss.skey(b, ca))}})

def fn_vec(kind, d, call, fix=None, **extra):
    vecs.append({'kind': kind, 'in': {'b64': b64(d), **extra}, 'out': res(call)})

def fix_civ(d): d = bytearray(d); d[0xc:0x10] = u32(len(d)); return d

# BNSH with the ray-tracing descriptor
bn = [mk_bnsh_rt(600, [mk_rt(601)]), mk_bnsh_rt(610, [mk_rt(611), None, mk_rt(612, 2)], stage_sets=({'vs', 'ps'}, {'ps'}, {'cs'})), mk_bnsh(R, 620, 2, [{'vs', 'ps'}, {'ps'}]), mk_bnsh_rt(630, [None])]
for d in bn:
    fn_vec('fn_bnsh', d, lambda: found_json(ss.bnsh_shaders(d, 0, '/t')))
    for _ in range(100):
        m = damage(d, V, head=0x200)
        fn_vec('fn_bnsh', m, lambda: found_json(ss.bnsh_shaders(m, 0, '/t')))
# the program's RT pointer aimed at things
for ptr in (0, 1, 0x20, 0x100, 2 ** 40, 2 ** 64 - 1):
    d = bytearray(bn[0]); prog = struct.unpack_from('<Q', d, 0x100 + 0x10)[0]; d[prog + 0x38:prog + 0x40] = u64(ptr); d = bytes(d)
    fn_vec('fn_bnsh', d, lambda: found_json(ss.bnsh_shaders(d, 0, '/t')))

# CIVBSF, both revisions
cv = [mk_civ(0x1118, [shd(700, 'vs'), shd(701, 'ps')]), mk_civ(0x1118, [shd(702, 'cs')]), mk_civ(0x1218, [shd(710, 'vs'), shd(711, 'ps'), shd(712, 'cs')]), mk_civ(0x1118, []), mk_civ(0x1318, [shd(720, 'vs')])]
for d in cv:
    fn_vec('fn_civbsf', d, lambda: found_json(ss.civbsf_shaders(d, '/t')))
    for _ in range(90):
        m = damage(d, V, fix=fix_civ, head=0x80)
        fn_vec('fn_civbsf', m, lambda: found_json(ss.civbsf_shaders(m, '/t')))

# bnd2, versions 2 and 3, compressed and stored
gl = glslc_pairs([shd(800, 'vs')])
bd = [mk_bnd2(2, [(1, 8, [b'', gl]), (2, 5, [lcg_fill(8, 60), b'', lcg_fill(9, 30)])]), mk_bnd2(3, [(7, 8, [b'', gl, b'', b'']), (8, 1, [b'', b'', b'abc' * 9, b'z'])], compress=False),
      mk_bnd2(2, [(1, 8, [b'', gl])], compress=False), mk_bnd2(3, [(3, 8, [b'', gl, b'', b''])]), (lambda x: x[:4] + u32(4) + x[8:])(mk_bnd2(3, [(3, 8, [b'', gl, b'', b''])]))]   # a version nothing reads
def bnd2_vec(d):
    vecs.append({'kind': 'fn_bnd2', 'in': {'b64': b64(d)}, 'out': {'resources': res(lambda: [[r, t, k, sha(x), len(x)] for r, t, k, x in ss.bnd2_resources(d, '/t')]), 'shaders': res(lambda: found_json(ss.bnd2_shaders(d, '/t')))}})
for d in bd:
    bnd2_vec(d)
    for _ in range(90): bnd2_vec(damage(d, V, head=0x80))

# Phyre: RYHP effects, loose programs, headerless compute, and `scan_blob` (the RYHP fall-through)
cs_ok, cs_magic = mk_headerless_cs(900), mk_headerless_cs(901, True)
ph = [mk_ryhp([shd(910, 'vs'), shd(911, 'ps'), cs_ok]), mk_ryhp([cs_magic]), mk_ryhp([cs_ok], lead=b'XXXX'), mk_phyre_loose([shd(912, 'ps')]), mk_phyre_loose([cs_ok]), mk_ryhp([shd(913, 'gs'), shd(914, 'hs')]),
      b'RYHP' + lcg_fill(3, 60) + mk_bnsh(R, 915, 1, [{'vs', 'ps'}]), mk_ryhp([shd(916, 'ps')]) + mk_bnsh(R, 917, 1, [{'ps'}]), b'RYHP' + glslc_pairs([shd(918, 'ps')])]
def blob(d, label='/t'):
    ss.BAHS_VAR.clear(); out = []
    with contextlib.redirect_stdout(io.StringIO()): ss.scan_blob(d, label, out)
    return found_json(out)
def phyre_vec(d):
    vecs.append({'kind': 'fn_phyre', 'in': {'b64': b64(d)}, 'out': {'phyre': res(lambda: found_json(ss.phyre_shaders(d, '/t'))), 'blob': res(lambda: blob(d))}})
for d in ph:
    phyre_vec(d)
    for _ in range(60): phyre_vec(damage(d, V, head=0x100))

# KIT: index and parts
def kit_res(idx, parts):
    import tempfile
    t = tempfile.mkdtemp(); path = f'{t}/x.kit'
    open(path, 'wb').write(idx)
    for p, b in parts.items(): open(f'{path}.{p}', 'wb').write(b)
    norm = lambda s: str(s).replace(t, '<P>')
    try: es = kitmod.entries(path)
    except (ValueError, OSError) as e: return {'entries': {'err': norm(f'{type(e).__name__}: {e}')}, 'reads': []}
    reads = []
    for e in es:
        try: reads.append({'ok': sha(kitmod.read(path, e))})
        except (ValueError, OSError, Exception) as x: reads.append({'err': norm(f'{type(x).__name__}: {x}')})
    return {'entries': {'ok': [[e.index, e.part, e.offset, e.length, e.size, e.compressed, e.sha1.hex()] for e in es]}, 'reads': reads}
kmem = [(mk_ryhp(fx_pairs[:2]), True), (b'asset text ' * 7000, True), (mk_bnsh(R, 950, 1, [{'ps'}]), False), (lcg_fill(5, 200), True), (b'', False)]
for parts_n in (1, 2):
    idx, parts = mk_kit(kmem, parts_n)
    vecs.append({'kind': 'kit', 'in': {'index': b64(idx), 'parts': {str(p): b64(b) for p, b in parts.items()}}, 'out': kit_res(idx, parts)})
    for _ in range(100):
        i2, p2 = bytearray(idx), {p: bytearray(b) for p, b in parts.items()}
        k = V.below(6)
        if k <= 1: i2 = bytearray(damage(bytes(i2), V, head=0x25 + 56 * 2))
        elif k == 2:
            p = V.pick(list(p2)); p2[p] = bytearray(damage(bytes(p2[p]), V, head=64))
        elif k == 3: i2[0x25 + 56 * V.below(len(kmem)) + V.below(56)] ^= 1 << V.below(8)     # inside a record
        elif k == 4:
            p = V.pick(list(p2)); p2[p] = p2[p][:V.below(len(p2[p]) + 1)]
        else:
            p = V.pick(list(p2)); p2[p][V.below(len(p2[p]))] ^= 1 << V.below(8)
        i2, p2 = bytes(i2), {p: bytes(b) for p, b in p2.items()}
        vecs.append({'kind': 'kit', 'in': {'index': b64(i2), 'parts': {str(p): b64(b) for p, b in p2.items()}}, 'out': kit_res(i2, p2)})

os.makedirs(os.path.dirname(vec_out) or '.', exist_ok=True)
with gzip.GzipFile(vec_out, 'wb', mtime=0) as f:
    f.write(('\n'.join(json.dumps(v, sort_keys=True, separators=(',', ':')) for v in vecs) + '\n').encode())
import collections
print('vectors:', len(vecs), dict(collections.Counter(v['kind'] for v in vecs)), os.path.getsize(vec_out) // 1024, 'KiB gz')
