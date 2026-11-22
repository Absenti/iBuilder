"""Generates the scenario cases (synthetic mod folders + synthetic NSO-like modules) twice (py/ and rs/ trees) and the two ops files."""
import hashlib, json, os, random, shutil, struct, sys
import lz4.block

N = int(sys.argv[1]); SEED = int(sys.argv[2]); ROOT = sys.argv[3]
rng = random.Random(SEED)
CLEAN = [True]
HEXD = '0123456789ABCDEF'

def rhex(n): return ''.join(rng.choice(HEXD) for _ in range(n))

def make_nso(bid, kind):
    sizes = [rng.choice([0, 0x10, 0x40, 0x100, 0x180, 0x200]) + rng.randrange(0, 0x40) for _ in range(3)]
    mem = []; at = rng.choice([0, 0, 0, 0x1000])
    for s in sizes:
        mem.append(at); at += s + rng.choice([0, 0, 0x100, 0x1000])
    segs = [bytes(rng.randrange(256) for _ in range(s)) for s in sizes]
    comp = [rng.random() < 0.5 for _ in range(3)]
    stored = []
    for s, c in zip(segs, comp):
        if not c: stored.append(s)
        elif rng.random() < 0.5 and s:
            stored.append(lz4.block.compress(s, store_size=False))
        else:
            n = len(s); tok = min(n, 15) << 4; out = bytearray([tok])
            if n >= 15:
                rest = n - 15
                while rest >= 255: out.append(255); rest -= 255
                out.append(rest)
            stored.append(bytes(out) + s)
    flags = sum(1 << i for i in range(3) if comp[i]) | (rng.randrange(8) << 3)
    h = bytearray(0x100); h[0:4] = b'NSO0'
    struct.pack_into('<I', h, 0xC, flags)
    fat = 0x100
    for i in range(3):
        struct.pack_into('<III', h, 0x10 + 0x10 * i, fat, mem[i], len(segs[i]))
        struct.pack_into('<I', h, 0x60 + 4 * i, len(stored[i]))
        h[0xA0 + 0x20 * i:0xC0 + 0x20 * i] = hashlib.sha256(segs[i]).digest()
        fat += len(stored[i])
    struct.pack_into('<I', h, 0x3C, rng.choice([0, 0x4000]))
    h[0x40:0x60] = bid
    data = bytes(h) + b''.join(stored)
    if kind == 'ok': return data, mem, sizes
    if kind == 'truncated': return data[:rng.randrange(0x100, len(data) + 1)], mem, sizes
    if kind == 'badlz4':
        b = bytearray(data)
        for _ in range(3):
            if len(b) > 0x100: b[rng.randrange(0x100, len(b))] = rng.randrange(256)
        return bytes(b), mem, sizes
    if kind == 'short': return data[:rng.randrange(0, 0x100)], mem, sizes
    if kind == 'nro': return b'NRO0' + data[4:], mem, sizes
    if kind == 'sizelie':
        b = bytearray(data); struct.pack_into('<I', b, 0x18, sizes[0] + rng.randrange(1, 50)); return bytes(b), mem, sizes
    return data, mem, sizes

def ips_bytes(total, wide, bad):
    out = b'IPS32' if wide else b'PATCH'
    for _ in range(rng.randrange(0, 5)):
        off = rng.choice([0x100 + rng.randrange(0, max(1, total)), 0x100 + rng.randrange(0, max(1, total)), rng.randrange(0, 0x100), rng.randrange(0, 1 << (32 if wide else 24))]) if not CLEAN[0] else rng.choice([0x100 + rng.randrange(0, max(1, total))] * 3 + [0x100 + rng.randrange(0, 1 << 20)])
        out += off.to_bytes(4 if wide else 3, 'big')
        if rng.random() < 0.2:
            out += struct.pack('>H', 0) + struct.pack('>HB', rng.randrange(0, 20), rng.randrange(256))
        else:
            n = rng.randrange(1, 9); out += struct.pack('>H', n) + bytes(rng.randrange(256) for _ in range(n))
    out += b'EEOF' if wide else b'EOF'
    if bad == 'cut' and len(out) > 5: out = out[:rng.randrange(0, len(out))]
    elif bad == 'noend': out = out[:-(4 if wide else 3)]
    elif bad == 'hdr': out = b'XATCH' + out[5:]
    elif bad == 'junk': out = bytes(rng.randrange(256) for _ in range(rng.randrange(0, 30)))
    return out

