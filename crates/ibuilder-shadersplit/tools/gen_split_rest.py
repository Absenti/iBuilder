#!/usr/bin/env python3
"""Writes tests/data/games/rest-*: synthetic games for the readers ported from nxnative 033793f0 after the earlier syncs: agl BAHS binary records (5df3dbe8), UbiArt IPK v5 (e0c30043),
REDengine 3 RDHS / FZLC caches (a4c82979), UE3 zero padding (02614c26), MT Framework ARC v9 and headed SPK (c931e584), PACx 403 bounds (760c7fe8), RAGE WAD VFP arrays (19c5379d),
NXCX archives (0ec204f5), Telltale .ttarch2 (c714bb92), GIANTS SHARC (f3950c50), in-process zstd with concatenated frames (6aaf73ba), and three readers the port refused before: Virtuos
VBF archives (vbf.py, bee218d9), Star Fox's chained-LZ4 zip members (viper_zip_member) and play captures (glslc_play, ae5d8f51: the split's ninth argument). Each game is a RomFS folder, an NSO `main` and a
profile, every byte made here (nothing comes from a game), split by the UPSTREAM engine's own `shader_split.py` (its command line); `expected.json` has the sha256 of what it wrote and
its log, in the format of tests/games.rs, which splits the same folders with this crate at 1, 4 and 8 threads.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4> -I tools/gen_split_rest.py <upstream checkout (the folder that holds dm/)> tests/data/games

shader_map.tsv line 2 and the @ROMFS@ path: as tools/gen_sync_readers.py.
"""
import hashlib, json, lzma, os, shutil, struct, subprocess, sys, tempfile, zlib
import lz4.block
from compression import zstd

engine, out_root = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
GAME = '0100TESTTEST0000'
GAME_TSV = f'title_id\t{GAME}\nshader_groups\t/Data/=data /content/=content /dlc/=dlc /Pack/=pack /aoc/=dlc\nshader_note\tsynthetic game\n'
PORT_LINE2 = '# column 5 records where each key first appears, a copy in the game before one in an add-on; no PC shader pairing supplied\n'
CTL, GFX, CS, GLSLC = bytes.fromhex('34127698'), bytes.fromhex('78563412'), bytes.fromhex('79563412'), bytes.fromhex('91688619')
SPH = {'vs': 1, 'hs': 2, 'ds': 3, 'gs': 4, 'ps': 5}


def sha(b): return hashlib.sha256(b).hexdigest()


class Rng:   # xorshift64*, so the bytes do not depend on Python's random module
    def __init__(self, s): self.s = s or 1
    def u64(self):
        x = self.s; x ^= x >> 12; x ^= (x << 25) & (2**64 - 1); x ^= x >> 27; self.s = x
        return (x * 0x2545F4914F6CDD1D) & (2**64 - 1)
    def bytes(self, n): return bytes(self.u64() >> 56 for _ in range(n))


MAGICS = (CTL, GLSLC, GFX, CS, b'\x28\xb5\x2f\xfd', b'BTCD', b'BNSH', b'BAHS', b'MGFX', b'HNSH', b'SLCT', b'NVNsh', b'Ninty', b'NXVS', b'NXFS', b'NXCS', b'NXGS', b'NXHS', b'NXDS',
          b'ZCTT', b'RAHS', b'Yaz0', b'SARC', b'PACx')


def clean(b):   # no magic a reader looks for inside random bytes
    b = bytearray(b)
    for m in MAGICS:
        i = b.find(m)
        while i >= 0: b[i] ^= 0x55; i = b.find(m, i + 1)
    return bytes(b)


def program(st, seed, nctl=0x880):
    """(control section, code area) of one NVN stage (as tools/gen_sync5.py)"""
    r = Rng(seed * 7919 + 13)
    code = bytearray(CS if st == 'cs' else GFX) + clean(r.bytes(0x2c))
    sph = bytearray(clean(r.bytes(0x50)))
    if st != 'cs':
        struct.pack_into('<I', sph, 0, (SPH[st] << 10) | 1 | (0x40 if st == 'vs' else 0x20))
        if st == 'ps': sph[0x18] = 1
    code += sph if st != 'cs' else b''
    code += clean(r.bytes(0x40 + 8 * (seed % 5)))
    ctl = bytearray(clean(r.bytes(nctl))); ctl[:4] = CTL
    struct.pack_into('<I', ctl, 0x20, nctl); struct.pack_into('<I', ctl, 0x704, len(code))
    ctl[0x748:0x758] = bytes(16)
    if st == 'cs': struct.pack_into('<3I', ctl, 0x748, 64, 1, 1); struct.pack_into('<I', ctl, 0x754, 0x100)
    return bytes(clean_tail(ctl)), bytes(code)


def clean_tail(ctl):   # the control magic only at +0
    c = bytearray(ctl); c[4:] = clean(bytes(c[4:])); return c


