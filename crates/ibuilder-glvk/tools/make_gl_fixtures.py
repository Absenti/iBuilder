#!/usr/bin/env python3
"""The binary fixtures of the GL port's tests, all synthetic (no game data):
  * tests/fixtures/gl/input/mvgl/sh.mvgl   a legacy-layout (Digimon Story Cyber Sleuth) MVGL archive: m_vp.shad doboz-compressed (the small encoder below), m_fp.shad stored,
                                           a .txt entry the GL port leaves out
  * tests/fixtures/gl/main.nso             an NSO0 with uncompressed segments whose read-only segment holds three '#version' strings: a vertex shader with main, a helper
                                           prefix without main, and a glslang-style '#version:' message
  * tests/fixtures/gl/doboz_vectors.txt    400 synthetic doboz streams and what mvgl.doboz answers for each (decoded bytes, ValueError text, or IndexError)
Usage: NXNATIVE=<engine checkout> python3 -I -B make_gl_fixtures.py <crate dir>   (the engine's mvgl.py checks the archive and answers the vectors)"""
import os
import random
import struct
import sys

root = os.environ.get('NXNATIVE')
if not root or len(sys.argv) != 2: sys.exit(__doc__)
sys.path.insert(0, os.path.join(root, 'dm', 'gfx', 'tools'))
sys.path.insert(0, os.path.join(root, 'dm', 'tools'))
import mvgl  # noqa: E402

fx = os.path.join(sys.argv[1], 'tests', 'fixtures', 'gl')


def doboz_encode(data):
    """greedy doboz version 0: 4-byte matches (type 7: offset << 11 | (length - 3) << 3 | 7) where 6 or more bytes repeat, literals elsewhere, the last 8 bytes literals"""
    items, i, n = [], 0, len(data)
    while i < n:
        best, boff = 0, 0
        if i < n - 8:
            for j in range(max(0, i - 2000), i):
                k = 0
                while i + k < n - 8 and k < 258 and data[j + k] == data[i + k]: k += 1
                if k > best: best, boff = k, i - j
        if best >= 6:
            items.append((1, struct.pack('<I', (boff << 11) | ((best - 3) << 3) | 7))); i += best
        else:
            items.append((0, data[i:i + 1])); i += 1
    body = bytearray()
    for g in range(0, len(items), 31):
        grp = items[g:g + 31]
        ctl = (1 << len(grp)) | sum(b << k for k, (b, _) in enumerate(grp))
        if len(grp) == 31: ctl = sum(b << k for k, (b, _) in enumerate(grp)) | (1 << 31)
        body += struct.pack('<I', ctl)
        for _, d in grp: body += d
    if len(items) % 31 == 0: body += struct.pack('<I', 1 << 31)
    return bytes([0x18]) + struct.pack('<II', n, len(body)) + bytes(body)   # widths of 4 bytes


def mvgl_legacy(entries):
    """entries: [(name, ext, stored bytes, length)] -> a legacy archive: a root tree node, one leaf per entry"""
    nt, nn, nd = len(entries) + 1, len(entries) + 1, len(entries)
    start = 0x14 + nt * 8 + nn * 0x40 + nd * 12
    data, table = bytearray(), bytearray()
    for _, _, stored, length in entries:
        table += struct.pack('<III', len(data), length, len(stored)); data += stored
    head = b'MDB1' + struct.pack('<HHIII', nt, nn, nd, start, start + len(data))
    tree = struct.pack('<HHHH', 0xffff, 0xffff, 1, 1) + b''.join(struct.pack('<HHHH', k, k, 0, 0) for k in range(nd))
    names = b'\0' * 0x40 + b''.join((ext.encode().ljust(4, b' ') + name.encode('latin1')).ljust(0x40, b'\0') for name, ext, _, _ in entries)
    return head + tree + names + bytes(table) + bytes(data)


VP = (b'#version 310 es\nprecision highp float;\nin vec4 a_pos;\nin vec2 a_uv;\nuniform mat4 u_mvp;\nuniform mat4 u_mvp_prev;\nout vec2 v_uv;\n'
      b'void main() {\n  v_uv = a_uv;\n  gl_Position = u_mvp * a_pos;\n  gl_Position = gl_Position + u_mvp * a_pos * 0.0;\n}\n')
FP = (b'#version 310 es\nprecision highp float;\nin vec2 v_uv;\nuniform sampler2D u_diffuse;\nuniform vec4 u_tint;\nout vec4 o_color;\n'
      b'void main() { o_color = texture(u_diffuse, v_uv) * u_tint; }\n')


