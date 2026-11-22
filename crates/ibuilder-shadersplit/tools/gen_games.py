#!/usr/bin/env python3
"""Writes tests/data/games/<name>/: small synthetic games (a RomFS folder, an NSO `main`, a profile, optional DLC folders) and `expected.json`: the sha256 of every file the engine's own
split writes for them (`shader_device.split` over `shader_split.py`, the phone's wrapper). The Rust test (`tests/games.rs`) splits the same folders and must write the same bytes.

  SHADER_DEVICE_DIR=<folder with shader_device.py> PYTHONDONTWRITEBYTECODE=1 python3 -I tools/gen_games.py <engine root (the folder that holds dm/)> <out folder>

Every byte is a PRNG's; nothing comes from a game. Each game exercises one container reader end to end: the job planning (which files a reader takes), the reader, the merge order and the tables.
"""
import sys, os, json, struct, hashlib, tempfile, io, contextlib, shutil, zlib, gzip
engine, out_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
tools = engine + '/dm/gfx/tools'
prof_tmp = tempfile.mkdtemp()
GAME_TSV = 'title_id\t0100TESTTEST0000\nshader_groups\t/Data/=data /Pack/=pack /aoc/=dlc\nshader_note\tsynthetic game\n'
os.makedirs(prof_tmp + '/0100TESTTEST0000'); open(prof_tmp + '/0100TESTTEST0000/game.tsv', 'w').write(GAME_TSV)
os.environ.update(DM_GAMES_DIR=prof_tmp, DM_GAMES_FIRST='1', DM_GAME='0100TESTTEST0000')
for sub in ('', '/../../tools', '/../../games'): sys.path.insert(0, os.path.abspath(tools + sub))
import shader_split as ss, unityfs
sys.path.insert(0, os.environ['SHADER_DEVICE_DIR'])
import shader_device as sd
from compression import zstd

# the generator's builders (the part of gen_vectors.py before its vectors)
src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'gen_vectors.py')).read()
exec(src[:src.index("rng = Rng(0x5eed)")].replace("tools, skip, out_path = sys.argv[1], sys.argv[2], sys.argv[3]", "skip = '0+0'"))
cont = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'gen_vectors_containers.py')).read()
recs = []
def emit(*a, **k): pass
def res(fn): return {'ok': fn()}
exec(cont[:cont.index("# ---- vectors")])
exec(cont[cont.index("# ==== Unity"):cont.index("for idx, ((cid, h), (root, layout)) in enumerate(KNOWN):")])
exec(cont[cont.index("# ---- serialized files and bundles"):cont.index("def unity_run")])

def sha(b): return hashlib.sha256(b).hexdigest()

