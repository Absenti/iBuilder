#!/usr/bin/env python3
"""Writes tests/data/games/sync-smash, sync-trinity and tests/data/games-err/err-smash-*, err-trinity-*: synthetic games for two archive readers of nxnative's shader_split.py,
Super Smash Bros. Ultimate's data.arc (smash_arc.py, smash_arc_jobs / smash_arc_shaders) and Game Freak's Trinity packs (data.trpfs + data.trpfd, trpfs_jobs / trinity_shaders).
Every byte is made here (nothing comes from a game). Each game is split by the UPSTREAM engine's own shader_split.py (its command line, as build_ipa_from_nsp.py runs it);
expected.json has the sha256 of what it wrote and its log (tests/games.rs), or the error it ended with (tests/rebase.rs, games-err).
Also writes tests/data/vectors-smash-trinity.jsonl.gz: damaged copies of the two containers (cut, bytes flipped where the readers look), each split end to end by the Python;
a vector is the damage (applied to the committed container, not stored whole) and what the split gave (tests/smash_trinity.rs).

  PYTHONDONTWRITEBYTECODE=1 <python 3.14> -I tools/gen_smash_trinity.py <upstream checkout (the folder that holds dm/)> tests/data

No Oodle: a Trinity file of compression 3 needs the game's own decoder (the Mac host); this port refuses it (tests/smash_trinity.rs), so no game here has one.
shader_map.tsv line 2: the upstream engine reworded it after the commit this port follows; the hash recorded is that of the Python's table with line 2 put back to the port's text.
"""
import gzip, hashlib, json, os, shutil, struct, subprocess, sys, tempfile, zlib
from compression import zstd

engine, data_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
GAME = '0100TESTTEST0000'
GAME_TSV = f'title_id\t{GAME}\nshader_groups\t/Data/=data /Pack/=pack /arc/=arc /aoc/=dlc\nshader_note\tsynthetic game\n'
PORT_LINE2 = '# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied\n'
CTL, GFX, CS, GLSLC = bytes.fromhex('34127698'), bytes.fromhex('78563412'), bytes.fromhex('79563412'), bytes.fromhex('91688619')
SPH = {'vs': 1, 'hs': 2, 'ds': 3, 'gs': 4, 'ps': 5}
SMASH_MAGIC = struct.pack('<Q', 0xABCDEF9876543210)


def sha(b): return hashlib.sha256(b).hexdigest()


class Rng:   # xorshift64*, so the bytes do not depend on Python's random module
    def __init__(self, s): self.s = s or 1
    def u64(self):
        x = self.s; x ^= x >> 12; x ^= (x << 25) & (2**64 - 1); x ^= x >> 27; self.s = x
        return (x * 0x2545F4914F6CDD1D) & (2**64 - 1)
    def bytes(self, n): return bytes(self.u64() >> 56 for _ in range(n))
    def below(self, n): return self.u64() % n


def clean(b):   # no control / code / GLSLC magic inside random bytes
    b = bytearray(b)
    for m in (CTL, GLSLC, GFX, CS, b'RYHP', b'BNSH', b'SARC', b'Yaz0', bytes.fromhex('28b52ffd')):
        i = b.find(m)
        while i >= 0: b[i] ^= 0x55; i = b.find(m, i + 1)
    return bytes(b)


def program(st, seed):
    """(control section, code area) of one stage (as tools/gen_sync_readers.py makes them)"""
    r = Rng(seed * 7919 + 13)
    code = bytearray(CS if st == 'cs' else GFX) + clean(r.bytes(0x2c))
    sph = bytearray(clean(r.bytes(0x50)))
    if st != 'cs':
        struct.pack_into('<I', sph, 0, (SPH[st] << 10) | 1 | (0x40 if st == 'vs' else 0x20))
        if st == 'ps': sph[0x18] = 1
    code += sph if st != 'cs' else b''
    code += clean(r.bytes(0x40 + 8 * (seed % 5)))
    ctl = bytearray(clean(r.bytes(0x880))); ctl[:4] = CTL
    struct.pack_into('<I', ctl, 0x20, 0x880); struct.pack_into('<I', ctl, 0x704, len(code))
    ctl[0x748:0x758] = bytes(16)
    if st == 'cs': struct.pack_into('<3I', ctl, 0x748, 64, 1, 1); struct.pack_into('<I', ctl, 0x754, 0x100)
    return bytes(ctl), bytes(code)