def pch_text(bids, total):
    L = ['// Test mod %s' % rng.choice(['1.0', '60 fps', 'é ü']), '@title 0100AAAA00000000', '@version 1.0.1']
    if rng.random() < 0.2: L.append('/* block\ncomment */')
    for bi in range(rng.randrange(1, 3)):
        L.append('@nsobid-%s' % rng.choice(bids + [rhex(40), rhex(16), rhex(16).lower()]))
        r = rng.random() if not CLEAN[0] else 0.0
        if r < 0.55: L.append('@flag offset_shift 0x100')
        elif r < 0.65: L.append('@flag offset_shift %s' % rng.choice(['256', '0', '-0x100', 'abc', '0x', '1_00', '010', '0b100000000', '0o400']))
        elif r < 0.7: L.append('@flag print_values')
        for blk in range(rng.randrange(0, 4)):
            if rng.random() < 0.8: L.append('// ' + rng.choice(['Unlock 60 fps', 'Alt: 30 fps', '--- Cheat ---', 'Fast', 'ünï', 'a // b', '']))
            L.append(rng.choice(['@enabled', '@disabled', '@Enabled', '@ENABLED', '@enabled // x']))
            for _ in range(rng.randrange(0, 4)):
                addr = rng.choice(['%08X' % (rng.randrange(0, max(1, total)) + rng.choice([0, 0x100])), '%08x' % rng.randrange(0, 0x400), '0x%X' % rng.randrange(0, 0x400), '%X' % rng.randrange(0x100, 0x500), rhex(18)]) if not CLEAN[0] else rng.choice(['%08X' % rng.randrange(0, total), '0x%X' % rng.randrange(0, total), '%08x' % rng.randrange(0, total)])
                if rng.random() < 0.03 and not CLEAN[0]: addr = '0x0x12'
                n = rng.randrange(1, 9) * 2
                val = rhex(n) if rng.random() < 0.9 else rng.choice(['"hello"', 'xyz', rhex(n - 1) or 'A', rhex(4) + ' ' + rhex(4)])
                L.append('%s %s' % (addr, val) + rng.choice(['', '', ' // note', '   ']))
            if rng.random() < 0.15: L.append('Credits: someone (prose)')
        if rng.random() < 0.3: L.append('00000500 AA')
        if rng.random() < 0.7: L.append('@stop')
        if rng.random() < 0.15: L.append('Prose after stop 1234')
    nl = rng.choice(['\n', '\n', '\r\n', '\r', '\n '])
    return nl.join(L) + nl

def sections_text():
    names = rng.sample(['Infinite HP', 'Shared', 'Own', 'God', 'Ammo', 'A [b]', ' Pad ', 'ünï', ''], rng.randrange(1, 5))
    L = []
    if rng.random() < 0.6: L.append('{Master}'); L.append('04000000 00123456 00000001')
    for n in names:
        L.append(rng.choice(['', ' ', '\t']) + '[%s]' % n + rng.choice(['', ' ', '\t', ' x', ']']))
        L.append('04000000 %08X 000003E7' % rng.randrange(1 << 24))
    nl = rng.choice(['\n', '\n', '\r\n', '\r'])
    t = nl.join(L) + rng.choice(['', nl])
    b = t.encode('utf-8')
    if rng.random() < 0.05: b += b'\xff\xfe[x]\n'
    return b

def build_case(side, idx, spec):
    base = os.path.join(ROOT, side, '%03d' % idx)
    shutil.rmtree(base, ignore_errors=True)
    docs = os.path.join(base, 'docs'); exe = os.path.join(base, 'ExeFS')
    tid = spec['tid']; g = os.path.join(docs, 'games', tid or 'none')
    os.makedirs(exe); os.makedirs(docs)
    for name, data in spec['modules'].items():
        if data is None: os.makedirs(os.path.join(exe, name))
        else: open(os.path.join(exe, name), 'wb').write(data)
    for rel, data in spec['files'].items():
        p = os.path.join(g, rel); os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, 'wb').write(data)
    for d in spec['dirs']: os.makedirs(os.path.join(g, d), exist_ok=True)
    for link, target in spec['links']:
        os.makedirs(os.path.dirname(os.path.join(g, link)), exist_ok=True); os.symlink(target, os.path.join(g, link))
    return base

