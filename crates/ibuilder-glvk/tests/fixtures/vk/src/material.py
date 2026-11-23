#!/usr/bin/env python3
"""A synthetic RenderDragon-like material file: the 8-byte material magic, padding, then a bgfx v5 vertex shader (VSH\\x05, a 4-byte I/O hash, one uniform record, the SPIR-V
size and words) and a second, damaged one whose size runs past the file (skipped). Usage: material.py <vertex.spv> <out.material.bin>"""
import struct
import sys

spv = open(sys.argv[1], 'rb').read()
uniform = bytes([5]) + b'bones' + bytes([4, 16, 0, 0, 0, 0])   # name length, name, type, count, register index and count (7 + len bytes)
blob = b'VSH\x05' + b'\x11\x22\x33\x44' + struct.pack('<H', 1) + uniform + struct.pack('<I', len(spv)) + spv
broken = b'FSH\x05' + b'\0' * 4 + struct.pack('<H', 0) + struct.pack('<I', 1 << 20) + spv[:64]
open(sys.argv[2], 'wb').write(struct.pack('<Q', 0x0A11DA1A) + b'\0' * 24 + blob + b'\0' * 12 + broken)