def nso(text):
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, 0), (0x100 + len(text), end, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text

def common_strings_text(): return b'AABB\0AnimationClip\0AnimationCurve\0' + b'AudioClip\0\0'

def make_game(name, files, main_shaders=(), dlc=None, main_extra=b'', update=None):
    """files: {path in RomFS: bytes}; main_shaders: GLSLC outputs in main's text; dlc: {folder name: {path: bytes}}"""
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True)
    os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    text = bytearray(lcg_fill(9, 0x1000)) + bytes(main_extra) + common_strings_text()
    for b in main_shaders: text += b + bytes(rng.below(16))
    text += lcg_fill(10, 0x200)
    open(g + '/main', 'wb').write(nso(bytes(text)))
    os.makedirs(g + '/profile/0100TESTTEST0000'); open(g + '/profile/0100TESTTEST0000/game.tsv', 'w').write(GAME_TSV)
    dlcs = []
    for dn, dfiles in (dlc or {}).items():
        for p, b in dfiles.items():
            os.makedirs(os.path.dirname(f'{g}/dlc/{dn}/{p}'), exist_ok=True); open(f'{g}/dlc/{dn}/{p}', 'wb').write(b)
        dlcs.append(f'{g}/dlc/{dn}')
    romfs_arg, main_arg, updates = g + '/RomFS', g + '/main', []
    if update:   # an update: its RomFS and main beside the base game's, in a folder named after the version (its files' labels carry the name)
        ufiles, ushaders = update
        for p, b in ufiles.items():
            os.makedirs(os.path.dirname(f'{g}/update/v1/RomFS/{p}'), exist_ok=True); open(f'{g}/update/v1/RomFS/{p}', 'wb').write(b)
        utext = bytearray(lcg_fill(19, 0x800)) + common_strings_text()
        for b in ushaders: utext += b + bytes(8)
        open(g + '/update/v1/main', 'wb').write(nso(bytes(utext)))
        romfs_arg, main_arg, updates = f'{g}/RomFS,{g}/update/v1/RomFS', f'{g}/main,{g}/update/v1/main', ['update/v1']
    work = tempfile.mkdtemp()
    log = []
    n = sd.split(romfs_arg, main_arg, work + '/pkg', work + '/tables', log=log.append, jobs=1, profile_dir=g + '/profile', game_id='0100TESTTEST0000', engine=engine, dlc=dlcs or None)
    expected = {}
    for sub in ('pkg', 'tables'):
        for fn in sorted(os.listdir(f'{work}/{sub}')): expected[f'{sub}/{fn}'] = sha(open(f'{work}/{sub}/{fn}', 'rb').read())
    json.dump({'shaders': n, 'files': expected, 'dlc': [os.path.basename(d) for d in dlcs], 'updates': updates, 'log': [l for l in log if not l.startswith('progress')]}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    print(name, n, 'shaders', len(expected), 'files', sum(len(b) for b in files.values()) // 1024, 'KiB')

rng = Rng(0xfeed)
def S(seed, *stages): return [mk_glslc(rng, seed + i, [st]) for i, st in enumerate(stages)]

# ---- plain Nintendo containers, GLSLC in main, DLC with a shared and an own shader ------------------------------------------------------------
files = {
    'Data/a.bnsh': mk_bnsh(rng, 1000, 2, [{'vs', 'ps'}, {'cs'}]),
    'Data/b.szs': yaz0_encode(mk_sarc([('x.bnsh', mk_bnsh(rng, 2000, 1, [{'vs', 'ps'}])), ('y.bnsh', mk_bnsh(rng, 1000, 1, [{'vs', 'ps'}]))]), rng),
    'Pack/c.sarc': mk_sarc([('n.glslc', mk_glslc(rng, 3000, ['ps'])), ('m.bnsh', mk_bnsh(rng, 3100, 1, [{'ps'}]))]),
    'Pack/z.gz': gzip.compress(mk_bnsh(rng, 4000, 1, [{'vs', 'ps'}]), mtime=0),
    'Pack/junk.bin': lcg_fill(5, 3000),
    'Pack/d.bahs': mk_bahs(rng, 5000, [0, 1, 1, 3], [0, 2]),
}
dlc = {'1': {'Data/a.bnsh': files['Data/a.bnsh'], 'Data/own.bnsh': mk_bnsh(rng, 6000, 1, [{'vs', 'ps'}])}}
make_game('plain', files, main_shaders=S(7000, 'ps', 'cs') + S(7100, 'vs', 'vs'), dlc=dlc)

# ---- zstd files, size-prefixed frames, MonoGame XNB (LZ4), MSHD, tfxo, PGSO, Needle-free ---------------------------------------------------------
import lz4.block
xnb_inner = b'XNBd' + bytes([5, 0x40]) + b'\0' * 0   # placeholder replaced below
def mk_xnb_lz4(payload):
    comp = lz4.block.compress(payload, store_size=False)
    return b'XNB' + b'w' + bytes([5, 0x40]) + struct.pack('<I', 14 + len(comp)) + struct.pack('<I', len(payload)) + comp
files = {
    'Data/a.bnsh.zs': zstd.compress(mk_bnsh(rng, 11000, 1, [{'vs', 'ps'}])),
    'Data/b.cmp': struct.pack('<I', len(mk_bnsh(rng, 11100, 1, [{'ps'}]))) + zstd.compress(mk_bnsh(rng, 11100, 1, [{'ps'}])),
    'Data/m.mshd': mk_mshd(rng, 12000, 2),
    'Data/t.tfxo': mk_tfxo(rng, 12100, 3),
    'Data/p.pso': mk_pgso(rng, 12200, 'ps'),
    'Data/x.xnb': mk_xnb_lz4(mk_bnsh(rng, 13000, 1, [{'vs', 'ps'}])),
    'Data/n.ndsr': mk_ndsr(rng, 14000, ['vs', 'ps', 'cs']),
}
make_game('misc', files, main_shaders=S(15000, 'ps'))

# ---- zip: stored and deflated members, a directory entry, a member that holds a SARC with a BNSH, a prefix before the archive ----------------------------
import zipfile
def mk_zip(members, prefix=b''):
    bio = io.BytesIO()
    with zipfile.ZipFile(bio, 'w') as z:
        for name, data, comp in members:
            zi = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0)); zi.compress_type = comp; zi.external_attr = 0o644 << 16
            z.writestr(zi, data)
    return prefix + bio.getvalue()
