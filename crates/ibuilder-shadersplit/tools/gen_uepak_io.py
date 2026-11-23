#!/usr/bin/env python3
"""Writes tests/data/games/ue-io-*/ and ue-aes-*/: synthetic Unreal games with IoStore containers (.utoc / .ucas) and paks whose index is AES-256 encrypted, and `expected.json`
(the tests/games.rs format): what the engine's own `uepak.py` + `shader_split.py` (upstream seedds/nxnative origin/main) write for them. The Rust test (`tests/games.rs`) splits the same
folders and must write the same bytes.

  PYTHONDONTWRITEBYTECODE=1 <python 3.14 with lz4, on macOS (CommonCrypto)> -I tools/gen_uepak_io.py <upstream tree (the folder that holds dm/)> <out folder (tests/data/games)>

As tools/gen_uepak.py (whose helpers this script reuses), the Python runs as the NSP script runs it: `uepak.job` and `finish` first, then `shader_split.py`, with no Oodle host. `uepak.job`
needs a decoder for IoStore shader libraries (the game's Oodle in the Mac host); it gets one that fails if it is ever called, so these games hold no Oodle data the job must decode: the
phone has no Oodle decoder, and an Oodle block where one is needed is an unsupported container there (tests/uepak_io.rs). Oodle blocks in export bundles cost their package, as without a host.

Every byte is a PRNG's or a constant; the AES key is a test key (bytes 7 to 38), not a game's. Nothing comes from a game.
"""
import sys, os, json, struct, tempfile, shutil, subprocess, zlib, ctypes

HERE = os.path.dirname(os.path.abspath(__file__))
exec(open(os.path.join(HERE, 'gen_uepak.py')).read().split('# ---- the games')[0])   # lcg, clean, lslg, filler, zarr, sarr, lz4rec, lz4c, fstr, pak11, legacy_pak, v2_archive, ...
sys.path.insert(0, tools)
import uepak   # the engine's: its CityHash64 names the shader libraries' chunk IDs

KEY = bytes(range(7, 39))   # a test key


def aes_encrypt(key, data):
    data = data + bytes(-len(data) % 16)
    cc = ctypes.CDLL('/usr/lib/libSystem.B.dylib').CCCrypt
    out = ctypes.create_string_buffer(len(data)); moved = ctypes.c_size_t()
    assert cc(0, 0, 2, key, len(key), None, data, len(data), out, len(data), ctypes.byref(moved)) == 0
    return out.raw[:moved.value]


def pad16(b): return b + bytes(-len(b) % 16)


def nso2(text, ro):
    """An NSO with a text and a read-only segment (the AES key search reads the read-only and data segments)"""
    h = bytearray(0x100); h[:4] = b'NSO0'
    end = (len(text) + 0xfff) & ~0xfff
    rend = (end + len(ro) + 0xfff) & ~0xfff
    text = text + bytes(end - len(text))
    for i, (fo, mo, sz) in enumerate([(0x100, 0, len(text)), (0x100 + len(text), end, len(ro)), (0x100 + len(text) + len(ro), rend, 0)]):
        h[0x10 + 0x10 * i:0x1c + 0x10 * i] = struct.pack('<III', fo, mo, sz)
    return bytes(h) + text + ro


# ---- IoStore -------------------------------------------------------------------------------------------------------------------
def chunk_id(n, kind): return struct.pack('<QHBB', n, 0, 0, kind)
def lib_id(name, kind=8): return uepak.city64(name.lower().encode('utf-16-le')).to_bytes(8, 'little') + struct.pack('<HBB', 0, 0, kind)