def nso(ro):
    """an NSO0 with uncompressed text (a ret), ro (the given bytes) and data segments, each one page"""
    page = 0x1000
    segs = [b'\xc0\x03\x5f\xd6'.ljust(page, b'\0'), ro.ljust(page, b'\0'), b'\0' * page]
    head = bytearray(0x100)
    head[0:4] = b'NSO0'
    for k, s in enumerate(segs):
        struct.pack_into('<III', head, 0x10 + 0x10 * k, 0x100 + page * k, page * k, len(s))
        struct.pack_into('<I', head, 0x60 + 4 * k, len(s))
    head[0x40:0x60] = bytes(range(0x20))
    return bytes(head) + b''.join(segs)


def main():
    os.makedirs(os.path.join(fx, 'input', 'mvgl'), exist_ok=True)
    vz = doboz_encode(VP)
    assert mvgl.doboz(vz) == VP and len(vz) < len(VP), (len(vz), len(VP))
    arc = os.path.join(fx, 'input', 'mvgl', 'sh.mvgl')
    with open(arc, 'wb') as f:
        f.write(mvgl_legacy([('shader\\m_vp', 'shad', vz, len(VP)), ('shader\\m_fp', 'shad', FP, len(FP)), ('readme', 'txt', b'x', 1)]))
    a = mvgl.Archive(arc)
    assert a.read('shader\\m_vp.shad') == VP and a.read('shader\\m_fp.shad') == FP, a.files
    a.close()
    ro = (b'GLES\0#version 310 es\nin vec4 p;\nvoid main(){ gl_Position = p; }\n\0'
          b'#version 150\nvec4 helper(vec4 x);\n\0'
          b'#version: not a shader\0#versionx\0x#version 310 es\nvoid main(){}\0')
    with open(os.path.join(fx, 'main.nso'), 'wb') as f: f.write(nso(ro))
    r = random.Random(7)
    out = []
    for n in range(400):
        size = r.randint(1, 60)
        body = bytearray()
        while len(body) < r.randint(4, 90):
            ctl = 0
            for b in range(31):
                if r.random() < 0.25: ctl |= 1 << b
            body += (ctl | 1 << 31).to_bytes(4, 'little')
            body += bytes(r.randint(0, 255) if r.random() < 0.5 else r.randint(0, 40) for _ in range(r.randint(0, 40)))
        s = bytes([0, size, len(body) if r.random() < 0.9 else r.randint(0, 255)]) + bytes(body)
        if n % 50 == 0: s = bytes([0x80, size, size]) + bytes(r.randint(0, 255) for _ in range(r.randint(0, 70)))
        try: res = 'OK ' + mvgl.doboz(s).hex()
        except ValueError as e: res = 'VE ' + str(e)
        except IndexError: res = 'IE'
        out.append(s.hex() + ' ' + res)
    with open(os.path.join(fx, 'doboz_vectors.txt'), 'w') as f: f.write('\n'.join(out) + '\n')
    text_cases()


TEXTS = [
    'void main() { gl_FragColor = texture2D(tex, uv) + textureCube (cube, d) + texture2DLod(t, uv, 0.0); }',
    'out vec4 gl_Position;\nin vec4 gl_FragCoord;\n  invariant out highp vec4 gl_Position ;\nout float gl_FragDepth;\nout vec4 gl_FragColor;\nvoid main(){gl_FragColor=vec4(1);}\n',
    'out vec4 gl_FrontColor;\nvoid main(){ gl_FrontColor = vec4(0); }\n',
    'in vec4 gl_Color;\nvoid main(){ vec4 c = gl_Color; }\n',
    'vec4 f(sampler2D sampler, vec2 uv) { return texture(sampler, uv); }\nint dm_sampler;\n',
    'void main() { x = 1; };\nstruct S { int a; };\nuniform B { int b; };\nvoid g() { if (a) { } } ;\nvoid h() {}\x1c;\n',
    'void main(){ c = textureOffset(s, uv, ivec2(o.x, 1)) + textureOffset(s, uv, ivec2(1, 0)) + textureOffset( s , f(uv) , off ); }',
    'void main(){ c = textureOffset(s, uv, o(1); }',
    'void main(){ c = textureOffset(textureOffset(s, a, b), uv, k); }',
    'precision mediump float;\r\nprecision highp int;\nattribute vec4 p;\nvarying lowp vec2 v;\nuniform sampler2D texture;\nvoid main(){ vec4 sample = texture2D(texture, v); gl_FragColor = texture (texture, v) * sample; }\n',
    'attribute vec3 gm_sample;\nvoid main(){ float sample = 1.0; float gm_sample_1; }',
    '#version 120\n#extension GL_ARB_x : enable\n\n#line 3\nvoid main(){ gl_TexCoord[0] = gl_TexCoord [ 10 ]; gl_FragData[2] = gl_FragData[0] + gl_Color + gl_SecondaryColor; gl_FrontColor = vec4(gl_FogFragCoord); }\n',
    '#version 120\nvoid main(){ }',
    'layout(binding = 2) uniform sampler2D a, b;\nuniform layout(std140, binding=0x1F) highp isampler3D c;\nlayout(location=1) uniform usamplerCube d;\nlayout() uniform sampler2D e;\nlayout(binding = 00) uniform sampler2D z;\n',
    'layout(binding = 010) uniform sampler2D a;\n',
    'caf\xe9 \xb2x main(){}',
]

