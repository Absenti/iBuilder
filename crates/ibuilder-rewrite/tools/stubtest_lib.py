"""Differential test of the ported per-game stub functions: a synthetic main with the registered build ID and the original
bytes at the stub sites, through patch_nso.py (scratch copy) and the Rust CLI."""
import os, subprocess, sys, shutil, struct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from synth import build_nso
S = os.environ.get('IBR_SCRATCH') or os.environ.get('IBUILDER_SCRATCH_OLD') or sys.exit('set IBR_SCRATCH (or IBUILDER_SCRATCH_OLD): the scratch folder with ref/venv, ref/engine and the Rust build; see scripts/local.env.example')
PY, PATCH, RUST = S + '/ref/venv/bin/python', S + '/ref/engine/dm/tools/patch_nso.py', S + '/target-rewrite/release/ibuilder-rewrite'
RET, NOP = 0xD65F03C0, 0xD503201F

def le(hexs): return struct.unpack('<I', bytes.fromhex(hexs))[0]

def ori():
    bid = bytes.fromhex('11c33c7c2dbd3c5491841f67495c7ee8') + bytes(16)
    poke = {}
    for fn, first in ((0x2d640, 'ff4302d1'), (0x34f80, 'ffc301d1'), (0x353c0, 'f60f1df8'), (0x24550, 'ff0302d1'), (0x24de0, 'ffc301d1'), (0x2d200, 'ff0302d1'),
                      (0x35510, 'ff0301d1'), (0x25110, 'fc6fbaa9'), (0x29330, '083858f9'), (0x29120, 'f40f1ef8')): poke[fn] = le(first)
    poke[0x2df18] = 0x90000008 | (0x7e48 & 3) << 29 | ((0x7e48 >> 2) << 5)   # adrp x8, +page (value irrelevant)
    poke[0x2df1c] = 0xF9400000 | (0x3c << 10) | (8 << 5) | 8                    # ldr x8, [x8, #0x1e0]
    return build_nso([[RET, NOP]], build_id=bid, text_min=0xcbec00, poke=poke)[0]

def ttyd():
    bid = bytes.fromhex('0effe4af1dec3a7966b934d4a7c3d2bf') + bytes(16)
    poke = {0x2e77f8: le('604600bd'), 0x2e77a4: le('49008052')}
    return build_nso([[RET, NOP]], build_id=bid, text_min=0x2e7800 + 0x100, poke=poke)[0]