def glslc(stages):
    d = bytearray(0x90 * (len(stages) + 1)); d[:4] = GLSLC; struct.pack_into('<I', d, 0x4c, len(stages))
    for i, (ctl, code) in enumerate(stages):
        at = len(d); d += ctl + code
        h = 0x90 + 0x90 * i; struct.pack_into('<III', d, h, len(ctl) + len(code), at, 0); struct.pack_into('<4I', d, h + 0x30, 0, len(ctl), len(code), len(ctl))
    struct.pack_into('<I', d, 0x44, len(d))
    return bytes(d)


def ryhp(stages): return b'RYHP' + bytes(0x3c) + b''.join(glslc([s]) for s in stages)


def nso(text):
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, 0), (0x100 + len(text), end, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text


S = program


# ---- Super Smash Bros. Ultimate's data.arc (the layout smash_arc.files reads) ----------------------------------------------------------------
def h40(s): return zlib.crc32(s.encode()), len(s)


def smash_arc(paths, old=(), regions=3, counts_split=True, pad_tsize=0x40, tsize=None):
    """paths: [(name, ext, entry, datas)]; entry None (a path-to-info index and file info of its own), 'bad_k' (an index past the table), 'bad_fi' (a file info past its table)
    or ('alias', i) (path i's index: a second path to the same data). datas: [(raw bytes, how, directory)] per region (regional when more than one: the regions not given
    repeat the last); how: 'zstd', 'stored', ('stored', length) (a stored length longer than the length), ('csz0',) (stored length 0), ('same',) (the previous region's data).
    old: data no path reaches (superseded copies). Directories: 0 at file section +0, 1 at +0x8000"""
    body = bytearray(0x10000)
    dir_base, used = [0, 0x8000], [0, 0]
    fd, f2d, fi, idx, prow = [], [], [], [], []
    def put(raw, how, d):
        if how == 'zstd': st, usz, flags = zstd.compress(raw), len(raw), 3
        elif isinstance(how, tuple) and how[0] == 'stored': st, usz, flags = raw, how[1], 0
        elif isinstance(how, tuple) and how[0] == 'csz0': st, usz, flags = b'', len(raw), 0
        else: st, usz, flags = raw, len(raw), 0
        at = dir_base[d] + used[d]
        while len(body) < at + len(st) + 4: body.extend(bytes(0x1000))
        if d == 0 and at + len(st) > dir_base[1]: raise ValueError('directory 0 overflows')
        body[at:at + len(st)] = st; used[d] += (len(st) + 3) & ~3
        fd.append(((at - dir_base[d]) // 4, len(st), usz, flags))
        return len(fd) - 1
    for raw, how, d in old:
        n = put(raw, how, d); f2d.append((d, n, 0)); fi.append((0, 0, len(f2d) - 1, 0))
    ks = []
    for pi, (name, ext, entry, datas) in enumerate(paths):
        c, l = h40(name); ec, el = h40(ext)
        if isinstance(entry, tuple):
            k = ks[entry[1]]
        else:
            first, last = len(f2d), None
            for raw, how, d in datas:
                n = last if how == ('same',) else put(raw, how, d); last = n; f2d.append((d, n, 0))
            if len(datas) > 1:
                while len(f2d) - first < regions: f2d.append(f2d[-1])
            fi.append((pi, len(idx), first, 0x8000 if len(datas) > 1 else 0))
            k = len(idx); idx.append((0, len(fi) - 1 if entry != 'bad_fi' else 0xfffff))
            if entry == 'bad_k': k = 0xffff
        ks.append(k)
        prow.append(struct.pack('<8I', c, l | k << 8, ec, el, 0, 0, c ^ 1, l))
    n_paths, n_idx, n_dirs = len(prow), len(idx), 1
    n_do, n_hfold, n_fi, n_f2d, n_fd = 2, 2, len(fi), len(f2d), len(fd)
    # the reader sums: directory offsets n_off1 + n_off2 + extra, file infos n_info + n_data2 + extra files, info-to-data n_sub + n_data2 + extra, data n_data + n_data2 + extra files
    n_off2, extra_dirs, n_data2, extra_files, extra_f2d = (1, 0, 1, 1, 1) if counts_split else (0, 0, 0, 0, 0)
    t = bytearray(0x58)
    struct.pack_into('<12I', t, 0, 0, n_paths, n_idx, n_dirs, n_do - n_off2 - extra_dirs, n_hfold, n_fi - n_data2 - extra_files, n_f2d - n_data2 - extra_f2d,
                     n_fd - n_data2 - extra_files, n_off2, n_data2, 0)
    struct.pack_into('<II', t, 0x30, 0x10, 0x10); t[0x38] = regions; t[0x39] = 2
    struct.pack_into('<7I', t, 0x3c, 0x000d0005, extra_dirs, extra_files, 0, extra_files, extra_f2d, 0)
    t += bytes(regions * 12)
    t += struct.pack('<4I', 1, 1, 2, 1) + bytes(12 + 20 + 8 + 16)
    t += struct.pack('<2I', 1, 2) + bytes(2 * 8 + 1 * 8)
    t += b''.join(prow)
    t += b''.join(struct.pack('<II', a, b) for a, b in idx)
    t += bytes(n_dirs * (8 + 0x34))
    for i in range(n_do): t += struct.pack('<Q5I', dir_base[i], 0, 0, 0, 0, i)
    t += bytes(n_hfold * 8)
    t += b''.join(struct.pack('<4I', *x) for x in fi)
    t += b''.join(struct.pack('<3I', *x) for x in f2d)
    t += b''.join(struct.pack('<4I', *x) for x in fd)
    t += bytes(pad_tsize)
    struct.pack_into('<I', t, 0, len(t) if tsize is None else tsize)
    file_off = 0x100
    fs_off = file_off + len(body)
    comp = zstd.compress(bytes(t))
    head = SMASH_MAGIC + struct.pack('<6Q', 0x38, file_off, fs_off, fs_off, fs_off + 16 + len(comp), 0)
    return head + bytes(file_off - len(head)) + bytes(body) + struct.pack('<4I', 0x10, len(t), len(comp), 16 + len(comp)) + comp


def ssbh(kind, payload):   # an SSBH file ('HBSS'): its resource type at +0x10, reversed in the label
    return b'HBSS' + bytes(0xc) + kind[::-1] + bytes(0xc) + payload


# ---- Trinity (FlatBuffers) ---------------------------------------------------------------------------------------------------------------------
class FB:
    """A small FlatBuffers writer: tables (a vtable right before each, soffset positive), their vectors and strings placed after the table that points at them"""
    def __init__(self, base=0, buf=None): self.b = bytearray(buf or b''); self.base = base
    def pos(self): return self.base + len(self.b)
    def align(self, n):
        while len(self.b) % n: self.b.append(0)
    def table(self, fields):
        """fields: [None | ('u64', v) | ('i8', v) | ('u8', v) | ('vq', [u64]) | ('vb', bytes) | ('vs', [str]) | ('vt', [fields])]; returns the table's position"""
        self.align(4)
        inline, refs = [], []
        for i, f in enumerate(fields):
            if f is None: continue
            k, v = f
            size = {'u64': 8, 'i8': 1, 'u8': 1}.get(k, 4)
            inline.append((i, k, v, size))
        vt_at = len(self.b)
        nf = len(fields)
        self.b += struct.pack('<HH', 4 + 2 * nf, 0) + bytes(2 * nf)
        self.align(4)
        t_at = len(self.b)
        self.b += struct.pack('<i', t_at - vt_at)
        for i, k, v, size in inline:
            off = len(self.b) - t_at
            struct.pack_into('<H', self.b, vt_at + 4 + 2 * i, off)
            if k == 'u64': self.b += struct.pack('<Q', v)
            elif k == 'i8': self.b += struct.pack('<b', v)
            elif k == 'u8': self.b += struct.pack('<B', v)
            else: refs.append((len(self.b), k, v)); self.b += bytes(4)
        struct.pack_into('<H', self.b, vt_at + 2, len(self.b) - t_at)
        for at, k, v in refs: self.patch(at, self.obj(k, v))
        return self.base + t_at
    def patch(self, at, target): struct.pack_into('<I', self.b, at, target - (self.base + at))
    def obj(self, k, v):
        self.align(8 if k == 'vq' else 4)
        if k == 'vq':
            if len(self.b) % 8 == 0: self.b += bytes(4)   # the elements 8-aligned after the count
            at = self.pos(); self.b += struct.pack(f'<I{len(v)}Q', len(v), *v); return at
        if k == 'vb': at = self.pos(); self.b += struct.pack('<I', len(v)) + v; return at
        at = self.pos(); n = len(v); self.b += struct.pack('<I', n) + bytes(4 * n); slots = [at - self.base + 4 + 4 * j for j in range(n)]
        for j, x in enumerate(v):
            if k == 'vs': self.align(4); s = self.pos(); bb = x.encode(); self.b += struct.pack('<I', len(bb)) + bb + b'\0'
            else: s = self.table(x)
            self.patch(slots[j], s)
        return at
    def root(self, fields):   # a buffer: u32 root offset, then the table
        start = len(self.b); self.b += bytes(4); t = self.table(fields); self.patch(start, t); return self.base + start


def trinity(packs, extra_desc_files=(), name_of=None, sizes=None):
    """packs: [(name, [(hash, file fields)])] in file order; file fields: dict comp / level / length / data (absent when missing).
    → (data.trpfs, data.trpfd). The descriptor lists every pack's files (and extra_desc_files: (hash, pack index)); packs appear in the trpfs in the order given"""
    fs = bytearray(b'ONEPACK\0' + bytes(8))
    offs = []
    for name, files in packs:
        while len(fs) % 16: fs.append(0)
        at = len(fs); offs.append(at)
        w = FB(at)
        tabs = []
        for h, f in files:
            tabs.append([None, ('i8', f['comp']) if 'comp' in f else None, ('u8', f.get('level', 0)), ('u64', f['length']) if 'length' in f else None, ('vb', f['data']) if 'data' in f else None])
        w.root([('vq', [h for h, _ in files]), ('vt', tabs)])
        fs += w.b
    while len(fs) % 16: fs.append(0)
    idx_at = len(fs)
    w = FB(idx_at)
    w.root([('vq', [zlib.crc32(n.encode()) for n, _ in packs]), ('vq', offs)])
    fs += w.b
    struct.pack_into('<Q', fs, 8, idx_at)
    # the descriptor: file hashes sorted, each with its pack's index; pack names; pack infos
    rows = sorted([(h, pi) for pi, (_, files) in enumerate(packs) for h, _ in files[:1]] + [(h, pi) for pi, (_, files) in enumerate(packs) for h, _ in files[1:]] + list(extra_desc_files))
    names = [n for n, _ in packs] if name_of is None else name_of
    sz = sizes or [sum(len(f.get('data', b'')) + 64 for _, f in files) for _, files in packs]
    d = FB()
    d.root([('vq', [h for h, _ in rows]), ('vs', names), ('vt', [[('u64', pi)] if pi else [None] for _, pi in rows]), ('vt', [[('u64', s), ('u64', len(files))] for s, (_, files) in zip(sz, packs)])])
    return bytes(fs), bytes(d.b)


def stored(data): return {'comp': 0, 'length': len(data), 'data': data}


# ---- the run ----------------------------------------------------------------------------------------------------------------------------
def write_game(g, files, main_extra=b''):
    shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    text = Rng(9).bytes(0x1000) + bytes(main_extra) + Rng(10).bytes(0x200)
    open(g + '/main', 'wb').write(nso(clean(text[:0x1000]) + bytes(main_extra) + clean(text[0x1000:])))
    os.makedirs(f'{g}/profile/{GAME}'); open(f'{g}/profile/{GAME}/game.tsv', 'w').write(GAME_TSV)


def run_python(g):
    """→ (return code, stdout lines with @ROMFS@, stderr, the work folder)"""
    work = tempfile.mkdtemp(); os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1')
    env.pop('DM_OODLE_HOST', None); env.pop('DM_OODLE_MODULES', None)
    p = subprocess.run([sys.executable, '-I', engine + '/dm/gfx/tools/shader_split.py', g + '/RomFS', g + '/main', work + '/pkg'] + [f'{work}/tables/{t}' for t in tables],
                       env=env, capture_output=True, text=True)
    log = [l.replace(g + '/RomFS', '@ROMFS@') for l in p.stdout.splitlines() if not l.startswith('progress')]
    return p.returncode, log, p.stderr.replace(g + '/RomFS', '@ROMFS@'), work


def outputs(work):
    expected = {}
    for sub in ('pkg', 'tables'):
        if not os.path.isdir(f'{work}/{sub}'): continue
        for fn in sorted(os.listdir(f'{work}/{sub}')):
            b = open(f'{work}/{sub}/{fn}', 'rb').read()
            if fn == 'shader_map.tsv':
                lines = b.decode().splitlines(keepends=True); lines[1] = PORT_LINE2; b = ''.join(lines).encode()
            expected[f'{sub}/{fn}'] = sha(b)
    return expected


def make_game(name, files, main_extra=b''):
    g = f'{data_root}/games/{name}'; write_game(g, files, main_extra)
    rc, log, err, work = run_python(g)
    if rc: sys.exit(f'{name}: the Python split failed ({rc}): {err[-2000:]}')
    expected = outputs(work)
    n = sum(1 for l in open(f'{work}/tables/shader_map.tsv') if not l.startswith(('#', 'package\t')))
    json.dump({'shaders': n, 'files': expected, 'dlc': [], 'updates': [], 'log': log}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    shutil.rmtree(work)
    print(name, n, 'shaders', len(expected), 'files;', *log, sep='\n  ')


def make_err_game(name, files):
    g = f'{data_root}/games-err/{name}'; write_game(g, files)
    rc, log, err, work = run_python(g); shutil.rmtree(work)
    last = err.strip().splitlines()[-1] if err.strip() else ''
    if not rc or not last.startswith('[RED] '): sys.exit(f'{name}: the Python split did not end in a red line ({rc}): {err[-2000:]}')
    json.dump({'error': last}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    print(name, '→', last)


# Smash: zstd SSBH shader databases (GLSLC outputs: the fallback after scan_blob), an effect ('EFFN') stored, an RYHP effect scan_blob reads, a regional file (one region shares
# region 0's copy, one has its own), a superseded copy no path reaches (its shader must not appear), two paths to one data (read once), a path past the path-to-info table and one
# past the file infos (skipped), an entry of stored length 0 (not planned) and one of length 0 (planned, not read), a stored entry longer than its length (cut), an SSBH whose type
# is not alphanumeric ('data'), data in the second directory; an .arc without the magic and a file with the magic that is not an .arc (plain files)
G = lambda *st: glslc([S(s, n) for s, n in st])
FX = lambda *p: ryhp([S(s, n) for s, n in p])
cut_body = ssbh(b'SHDR', G(('vs', 115), ('ps', 116)))
smash_paths = [
    ('fighter/mario/shader.nushdb', 'nushdb', None, [(ssbh(b'SHDR', bytes(0x20) + G(('vs', 101), ('ps', 102)) + bytes(0x40) + G(('cs', 103))), 'zstd', 0)]),
    ('effect/fighter/mario/ef_mario.eff', 'eff', None, [(b'EFFN' + bytes(0x1c) + G(('vs', 104), ('ps', 105)), 'stored', 1)]),
    ('effect/system/common.eff', 'eff', None, [(FX(('vs', 106), ('ps', 107)), 'zstd', 1)]),
    ('ui/layout/regional.arc', 'arc', None, [(b'EFFN' + bytes(12) + G(('ps', 108)), 'zstd', 0), (None, ('same',), 0), (b'EFFN' + bytes(12) + G(('ps', 109)), 'zstd', 1)]),
    ('render/shader/bin/twice_a.nushdb', 'nushdb', None, [(ssbh(b'SHDR', G(('vs', 110))), 'zstd', 0)]),
    ('render/shader/bin/twice_b.nushdb', 'nushdb', ('alias', 4), []),
    ('render/shader/bin/past_k.nushdb', 'nushdb', 'bad_k', [(ssbh(b'SHDR', G(('vs', 111))), 'zstd', 0)]),
    ('render/shader/bin/past_fi.nushdb', 'nushdb', 'bad_fi', [(ssbh(b'SHDR', G(('vs', 112))), 'zstd', 0)]),
    ('render/shader/bin/empty_stored.nushdb', 'nushdb', None, [(ssbh(b'SHDR', G(('vs', 113))), ('csz0',), 0)]),
    ('render/shader/bin/zero_len.nushdb', 'nushdb', None, [(ssbh(b'SHDR', G(('vs', 114))), ('stored', 0), 1)]),
    ('render/shader/bin/cut.nushdb', 'nushdb', None, [(cut_body + G(('cs', 117)), ('stored', len(cut_body)), 1)]),
    ('render/shader/bin/odd.nushdb', 'nushdb', None, [(b'HBSS' + bytes(0xc) + b'R\x01HS' + bytes(0xc) + G(('ps', 118)), 'zstd', 1)]),
    ('stream/other.bin', 'bin', None, [(clean(Rng(77).bytes(0x300)), 'zstd', 0)]),
]
smash_old = [(ssbh(b'SHDR', G(('vs', 190), ('ps', 191))), 'zstd', 0)]
smash = smash_arc(smash_paths, old=smash_old)
make_game('sync-smash', {
    'data.arc': smash,
    'Data/other.arc': b'XARC' + clean(Rng(12).bytes(0x400)),   # not MT Framework's ARC either (another reader)
    'Data/notarc.bin': SMASH_MAGIC + clean(Rng(13).bytes(0x100)) + G(('vs', 120)),
})

# Trinity: four packs, stored files holding RYHP effects, a file with no compression field (stored), a file whose data vector is absent with length 0, a file of compression 0
# whose data is not its length (red: that range's shaders are left out); data.trpfs beside a second .trpfs without a descriptor (a plain file), a .trpfd alone
packs = [
    ('arc/pack_a.trpak', [(0x1111000000000001, stored(FX(('vs', 201), ('ps', 202)))), (0x1111000000000002, stored(clean(Rng(31).bytes(0x200))))]),
    ('arc/pack_b.trpak', [(0x2222000000000001, {'length': len(FX(('vs', 203))), 'data': FX(('vs', 203))}), (0x2222000000000002, {'comp': 0, 'length': 0})]),
    ('arc/pack_c.trpak', [(0x3333000000000001, stored(FX(('ps', 204), ('cs', 205)))), (0x3333000000000002, {'comp': 0, 'length': 9, 'data': b'short'})]),
    ('arc/pack_d.trpak', [(0x4444000000000001, stored(FX(('vs', 206), ('ps', 207))))]),
]
trpfs, trpfd = trinity(packs)
make_game('sync-trinity', {
    'arc/data.trpfs': trpfs,
    'arc/data.trpfd': trpfd,
    'arc/lonely.trpfs': b'ONEPACK\0' + clean(Rng(41).bytes(0x80)) + FX(('vs', 210)),
    'arc/other.trpfd': trpfd[:0x40],
    'Data/loose.fx': FX(('ps', 211)),
})

# errors the Python ends the whole split with (red() in a planner, before any job): the engine's text
bad = bytearray(smash); fsoff = struct.unpack_from('<Q', smash, 0x20)[0]; struct.pack_into('<I', bad, fsoff + 4, struct.unpack_from('<I', smash, fsoff + 4)[0] + 1)
make_err_game('err-smash-size', {'data.arc': bytes(bad)})
make_err_game('err-smash-layout', {'data.arc': smash_arc(smash_paths[:3], tsize=0x80)})
_, fd_l = trinity(packs[:2])
fs_l, _ = trinity([packs[0], ('arc/pack_b.trpak', [(0x2222000000000009, stored(b'x'))] + packs[1][1])])   # pack_b's first file is not in the descriptor
make_err_game('err-trinity-unlisted', {'arc/data.trpfs': fs_l, 'arc/data.trpfd': fd_l})