FREQ = {b'SV': 0, b'SP': 3, b'SC': 5}
def library(groups, hash_size=20):
    """FIoStoreShaderCodeArchive: groups [(chunk ID, [shader records])], every group stored (its raw size its stored size)"""
    recs = [(g, r) for g, (_, rs) in enumerate(groups) for r in rs]
    n = len(recs)
    b = struct.pack('<I', 1) + struct.pack('<I', 1) + lcg(7, hash_size) + struct.pack('<I', n) + b''.join(lcg(100 + i, hash_size) for i in range(n))
    b += struct.pack('<I', len(groups)) + b''.join(gid for gid, _ in groups)
    b += struct.pack('<I', 1) + struct.pack('<II', 0, n)
    ents, offs = [], {}
    for g, (_, rs) in enumerate(groups):
        o = 0
        for r in rs: ents.append(FREQ[r[4:6]] | g << 4 | o << 34); o += len(r)
    b += struct.pack('<I', n) + struct.pack(f'<{n}Q', *ents)
    b += struct.pack('<I', len(groups)); first = 0
    for _, rs in groups:
        raw = sum(map(len, rs)); b += struct.pack('<IIII', first, len(rs), raw, raw); first += len(rs)
    return b + struct.pack(f'<I{n}I', n, *range(n))


def directory(paths, mount='../../../'):
    """FIoDirectoryIndexResource for {chunk index: 'Dir/File'} (upstream's test builder)"""
    none = 0xffffffff
    tops = sorted({v.split('/')[0] for v in paths.values()}); strs = tops + sorted(v.split('/')[1] for v in paths.values())
    dirs = [(none, 1 if tops else none, none, none)]; files = []
    for k, top in enumerate(tops):
        first = none
        for chunk, v in sorted(paths.items()):
            if v.split('/')[0] == top: files.append((strs.index(v.split('/')[1]), first, chunk)); first = len(files) - 1
        dirs.append((strs.index(top), none, k + 2 if k + 1 < len(tops) else none, first))
    b = fstr(mount) + struct.pack('<i', len(dirs)) + b''.join(struct.pack('<4I', *d) for d in dirs)
    b += struct.pack('<i', len(files)) + b''.join(struct.pack('<3I', *f) for f in files)
    return b + struct.pack('<i', len(strs)) + b''.join(fstr(x) for x in strs)


COMP = {0: lambda b: b, 1: zlib.compress, 2: lz4c, 3: lambda b: b'\x8c\x0a' + b}   # stored, Zlib, LZ4, "Oodle" (never decoded: its package is red)
def container(chunks, *, version=8, block=0x800, parts=1, key=None, directory_index=b''):
    """→ {'.utoc': bytes, '.ucas': bytes, '_s1.ucas': ...}. chunks: [(chunk ID, data, method index)], each chunk from a block boundary, cut into blocks of `block` bytes,
    the blocks dealt to the partitions in turn (partition size 1 << 20); key: data and directory index encrypted"""
    part_size = 1 << 20
    ucas = [bytearray() for _ in range(parts)]
    ids, ranges, blocks = bytearray(), bytearray(), bytearray()
    stream, nblk = 0, 0
    for gid, data, meth in chunks:
        ids += gid; ranges += stream.to_bytes(5, 'big') + len(data).to_bytes(5, 'big')
        pieces = [data[i:i + block] for i in range(0, len(data), block)] or [b'']
        for p in pieces:
            k = nblk % parts; stored = COMP[meth](p)
            at = k * part_size + len(ucas[k])
            blocks += at.to_bytes(5, 'little') + len(stored).to_bytes(3, 'little') + len(p).to_bytes(3, 'little') + bytes([meth])
            ucas[k] += aes_encrypt(key, stored) if key else stored
            nblk += 1
        stream += len(pieces) * block
    d = pad16(directory_index) if key and directory_index else directory_index
    if key and d: d = aes_encrypt(key, d)
    head = bytearray(144); head[:16] = uepak.TOC_MAGIC; head[16] = version
    struct.pack_into('<9I', head, 20, 144, len(chunks), nblk, 12, 3, 32, block, len(d), parts)
    head[80] = 1 | (2 if key else 0) | (8 if d else 0)
    if version >= 3 and parts > 1: struct.pack_into('<Q', head, 88, part_size)
    names = b''.join(m.ljust(32, b'\0') for m in (b'Zlib', b'LZ4', b'Oodle'))
    out = {'.utoc': bytes(head + ids + ranges + blocks + names + d)}
    for k, u in enumerate(ucas): out['.ucas' if not k else f'_s{k}.ucas'] = bytes(u)
    return out


def put_container(files, stem, c):
    for suffix, b in c.items(): files[stem + suffix] = b