cases = []
for idx in range(N):
    spec = {'files': {}, 'modules': {}, 'dirs': [], 'links': []}
    CLEAN[0] = rng.random() < 0.7
    tid = '0100AAAA00000000' if rng.random() > 0.04 else ''
    spec['tid'] = tid
    bids = []
    mnames = rng.sample(['main', 'sdk', 'subsdk0', 'subsdk1', 'rtld'], rng.randrange(1, 4))
    for m in mnames:
        bid = bytes(rng.randrange(256) for _ in range(32))
        if rng.random() < 0.15: bid = bid[:16] + bytes(16)
        kind = rng.choices(['ok', 'truncated', 'badlz4', 'short', 'nro', 'sizelie', 'dir', 'none'], [60, 5, 5, 3, 3, 3, 2, 2])[0]
        if kind == 'dir': spec['modules'][m] = None
        elif kind == 'none': pass
        else:
            data, mem, sizes = make_nso(bid, kind)
            spec['modules'][m] = data
            bids.append(bid.hex().upper())
    if rng.random() < 0.3: spec['modules']['main.npdm'] = b'META' + bytes(50)
    if rng.random() < 0.03 and not CLEAN[0]: spec['modules']['bïn'] = b'NSO0' + bytes(0x100)
    total = 0x600
    state = []
    modnames = rng.sample(['60fps', 'Cheater', 'Other', 'Skins', 'Zed', 'with space', 'ünï', 'a:b', '.hidden', 'Z', 'a'], rng.randrange(1, 6))
    for mn in modnames:
        if mn == '.hidden':
            spec['dirs'].append('mods/.hidden'); continue
        if rng.random() < 0.9: state.append('mod\t%s\t%s' % (mn, rng.choice(['1', '1', '0', ' 1', '1 ', 'x'])))
        for _ in range(rng.randrange(0, 3)):
            b = rng.choice(bids + [rhex(40)]) if bids else rhex(40)
            kind = rng.choice(['full', 'b16', 'b16l', 'zero', 'wrong', 'named', 'upper', 'short'])
            stem = {'full': b, 'b16': b[:16], 'b16l': b[:16].lower(), 'zero': b[:16] + '0' * 16, 'wrong': rhex(32), 'named': 'main', 'upper': b, 'short': b[:10]}[kind]
            ext = '.IPS' if kind == 'upper' else '.ips'
            bad = rng.choices([None, 'cut', 'noend', 'hdr', 'junk'], [70, 8, 6, 8, 8])[0]
            spec['files']['mods/%s/exefs/%s%s' % (mn, stem, ext)] = ips_bytes(total, rng.random() < 0.3, bad)
        if rng.random() < 0.6:
            t = pch_text(bids, total).encode('utf-8')
            if rng.random() < 0.05 and not CLEAN[0]: t += b'\xff\xc3(\x80'
            spec['files']['mods/%s/exefs/%s.pchtxt' % (mn, rng.choice(['test', 'sub/60fps', 'a b']))] = t
        if rng.random() < 0.2: spec['files']['mods/%s/exefs/readme.txt' % mn] = b'hello'
        if rng.random() < 0.15: spec['files']['mods/%s/exefs/.hidden.ips' % mn] = b'PATCH'
        for _ in range(rng.randrange(0, 3)):
            b = rng.choice(bids + [rhex(16)]) if bids else rhex(16)
            nm = rng.choice([b[:16], b[:16].lower(), b, 'readme', 'sub/' + b[:16], b[:20], b[:16] + '.cheats'])
            spec['files']['mods/%s/cheats/%s%s' % (mn, nm, '' if '.cheats' in nm else '.txt')] = sections_text()
            for sec in ('Infinite HP', 'Shared', 'Own', 'God', 'Ammo', ' Pad '):
                if rng.random() < 0.3: state.append('cheat\t%s\t%s\t%s' % (mn, sec, rng.choice(['1', '0'])))
        for f in rng.sample(['ui/icon.bin', 'new.txt', 'a/b/c.dat', '.DS_Store'], rng.randrange(0, 3)):
            spec['files']['mods/%s/romfs/%s' % (mn, f)] = bytes(rng.randrange(256) for _ in range(rng.randrange(0, 20)))
        for nm in ('Unlock 60 fps', 'Alternative: 30 fps', 'patch 1', 'patch 2', 'ips', 'Fast'):
            if rng.random() < 0.2: state.append('patch\t%s\t%s\t%s' % (mn, nm, rng.choice(['1', '0'])))
    if rng.random() < 0.1: state.insert(rng.randrange(len(state) + 1), rng.choice(['', 'junk', 'mod', 'mod\tonly', '# comment', 'cheat\ta\tb']))
    nl = rng.choice(['\n', '\n', '\n', '\r\n', '\r'])
    text = (nl.join(state) + nl).encode('utf-8')
    if rng.random() < 0.02 and not CLEAN[0]: text += b'\xff\xfe'
    if rng.random() < 0.93: spec['files']['mods.tsv'] = text
    if rng.random() < 0.05 and modnames: spec['links'].append(('mods/%s/exefs/link' % modnames[0], '/tmp'))
    if rng.random() < 0.5: spec['files']['build_ids.txt'] = b'old\n'
    cases.append(spec)

ops = {'py': [], 'rs': []}
for idx, spec in enumerate(cases):
    for side in ('py', 'rs'):
        base = build_case(side, idx, spec)
        tid = spec['tid']
        modules = list(spec['modules'])
        modules = [m for m in modules if not m.endswith('.npdm')] + (['absent'] if rng.random() < 0.2 else [])
        env = {}
        steps = [{'k': k} for k in ('list', 'state', 'patches', 'record', 'plan', 'apply', 'mark', 'signature')]
        steps.append({'k': 'boot', 'env': env})
        steps += [{'k': 'cheats'}, {'k': 'plan', 'quiet': True}, {'k': 'remove'}, {'k': 'signature'}]
        if idx % 7 == 3: steps[8]['env'] = {'DM_ROMFS_OVERLAY': '/x', 'DM_CHEATS': '/y'}
        ops[side].append({'op': 'scenario', 'docs': os.path.join(base, 'docs'), 'tid': tid, 'exe': os.path.join(base, 'ExeFS'), 'modules': modules, 'steps': steps})
    # the module list / steps must be identical between sides: use the py draw for both
for i in range(N):
    for k in ('modules', 'steps'): ops['rs'][i][k] = ops['py'][i][k]
for side in ('py', 'rs'):
    json.dump(ops[side], open(os.path.join(ROOT, 'ops_%s.json' % side), 'w'))
print('generated', N, 'cases')