def nvn2(st, seed, nctl=0xe0, ncode=0x100):
    """(control section, code area) of one NVN2 stage: version 0xc9, its length at +0x30, the code's at +0x44, the stage byte at +0x56 (5 compute), the SPH word at the code's start"""
    r = Rng(seed * 104729 + 7)
    code = bytearray(clean(r.bytes(ncode)))
    if st != 'cs': struct.pack_into('<I', code, 0, (SPH[st] << 10) | 1)
    ctl = bytearray(clean(r.bytes(nctl))); ctl[:8] = CTL + b'\xc9\0\0\0'
    struct.pack_into('<I', ctl, 0x30, nctl); struct.pack_into('<I', ctl, 0x44, len(code)); ctl[0x56] = 5 if st == 'cs' else 0
    if st == 'cs': struct.pack_into('<3I', ctl, 0x58, 32, 1, 1)
    return bytes(ctl), bytes(code)


def glslc(stages):
    d = bytearray(0x90 * (len(stages) + 1)); d[:4] = GLSLC; struct.pack_into('<I', d, 0x4c, len(stages))
    for i, (ctl, code) in enumerate(stages):
        at = len(d); d += ctl + code
        h = 0x90 + 0x90 * i; struct.pack_into('<III', d, h, len(ctl) + len(code), at, 0); struct.pack_into('<4I', d, h + 0x30, 0, len(ctl), len(code), len(ctl))
    struct.pack_into('<I', d, 0x44, len(d))
    return bytes(d)