# ---- encrypted paks ---------------------------------------------------------------------------------------------------------------
def pak11_enc(files, key, mount='../../../', methods=('Zlib', 'Zstd', 'LZ4')):
    """version 11, index and directory index encrypted; files [(name, raw, method index (0 stored), block size, encrypted entry)]"""
    data, enc, dirs = bytearray(), bytearray(), []
    for name, raw, meth, bsz, e in files:
        at = len(data)
        parts = [zlib.compress(raw[i:i + bsz]) for i in range(0, len(raw), bsz)] if meth else []
        nblk = len(parts)
        data += bytes(53 + (4 + 16 * nblk if meth else 0))
        if meth: data += b''.join(aes_encrypt(key, p) if e else p for p in parts)
        else: data += aes_encrypt(key, raw) if e else raw
        bits = 0x3f if meth else 0
        v = bits | (nblk << 6) | (int(e) << 22) | (meth << 23) | (1 << 29) | (1 << 30) | (1 << 31)
        dirs.append((name, len(enc)))
        enc += struct.pack('<I', v)
        if bits == 0x3f: enc += struct.pack('<I', bsz)
        enc += struct.pack('<II', at, len(raw))
        if meth: enc += struct.pack('<I', sum(map(len, parts)))
        if nblk != 1 or e:
            for p in parts: enc += struct.pack('<I', len(p))
    ioff = len(data)
    bydir = {}
    for name, o in dirs:
        d, _, f = name.rpartition('/'); bydir.setdefault('/' + d + '/' if d else '/', []).append((f, o))
    dirindex = struct.pack('<i', len(bydir)) + b''.join(fstr(d) + struct.pack('<i', len(fs)) + b''.join(fstr(f) + struct.pack('<i', o) for f, o in fs) for d, fs in bydir.items())
    index = fstr(mount) + struct.pack('<i', len(files)) + struct.pack('<Q', 0) + struct.pack('<i', 0) + struct.pack('<i', 1)
    tail = 8 + 8 + 20 + 4 + len(enc) + 4
    doff = ioff + len(pad16(index + bytes(tail)))
    index += struct.pack('<QQ', doff, len(dirindex)) + bytes(20) + struct.pack('<i', len(enc)) + bytes(enc) + struct.pack('<i', 0)
    names = b''.join(m.encode().ljust(32, b'\0') for m in (list(methods) + [''] * 5)[:5])
    foot = bytes(16) + b'\1' + struct.pack('<IIQQ', PAK_MAGIC, 11, ioff, len(index)) + bytes(20) + names
    return bytes(data) + aes_encrypt(key, index) + aes_encrypt(key, dirindex) + foot


def legacy4_enc(files, key):
    """A version 4 pak: each file stored behind a 53-byte entry header, its bytes and the index encrypted (Little Nightmares II's layout, upstream's test builder)"""
    data, index = bytearray(), fstr('../../../') + struct.pack('<i', len(files))
    for name, body in files:
        off = len(data)
        data += bytes(53) + aes_encrypt(key, body)
        index += fstr(name) + struct.pack('<qqqi', off, len(body), len(body), 0) + bytes(20) + struct.pack('<BI', 1, 0)
    ioff = len(data)
    data += aes_encrypt(key, bytes(index))
    data += struct.pack('<BIIQQ', 1, PAK_MAGIC, 4, ioff, len(index)) + bytes(20)
    return bytes(data)


def frozen_enc(files, key):
    """gen_uepak.frozen_pak with its frozen index encrypted (version 9, FrozenIndex 1)"""
    p = bytearray(frozen_pak(files))
    foot = len(p) - 222
    ioff, isz = struct.unpack_from('<QQ', p, foot + 25)
    index = bytes(p[ioff:ioff + isz])
    p[foot + 16] = 1
    return bytes(p[:ioff]) + aes_encrypt(key, index) + bytes(p[ioff + isz:])


# ---- running the engine ------------------------------------------------------------------------------------------------------------
JOB = r'''
import sys; sys.path.insert(0, sys.argv[1]); import uepak
def decode(b, n): raise ValueError('the Oodle decoder was called')
try: uepak.job(sys.argv[2], sys.argv[3], sys.argv[4], decode=decode)
except ValueError as e: sys.exit(f'[RED] {e}')
'''

