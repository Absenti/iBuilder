"""Generate synthetic ExeFS + RomFS manifest + control.nacp variants that exercise the profile rules (oracle inputs)."""
import hashlib, json, os, random, struct, sys, zlib
sys.dont_write_bytecode = True
import lz4.block

def build_nso(symbols, extra_ro=b'', bss=0, compress=True, check_hash=True, bid=None, data_pad=0x1000):
    """symbols: [(name, defined, bind)]"""
    names = b'\0'; offs = {}
    for n, _, _ in symbols:
        offs[n] = len(names); names += n.encode('latin1') + b'\0'
    nsym = len(symbols) + 1
    ro_base = 0x1000
    mod0_off = ro_base
    dyn_off = mod0_off + 0x20
    ndyn = 4
    hash_off = dyn_off + ndyn * 16
    hash_sz = 8 + 4 * 2
    symtab_off = (hash_off + hash_sz + 7) & ~7
    strtab_off = symtab_off + nsym * 24
    extra_off = strtab_off + len(names)
    ro = bytearray(extra_off + len(extra_ro) - ro_base)
    ro[0:4] = b'MOD0'; struct.pack_into('<i', ro, 4, dyn_off - mod0_off)
    for i, (t, v) in enumerate([(4, hash_off), (5, strtab_off), (6, symtab_off), (0, 0)]):
        struct.pack_into('<qQ', ro, dyn_off - ro_base + i * 16, t, v)
    struct.pack_into('<II', ro, hash_off - ro_base, 1, nsym)
    for i, (n, defined, bind) in enumerate(symbols, 1):
        struct.pack_into('<IBBHQQ', ro, symtab_off - ro_base + i * 24, offs[n], (bind << 4) | 2, 0, 1 if defined else 0, 0x1000 if defined else 0, 8)
    ro[strtab_off - ro_base:strtab_off - ro_base + len(names)] = names
    ro[extra_off - ro_base:] = extra_ro
    while len(ro) % 0x1000: ro.append(0)
    text = bytearray(0x1000); struct.pack_into('<I', text, 4, mod0_off)
    data_base = ro_base + len(ro)
    data = bytes(data_pad)
    segs = [bytes(text), bytes(ro), data]
    mem = [0, ro_base, data_base]
    bid = bid or os.urandom(0x20)
    flags = 0
    hdr = bytearray(0x100); hdr[0:4] = b'NSO0'
    stored = []; off = 0x100
    comp = []
    for i, s in enumerate(segs):
        c = lz4.block.compress(s, store_size=False, mode='high_compression') if compress else s
        comp.append(c)
        if compress: flags |= 1 << i
        if check_hash: flags |= 1 << (3 + i)
    struct.pack_into('<I', hdr, 0xc, flags)
    for i, s in enumerate(segs):
        struct.pack_into('<IIII', hdr, 0x10 + i * 0x10, off, mem[i], len(s), 0)
        struct.pack_into('<I', hdr, 0x60 + i * 4, len(comp[i]))
        hdr[0xa0 + 0x20 * i:0xc0 + 0x20 * i] = hashlib.sha256(s).digest()
        off += len(comp[i])
    struct.pack_into('<I', hdr, 0x3c, bss); hdr[0x40:0x60] = bid
    return bytes(hdr) + b''.join(comp)

def build_npdm(sysres, program):
    d = bytearray(0x200); d[0:4] = b'META'; d[0xc] = 1; struct.pack_into('<I', d, 0x14, sysres)
    d[0x80:0x84] = b'ACI0'; struct.pack_into('<Q', d, 0x90, program)
    return bytes(d)

