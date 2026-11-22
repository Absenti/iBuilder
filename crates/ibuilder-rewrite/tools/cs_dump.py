# Python side of the Capstone equivalence probe: decodes the (address, word) list with capstone 5.0.7 (the oracle)
import sys, struct
from capstone import *
from capstone.arm64 import ARM64_OP_IMM, ARM64_OP_MEM, ARM64_OP_REG
md = Cs(CS_ARCH_ARM64, CS_MODE_ARM); md.detail = True
d = open(sys.argv[1], 'rb').read()
out = open(sys.argv[2], 'w')
for i in range(0, len(d), 8):
    o, w = struct.unpack_from('<II', d, i)
    ins = next(md.disasm(struct.pack('<I', w), o), None)
    if ins is None:
        out.write(f'{o:#x}\t-\n'); continue
    ops = []
    for op in ins.operands:
        if op.type == ARM64_OP_REG: ops.append('R' + ins.reg_name(op.reg))
        elif op.type == ARM64_OP_IMM: ops.append(f'I{op.imm}')
        elif op.type == ARM64_OP_MEM: ops.append(f'M{ins.reg_name(op.mem.base) or ""},{ins.reg_name(op.mem.index) or ""},{op.mem.disp}')
        else: ops.append(f'T{op.type}')
    g = ''.join('1' if ins.group(x) else '0' for x in (CS_GRP_JUMP, CS_GRP_CALL, CS_GRP_RET, CS_GRP_INT, CS_GRP_IRET))
    try:
        r, wr = ins.regs_access()
        rr, rw = ','.join(ins.reg_name(x) for x in r), ','.join(ins.reg_name(x) for x in wr)
    except CsError:
        rr = rw = 'ERR'
    out.write(f'{o:#x}\t{ins.mnemonic}\t{ins.op_str}\t{g}\t{int(ins.writeback)}\t{";".join(ops)}\t{rr}\t{rw}\n')