def make_game(name, files, main):
    """files: {path in RomFS: bytes}; main: the NSO bytes"""
    g = f'{out_root}/{name}'; shutil.rmtree(g, ignore_errors=True)
    os.makedirs(g + '/RomFS')
    for p, b in files.items():
        os.makedirs(os.path.dirname(f'{g}/RomFS/{p}') or '.', exist_ok=True); open(f'{g}/RomFS/{p}', 'wb').write(b)
    open(g + '/main', 'wb').write(main)
    os.makedirs(g + '/profile/' + GID); open(f'{g}/profile/{GID}/game.tsv', 'w').write(GAME_TSV)
    work = tempfile.mkdtemp(); c = work + '/game'; shutil.copytree(g, c)
    env = dict(os.environ, DM_GAMES_DIR=c + '/profile', DM_GAMES_FIRST='1', DM_GAME=GID, J='1', PYTHONDONTWRITEBYTECODE='1')
    for k in ('DM_OODLE_HOST', 'DM_OODLE_MODULES', 'DM_UE_SHADERS'): env.pop(k, None)
    run = lambda *a: subprocess.run([PY, '-I', *a], env=env, capture_output=True, text=True, cwd=work)
    r = run('-c', JOB, tools, c + '/RomFS', c + '/main', c + '/uejob')
    if r.returncode: sys.exit(f'{name}: uepak job failed:\n{r.stdout}{r.stderr}')
    r = run(tools + '/uepak.py', 'finish', c + '/uejob', c + '/ue_shaders')
    if r.returncode: sys.exit(f'{name}: uepak.py finish failed:\n{r.stdout}{r.stderr}')
    os.makedirs(work + '/pkg'); os.makedirs(work + '/tables')
    t = lambda n: work + '/tables/' + n
    r = run(tools + '/shader_split.py', c + '/RomFS', c + '/main', work + '/pkg', t('shader_map.tsv'), t('shader_facts.tsv'), t('shader_pairs.tsv'), t('pso_pairs.tsv'))
    if r.returncode: sys.exit(f'{name}: shader_split.py failed:\n{r.stdout}{r.stderr}')
    log = [l.replace(c + '/RomFS', '@ROMFS@') for l in r.stdout.splitlines() if not l.startswith('progress')]
    lines = open(t('shader_map.tsv')).readlines(); lines[1] = OUR_MAP_LINE2; open(t('shader_map.tsv'), 'w').writelines(lines)
    expected = {}
    for sub in ('pkg', 'tables'):
        for fn in sorted(os.listdir(f'{work}/{sub}')): expected[f'{sub}/{fn}'] = sha(open(f'{work}/{sub}/{fn}', 'rb').read())
    n = sum(1 for _ in open(t('shader_map.tsv'))) - 3
    json.dump({'shaders': n, 'files': expected, 'dlc': [], 'updates': [], 'log': log}, open(g + '/expected.json', 'w'), indent=1, sort_keys=True)
    print(name, n, 'shaders;', '\n  '.join(log))
    shutil.rmtree(work)


GAME_TSV = f'title_id\t{GID}\nshader_groups\t/Game/=game /Vermillion/=verm /Proj/=proj /ue_shaders/=ue\nshader_note\tsynthetic Unreal game\n'
plain_main = nso2(lcg(9, 0x1000), filler(8, 0x800))