LOGS = [
    'f.vert\nUniform reflection:\nu_a: offset 0, type 8b52, size 1, index 0, binding -1, stages 1\nu_s.m: offset 16, type 1406, size 1, index 0, binding -1, stages 1\n'
    'samp: offset -1, type 8b5e, size 1, index -1, binding 0, stages 1\nother: offset 0, type 1406, size 1, index 1, binding -1, stages 1\n\n'
    'Uniform block reflection:\ngl_DefaultUniformBlock: offset -1, type ffffffff, size 32, index 0, binding 0, stages 1\nB: offset -1, type ffffffff, size 4, index 1, binding 1, stages 1\n\n'
    'Buffer variable reflection:\n',
    'Uniform reflection:\nx: offset 0, type 1406, size 1, index 1, binding -1, stages 1\n\nUniform block reflection:\nB: offset -1, type ffffffff, size 4, index 1, binding 1, stages 1\n',
    'no reflection here\n',
    'Uniform reflection:\n\nUniform block reflection:\n',
]


def text_cases():
    import json
    import gl_shaders as g
    cases = []

    def call(f, *a):
        try: return ['ok', f(*a)]
        except ValueError as e: return ['ValueError', str(e)]
    for t in TEXTS:
        for st in ('vert', 'frag'):
            r = {}
            cases.append({'fn': 'fixups', 'in': [t, st], 'out': call(g.fixups, t, st, r), 'renamed': sorted(r.items())})
            for desktop in (False, True):
                r = {}
                cases.append({'fn': 'promote_legacy', 'in': [t, st, desktop], 'out': call(g.promote_legacy, t, st, r, desktop), 'renamed': sorted(r.items())})
            cases.append({'fn': 'legacy_desktop_io', 'in': [t, st], 'out': call(g.legacy_desktop_io, t, st)})
        cases.append({'fn': 'function_semicolons', 'in': [t], 'out': call(g.function_semicolons, t)})
        b = call(g.explicit_sampler_bindings, t)
        cases.append({'fn': 'explicit_sampler_bindings', 'in': [t], 'out': [b[0], sorted(b[1].items()) if b[0] == 'ok' else b[1]]})
        cases.append({'fn': 'is_standalone_glsl', 'in': [t], 'out': ['ok', g.is_standalone_glsl(t.encode('latin1'))]})
        for name, base, keep in (('sampler', 'dm_sampler', False), ('texture', 'dm_texture', True), ('sample', 'gm_sample', False)):
            r = {}
            cases.append({'fn': 'rename_identifier', 'in': [t, name, base, keep], 'out': call(g.rename_identifier, t, name, base, r, keep), 'renamed': sorted(r.items())})
    for log in LOGS:
        a = g.active_default_uniforms(log)
        cases.append({'fn': 'active_default_uniforms', 'in': [log], 'out': ['ok', None if a is None else sorted(a)]})
    for f, r in ((['a:dm_texture=0:vec2', 'u:gm_sample.x=4:float:0:0', 't:dm_texture=1:-1:sampler2D', 'r:0:2', 'o:gm_FragColor=0'], {'dm_texture': 'texture', 'gm_sample': 'sample'}),):
        cases.append({'fn': 'game_names', 'in': [f, sorted(r.items())], 'out': ['ok', g.game_names(f, r)]})
    with open(os.path.join(fx, 'text_cases.json'), 'w') as fh: json.dump(cases, fh, indent=0, ensure_ascii=True)


if __name__ == '__main__':
    main()