def build_nacp(rng, compressed):
    t = bytearray(0x6000 if compressed else 0x3000)
    langs = [('Grizzy ™ & Co', 'Pub')] + [('', '')] * 3
    k = rng.randrange(0, 4)
    for i in range(k, k + 2):
        nm = ['The Legend of Zelda: Link’s Awakening', 'DRAGON QUEST XI S', 'abé ゲーム', "O'Reilly's 2 Game!!"][rng.randrange(4)]
        t[i * 0x300:i * 0x300 + len(nm.encode())] = nm.encode()
    b = bytearray(0x4000)
    if compressed:
        c = zlib.compressobj(9, zlib.DEFLATED, -15); z = c.compress(bytes(t)) + c.flush()
        b[0:2] = struct.pack('<H', len(z)); b[2:2 + len(z)] = z
    else:
        b[0:len(t)] = t
    ver = rng.choice([b'1.0.0', b'1.2.3.4', b'12', b'v1', b'2.0', b'', b'3.1.4beta'])
    b[0x3060:0x3060 + len(ver)] = ver
    return bytes(b)

def make_manifest(rng, kind):
    dirs = ['Data', 'data', 'Content', 'Paks', 'Main!', 'main', 'other', 'g', 'Sound', 'x' * 20, 'école', 'A B', 'Content/Paks']
    exts = ['.assets', '.resS', '.dat', '.bytes', '.pak', '.mp4', '.ogg', '.png', '.txt', '', '.UE4', '.bin', '.json', '.PAK']
    magics = [None, b'UnityFS\0', b'PK\x03\x04', b'OggS', b'\x89PNG', b'KPKA', b'BNSH', bytes.fromhex('03022307'), b'RIFF', b'(\xb5/\xfd', b'ONEPACK\0', b'SARC', None, None]
    lines = ['# path\tbytes\tfirst 32 bytes (hex)']
    n = rng.choice([0, 1, 5, 40, 200, 1000])
    for i in range(n):
        d = rng.choice(dirs); e = rng.choice(exts)
        depth = rng.choice([0, 1, 1, 2, 3])
        path = '/' + '/'.join([d] * min(depth, 1) + ['sub%d' % rng.randrange(3)] * max(0, depth - 1)) + ('/' if depth else '') + 'f%d%s' % (i, e)
        if rng.random() < .05: path = '/' + 'hagi.config'
        if kind == 'hagi' and i == 0: path = '/hagi.config'
        size = rng.choice([0, 10, 1000, 5 * 1024 * 1024, 3 << 30, rng.randrange(1 << 34)])
        m = rng.choice(magics); head = (m or b'zz') + bytes(rng.randrange(256) for _ in range(rng.randrange(0, 20)))
        lines.append('%s\t%d\t%s' % (path, size, head[:32].hex()))
    if kind == 're': lines.append('/re_chunk_000/pak\t100\t' + b'KPKA'.hex())
    if kind == 'unreal': lines.append('/Content/Paks/pakchunk0.pak\t100\t' + b'zz'.hex())
    return '\n'.join(lines) + '\n'