# ---- 1. FINAL FANTASY RESONANCE's layout (UE 5.6): the shader libraries in the IoStore container, a pak of configs only, a patch container, two partitions ---------------------------------
r = {k: lslg(400 + k, st) for k, st in enumerate(['vs', 'ps', 'cs', 'ps', 'vs', 'ps', 'ps'])}
G1, G2, G3, G4, G5 = (chunk_id(0x10 + i, 9) for i in range(5))
GLOBAL, GAMELIB = 'ShaderArchive-Global-SF_GLSL_SWITCH', 'ShaderArchive-Game-SF_GLSL_SWITCH-SP_GLSL_SWITCH'
manifest = ('Game/Content/Paks/Game-Switch.pak\t0\nGame/Content/' + GLOBAL + '.ushaderbytecode\t1\nGame/Content/' + GAMELIB + '.ushaderbytecode\t2\n').encode()
base = container([
    (lib_id(GLOBAL[len('ShaderArchive-'):]), library([(G1, [r[0], r[1]]), (G2, [r[2]])]), 1),
    (G1, r[0] + r[1], 1), (G2, r[2], 2),
    (lib_id(GAMELIB[len('ShaderArchive-'):]), library([(G3, [r[3]])], hash_size=8), 0),
    (G3, r[3], 0),
    (chunk_id(0x99, 8), library([(G4, [r[4]])]), 2), (G4, r[4], 1),   # a library no manifest names
    (chunk_id(0x40, 1), filler(50, 0x900), 1), (chunk_id(0x41, 6), filler(51, 0x300), 3),   # an export bundle and an Oodle chunk nothing reads
], parts=2)
patch = container([
    (lib_id(GLOBAL[len('ShaderArchive-'):]), library([(G1, [r[0], r[1]]), (G5, [r[5], r[6]])]), 0),   # G1 stays the base container's
    (G5, r[5] + r[6], 1),
], version=8)
files = {'Manifest_UFSFiles_Switch.txt': manifest,
         'Game/Content/Paks/Game-Switch.pak': pak11([('Game/Config/DefaultEngine.ini', b'[Core]\n', 0, 0, 0, [])])}
put_container(files, 'Game/Content/Paks/Game-Switch', base)
put_container(files, 'Game/Content/Paks/Game-Switch_0_P', patch)
make_game('ue-io-lib', files, plain_main)

# ---- 2. Mario & Luigi: Brothership's layout (UE 4.27, IoStore on): the pak holds the global shader cache, the containers beside it the cooked packages as export bundles ---------------------
gsc = sarr(lslg(501, 'vs')) + sarr(lslg(502, 'ps')) + filler(52, 0x40)
pak = pak11([('Engine/GlobalShaderCache-SF_GLSL_SWITCH.bin', gsc, 1, 0, 0, [zlib.compress(gsc)]),
             ('Vermillion/Config/DefaultGame.ini', b'[x]\n', 0, 0, 0, [])])
mA = filler(53, 0xa00) + zarr(lslg(503, 'ps')) + filler(54, 0x900)   # crosses blocks
mB = filler(55, 0x100) + lz4rec(lslg(504, 'vs'), 0) + filler(56, 0x40)
mB2 = filler(57, 0x80) + lz4rec(lslg(505, 'ps'), 3)
mC = filler(58, 0x200)
E = lambda n: chunk_id(n, 2)   # Unreal Engine 4.27's export bundle type
v3 = container([(E(1), mA, 1), (E(2), mB, 2), (E(3), mC, 3), (chunk_id(4, 1), sarr(lslg(506, 'cs')), 0)], version=3,
               directory_index=directory({0: 'Content/M_A.uasset', 1: 'Content/M_B.uasset', 2: 'Maps/L_C.umap'}, mount='../../../Vermillion/'))
v3p = container([(E(2), mB2, 1)], version=3)   # a newer copy of M_B, no directory index
v8 = container([(chunk_id(7, 1), filler(59, 0x30) + sarr(lslg(507, 'cs')), 0)], version=8)   # Unreal Engine 5 numbering beside it
files = {'Vermillion/Content/Paks/Vermillion-Switch.pak': pak,
         'Vermillion/Content/Paks/Broken-Switch.utoc': b'-==--==-not a TOC' + bytes(200)}
put_container(files, 'Vermillion/Content/Paks/Vermillion-Switch', v3)
put_container(files, 'Vermillion/Content/Paks/Vermillion-Switch_1_P', v3p)
put_container(files, 'Vermillion/Content/Paks/Extra-Switch', v8)
put_container(files, 'Vermillion/Content/Other/Far-Switch', container([(E(9), sarr(lslg(508, 'vs')), 0)], version=3))   # not beside the pak
make_game('ue-io-bundles', files, plain_main)

