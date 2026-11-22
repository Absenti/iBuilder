"""Hand-built jump table functions (the br x18 forms patch_nso.py recognises) for the differential test."""
import random, struct, sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from synth import build_nso

NOP, RET = 0xD503201F, 0xD65F03C0
def cmpw(n, imm): return 0x7100001F | imm << 10 | n << 5
def bhi(frm, to): return 0x54000008 | (((to - frm) >> 2) & 0x7FFFF) << 5
def addi(d, n, imm): return 0x91000000 | (imm & 0xFFF) << 10 | n << 5 | d
def adrp(d, pc, target):
    imm = ((target & ~0xFFF) - (pc & ~0xFFF)) >> 12
    return 0x90000000 | (imm & 3) << 29 | ((imm >> 2) & 0x7FFFF) << 5 | d
def adr(d, pc, target):
    imm = target - pc
    return 0x10000000 | (imm & 3) << 29 | ((imm >> 2) & 0x7FFFF) << 5 | d
def ldrsw_x(t, n, m): return 0xB8A07800 | m << 16 | n << 5 | t
def ldrb_x(t, n, m): return 0x38606800 | m << 16 | n << 5 | t
def ldrh_x(t, n, m): return 0x78607800 | m << 16 | n << 5 | t
def addr(d, n, m): return 0x8B000000 | m << 16 | n << 5 | d
def add_lsl2(d, n, m): return 0x8B000800 | m << 16 | n << 5 | d
def br(n): return 0xD61F0000 | n << 5
def b(frm, to): return 0x14000000 | (((to - frm) >> 2) & 0x3FFFFFF)

def build(variant, rng):
    """returns (funcs, tables) where a function is a word list possibly with ('adrp'|'add'|'adr', ...) fixups resolved later"""
    n = rng.randint(2, 9)
    body = []        # list of words or callables(ctx) -> word
    ncase_words = rng.randint(1, 3)
    return n, ncase_words

def make(seed):
    """one module whose function 0 holds a jump table of `variant`; returns the NSO bytes"""
    rng = random.Random(seed)
    variant = rng.choice(['off', 'off_mov', 'off_masked', 'adr_off', 'byte', 'half', 'tail', 'off_far', 'off_fn2'])
    n = rng.randint(2, 9)
    cw = rng.randint(1, 3)
    pad1, pad2 = rng.randint(0, 3), rng.randint(0, 4)
    # layout of function 0 (word indexes): prologue words, then cases; table in ro_extra
    for attempt in range(2):
        pass
    prologue = []
    def build_func(ro_off, extra_off):
        tab_addr = ro_off + extra_off
        words = []
        pc0 = None
        def here(): return len(words)
        # the function starts at an address known only after layout: use placeholder start = text offset passed via closure
        return words
    # two-pass: first with dummy table address to learn the layout
    def gen(start, tab):
        w = []
        def pc(): return start + 4 * len(w)
        case_at = []
        if variant in ('off', 'off_far'):
            if variant == 'off_far': w += [NOP] * pad1
            w += [cmpw(0, n - 1)]
            hi_at = len(w); w += [0]                         # b.hi default
            w += [adrp(17, pc(), tab)]
            w += [addi(17, 17, tab & 0xFFF)]
            if variant == 'off_far': w += [NOP] * pad2
            w += [ldrsw_x(18, 17, 0), addr(18, 18, 17), br(18)]
        elif variant == 'off_mov':
            w += [cmpw(0, n - 1)]; hi_at = len(w); w += [0]
            w += [0x2A0003E1]                                 # mov w1, w0
            w += [adrp(17, pc(), tab), addi(17, 17, tab & 0xFFF)]
            w += [ldrsw_x(18, 17, 1), addr(18, 18, 17), br(18)]
        elif variant == 'off_masked':
            w += [0x12000810 | 0]                              # and w16, w0, #7
            w += [cmpw(16, n - 1)]; hi_at = len(w); w += [0]
            w += [adrp(17, pc(), tab), addi(17, 17, tab & 0xFFF)]
            w += [0x92400800 | 0 << 5 | 1]                     # and x1, x0, #7
            w += [ldrsw_x(18, 17, 1), addr(18, 18, 17), br(18)]
        elif variant == 'adr_off':
            w += [cmpw(0, n - 1)]; hi_at = len(w); w += [0]
            w += [adrp(17, pc(), tab), addi(17, 17, tab & 0xFFF)]
            adr_at = len(w); w += [0]                          # adr x18, L (the first case)
            w += [ldrsw_x(1, 17, 0), addr(18, 18, 1), br(18)]
        elif variant in ('byte', 'half'):
            w += [cmpw(0, n - 1)] if variant == 'half' else []
            hi_at = len(w) if variant == 'half' else None
            if variant == 'half': w += [0]
            w += [adrp(17, pc(), tab), addi(17, 17, tab & 0xFFF)]
            adr_at = len(w); w += [0]                          # adr x18, L0
            w += [(ldrh_x if variant == 'half' else ldrb_x)(1, 17, 0)]
            w += [add_lsl2(18, 18, 1), br(18)]
        elif variant == 'tail':
            w += [0xF9400C00 | 0 << 5 | 18, NOP, br(18)]       # ldr x18,[x0,#0x18]; nop; br x18
            return w, [], None, None, 0
        else:  # off_fn2: the table dispatch inside a longer function with a loop around it
            w += [NOP, cmpw(0, n - 1)]; hi_at = len(w); w += [0]
            w += [adrp(17, pc(), tab), addi(17, 17, tab & 0xFFF)]
            w += [ldrsw_x(18, 17, 0), addr(18, 18, 17), br(18)]
        # cases
        cases = []
        for i in range(n):
            cases.append(len(w))
            w += [NOP] * (cw + (i % 2)) + [RET]
        default = len(w); w += [RET]
        if hi_at is not None: w[hi_at] = bhi(start + 4 * hi_at, start + 4 * default)
        if variant in ('adr_off', 'byte', 'half'):
            w[adr_at] = adr(18, start + 4 * adr_at, start + 4 * cases[0])
        # table entries: relative to the table (off*) or to the adr base (adr_off), or pad indexes (byte/half)
        if variant in ('off', 'off_far', 'off_mov', 'off_masked', 'off_fn2'):
            ent = [(start + 4 * c) - tab for c in cases]; data = b''.join(struct.pack('<i', e) for e in ent)
        elif variant == 'adr_off':
            ent = [(start + 4 * c) - (start + 4 * cases[0]) for c in cases]; data = b''.join(struct.pack('<i', e) for e in ent)
        elif variant == 'byte':
            data = bytes(i % 256 for i in range(256))          # entry i = pad index: pads are 4 bytes apart from the adr base
        else:
            data = b''.join(struct.pack('<H', i) for i in range(n))
        return w, cases, default, data, 0
    # pass 1: sizes
    w0, cases, default, data, _ = gen(0x28, 0)
    funcs = [w0, [RET, NOP]]
    nsof, starts, ro_off, extra_off = build_nso(funcs, ro_extra=(data or b''))
    tab = ro_off + extra_off
    w, cases, default, data, _ = gen(starts[0], tab)
    assert len(w) == len(w0)
    funcs = [w, [RET, NOP]]
    nsof, starts, ro_off, extra_off = build_nso(funcs, ro_extra=(data or b''))
    return nsof, variant
