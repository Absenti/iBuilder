"""Synthetic NSO builder for the differential tests of the per-game stub ports (tools/stubtest.py): a main whose text has functions (with unwind
entries) at chosen offsets, instruction words poked at chosen offsets (the original bytes at a stub's sites), the registered build ID, and optionally the
import table (a PLT stub and a JMPREL relocation per imported symbol) that an anchored MATCH_FRAME_STEP rule looks its function up by.
No game code is in it: a function is nops and a `ret`, with the words a patch's sites need."""
import struct

NOP, RET = 0xD503201F, 0xD65F03C0
PLT_WORDS = 4


def align(n, a): return (n + a - 1) & ~(a - 1)


def build(functions, pokes, text_size, build_id, bss=0x1000, imports=()):
    """functions: [(start offset, size in bytes)], each filled with nops and a ret; pokes: {text offset: word}, applied after the fill;
    imports: [(symbol, plt offset)]: a PLT stub (adrp x16 / ldr x17 / add x16 / br x17) at plt offset and the relocation that names its GOT slot.
    Returns (nso bytes, {'ro_off', 'data_off', 'slots': {symbol: GOT slot}})"""
    assert text_size % 4 == 0 and len(build_id) == 16
    words = [0] * (text_size // 4)
    words[0], words[1] = 0x14000000 | 10, 8                         # b +0x28; .word 8 (the MOD0 offset); MOD0 follows
    funcs = sorted(functions)
    for start, size in funcs:
        assert start % 4 == 0 and size % 4 == 0 and start >= 0x28
        for i in range(start // 4, (start + size) // 4): words[i] = NOP
        words[(start + size) // 4 - 1] = RET
    for off, w in pokes.items(): words[off // 4] = w & 0xFFFFFFFF
    ro_off = align(text_size, 0x1000)
    # ro: dynamic | eh_frame | eh_frame_hdr | extra (relocations, symbols, strings)
    nimp = len(imports)
    ndyn = 6 if nimp else 0
    dyn_size = 16 * ndyn if ndyn else 16
    cie = struct.pack('<IB', 0, 1) + b'zR\0' + bytes([1, 0x7c, 30, 1, 0x1b])   # CIE id 0, version 1, "zR", code align 1, data align -4, return register 30, DW_EH_PE_pcrel|sdata4
    cie = cie + b'\0' * (-(len(cie) + 4) % 8)
    cie = struct.pack('<I', len(cie)) + cie
    eh = bytearray(cie)
    fde_offs = []
    for start, size in funcs:
        fde_offs.append(len(eh))
        body_len = 4 + 4 + 4 + 1
        pad = -(body_len + 4) % 8
        eh += struct.pack('<I', body_len + pad) + b'\0\0\0\0' + b'\0' * 4 + struct.pack('<i', size) + b'\0' + b'\0' * pad
    eh_off_in_ro = dyn_size
    hdr_off_in_ro = align(eh_off_in_ro + len(eh), 8)
    for (start, size), fo in zip(funcs, fde_offs):
        fde_addr = ro_off + eh_off_in_ro + fo
        eh[fo + 4:fo + 8] = struct.pack('<I', (fde_addr + 4) - (ro_off + eh_off_in_ro))
        eh[fo + 8:fo + 12] = struct.pack('<i', start - (fde_addr + 8))
    hdr = bytearray()
    hdr += bytes([1, 0x1b, 0x03, 0x3b]) + struct.pack('<i', (ro_off + eh_off_in_ro) - (ro_off + hdr_off_in_ro + 4)) + struct.pack('<I', len(funcs))
    hdr_start = ro_off + hdr_off_in_ro
    for (start, size), fo in zip(funcs, fde_offs):
        hdr += struct.pack('<i', start - hdr_start) + struct.pack('<i', (ro_off + eh_off_in_ro + fo) - hdr_start)
    extra_off_in_ro = align(hdr_off_in_ro + len(hdr), 16)
    extra = bytearray()
    slots = {}
    # sizes first: rela (24 per import), symtab (24 per symbol, index 0 is null), strtab
    rela_off, sym_off = 0, 24 * nimp
    str_off = sym_off + 24 * (nimp + 1)
    strtab = bytearray(b'\0') if nimp else bytearray()
    name_at = {}
    for sym, _ in imports:
        name_at[sym] = len(strtab); strtab += sym.encode() + b'\0'
    extra_len = (str_off + len(strtab)) if nimp else 0
    ro_size = extra_off_in_ro + extra_len
    data_off = ro_off + align(ro_size, 0x1000)
    for i, (sym, plt) in enumerate(imports):
        slot = data_off + 0x10 + 8 * i
        slots[sym] = slot
        extra += struct.pack('<QQq', slot, (i + 1) << 32 | 1026, 0)
        # PLT stub: adrp x16, page(slot); ldr x17,[x16,#lo]; add x16,x16,#lo; br x17
        imm = ((slot & ~0xfff) - (plt & ~0xfff)) >> 12
        lo = slot & 0xfff
        assert lo % 8 == 0 and lo < 0x1000
        pokes_plt = (0x90000010 | (imm & 3) << 29 | ((imm >> 2) & 0x7ffff) << 5, 0xf9400211 | (lo // 8) << 10, 0x91000210 | lo << 10, 0xd61f0220)
        for k, w in enumerate(pokes_plt): words[plt // 4 + k] = w
    if nimp:
        sym = bytearray(24)               # the null symbol
        for s, _ in imports: sym += struct.pack('<IBBHQQ', name_at[s], 0x12, 0, 0, 0, 0)
        extra += sym + strtab
    assert len(extra) == extra_len, (len(extra), extra_len)
    dyn = bytearray()
    if ndyn:
        for tag, val in ((20, 7), (23, ro_off + extra_off_in_ro + rela_off), (2, 24 * nimp), (6, ro_off + extra_off_in_ro + sym_off), (5, ro_off + extra_off_in_ro + str_off), (0, 0)):
            dyn += struct.pack('<qQ', tag, val)
    else:
        dyn = bytearray(16)
    assert len(dyn) == dyn_size
    ro = bytearray(dyn) + eh
    ro += b'\0' * (hdr_off_in_ro - len(ro)) + hdr
    ro += b'\0' * (extra_off_in_ro - len(ro)) + extra
    ro = bytes(ro)
    assert len(ro) == ro_size
    text = bytearray(b''.join(struct.pack('<I', w) for w in words))
    mod0 = b'MOD0' + struct.pack('<i', ro_off - 8) + struct.pack('<II', 0, 0) + struct.pack('<ii', hdr_start - 8, hdr_start + len(hdr) - 8) + struct.pack('<i', 0)
    text[8:8 + len(mod0)] = mod0
    text = bytes(text)
    data = b'\0' * 0x1000
    h = bytearray(0x100)
    h[0:4] = b'NSO0'
    file_text = 0x100
    file_ro = file_text + len(text)
    file_data = file_ro + len(ro)
    struct.pack_into('<III', h, 0x10, file_text, 0, len(text))
    struct.pack_into('<III', h, 0x20, file_ro, ro_off, len(ro))
    struct.pack_into('<III', h, 0x30, file_data, data_off, len(data))
    struct.pack_into('<I', h, 0x3c, bss)
    h[0x40:0x60] = build_id + bytes(16) if len(build_id) == 16 else build_id
    struct.pack_into('<III', h, 0x60, len(text), len(ro), len(data))
    return bytes(h) + text + ro + data, {'ro_off': ro_off, 'data_off': data_off, 'slots': slots}
