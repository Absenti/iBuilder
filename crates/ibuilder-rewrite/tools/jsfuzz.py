"""Random dispatch code for the differential test of the jump solver (tools/difftest.py jsfuzz <first> <count>): no game code.
Each module has a few functions, each with a few table dispatches of random shapes: the index guarded (cmp / b.hi, b.hs, b.ls into
the dispatch, b.lo, on a copy, on the 64-bit register, tst / b.ne, tbnz, cbz) or not; masked, copied, extended, offset, selected
from constants or shifted before or after the guard; a call between the guard and the load; 32-bit indexes with uxtw / sxtw, 64-bit
ones with lsl; word tables relative to the table or to an adr, byte and halfword tables of pads (the add near the br, or far from
it), signed byte / halfword tables; dispatches through x18 and through other registers; case blocks that return, fall into the
next dispatch, loop back to the guard with the index changed, or branch into another function. Tables live in ro (adrp + add)
and their entries are right, wrong, or past the guard's bound. Some modules have C++ exception tables (an LSDA per function, tools/synth.py): calls
in the case blocks are caught by landing pads that go back to a dispatch, so the solver's landing-pad states (the callee-saved registers as the
call left them) are reached. The point is to reach the solver's paths (guards, joins, widening,
second reading, failures and their messages), not to look like a compiler's output."""
import random, struct, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from synth import build_nso

NOP, RET = 0xD503201F, 0xD65F03C0
COND = {'eq': 0, 'ne': 1, 'hs': 2, 'lo': 3, 'mi': 4, 'pl': 5, 'hi': 8, 'ls': 9, 'ge': 10, 'lt': 11, 'gt': 12, 'le': 13}


def enc(item, pc, lab, tabs):
    """the word of a symbolic item at address pc"""
    if isinstance(item, int): return item
    k = item[0]
    def rel19(t): return (((t - pc) >> 2) & 0x7FFFF) << 5
    if k == 'bc': return 0x54000000 | rel19(lab[item[2]]) | COND[item[1]]
    if k == 'b': return 0x14000000 | (((lab[item[1]] - pc) >> 2) & 0x3FFFFFF)
    if k == 'bl': return 0x94000000 | (((lab[item[1]] - pc) >> 2) & 0x3FFFFFF)
    if k == 'cb': return item[1] | rel19(lab[item[3]]) | item[2]                      # (cb, base opcode, rt, label)
    if k == 'tb':                                                                       # (tb, nz, rt, bit, label)
        b = item[3]
        return (0x37000000 if item[1] else 0x36000000) | (b >> 5) << 31 | (b & 31) << 19 | (((lab[item[4]] - pc) >> 2) & 0x3FFF) << 5 | item[2]
    if k == 'adr':
        imm = lab[item[2]] - pc
        return 0x10000000 | (imm & 3) << 29 | ((imm >> 2) & 0x7FFFF) << 5 | item[1]
    if k == 'adrp':
        t = tabs[item[2]]; imm = ((t & ~0xFFF) - (pc & ~0xFFF)) >> 12
        return 0x90000000 | (imm & 3) << 29 | ((imm >> 2) & 0x7FFFF) << 5 | item[1]
    if k == 'lo12': return 0x91000000 | (tabs[item[2]] & 0xFFF) << 10 | item[1] << 5 | item[1]
    raise ValueError(item)