def main(outdir, n, hooks):
    hooks = sorted(hooks)
    for v in range(n):
        rng = random.Random(1000 + v)
        d = os.path.join(outdir, 'v%03d' % v); exdir = os.path.join(d, 'extract'); exe = os.path.join(exdir, 'ExeFS'); os.makedirs(exe, exist_ok=True)
        kind = rng.choice(['plain', 'plain', 'unreal', 're', 'hagi', 'heap', 'giant', 'ounce', 'sysres_heap', 'drop'])
        heap = kind in ('heap', 'giant', 'sysres_heap') or rng.random() < .3
        sysres = rng.choice([0, 0x1000000, 0x2000000]) if kind != 'sysres_heap' else 0x1000000
        main_imports = [('memcpy', False, 1), ('nvnDeviceInitialize', False, 1)] + [(rng.choice(hooks), False, 1) for _ in range(5)]
        if heap: main_imports.append(('_ZN2nn2os17SetMemoryHeapSizeEm', False, 1))
        if kind == 'unreal': main_imports.append(('nvnBootstrapLoader', False, 1))
        extra = b'\0'.join([b'nvnBootstrapLoader', b'UnityEngine', b'sead::', b'agl::', b'Wwise', b'global-metadata.dat', b'2022.3.12f1', b'xx 2019.4.1f2 yy',
                            b'NintendoSDK_nnSdk-%d_%d_0' % (rng.choice([12, 16, 18, 19, 20, 21]), rng.randrange(5))]
                           + ([b'Unreal'] if kind == 'unreal' else []) + ([b'via.render.'] if kind == 're' else []))
        extra += b'\0#version 450\nvoid main(){}\0' + b'\x91\x68\x86\x19' * rng.randrange(3) + b'glShaderSource' * rng.randrange(3) + b'\0#version 300\0'
        main_exports = [('nnMain', True, 1), ('weak_x', True, 2), ('local_y', True, 0)]
        bss = 120 * 1024 * 1024 if kind == 'giant' else rng.choice([0, 0x4000, 4 << 20])
        files = {}
        files['main'] = build_nso(main_imports + main_exports, extra, bss=bss, compress=rng.random() < .8, check_hash=rng.random() < .8)
        sdk_ex = [('nnos_%d' % i, True, 1) for i in range(rng.randrange(1, 50))] + [(h, True, 1) for h in rng.sample(hooks, 3)]
        sdk_extra = b'NintendoSDK_nnSdk-Generic_22_%d_0\0' % rng.randrange(3) + (b'spec: Ounce\0' if kind == 'ounce' else b'')
        files['sdk'] = build_nso(sdk_ex + [('x', False, 1)], sdk_extra, bss=rng.choice([0, 1 << 20]))
        files['rtld'] = build_nso([('a', False, 1)], b'')
        for i in range(rng.randrange(0, 4)):
            nm = 'subsdk%d' % i if rng.random() < .85 else 'subsdkx%d' % i
            ex = []
            imp_names = [s[0] for s in main_imports]
            r = rng.random()
            if r < .35: ex = [(rng.choice(imp_names[2:]), True, 1)]                    # exports something main imports (maybe a hook)
            elif r < .6: ex = [('only_in_%s_%d' % (nm, j), True, 1) for j in range(rng.randrange(1, 40))]
            elif r < .8: ex = [('movie_fn', True, 1)]; imp_names.append('movie_fn')
            comp = rng.choice([b'', b'NintendoSDK_movie-3_1_0', b'NintendoSDK_OpenGL-1_2', b'NintendoSDK_Vulkan-3_0', b'SDK MW+Nintendo+glslc-9_9\0'])
            if rng.random() < .3: ex.append(('glslcCompile', True, 1))
            if rng.random() < .3 and comp == b'': ex = [('_ZN2nn5movie%d' % j + 'XE', True, 1) for j in range(3)] + [('_ZN2nn2os%dYE' % j, True, 1) for j in range(2)]
            if ex and rng.random() < .5:  # make main import the first export so the module is kept
                files['main'] = build_nso(main_imports + [(ex[0][0], False, 1)] + main_exports, extra, bss=bss, compress=True)
            files[nm] = build_nso(ex + [('imp', False, 1)], comp, bss=rng.choice([0, 0x10000, 100 << 20]))
        for n, b in files.items(): open(os.path.join(exe, n), 'wb').write(b)
        if rng.random() < .9:
            program = 0x0100000000000000 | rng.randrange(1 << 48) << 12 | 0x000
            program = (program & ~0xFFF) | 0
            open(os.path.join(exe, 'main.npdm'), 'wb').write(build_npdm(sysres, program))
        else:
            program = None
            if rng.random() < .5: open(os.path.join(exdir, 'RomFS.img.id'), 'w').write(json.dumps({'title': '0100%012X' % rng.randrange(1 << 48), 'other': [1, {'title': 'x'}]}))
        open(os.path.join(exdir, 'romfs_manifest.tsv'), 'w').write(make_manifest(rng, kind))
        if rng.random() < .8: open(os.path.join(exdir, 'control.nacp'), 'wb').write(build_nacp(rng, rng.random() < .4))
        print(v, kind, 'heap' if heap else '', hex(sysres), sorted(files))

if __name__ == '__main__':
    sys.path.insert(0, sys.argv[4]); import nso_syms
    main(sys.argv[1], int(sys.argv[2]), nso_syms.host_hooks(sys.argv[3], forced=False))