sarc_in = mk_sarc([('s.bnsh', mk_bnsh(rng, 21100, 1, [{'vs', 'ps'}]))])
files = {
    'game.zip': mk_zip([('d/', b'', zipfile.ZIP_STORED), ('d/a.bnsh', mk_bnsh(rng, 21000, 1, [{'vs', 'ps'}]), zipfile.ZIP_DEFLATED), ('d/b.nvsb', bytes(0x60) + mk_glslc(rng, 21200, ['vs', 'ps']), zipfile.ZIP_STORED),
                        ('d/c.sarc', sarc_in, zipfile.ZIP_DEFLATED), ('d/junk', lcg_fill(3, 2000), zipfile.ZIP_DEFLATED)]),
    'Data/pre.zip': mk_zip([('x.glslc', mk_glslc(rng, 21300, ['ps']), zipfile.ZIP_DEFLATED)], prefix=b'VZIP' + lcg_fill(4, 100)),
    'Data/plain.bnsh': mk_bnsh(rng, 21400, 1, [{'ps'}]),
}
make_game('zip', files)

# ---- RE Engine KPKA: SDF files (stored, deflated, zstd), a patch pak that replaces one, an OCIO file, a pak with a block table ------------------------------
def mk_sdf(rng, seed, stage_sets, layout=0):
    base, size, lens, at = ((0x10, 0x108, 0, 8), (0x18, 0x110, 8, 0x58), (0x18, 0x110, 8, 0x60))[layout]
    n = len(stage_sets)
    d = bytearray(base + size * n); d[0:4] = b'SDF\0'; d[4:8] = struct.pack('<HH', 1, n)
    for i, sset in enumerate(stage_sets):
        for si, st in enumerate(ss.SDF_STAGES):
            if st not in sset: continue
            ctl, code = mk_shader(rng, seed + 10 * i + si, st)
            o = len(d)
            rec = bytearray(at) + ctl + code
            rec[lens:lens + 8] = struct.pack('<II', len(ctl), len(code))
            d += rec
            d[base + size * i + 0x10 + 8 * si: base + size * i + 0x18 + 8 * si] = struct.pack('<Q', o)
    return bytes(d)
def mk_ocio(rng, seed, n):
    d = bytearray(b'OCIO' + lcg_fill(seed, 0x40))
    for i in range(n):
        ctl, code = mk_shader(rng, seed + i, rng.pick(['vs', 'ps', 'cs']), nvn2=True)
        hdr = bytearray(lcg_fill(seed + 100 + i, 0x58)); hdr[8:16] = struct.pack('<II', len(ctl), len(code))
        d += hdr + ctl + code
    return bytes(d)
def kcomp(attr, raw):
    return {0: raw, 1: zlib.compressobj(wbits=-15).compress(raw) + zlib.compressobj(wbits=-15).flush() if False else None, 2: zstd.compress(raw)}[attr]
def deflate_raw(raw):
    c = zlib.compressobj(9, zlib.DEFLATED, -15); return c.compress(raw) + c.flush()