# ---- 3. encrypted paks: the key a 32-byte window of main's read-only data. v11 inline (encrypted entries), v4 inline (Little Nightmares II), v11 archives, and DRAGON BALL Sparking! ZERO's encrypted containers
ro = bytearray(filler(60, 0x3000)); ro[0x1240:0x1260] = KEY
enc_main = nso2(lcg(9, 0x1000), bytes(ro))
uexp = filler(61, 0x1500) + zarr(lslg(601, 'vs')) + filler(62, 0x100)
p0 = pak11_enc([('Proj/Content/M_Enc.uexp', uexp, 1, 0x1000, True),
                ('Engine/GlobalShaderCache-SP_GLSL_SWITCH.bin', sarr(lslg(602, 'ps')), 0, 0, True),
                ('Proj/Content/M_Clear.uexp', zarr(lslg(603, 'cs')), 1, 0x1000, False)], KEY)
p1 = pak11_enc([], KEY, mount='/')
l4 = legacy4_enc([('Proj/Content/M_Legacy.uexp', filler(63, 0x80) + sarr(lslg(604, 'vs'))), ('Proj/Config/A.ini', b'[a]\n')], KEY)
a1, a2 = lslg(605, 'vs'), lslg(606, 'ps')
arch = v2_archive([(lz4c(a1), len(a1), 0), (a2, len(a2), 3)])
sh = pak11_enc([('Proj/Content/ShaderArchive-Proj-SF_GLSL_SWITCH.ushaderbytecode', arch, 0, 0, True)], KEY)
c0 = container([(chunk_id(1, 1), filler(64, 0x300) + zarr(lslg(607, 'ps')), 1), (chunk_id(2, 6), b'header' + bytes(10), 0)], version=5, key=KEY,
               directory_index=directory({0: 'Content/M_Io.uasset'}, mount='../../../Proj/'))
c1 = container([(chunk_id(3, 1), sarr(lslg(608, 'vs')), 0)], version=5, key=KEY)
files = {'Proj/Content/Paks/pakchunk0-Switch.pak': p0, 'Proj/Content/Paks/pakchunk1-Switch.pak': p1,
         'Proj/Content/Paks/Legacy-Switch.pak': l4, 'Proj/Content/Paks/Shaders-Switch.pak': sh}
put_container(files, 'Proj/Content/Paks/pakchunk0-Switch', c0)
put_container(files, 'Proj/Content/Paks/pakchunk1-Switch', c1)
make_game('ue-aes-window', files, enc_main)

# ---- 4. the key in two 16-byte literals main's code stores together (DRAGON QUEST III HD-2D Remake), a frozen version 9 index encrypted with it ----------------------------------------------
def adrp(rd, pc, target):
    imm = ((target & ~0xFFF) - (pc & ~0xFFF)) >> 12 & 0x1FFFFF
    return 0x90000000 | (imm & 3) << 29 | (imm >> 2) << 5 | rd
def ldr_q(rt, rn, off): return 0x3DC00000 | (off // 16) << 10 | rn << 5 | rt
lo = 0x1000; ro = bytearray(filler(65, 0x4000)); a, b = lo + 0x100, lo + 0x2450
ro[a - lo:a - lo + 16], ro[b - lo:b - lo + 16] = KEY[:16], KEY[16:]
code = [adrp(8, 0x40, a), adrp(9, 0x44, b), ldr_q(0, 8, a & 0xFFF), ldr_q(1, 9, b & 0xFFF), 0xAD000400, 0xD65F03C0]
text = bytearray(lcg(10, lo)); text[0x40:0x40 + 4 * len(code)] = struct.pack(f'<{len(code)}I', *code)
fz = frozen_enc([('/Game/Content/', 'M_Frozen.uexp', filler(66, 0x4100) + zarr(lslg(701, 'vs')) + zarr(lslg(702, 'ps')), 1, 0x4000),
                 ('/Game/Config/', 'Default.ini', b'[x]\n', 0, 0)], KEY)
make_game('ue-aes-code', {'Game/Content/Paks/Frozen-Switch.pak': fz}, nso2(bytes(text), bytes(ro)))
