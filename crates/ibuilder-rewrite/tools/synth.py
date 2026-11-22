"""Synthetic NSO builder for differential tests of the Rust rewriter against patch_nso.py.
A module = a list of functions (lists of 32-bit words) laid out after a MOD0 header at text offset 8, with a valid
eh_frame_hdr / eh_frame (one FDE per function) in ro, so both implementations find the function boundaries.
"""
import struct

def align(n, a): return (n + a - 1) & ~(a - 1)

def uleb(v):
    out = b''
    while True:
        b = v & 0x7F; v >>= 7
        out += bytes([b | (0x80 if v else 0)])
        if not v: return out


def build_nso(funcs, ro_extra=b'', bss=0x1000, text_pad=0, build_id=None, text_min=0, poke=None, pad_before_last=0, lsdas=None):
    """funcs: list of word lists. Returns (nso bytes, function start offsets, ro offset (memory), ro_extra offset in ro).
    lsdas: {function index: [(call start, call length, landing pad), ...] in bytes from the function's start}: C++ exception tables. With it every FDE
    uses a well-formed "zPLR" CIE (personality absptr 0, LSDA absptr, FDE pcrel sdata4) and points at its LSDA (0: none); without it the layout is
    what it always was."""
    # text: b +0x28 ; .word 8 (MOD0 offset) ; MOD0 (0x20 bytes) ; functions
    hdr_words = 10
    starts, words = [], [0x14000000 | 10, 8]
    words += [0] * 8   # MOD0 placeholder (filled below)
    for fi, f in enumerate(funcs):
        if fi == len(funcs) - 1 and pad_before_last: words += [0] * pad_before_last
        starts.append(len(words) * 4)
        words += list(f)
    words += [0] * text_pad
    if len(words) * 4 < text_min: words += [0] * ((text_min - len(words) * 4) // 4)
    for off, w in (poke or {}).items(): words[off // 4] = w
    text = b''.join(struct.pack('<I', w & 0xFFFFFFFF) for w in words)
    text_size = len(text)
    ro_off = align(text_size, 0x1000)
    # ro: dynamic (16 zero bytes) | eh_frame | eh_frame_hdr | extra
    dyn = b'\0' * 16
    cie = struct.pack('<IIB', 0, 0, 1) + b'zR\0' + bytes([1, 0x7c, 30, 1, 0x1b])
    cie = cie + b'\0' * (-(len(cie) + 4) % 8)
    cie = struct.pack('<I', len(cie)) + cie
    if lsdas is not None:
        cie = struct.pack('<IB', 0, 1) + b'zPLR\0' + bytes([1, 0x7c, 30, 11, 0x00]) + b'\0' * 8 + bytes([0x00, 0x1b])
        cie = cie + b'\0' * (-(len(cie) + 4) % 8)
        cie = struct.pack('<I', len(cie)) + cie
    eh = bytearray(cie)
    fde_offs = []
    for s, f in zip(starts, funcs):
        fde_offs.append(len(eh))
        if lsdas is not None:
            # length, cie pointer, pc_begin (pcrel sdata4), pc_range (sdata4), aug len 8, LSDA (absptr, fixed below), pad to 8
            body_len = 4 + 4 + 4 + 1 + 8
            pad = -(body_len + 4) % 8
            eh += struct.pack('<I', body_len + pad) + b'\0\0\0\0' + b'\0' * 4 + struct.pack('<i', len(f) * 4) + bytes([8]) + b'\0' * 8 + b'\0' * pad
            continue
        # length, cie pointer, pc_begin (pcrel sdata4), pc_range (sdata4), aug len 0, pad to 8
        body_len = 4 + 4 + 4 + 1
        pad = -(body_len + 4) % 8
        eh += struct.pack('<I', body_len + pad) + b'\0\0\0\0' + b'\0' * 4 + struct.pack('<i', len(f) * 4) + b'\0' + b'\0' * pad
    eh_off_in_ro = len(dyn)
    hdr_off_in_ro = align(eh_off_in_ro + len(eh), 8)
    # fix FDE cie pointers and pc_begin now that offsets are known (memory offsets)
    for i, (s, fo) in enumerate(zip(starts, fde_offs)):
        fde_addr = ro_off + eh_off_in_ro + fo
        cie_ptr_field = fde_addr + 4
        eh[fo + 4:fo + 8] = struct.pack('<I', cie_ptr_field - (ro_off + eh_off_in_ro))
        pc_field = fde_addr + 8
        eh[fo + 8:fo + 12] = struct.pack('<i', s - pc_field)
    hdr = bytearray()
    hdr += bytes([1, 0x1b, 0x03, 0x3b]) + struct.pack('<i', (ro_off + eh_off_in_ro) - (ro_off + hdr_off_in_ro + 4)) + struct.pack('<I', len(funcs))
    hdr_start = ro_off + hdr_off_in_ro
    for s, fo in zip(starts, fde_offs):
        hdr += struct.pack('<i', s - hdr_start) + struct.pack('<i', (ro_off + eh_off_in_ro + fo) - hdr_start)
    ro = bytearray(dyn) + eh
    ro += b'\0' * (hdr_off_in_ro - len(ro))
    ro += hdr
    if lsdas is not None:   # the LSDAs after the hdr; each FDE's LSDA pointer (absolute) set now
        for fi, (s, fo) in enumerate(zip(starts, fde_offs)):
            recs = lsdas.get(fi) or []
            if not recs: continue
            ro += b'\0' * (-len(ro) % 4)
            at = ro_off + len(ro)
            body = b''.join(uleb(a) + uleb(n) + uleb(lp) + uleb(0) for a, n, lp in recs)
            ro += bytes([0xFF, 0xFF, 0x01]) + uleb(len(body)) + body
            p = eh_off_in_ro + fo + 4 + 4 + 4 + 4 + 1
            ro[p:p + 8] = struct.pack('<Q', at)
    extra_off_in_ro = align(len(ro), 16)
    ro += b'\0' * (extra_off_in_ro - len(ro)) + ro_extra
    ro = bytes(ro)
    # MOD0
    mod0 = b'MOD0' + struct.pack('<i', (ro_off + 0) - 8) + struct.pack('<II', 0, 0) + struct.pack('<ii', hdr_start - 8, hdr_start + len(hdr) - 8) + struct.pack('<i', 0)
    text = text[:8] + mod0 + text[8 + len(mod0):]
    ro_size = len(ro)
    data_off = ro_off + align(ro_size, 0x1000)
    data = b'\0' * 0x1000
    h = bytearray(0x100)
    h[0:4] = b'NSO0'
    file_text = 0x100
    file_ro = file_text + len(text)
    file_data = file_ro + len(ro)
    struct.pack_into('<III', h, 0x10, file_text, 0, text_size)
    struct.pack_into('<III', h, 0x20, file_ro, ro_off, ro_size)
    struct.pack_into('<III', h, 0x30, file_data, data_off, len(data))
    struct.pack_into('<I', h, 0x3c, bss)
    h[0x40:0x60] = build_id or bytes(range(1, 33))
    struct.pack_into('<III', h, 0x60, text_size, ro_size, len(data))
    return bytes(h) + text + ro + data, starts, ro_off, extra_off_in_ro