def mk_kpka(entries, blocks=None):
    """entries: [(hash, attr, raw bytes)] (attr bits 0-3: 0 stored, 1 deflate, 2 zstd); blocks: (block length, [(hash, raw)]) for entries whose data lies in the block table"""
    feat = 0x20 if blocks else 0
    nb = len(entries) + (len(blocks[1]) if blocks else 0)
    head = b'KPKA' + bytes([4, 0]) + struct.pack('<HII', feat, nb, 0)
    table_end = 16 + 0x30 * nb + (8 + 8 * 8 if blocks else 0)
    body = bytearray(); ents = []
    for h, attr, raw in entries:
        data = raw if attr == 0 else deflate_raw(raw) if attr == 1 else zstd.compress(raw)
        ents.append(struct.pack('<Qqqqqq', h, table_end + len(body), len(data), len(raw), attr, 0)); body += data
    blk_rows = []; extra = bytearray()
    if blocks:
        bl, items = blocks
        for h, raw in items:
            first = len(blk_rows)
            stored_total = 0
            for o in range(0, len(raw), bl):
                chunk = raw[o:o + bl]
                if len(chunk) == bl: c = chunk
                else: c = zstd.compress(chunk)
                blk_rows.append((len(c), table_end + len(body) + len(extra))); extra += c; stored_total += len(c)
            ents.append(struct.pack('<Qqqqqq', h, first, stored_total, len(raw), 0x1000000, 0))
        blk = struct.pack('<II', bl, len(blk_rows)) + b''.join(struct.pack('<Q', (n << 42) | o) for n, o in blk_rows)
        # the block table is right after the entries, so offsets are known only now: rebuild with the real table size
        table_end2 = 16 + 0x30 * nb + len(blk)
        shift = table_end2 - table_end
        blk = struct.pack('<II', bl, len(blk_rows)) + b''.join(struct.pack('<Q', (n << 42) | (o + shift)) for n, o in blk_rows)
        ents = [e if i >= len(entries) else struct.pack('<Qqqqqq', *(lambda t: (t[0], t[1] + shift, t[2], t[3], t[4], t[5]))(struct.unpack('<Qqqqqq', e))) for i, e in enumerate(ents)]
        return head + b''.join(ents) + blk + bytes(body) + bytes(extra)
    return head + b''.join(ents) + bytes(body)
sdf_a, sdf_b, sdf_c, sdf_d = (mk_sdf(rng, 22000 + 1000 * i, [{'vs', 'ps'}, {'cs'}] if i % 2 == 0 else [{'vs', 'ps', 'gs'}], layout=i % 3) for i in range(4))
sdf_new = mk_sdf(rng, 23000, [{'vs', 'ps'}])
files = {
    're_chunk_000.pak': mk_kpka([(0x1111, 0, sdf_a), (0x2222, 1, sdf_b), (0x3333, 2, sdf_c), (0x4444, 2, mk_ocio(rng, 24000, 3)), (0x5555, 0, lcg_fill(8, 100)), (0x6666, 2, b'tiny')]),
    're_chunk_000.pak.patch_001.pak': mk_kpka([(0x2222, 2, sdf_new), (0x7777, 1, sdf_d)]),
    'Data/blocks.pak': mk_kpka([(0x8888, 0, mk_sdf(rng, 25000, [{'vs', 'ps'}]))], blocks=(0x800, [(0x9999, mk_sdf(rng, 26000, [{'ps'}, {'vs', 'ps'}]) + bytes(100))])),
    'Data/loose.bnsh': mk_bnsh(rng, 27000, 1, [{'vs', 'ps'}]),
}
make_game('kpka', files)

# ---- FromSoftware dvdbnd (.bdt + .bhd5, DCX zlib around BND3 of GLSLC outputs) and PlatinumGames PKZ (stored and zstd entries) ----------------------------------
def mk_bnd3(members):
    n = len(members); hdr = 0x20 + 28 * n
    names = bytearray(); data = bytearray(); rows = []
    name_off = hdr
    pos = hdr + sum(len(nm.encode()) + 1 for nm, _ in members)
    for nm, b in members:
        rows.append((pos + len(data), len(b), hdr + len(names))); names += nm.encode() + b'\0'; data += b
    d = bytearray(b'BND3' + bytes(8) + bytes([0x5c, 0, 0, 0]) + struct.pack('<ii', n, pos) + bytes(8))
    for off, ln, noff in rows: d += struct.pack('<B3xiQiIi', 2, ln, off, 1, noff, ln)
    return bytes(d) + bytes(names) + bytes(data)
def mk_dcx(raw):
    z = zlib.compress(raw)
    d = bytearray(0x4c); d[0:4] = b'DCX\0'; d[0x18:0x1c] = b'DCS\0'; d[0x1c:0x24] = struct.pack('>II', len(raw), len(z)); d[0x24:0x28] = b'DCP\0'; d[0x28:0x2c] = b'DFLT'; d[0x44:0x48] = b'DCA\0'; d[0x48:0x4c] = struct.pack('>I', 8)
    return bytes(d) + z