def nso(text):
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, 0), (0x100 + len(text), end, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text


S = program
G = lambda *sts, seed=0: glslc([S(st, seed + i) for i, st in enumerate(sts)])


# ---- agl BAHS (5df3dbe8) ----------------------------------------------------------------------------------------------------------------
def bahs(records, programs, stray=False, damage=None):
    """records: [(kind, (ctl, code))], kind 0 vertex 1 fragment 3 compute; programs: [(name, first binary index)]. The binary array from 0x20 (each record: u32 size, u32 kind, two u32, the
    16-byte entry {code offset, 0, control length, control distance}, the control section 0x20 after the entry), the program array right after it, a stray descriptor after that (an entry
    outside every record that pairs record 0's control with record 1's code), the code region at a 0x100 boundary"""
    arr = 0x20
    recs = bytearray(); offs, at = [], 0
    for kind, (ctl, code) in records:
        offs.append(at); at += (len(code) + 0xff) & ~0xff
    for i, (kind, (ctl, code)) in enumerate(records):
        size = (0x30 + len(ctl) + 3) & ~3
        r = bytearray(size); struct.pack_into('<IIII', r, 0, size, kind, 0, 0); struct.pack_into('<IIIi', r, 0x10, offs[i], 0, len(ctl), 0x20); r[0x30:0x30 + len(ctl)] = ctl
        recs += r
    binary = struct.pack('<II', 8 + len(recs), len(records)) + recs
    progs = bytearray()
    for name, base in programs:
        nm = name.encode() + b'\0'; size = (16 + len(nm) + 3) & ~3
        p = bytearray(size); struct.pack_into('<IIII', p, 0, size, len(nm), 3, base); p[16:16 + len(nm)] = nm; progs += p
    tables = binary + struct.pack('<II', 8 + len(progs), len(programs)) + progs
    d = bytearray(arr) + tables + bytes(0x40)
    if stray:   # record 0's control, record 1's code (same code length), four-byte aligned
        s = len(d) - 0x30; s &= ~3
        c0 = arr + 8 + 0x30
        struct.pack_into('<IIIi', d, s, offs[1], 0, len(records[0][1][0]), c0 - s)
    region = (len(d) + 0xff) & ~0xff
    d += bytes(region - len(d))
    for (kind, (ctl, code)), o in zip(records, offs):
        d += bytes(region + o - len(d)) + code
    d += bytes((len(d) + 0xff & ~0xff) - len(d))
    d[:4] = b'BAHS'; struct.pack_into('<II', d, 0x18, len(d) - region, region)
    if damage: damage(d, arr)
    return bytes(d)


def bahs_records(seed, nvn=2):
    mk = (lambda st, s: nvn2(st, s)) if nvn == 2 else (lambda st, s: program(st, s))
    return [(0, mk('vs', seed)), (1, mk('ps', seed + 1)), (0, mk('vs', seed + 2)), (1, mk('ps', seed + 3)), (3, mk('cs', seed + 4))]


# ---- UbiArt IPK v5 / v7 (e0c30043) ------------------------------------------------------------------------------------------------------
def ipk(entries, ver=5):
    """entries: [(directory, name, raw, compressed)]; version 5: a 0x30-byte header, name then directory; version 7: 0x38, directory then name"""
    hdr = 0x30 if ver == 5 else 0x38
    t = bytearray(); data = bytearray()
    for dname, name, raw, comp in entries:
        b = zlib.compress(raw) if comp else raw
        s1, s2 = (name, dname) if ver == 5 else (dname, name)
        t += struct.pack('>III', 1, len(raw), len(b) if comp else 0) + struct.pack('>Q', 0) + struct.pack('>Q', len(data))
        t += struct.pack('>I', len(s1)) + s1.encode('latin1') + struct.pack('>I', len(s2)) + s2.encode('latin1') + struct.pack('>II', 0x1234, 0)
        data += b
    base = hdr + len(t)
    return struct.pack('>IIIII', 0x50ec12ba, ver, 0, base, len(entries)) + bytes(hdr - 20) + bytes(t) + bytes(data)


# ---- REDengine 3 (a4c82979) -------------------------------------------------------------------------------------------------------------
def rdhs(blobs, index=b'materials/a.w2mg\0', raw_delta=0, flag=1, ver=3):
    d = bytearray()
    for i, b in enumerate(blobs):
        z = zstd.compress(b); d += struct.pack('<QBII', 0x1000 + i, flag if i == 0 else 1, len(b) + (raw_delta if i == len(blobs) - 1 else 0), len(z)) + z
    at = len(d)
    return bytes(d) + index + struct.pack('<Q4sI', at, b'RDHS', ver)


def fzlc(data, chunk, compress=True):
    parts = [data[i:i + chunk] for i in range(0, len(data), chunk)]
    stored = [zstd.compress(p) if compress else p for p in parts]
    at = 12 + 12 * len(parts); head = bytearray(b'FZLC' + struct.pack('<II', len(parts), at)); pos = at
    for p, s in zip(parts, stored): pos += len(s); head += struct.pack('<3I', len(s), len(p), pos)
    return bytes(head) + b''.join(stored)


# ---- Unreal Engine 3 packages (02614c26) ------------------------------------------------------------------------------------------------
def ue3pkg(body, bs=0x1000, pad=b''):
    nb = (len(body) + bs - 1) // bs; rows, blocks = [], b''
    for i in range(nb):
        raw = body[i * bs:(i + 1) * bs]; z = zlib.compress(raw)
        if len(z) >= len(raw): z = raw
        rows.append((len(z), len(raw))); blocks += z
    tag = bytes([0xc1, 0x83, 0x2a, 0x9e])
    chunk = tag + struct.pack('<III', bs, len(blocks), len(body)) + b''.join(struct.pack('<II', c, u) for c, u in rows) + blocks
    return tag + struct.pack('<H', 0x360) + clean(Rng(5).bytes(60)) + chunk + pad


def ue3_rec(st, seed):
    ctl, code = program(st, seed)
    return struct.pack('<III', 8 + len(code) + len(ctl), len(code), len(ctl)) + code + ctl


# ---- MT Framework ARC v7 / v9, SPK (c931e584; dm/gfx/tests/test_mt_framework.py's layouts) ----------------------------------------------
def spk_control_first(shaders):
    return b'SPK\0' + bytes(28) + b''.join(ctl + code for ctl, code in shaders)


def spk_headed(shaders, h=b'\xf2R\xa3c', extra_magic=False):
    p = bytearray(b'SPK\0' + h + bytes(24))
    for i, (ctl, code) in enumerate(shaders):
        p += bytes(8 + 4 * i) + struct.pack('<4s7I', h, len(ctl) + len(code), 0x10003, 4, 0, len(ctl), len(code), len(ctl)) + ctl + code
    if extra_magic: p += bytes(16) + CTL + bytes(12)
    return bytes(p + bytes(20))


def arc_v9(rows, bad_offset=False):
    """rows: [(path, plain, how)], how: 'z' a zlib stream, 's' stored (a stored member that is itself a zlib stream of another length stays stored)"""
    table, data = bytearray(struct.pack('<4sHH', b'ARC\0', 9, len(rows))), bytearray()
    for path, plain, how in rows:
        stored = zlib.compress(plain) if how != 's' else plain
        size = len(plain)
        off = 0x8000 + len(data) if not bad_offset else 0x10
        table.extend(struct.pack('<64s5I', path, 0x02358e1a, len(stored), size, 2, off))
        data.extend(stored)
    return bytes(table.ljust(0x8000, b'\0') + data)


def arc_v7(rows, width=64, ver=7):
    """rows: [(path, stored, plain length)]"""
    table = bytearray(struct.pack('<4sHH', b'ARC\0', ver, len(rows)))
    data = bytearray()
    for path, stored, n in rows:
        off = 8 + len(rows) * (width + 16) + len(data)
        table.extend(struct.pack(f'<{width}s4I', path, 1, len(stored), n | (1 << 29), off))
        data.extend(stored)
    return bytes(table + data + bytes(16))


# ---- PACx 403 (760c7fe8; dm/gfx/tests/test_needle.py's pacx403) -------------------------------------------------------------------------
def pacx403_cut(deps=0x18):
    root = b'PACx402L' + bytes(0x28)
    d = bytearray(0x30 + deps + 4 + 8)
    d[:8] = b'PACx403L'
    struct.pack_into('<III4xII', d, 0x10, len(d), len(root), len(root), deps, 12)
    struct.pack_into('<III', d, 0x30 + deps, 1, len(root), len(root))
    return bytes(d + root)


def pacx402_root_cut():
    root = b'PACx402LShader'
    d = bytearray(0x3c); d[:8] = b'PACx403L'
    struct.pack_into('<III4xII', d, 0x10, len(d), len(root), len(root), 0, 12)
    struct.pack_into('<III', d, 0x30, 1, len(root), len(root))
    return bytes(d + root)


# ---- RAGE WAD VFP (19c5379d) ------------------------------------------------------------------------------------------------------------
def vfp(stages, bad_span=False):
    headers, ctls, codes = bytearray(), bytearray(), bytearray()
    for ctl, code in stages:
        h = bytearray(clean(Rng(len(headers) + 77).bytes(0x120))); h[:4] = GLSLC
        struct.pack_into('<4I', h, 0xc0, len(ctls) + (4 if bad_span and headers else 0), len(codes), len(code), len(ctl))
        headers += h; ctls += ctl; codes += code
    return struct.pack('<4Q', 0, len(headers), len(ctls), len(codes)) + bytes(0x20) + headers + ctls + codes


def wad(members, dir_order=None):
    """members: [(name, bytes)] in data order; the directory lists them in dir_order (its last entry must name the member that ends the data: the names follow it), the names in the
    directory's order"""
    n = len(members); start = 8 + 12 * n
    order = dir_order or list(range(n))
    assert order[-1] == n - 1
    offs, data = [], bytearray()
    for _, b in members: offs.append(start + len(data)); data += b
    d = bytearray(b'WAD\x01' + struct.pack('<I', n))
    for i in order: d += struct.pack('<III', 0x77, offs[i], len(members[i][1]))
    d += data
    for i in order: nm = members[i][0].encode(); d += struct.pack('<H', len(nm)) + nm
    return bytes(d)


# ---- Telltale .ttarch2 (c714bb92; dm/gfx/tests/test_ttarch2.py's builders) --------------------------------------------------------------
def deflate(b):
    c = zlib.compressobj(9, zlib.DEFLATED, -15)
    return c.compress(b) + c.flush()


def zctt(data, chunk=0x10000, damage_chunk=None, cut_chunk=None):
    parts = [deflate(data[i:i + chunk]) for i in range(0, len(data), chunk)]
    if damage_chunk is not None: parts[damage_chunk] = b'\x07' + parts[damage_chunk][1:]
    if cut_chunk is not None: parts[cut_chunk] = parts[cut_chunk][:len(parts[cut_chunk]) // 2]
    head = 12 + 8 * (len(parts) + 1)
    offs, at = [], head
    for p in parts: offs.append(at); at += len(p)
    return b'ZCTT' + struct.pack('<II', 0x10000, len(parts)) + struct.pack(f'<{len(parts) + 1}Q', *offs, at) + b''.join(parts)


def tt_records(stages, gap=37):
    out = bytearray()
    for i, (ctl, code) in enumerate(stages): out += clean(Rng(900 + i).bytes(gap + i)) + code + struct.pack('<I', len(ctl)) + ctl
    return bytes(out)


def att_table(names): return b'4ATT' + b''.join(n.encode() + b'\0' for n in names) + bytes(16)


# ---- GIANTS SHARC (f3950c50; dm/tools/tests/test_split_sharc.py's layout) ---------------------------------------------------------------
FILTERS = [{'id': lzma.FILTER_LZMA1, 'dict_size': 1 << 20, 'lc': 3, 'lp': 0, 'pb': 2}]


def lz(data, marker=True):
    raw = lzma.compress(data, format=lzma.FORMAT_RAW, filters=FILTERS)
    return b'\x5d' + struct.pack('<I', 1 << 20) + (raw if marker else raw[:-1])


def sharc(members, version=11, names=(b'test',)):
    """members: [compressed member bytes]; one metadata block, the members with one property record, one side record and the names"""
    header = struct.pack('<4s10I', b'RAHS', version, 0, 0, 0x2c, 1, len(members), 1, 0, 0, 0)
    metadata = struct.pack('<IH', 4, 1) + bytes(18)
    table = b''.join(struct.pack('<8I', len(m), 1, 1, 0, 0, 0, 0, len(names)) for m in members)
    body = b'meta' + bytes(5)
    for m in members: body += m + bytes(5 + 16) + b''.join(b'\x00' + bytes([len(n)]) + n for n in names)
    return header + metadata + table + bytes(20) + body


# ---- Virtuos VBF (vbf.py) ---------------------------------------------------------------------------------------------------------------
def vbf(members, bad_name=False):
    """members: [(name, data, how)], how 'z' zlib blocks, 'l' raw LZ4 blocks, 's' stored, None a missing entry (offset UINT64_MAX)"""
    count = len(members)
    names = bytearray()
    name_offs = []
    for nm, _, _ in members: name_offs.append(len(names)); names += nm.encode() + b'\0'
    strings = 16 + count * 64
    slen = 4 + len(names)
    blocks_at = strings + slen
    lengths, datas, bis = [], [], []
    for nm, data, how in members:
        bis.append(len(lengths)); out = bytearray()
        for i in range(0, max(len(data), 1), 0x10000):
            raw = data[i:i + 0x10000]
            if not raw: break
            b = zlib.compress(raw) if how == 'z' else lz4.block.compress(raw, store_size=False) if how == 'l' else raw
            if len(b) >= len(raw): b = raw
            lengths.append(0 if len(b) == 0x10000 else len(b)); out += b
        datas.append(bytes(out))
    hlen = blocks_at + 2 * len(lengths)
    head = bytearray(b'SRYK' + struct.pack('<IQ', hlen, count)) + bytes(count * 32)
    pos = hlen
    recs = bytearray()
    for i, (nm, data, how) in enumerate(members):
        off = 0xffffffffffffffff if how is None else pos
        recs += struct.pack('<IIQQQ', bis[i], 0, len(data), off, name_offs[i] + (0x10000 if bad_name and i == 1 else 0))
        if how is not None: pos += len(datas[i])
    head += recs + struct.pack('<I', slen) + names + b''.join(struct.pack('<H', l) for l in lengths)
    assert len(head) == hlen
    return bytes(head) + b''.join(d for d, (_, _, how) in zip(datas, members) if how is not None)


def phyre_blob(stages, seed):
    out = bytearray(clean(Rng(seed).bytes(40)))
    for ctl, code in stages: out += code + ctl + clean(Rng(seed + len(out)).bytes(13))
    return bytes(out)


# ---- Star Fox's data.zip (dm/tools/tests/test_split_viper.py's builders) ----------------------------------------------------------------
def viper(ctl, code): return GLSLC + struct.pack('<IIIQ', len(code), len(ctl), 0, 0x1234567887654321) + code + ctl


def chained(data):
    out, prev = bytearray(), b''
    for at in range(0, len(data), 0x10000):
        chunk = data[at:at + 0x10000]
        block = lz4.block.compress(chunk, store_size=False, dict=prev) if prev else lz4.block.compress(chunk, store_size=False)
        if len(block) >= len(chunk): block = chunk
        out += struct.pack('<I', len(block)) + block; prev = chunk
    return bytes(out)


def vzip(members, preamble=0x40):
    out = bytearray(b'VZIP' + bytes(preamble - 4)); central = bytearray()
    for name, disk, size, crc, method in members:
        nm = name.encode(); at = len(out)
        out += struct.pack('<4sHHHHHIIIHH', b'PK\x03\x04', 45, 0, method, 0, 0, crc, len(disk), size, len(nm), 0) + nm + disk
        central += struct.pack('<4sHHHHHHIIIHHHHHII', b'PK\x01\x02', 45, 45, 0, method, 0, 0, crc, len(disk), size, len(nm), 0, 0, 0, 0, 0, at) + nm
    cd = len(out); out += central
    out += struct.pack('<4sHHHHIIH', b'PK\x05\x06', 0, 0, len(members), len(members), len(central), cd, 0)
    return bytes(out)


def zmember(name, data, chain=True, crc=None):
    disk = chained(data) if chain else data
    return (name, disk, len(data), (zlib.crc32(data) & 0xffffffff) if crc is None else crc, 0)


# ---- the run ----------------------------------------------------------------------------------------------------------------------------
def make_game(name, files, main_extra=b'', play=None):
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True); os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}'), exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    text = Rng(9).bytes(0x1000) + bytes(main_extra) + Rng(10).bytes(0x200)
    open(g + '/main', 'wb').write(nso(clean(text[:0x1000]) + bytes(main_extra) + clean(text[0x1000 + len(main_extra):])))
    os.makedirs(f'{g}/profile/{GAME}'); open(f'{g}/profile/{GAME}/game.tsv', 'w').write(GAME_TSV)
    work = tempfile.mkdtemp(); os.makedirs(work + '/tables')
    tables = ['shader_map.tsv', 'shader_facts.tsv', 'shader_pairs.tsv', 'pso_pairs.tsv']
    extra = []
    if play is not None:   # the play captures beside the game, the manifest (absolute paths) in the work folder: the split's eighth (DLC, none) and ninth arguments
        for p, b in play.items():
            if b is not None: os.makedirs(os.path.dirname(f'{g}/{p}'), exist_ok=True); open(f'{g}/{p}', 'wb').write(b)
        json.dump([f'{g}/{p}' for p in play], open(work + '/play.json', 'w'))
        extra = ['', work + '/play.json']
    env = dict(os.environ, DM_GAMES_DIR=g + '/profile', DM_GAMES_FIRST='1', DM_GAME=GAME, J='2', PYTHONDONTWRITEBYTECODE='1')
    p = subprocess.run([sys.executable, '-I', engine + '/dm/gfx/tools/shader_split.py', g + '/RomFS', g + '/main', work + '/pkg'] + [f'{work}/tables/{t}' for t in tables] + extra,
                       env=env, capture_output=True, text=True)
    if p.returncode: sys.exit(f'{name}: the Python split failed ({p.returncode}): {p.stdout[-2000:]} {p.stderr[-2000:]}')
    log = [l.replace(g + '/RomFS', '@ROMFS@').replace(g, '@GAME@') for l in p.stdout.splitlines() if not l.startswith('progress')]
    expected = {}
    for sub in ('pkg', 'tables'):
        for fn in sorted(os.listdir(f'{work}/{sub}')):
            b = open(f'{work}/{sub}/{fn}', 'rb').read()
            if fn == 'shader_map.tsv':
                lines = b.decode().splitlines(keepends=True); lines[1] = PORT_LINE2; b = ''.join(lines).encode()
            expected[f'{sub}/{fn}'] = sha(b)
    n = sum(1 for l in open(f'{work}/tables/shader_map.tsv') if not l.startswith(('#', 'package\t')))
    exp = {'shaders': n, 'files': expected, 'dlc': [], 'updates': [], 'log': log}
    if play is not None: exp['glslc_play'] = list(play)
    json.dump(exp, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    shutil.rmtree(work)
    print(name, n, 'shaders', len(expected), 'files;', *log, sep='\n  ')


def damage_count(d, arr): struct.pack_into('<I', d, arr + 4, 0xffff)          # more binaries than the array holds: no layout
def damage_overlap(d, arr): struct.pack_into('<I', d, arr, struct.unpack_from('<I', d, 0x1c)[0])   # the array runs into the code region


# BAHS: a validated array with a stray descriptor outside it (dropped, the variations paired); arrays that do not read back (the candidates kept, no pairs); NVN1 records
make_game('rest-bahs', {
    'Data/payload.sharcb': bahs(bahs_records(100), [('ProgA', 0), ('ProgB', 2)], stray=True),
    'Data/count.sharcb': bahs(bahs_records(200), [('ProgA', 0)], stray=True, damage=damage_count),
    'Data/overlap.sharcb': bahs(bahs_records(300), [('ProgA', 0), ('ProgB', 4)], stray=True, damage=damage_overlap),
    'Data/nvn1.sharcb': bahs(bahs_records(400, nvn=1), [('P', 0), ('Q', 2), ('R', 4)]),
})

# IPK: version 5 (name then directory, directories without a trailing slash), version 7, a v5 table cut short, version 6, a file shorter than the v5 header
v5 = [('enginedata/shaders/compiled/nx', 'renderpct.fxb', G('vs', 'ps', seed=1000), True), ('enginedata/shaders_dev/compiled', 'renderpct.fxb', G('ps', seed=1010), False),
      ('cooked', 'world.ckd', clean(Rng(3).bytes(300)), True), ('enginedata/shaders/compiled/nx/', 'font.fxb', G('cs', seed=1020), False), ('', 'root.fxb', G('vs', seed=1030), True)]
v5_cut = ipk(v5[:3]); v5_cut = v5_cut[:0x30 + 60]
v5_cut = v5_cut[:12] + struct.pack('>I', len(v5_cut)) + v5_cut[16:]
make_game('rest-ipk', {
    'Data/bundle_nx.ipk': ipk(v5),
    'Data/legacy.ipk': ipk([('shaders/nx', 'afterfx.fxb', G('vs', 'ps', seed=1100), True), ('x', 'y.ckd', b'data', False)], ver=7),
    'Data/cut.ipk': v5_cut,
    'Data/v6.ipk': (lambda d: d[:4] + struct.pack('>I', 6) + d[8:])(ipk(v5[:1])),
    'Data/tiny.ipk': bytes.fromhex('50ec12ba') + struct.pack('>I', 5) + bytes(0x18),
})

# REDengine 3: material caches (records), static caches (chunks a program runs across), the DLC stub, damaged records and chunks, a flag without the footer
p1, p2, p3 = G('vs', 'ps', seed=2000), G('vs', 'ps', seed=2010), G('cs', seed=2020)
static = fzlc(p1 + G('hs', 'ds', seed=2030) + p3, 0x700)
first = G('vs', seed=2040)
missing = bytearray(fzlc(first + G('ps', seed=2041), len(first), compress=False)); struct.pack_into('<I', missing, 24, 0)
make_game('rest-redengine', {
    'content/content0/shadernx.cache': rdhs([p1, p2]),
    'content/content0/staticshadernx.cache': static,
    'dlc/dlc1/content/staticshadernx.cache': fzlc(bytes(28), 28, compress=False),
    'content/patch0/shadernx.cache': rdhs([G('ps', seed=2050)], raw_delta=4),
    'content/patch1/shadernx.cache': rdhs([G('ps', seed=2060)], flag=2),
    'content/patch2/shadernx.cache': rdhs([G('ps', seed=2070)], ver=2),
    'content/patch3/staticshadernx.cache': bytes(missing),
    'content/patch4/staticshadernx.cache': b'FZLC' + bytes(4),
    'content/patch5/material.bin': rdhs([G('vs', seed=2080)]),
    'Data/other.bin': bytes(8) + b'\x01' + bytes(7) + G('vs', seed=2090),
})

# UE3: a package padded with zeros after its last chunk; one whose tail is not all zeros
body = b''.join(ue3_rec(st, 2100 + i) for i, st in enumerate(['vs', 'ps', 'cs']))
make_game('rest-ue3pad', {
    'Data/padded.xxx': ue3pkg(clean(Rng(11).bytes(100)) + body, pad=bytes(0x7c0)),
    'Data/tail.xxx': ue3pkg(clean(Rng(12).bytes(100)) + b''.join(ue3_rec(st, 2200 + i) for i, st in enumerate(['vs', 'ps'])), pad=bytes(0x20) + b'\x01' + bytes(7)),
})

# MT Framework: ARC v9 with a headed SPK (zlib members, one stored as long as its data, an empty member), v7 control-first SPK with a zlib member that decodes to more than its entry
# says, version 8, a v9 directory that points into the table, a headed SPK with a control magic of no shader
sh = [S('vs', 2300), S('ps', 2301), S('vs', 2302, nctl=0x850)]
hello = zlib.compress(b'hello world, hello world')
make_game('rest-mtarc', {
    'Data/re/root.arc': arc_v9([(b'misc', b'hello', 'z'), (b'sc\\NX_64\\root', spk_headed(sh), 'z'), (b'stored', hello, 's'), (b'empty', b'', 's')]),
    'Data/re/bad.arc': arc_v9([(b'misc', b'hello', 'z')], bad_offset=True),
    'Data/re/extra.arc': arc_v9([(b'sc\\NX_64\\root', spk_headed(sh[:2], extra_magic=True), 'z')]),
    'Data/sf/root.arc': arc_v7([(b'misc', b'hello', 5), (b'sc\\NX_64\\root', zlib.compress(spk_control_first([S('vs', 2310), S('ps', 2311)])), len(spk_control_first([S('vs', 2310), S('ps', 2311)])))]),
    'Data/sf/long.arc': arc_v7([(b'sc\\NX_64\\long', zlib.compress(b'x' * 100), 50)]),
    'Data/sf/v8.arc': arc_v7([(b'misc', b'hello', 5)], ver=8),
})

# PACx 403: headers and the root's chunk table cut short; a root too short for its header
cut = pacx403_cut()
make_game('rest-pacx', {
    'Data/a.pac': cut[:0x24],
    'Data/b.pac': cut[:0x30 + 0x18 + 3],
    'Data/c.pac': cut[:0x34 + 0x18 + 7],
    'Data/d.pac': pacx402_root_cut(),
})

# RAGE WAD: VFP members among other members, by the engine's file name; unordered members; a VFP member whose spans do not continue; another name (read as a loose file)
m1 = vfp([S('vs', 2400), S('ps', 2401), S('cs', 2402)]); m2 = vfp([S('vs', 2410), S('ps', 2411)])
make_game('rest-wad', {
    'Data/a/out.wad.nx': wad([('tex/a.dds', clean(Rng(13).bytes(64))), ('shaders/main.vfp.nx', m1), ('x/y.bin', b'zz'), ('shaders/ui.vfp.nx', m2)]),
    'Data/b/out.wad.nx': wad([('shaders/a.vfp.nx', vfp([S('vs', 2420)])), ('shaders/b.vfp.nx', vfp([S('ps', 2421)])), ('tex/c.dds', b'cc')], dir_order=[1, 0, 2]),
    'Data/c/out.wad.nx': wad([('shaders/a.vfp.nx', vfp([S('vs', 2430), S('ps', 2431)], bad_span=True))]),
    'Data/d/out.wad.nx': wad([('tex/a.dds', b'abc')]),
    'Data/e/other.wad.nx': wad([('shaders/main.vfp.nx', vfp([S('vs', 2440)]))]),
})

# Telltale: a ZCTT archive whose table names an effect pack (records across 64 KB chunks), an NCTT archive (the name in its first block, records in the next, a block with none), an archive
# with no effect pack (not inflated), a damaged and a cut chunk
recs = tt_records([S('vs', 2500), S('ps', 2501), S('cs', 2502), S('vs', 2503, nctl=0x850)] + [S('ps', 2504 + i) for i in range(30)])
zc = zctt(att_table(['SM3_nx_Shaders_mem.t3fxpack', 'a.lua']) + recs)
nctt = b'NCTT' + bytes(0x500) + zctt(att_table(['ShadersPatch.t3fxpack'])) + bytes(9) + zctt(tt_records([S('vs', 2540), S('ps', 2541)])) + zctt(clean(Rng(14).bytes(5000)))
make_game('rest-telltale', {
    'Data/SM3_nx_Shaders.ttarch2': zc,
    'Data/Patch.ttarch2': nctt,
    'Data/Textures.ttarch2': b'NCTT' + bytes(12) + zctt(att_table(['a.d3dtx']) + tt_records([S('vs', 2550)])),
    'Data/Bad.ttarch2': zctt(att_table(['x.t3fxpack']) + recs, damage_chunk=1),
    'Data/Cut.ttarch2': zctt(att_table(['x.t3fxpack']) + recs, cut_chunk=0),
})

# SHARC: members with and without the LZMA end marker (a compute and a two-stage output), one cut inside its output, version 10, an invalid property byte, a damaged first byte
sa, sb, sc = G('cs', seed=2600), G('vs', 'ps', seed=2610), G('ps', seed=2620)
nomark = lz(sb + bytes(2), marker=True)[:-1]
make_game('rest-sharc', {
    'Data/shared/shader_switch.sharc': sharc([lz(sa), nomark, lz(sc)]),
    'Data/cut.sharc': sharc([lz(sa)[:-40]]),
    'Data/v10.sharc': sharc([lz(sa)], version=10),
    'Data/props.sharc': sharc([b'\xff' + lz(sa)[1:]]),
    'Data/corrupt.sharc': sharc([lz(sa)[:5] + b'\x01' + lz(sa)[6:]]),
    'Data/short.sharc': sharc([lz(sa)[:5]]),
})

# NXCX: GLSLC outputs and a BAHS name in a texture archive; one with no magic (nothing read); one with a BNSH magic (the loose-file reader); an output longer than the file
gl = G('vs', 'ps', seed=2700); gl2 = G('cs', seed=2710)
make_game('rest-nxcx', {
    'Data/w_allpac.mrg': b'NXCX' + bytes(12) + clean(Rng(15).bytes(333)) + b'BAHS' + gl + clean(Rng(16).bytes(77)) + gl2 + clean(Rng(17).bytes(50)),
    'Data/plain.mrg': b'NXCX' + bytes(12) + clean(Rng(18).bytes(500)),
    'Data/bnsh.mrg': b'NXCX' + bytes(12) + b'BNSH\0\0\0\0' + bytes(8) + G('ps', seed=2720),
    'Data/long.mrg': b'NXCX' + bytes(12) + gl[:0x200],
})

# zstd: a file of two frames, cut inside a GLSLC output (6aaf73ba: decoded in process, both frames)
two = G('vs', 'ps', seed=2800)
make_game('rest-zstd', {
    'Data/two.zs': zstd.compress(two[:0x400]) + zstd.compress(two[0x400:]),
    'Data/one.zst': zstd.compress(G('cs', seed=2810)),
})

# VBF: zlib and LZ4 blocks (a member over 64 KB), a skipped texture, a missing entry; a VBF whose name offset is outside its string table. Star Fox's zip: chained LZ4 members of shader
# binaries (one over 64 KB, its second chunk against the first), a member whose CRC is not its data's. Play captures: a whole output (one stage a game file also has), one that is not whole,
# a path that is not there
fxa = phyre_blob([S('vs', 2900), S('ps', 2901)] + [S('ps', 2910 + i) for i in range(30)], 31)
vsb = phyre_blob([S('vs', 2950)], 32)
vb = vbf([('shaders/a.fx', fxa, 'z'), ('b.vs.bin', vsb, 'l'), ('tex/t.dds', clean(Rng(19).bytes(200)), 'z'), ('gone.fx', b'', None), ('c.ps.bin', phyre_blob([S('ps', 2960)], 33), 's')])
big = viper(*nvn2('cs', 3000, ncode=0x22000))   # three chunks, the second and third against the one before
shared = S('ps', 3100)
make_game('rest-vbf-zip-play', {
    'Data/ffx.vbf': vb,
    'Data/bad.vbf': vbf([('a.fx', fxa[:500], 'z'), ('b.fx', vsb, 'z')], bad_name=True),
    'Data/data.zip': vzip([zmember('shader/a.bin', viper(*nvn2('vs', 3050))), zmember('shader/big.bin', big), zmember('shader/p.bin', viper(*nvn2('ps', 3051))), zmember('x.txt', b'plain text', chain=False)]),
    'Pack/crc.zip': vzip([zmember('shader/c.bin', viper(*nvn2('ps', 3060)), crc=0x1234)]),
    'Data/loose.glslc': glslc([shared]),
}, play={'play/a.glslc': glslc([S('vs', 3110), shared]), 'play/b.glslc': glslc([S('ps', 3120)])[:-8], 'play/missing.glslc': None})