def make(seed):
    rng = random.Random(seed)
    nfn = rng.randint(1, 3)
    eh = rng.random() < 0.4          # C++ exception tables
    lsdas = {} if eh else None
    fns, tables = [], []          # fns: [(label, items)], items: words or symbolic; tables: [(label, entry fmt, [(kind, arg)])]
    uid = [0]
    def new(p): uid[0] += 1; return f'{p}{uid[0]}'
    for fi in range(nfn):
        items, labs_at, catches, heads = [], {}, [], []
        def mark(l): labs_at[l] = len(items)
        fl = f'F{fi}'; mark(fl)
        items += [NOP] * rng.randint(0, 2)
        nd = rng.randint(1, 3)
        for di in range(nd):
            n = rng.randint(1, 9)
            r = rng.choice([0, 1, 2, 3, 8, 9, 10, 11, 13] + ([19, 20, 21] if eh else []))   # the index register
            c = rng.choice([x for x in (4, 5, 6, 7, 12) if x != r])   # a copy register
            head, dflt = new('H'), new('D')
            mark(head); heads.append(head)
            # before the guard
            pre = rng.choice(['none'] * 4 + ['mask', 'copy', 'uxtb', 'sub', 'csel', 'lsr', 'sxtw', 'movc', 'cset'])
            gr = r
            if pre == 'mask': items += [0x12000000 | rng.choice([0, 1, 2, 3]) << 10 | r << 5 | r]       # and wR, wR, #(1..15)
            elif pre == 'copy': items += [0x2A0003E0 | r << 16 | c]; gr = rng.choice([r, c])          # mov wC, wR
            elif pre == 'uxtb': items += [0x53001C00 | r << 5 | r]
            elif pre == 'sub': items += [0x51000000 | rng.randint(1, 3) << 10 | r << 5 | r]          # sub wR, wR, #k
            elif pre == 'csel':
                items += [0x52800000 | rng.randint(0, n) << 5 | 14, 0x52800000 | rng.randint(0, n + 2) << 5 | 15,
                          0x1A800000 | 15 << 16 | rng.choice([0, 1, 8]) << 12 | 14 << 5 | r]           # csel wR, w14, w15, cond
            elif pre == 'lsr': items += [0xD3400000 | 32 << 16 | 63 << 10 | r << 5 | r]               # lsr xR, xR, #32
            elif pre == 'sxtw': items += [0x93407C00 | r << 5 | r]
            elif pre == 'movc': items += [0x52800000 | rng.randint(0, n + 1) << 5 | r]               # mov wR, #k
            elif pre == 'cset': items += [0x1A9F07E0 | rng.choice([0, 1, 8]) << 12 | r]
            # the guard
            g = rng.choice(['hi', 'hi', 'hi', 'hs', 'ls_in', 'lo_in', 'hi_x', 'tbnz', 'tst', 'cbz', 'none', 'hi_copy', 'subs'])
            bound = n - 1
            if g == 'hi_copy' and pre != 'copy': g = 'hi'
            if g in ('hi', 'hs', 'hi_copy', 'ls_in', 'lo_in'):
                imm = bound if g in ('hi', 'hi_copy', 'ls_in') else bound + 1
                if rng.random() < 0.15: imm = rng.randint(0, 12)
                items += [0x7100001F | imm << 10 | gr << 5]                                    # cmp wG, #imm
                if g in ('ls_in', 'lo_in'):
                    inl = new('I'); items += [('bc', 'ls' if g == 'ls_in' else 'lo', inl), ('b', dflt)]; mark(inl)
                else: items += [('bc', 'hs' if g == 'hs' else 'hi', dflt)]
            elif g == 'hi_x': items += [0xF100001F | bound << 10 | r << 5, ('bc', 'hi', dflt)]          # cmp xR, #imm
            elif g == 'subs': items += [0x71000000 | bound << 10 | r << 5 | 16, ('bc', 'hi', dflt)]     # subs w16, wR, #imm
            elif g == 'tbnz':
                bit = rng.choice([2, 3, 4]); items += [('tb', 1, r, bit, dflt)]; bound = (1 << bit) - 1
            elif g == 'tst':
                im = rng.choice([0, 1, 2, 3]); items += [0x7200001F | im << 10 | r << 5, ('bc', 'ne', dflt)]   # tst wR, #mask
            elif g == 'cbz': items += [('cb', 0x34000000, r, dflt)]
            # after the guard
            post = rng.choice(['none'] * 5 + ['mask', 'call', 'copy_after', 'add'])
            ir = r
            if post == 'mask': items += [0x92400000 | rng.choice([1, 2, 3]) << 10 | r << 5 | r]       # and xR, xR, #mask
            elif post == 'call' and nfn > 1: items += [('bl', f'F{(fi + 1) % nfn}')]
            elif post == 'copy_after': items += [0x2A0003E0 | r << 16 | c]; ir = c                   # mov wC, wR
            elif post == 'add': items += [0x11000400 | r << 5 | r]                                    # add wR, wR, #1
            kind = rng.choice(['sw', 'sw', 'sw_adr', 'b', 'h', 'b_far', 'h_far', 'sb', 'sh', 'sw_reg'])
            ext = rng.choice(['lsl', 'lsl', 'uxtw', 'uxtw', 'sxtw'])
            opt = {'lsl': 3, 'uxtw': 2, 'sxtw': 6}[ext]
            dreg = 18 if rng.random() < 0.85 else 16
            tl = new('T'); cases = [new('C') for _ in range(n)]
            items += [('adrp', 17, tl), ('lo12', 17, tl)]
            ent_n = n + rng.choice([0, 0, 0, 1, 3])
            pick = [rng.randrange(n) if rng.random() < 0.3 else i % n for i in range(ent_n)]
            bad = rng.random() < 0.06
            if kind in ('sw', 'sw_reg'):
                items += [0xB8A00800 | ir << 16 | opt << 13 | 1 << 12 | 17 << 5 | dreg, 0x8B000000 | 17 << 16 | dreg << 5 | dreg, 0xD61F0000 | dreg << 5]
                tables.append((tl, '<i', [('rel_tab', cases[p]) for p in pick], bad))
            elif kind == 'sw_adr':
                base = cases[0]
                items += [('adr', dreg, base), 0xB8A00800 | ir << 16 | opt << 13 | 1 << 12 | 17 << 5 | 1, 0x8B000000 | 1 << 16 | dreg << 5 | dreg, 0xD61F0000 | dreg << 5]
                tables.append((tl, '<i', [('rel', (cases[p], base)) for p in pick], bad))
            else:
                size = {'b': 1, 'h': 2, 'b_far': 1, 'h_far': 2, 'sb': 1, 'sh': 2}[kind]
                signed = kind in ('sb', 'sh')
                lop = (0x38E00800 if signed else 0x38600800) if size == 1 else (0x78E00800 if signed else 0x78600800)
                base = cases[0]
                items += [('adr', dreg, base), lop | ir << 16 | opt << 13 | (1 << 12 if size == 2 else 0) | 17 << 5 | 1]
                items += [0x8B000800 | 1 << 16 | dreg << 5 | dreg]                                   # add xD, xD, x1, lsl #2
                if kind.endswith('_far'): items += [NOP] * rng.randint(14, 20)
                items += [0xD61F0000 | dreg << 5]
                # pads: case i is i words after the first: entry = index of the case
                tables.append((tl, '<b' if size == 1 and signed else '<B' if size == 1 else '<h' if signed else '<H',
                               [('pad', (cases[p], base)) for p in pick], bad))
            # cases: pads are consecutive single words when the table holds pad indexes
            padded = kind not in ('sw', 'sw_reg', 'sw_adr')
            for i, cl in enumerate(cases):
                mark(cl)
                if padded:
                    items += [('b', f'E{fi}_{di}_{i}')]
                else:
                    end = rng.choice(['ret', 'ret', 'next', 'loop', 'other'])
                    items += [0x91000400 | 2 << 5 | 2] * rng.randint(0, 2)                          # add x2, x2, #1
                    if eh and rng.random() < 0.4:                                                # a call a landing pad catches
                        pad = new('P'); catches.append((len(items), pad)); items += [('bl', f'F{rng.randrange(nfn)}')]
                    if end == 'ret' or (end == 'other' and nfn == 1): items += [RET]
                    elif end == 'next': items += [('b', dflt)]
                    elif end == 'loop': items += [0x11000400 | r << 5 | r, ('b', head)]             # add wR, wR, #1; b head
                    else: items += [('b', f'F{(fi + 1) % nfn}')]
            if padded:
                for i in range(n):
                    mark(f'E{fi}_{di}_{i}'); items += [RET if rng.random() < 0.7 else ('b', head)]
            mark(dflt)
            items += [NOP]
        items += [RET]
        for k, pad in catches:   # the landing pads: back to a dispatch, an index set or not
            mark(pad)
            if rng.random() < 0.5: items += [0x52800000 | rng.randint(0, 9) << 5 | rng.choice([19, 20, 21, 0])]   # mov wR, #k
            items += [('b', rng.choice(heads)) if rng.random() < 0.8 else RET]
        if eh: lsdas[fi] = [(4 * k, 4, 4 * labs_at[pad]) for k, pad in catches]
        fns.append((items, labs_at))
    # tables in ro_extra, 8-aligned, in order
    def table_bytes(lab, tab_addr):
        out = b''
        for tl, fmt, ents, bad in tables:
            a = tab_addr[tl]; vals = []
            for k, arg in ents:
                if k == 'rel_tab': v = lab[arg] - a
                elif k == 'rel': v = lab[arg[0]] - lab[arg[1]]
                else: v = (lab[arg[0]] - lab[arg[1]]) // 4
                vals.append(v)
            if bad and vals: vals[random.Random(seed + 1).randrange(len(vals))] += 0x40000
            lim = {'<i': (-2**31, 2**31 - 1), '<B': (0, 255), '<b': (-128, 127), '<H': (0, 65535), '<h': (-32768, 32767)}[fmt]
            out += b''.join(struct.pack(fmt, min(max(v, lim[0]), lim[1])) for v in vals)
            out += b'\0' * (-len(out) % 8)
        return out
    offs, o = {}, 0
    for tl, fmt, ents, bad in tables:
        offs[tl] = o; o += struct.calcsize(fmt) * len(ents); o += -o % 8
    def layout(lab, tab_addr):
        return [[enc(it, 0, {}, {}) if isinstance(it, int) else 0 for it in items] for items, _ in fns] if lab is None else \
               [[enc(it, lab['@' + str(fi)] + 4 * k, lab, tab_addr) for k, it in enumerate(items)] for fi, (items, _) in enumerate(fns)]
    nsof, starts, ro_off, extra_off = build_nso(layout(None, None), ro_extra=b'\0' * o, lsdas=lsdas)
    lab = {}
    for fi, (items, labs_at) in enumerate(fns):
        lab['@' + str(fi)] = starts[fi]
        for l, k in labs_at.items(): lab[l] = starts[fi] + 4 * k
    tab_addr = {tl: ro_off + extra_off + offs[tl] for tl in offs}
    data = table_bytes(lab, tab_addr)
    nsof, starts2, _, _ = build_nso(layout(lab, tab_addr), ro_extra=data, lsdas=lsdas)
    assert starts2 == starts
    return nsof


if __name__ == '__main__':
    open(sys.argv[2], 'wb').write(make(int(sys.argv[1])))