b1 = mk_dcx(mk_bnd3([('N:\\FRPG\\Source\\Shader\\a.vpo', mk_glslc(rng, 31000, ['vs'])), ('N:\\FRPG\\Source\\Shader\\b.fpo', mk_glslc(rng, 31100, ['ps'])), ('other.txt', b'text')]))
b2 = mk_bnd3([('c.fpo', mk_glslc(rng, 31200, ['ps']))])
b3 = lcg_fill(4, 500)
bdt = bytearray(b'BDF307D7R6\0\0'); ents = []
for b in (b1, b2, b3): ents.append((len(ents) + 100, len(b), len(bdt))); bdt += b
bhd = bytearray(b'BHD5\xff\0\0\0' + struct.pack('<I', 1) + struct.pack('<I', 0) + struct.pack('<II', 1, 0) + struct.pack('<Q', 0x20))
bhd += struct.pack('<IIQ', len(ents), 0, 0x30) + b''.join(struct.pack('<IIQ', h, size, off) for h, size, off in ents)
bhd[0x10:0x14] = struct.pack('<I', 1); bhd[0x18:0x20] = struct.pack('<Q', 0x20)
def mk_pkz(entries):
    n = len(entries); names = bytearray(); rows = []
    def nm(s):
        o = len(names); names.extend(s.encode() + b'\0'); return o
    data = bytearray(); tab_len = 0x20 * n
    comps = {}
    items = []
    for name, comp, raw in entries:
        c = raw if comp == 'None' else zstd.compress(raw)
        items.append((nm(name), nm(comp), len(raw), len(c), c))
    names_len = len(names); pos = 0x20 + tab_len + names_len
    rowb = b''
    for a, c_, usz, csz, c in items:
        rowb += struct.pack('<IIQQQ', a, c_, usz, pos + len(data), csz); data += c
    return b'pkzl' + struct.pack('<IQIII', 0x20000, 0, n, 0x20, names_len) + bytes(4) + rowb + bytes(names) + bytes(data)
files = {
    'dvdbnd0.bdt': bytes(bdt), 'dvdbnd0.bhd5': bytes(bhd),
    'root_000.pkz': mk_pkz([('shader/a.dat', 'None', mk_bnsh(rng, 32000, 1, [{'vs', 'ps'}])), ('shader/b.dat', 'ZStandard', mk_sarc([('x.bnsh', mk_bnsh(rng, 32100, 1, [{'ps'}]))])), ('misc.bin', 'ZStandard', lcg_fill(5, 400))]),
    'Data/other.bnsh': mk_bnsh(rng, 32200, 1, [{'cs'}]),
}
make_game('bdt-pkz', files)

# ---- Monolith ARH2 + ARD with XBC1 (zlib) entries holding GLSLC outputs ----------------------------------------------------------------------------------
def mk_xbc1(raw):
    z = zlib.compress(raw)
    return b'xbc1' + struct.pack('<III', 1, len(raw), len(z)) + bytes(32) + z
ent = [mk_xbc1(mk_glslc(rng, 33000, ['vs', 'ps'])), mk_glslc(rng, 33100, ['ps']), mk_xbc1(mk_glslc(rng, 33200, ['cs']))]
align = 16
arh = bytearray(b'arh2' + struct.pack('<II', len(ent), align) + bytes(4)); ard = bytearray()
for i, e in enumerate(ent):
    arh += struct.pack('<QII', 0xabc0 + i, len(e), len(e)); ard += e + bytes((-len(e)) % align)
files = {'bf.arh': bytes(arh), 'bf.ard': bytes(ard), 'Data/loose.bnsh': mk_bnsh(rng, 33300, 1, [{'vs', 'ps'}])}
make_game('arh', files)

# ---- Koei Tecmo: LINKDATA (chunked zlib), RDB (IDRK entries, zlib / new zlib / LZ4 chunks) and PDRK ------------------------------------------------------------
def mk_ktgz(data, cs=0x10000):
    n = (len(data) + cs - 1) // cs
    sizes, chunks = [], []
    for i in range(n):
        raw = data[i * cs:(i + 1) * cs]
        z = zlib.compress(raw)
        c = struct.pack('<I', len(z)) + z
        if len(c) >= len(raw): c = raw                         # does not compress: stored as it is
        sizes.append(len(c)); chunks.append(c)
    o = (12 + 4 * n + 0x7f) & ~0x7f
    out = bytearray(struct.pack('<III', cs, n, len(data)) + b''.join(struct.pack('<I', s) for s in sizes)); out += bytes(o - len(out))
    for c in chunks:
        out += c; out += bytes((-len(out)) % 0x80)
    return bytes(out)
g2s = lambda seed, *stages: b'_S2G3000' + lcg_fill(seed, 40) + b''.join(mk_glslc(rng, seed + 10 * i, [st]) for i, st in enumerate(stages))
ents = [(mk_ktgz(g2s(41000, 'vs', 'ps')), 1, len(g2s(41000, 'vs', 'ps'))), (g2s(41100, 'ps'), 0, len(g2s(41100, 'ps'))), (mk_ktgz(mk_sarc([('s.bnsh', mk_bnsh(rng, 41200, 1, [{'vs', 'ps'}]))])), 1, 0)]
d1 = bytearray(lcg_fill(2, 64)); idx = bytearray()
for raw, comp, n in ents:
    n = n or len(raw) * 2
    idx += struct.pack('<QQQQ', len(d1), n, len(raw), comp); d1 += raw; d1 += bytes((-len(d1)) % 16)
files = {'DATA0.bin': bytes(idx), 'DATA1.bin': bytes(d1), 'Data/loose.bnsh': mk_bnsh(rng, 41300, 1, [{'ps'}])}
make_game('linkdata', files)

def mk_idrk(entries, pdrk=False):
    out = bytearray(b'PDRK0000' + struct.pack('<Q', 0x10) if pdrk else b'')
    for flags, content in entries:
        if flags & 0x500000 == 0x500000:
            data = b''
            for o in range(0, len(content), 0x4000):
                c = lz4.block.compress(content[o:o + 0x4000], store_size=False); data += struct.pack('<H', len(c)) + bytes(8) + c
            data += struct.pack('<H', 0) + bytes(8)
        elif flags & 0x400000:
            data = b''
            for o in range(0, len(content), 0x4000):
                c = zlib.compress(content[o:o + 0x4000]); data += struct.pack('<H', len(c)) + bytes(8) + c
            data += struct.pack('<H', 0) + bytes(8)
        elif flags & 0x100000:
            data = b''
            for o in range(0, len(content), 0x4000):
                c = zlib.compress(content[o:o + 0x4000]); data += struct.pack('<I', len(c)) + c
            data += struct.pack('<I', 0)
        else: data = content
        esz = 0x30 + len(data); esz += (-esz) % 16
        data += bytes(esz - 0x30 - len(data))
        hdr = b'IDRK0000' + struct.pack('<QQQ', esz, len(data), len(content)) + struct.pack('<IIII', 0, 1, 2, flags)
        out += hdr + data
    return bytes(out)
files = {
    'asset/a.rdb.bin': mk_idrk([(0x100000, g2s(42000, 'vs', 'ps')), (0, lcg_fill(3, 50)), (0, g2s(42100, 'cs'))]),
    'asset/b.rdb.bin': mk_idrk([(0x400000, g2s(42200, 'ps')), (0x500000, g2s(42300, 'vs', 'ps')), (0x100000, b'_DOK0000' + mk_glslc(rng, 42400, ['ps']) + mk_glslc(rng, 42500, ['vs']))], pdrk=True),
    'Data/loose.bnsh': mk_bnsh(rng, 42600, 1, [{'ps'}]),
}
make_game('idrk', files)

# ---- UbiArt IPK, Double Eleven packs, Snowdrop BERG, Unreal Engine 3 caches ------------------------------------------------------------------------------------
def mk_ipk(entries):
    t = bytearray(); data = bytearray()
    for name, raw, comp in entries:
        b = zlib.compress(raw) if comp else raw
        d_, n_ = name.rsplit('/', 1) if '/' in name else ('', name)
        d_ = d_ + '/' if d_ else ''
        t += struct.pack('>III', 1, len(raw), len(b) if comp else 0) + struct.pack('>Q', 0) + struct.pack('>Q', len(data))
        t += struct.pack('>I', len(d_)) + d_.encode() + struct.pack('>I', len(n_)) + n_.encode() + struct.pack('>I', 0) + bytes(4)
        data += b
    base = 0x38 + len(t)
    return struct.pack('>IIIII', 0x50ec12ba, 7, 0, base, len(entries)) + bytes(0x38 - 20) + bytes(t) + bytes(data)
def mk_d11(members):
    idx = struct.pack('<3I', 3, 0, len(members)); data = bytearray()
    for h, raw, comp in members:
        b = zlib.compress(raw) if comp else raw
        idx += struct.pack('<4I', len(raw), len(b), len(data), h); data += b
    return bytes(idx), bytes(data)
idx, dat = mk_d11([(0x1234, mk_glslc(rng, 43100, ['ps']), True), (0x5678, mk_glslc(rng, 43200, ['vs']), False), (0x9abc, lcg_fill(2, 100), True), (0xdef0, mk_bnsh(rng, 43300, 1, [{'vs', 'ps'}]), False)])
def snow_rec(kind, raw):
    c = zlib.compress(raw) if kind == 0xc5 else zstd.compress(raw)
    return bytes([kind, 0, 0xcd, 0xab]) + struct.pack('<IIII', 0, 0, len(raw), len(c)) + c
def shaders_raw(seed, *stages): return b''.join(b''.join(mk_shader(rng, seed + 10 * i, st)) for i, st in enumerate(stages))
inner = snow_rec(0xc5, shaders_raw(44100, 'ps'))
berg = b'BERG' + lcg_fill(1, 28) + snow_rec(0xc5, shaders_raw(44000, 'vs', 'ps')) + lcg_fill(2, 9) + snow_rec(0xc6, shaders_raw(44200, 'cs', 'vs')) + zstd.compress(inner) + lcg_fill(3, 17)
def mk_ue3pkg(body, bs=0x1000):
    nb = (len(body) + bs - 1) // bs; rows, blocks = [], b''
    for i in range(nb):
        raw = body[i * bs:(i + 1) * bs]; z = zlib.compress(raw)
        if len(z) >= len(raw): z = raw
        rows.append((len(z), len(raw))); blocks += z
    chunk = bytes([0xc1, 0x83, 0x2a, 0x9e]) + struct.pack('<III', bs, len(blocks), len(body)) + b''.join(struct.pack('<II', c, u) for c, u in rows) + blocks
    return bytes([0xc1, 0x83, 0x2a, 0x9e]) + struct.pack('<H', 0x360) + lcg_fill(5, 60) + chunk
def ue3_rec(seed, st, k=4):
    ctl, code = mk_shader(rng, seed, st)
    return struct.pack('<III', 8 + len(code) + len(ctl), len(code), len(ctl)) + (bytes(4) if k == 8 else b'') + code + ctl
body = b''.join(ue3_rec(45000 + 10 * i, st) for i, st in enumerate(['vs', 'ps', 'cs']))
files = {
    'bundle.ipk': mk_ipk([('shaders/compiled/nx/a.fxb', mk_glslc(rng, 43000, ['vs', 'ps']), True), ('shaders_dev/b.fxb', mk_glslc(rng, 43010, ['ps']), False), ('x/c.ckd', lcg_fill(7, 300), False), ('shaders/compiled/nx/d.fxb', mk_bnsh(rng, 43020, 1, [{'ps'}]), False)]),
    'd11extra/langselect.pkdswitch': dat, 'd11extra/langselect.pkiswitch': idx,
    'Data/slice.dat': berg,
    'Data/cache.bin': b'BMSG' + lcg_fill(8, 40) + ue3_rec(45500, 'ps') + ue3_rec(45510, 'vs', 8),
    'Data/pkg.xxx': mk_ue3pkg(lcg_fill(9, 100) + body),
}
make_game('ipk-d11-berg-ue3', files)

# ---- an update: the base game's files and main, then the update's (its labels carry the version's folder name), a DLC on top -------------------------------------------
base_files = {'Data/a.bnsh': mk_bnsh(rng, 51000, 1, [{'vs', 'ps'}]), 'Data/b.bnsh': mk_bnsh(rng, 51100, 1, [{'ps'}])}
upd_files = {'Data/a.bnsh': mk_bnsh(rng, 51000, 1, [{'vs', 'ps'}]), 'Data/c.bnsh': mk_bnsh(rng, 51200, 1, [{'vs', 'cs'}]), 'Pack/new.szs': yaz0_encode(mk_sarc([('n.bnsh', mk_bnsh(rng, 51300, 1, [{'ps'}]))]), rng)}
make_game('update-dlc', base_files, main_shaders=S(52000, 'ps'), update=(upd_files, S(52000, 'ps') + S(52100, 'vs')), dlc={'2': {'Data/dl.bnsh': mk_bnsh(rng, 53000, 1, [{'vs', 'ps'}]), 'Data/b.bnsh': base_files['Data/b.bnsh']}})
