#!/bin/bash
# app/compare_ipa.sh <old.ipa> <new.ipa> -- structure of two iBuilder .ipa files side by side: what is gone, what is new, what changed in size.
# Reads both archives with the zip directory only (never changes them, unpacks nothing). Prints:
#   * the archive sizes and entry counts; the uncompressed size and the compressed size (as zip stored it) of every top-level entry, side by side;
#   * the Python pieces of the old one (py/, pyext/, dm/tools/*.py, shaderengine/, engine/, *.py, Python.framework, the interpreter's module frameworks) and their sizes;
#   * the entries only in one of the two, by top-level name and, inside a shared folder, by file.
set -uo pipefail
OLD="${1:?usage: compare_ipa.sh <old.ipa> <new.ipa>}"; NEW="${2:?usage: compare_ipa.sh <old.ipa> <new.ipa>}"
python3 -I - "$OLD" "$NEW" <<'PY'
import os, sys, zipfile, collections
old_ipa, new_ipa = sys.argv[1:3]
def load(ipa, root):
    files = {}
    if os.path.isdir(ipa):   # an unzipped archive (its Payload/<app>.app folder, or the folder holding Payload/): compressed sizes are unknown (0)
        base = ipa
        if os.path.isdir(os.path.join(ipa, 'Payload')):
            base = os.path.join(ipa, 'Payload', sorted(os.listdir(os.path.join(ipa, 'Payload')))[0])
        for d, _, fs in os.walk(base):
            for f in fs:
                p = os.path.join(d, f)
                if os.path.islink(p): continue
                files[os.path.relpath(p, base)] = (os.path.getsize(p), 0)
        return os.path.basename(base), files
    z = zipfile.ZipFile(ipa)
    infos = [i for i in z.infolist() if not i.is_dir()]
    app = sorted({i.filename.split('/')[1] for i in infos if i.filename.startswith('Payload/')})[0]
    for i in infos:
        rel = i.filename.split('/', 2)[2] if i.filename.count('/') >= 2 else i.filename
        files[rel] = (i.file_size, i.compress_size)
    return app, files
def top(files):
    t = collections.OrderedDict()
    for rel, (u, c) in files.items():
        k = rel.split('/')[0]
        a = t.setdefault(k, [0, 0, 0]); a[0] += u; a[1] += c; a[2] += 1
    return t
oa, of = load(old_ipa, None); na, nf = load(new_ipa, None)
mb = lambda b: b / 1048576.0
print('old: %s  %s, %d files, %.1f MB uncompressed' % (old_ipa, ('%d bytes' % os.path.getsize(old_ipa)) if os.path.isfile(old_ipa) else 'an unzipped tree', len(of), mb(sum(v[0] for v in of.values()))))
print('new: %s  %s, %d files, %.1f MB uncompressed' % (new_ipa, ('%d bytes' % os.path.getsize(new_ipa)) if os.path.isfile(new_ipa) else 'an unzipped tree', len(nf), mb(sum(v[0] for v in nf.values()))))
if os.path.isfile(old_ipa) and os.path.isfile(new_ipa):
    print('ipa size: %.2f MB -> %.2f MB (%+.2f MB, %+.1f %%)' % (mb(os.path.getsize(old_ipa)), mb(os.path.getsize(new_ipa)), mb(os.path.getsize(new_ipa) - os.path.getsize(old_ipa)), 100.0 * (os.path.getsize(new_ipa) / os.path.getsize(old_ipa) - 1)))
print('uncompressed: %.2f MB -> %.2f MB (%+.2f MB, %+.1f %%)' % (mb(sum(v[0] for v in of.values())), mb(sum(v[0] for v in nf.values())), mb(sum(v[0] for v in nf.values()) - sum(v[0] for v in of.values())), 100.0 * (sum(v[0] for v in nf.values()) / sum(v[0] for v in of.values()) - 1)))
to, tn = top(of), top(nf)
print('\nper top-level entry of Payload/<app>.app  (uncompressed MB / compressed MB / files):')
print('%-52s %22s %22s' % ('entry', 'old', 'new'))
for k in sorted(set(to) | set(tn), key=lambda k: -(to.get(k, [0])[0] + tn.get(k, [0])[0])):
    o, n = to.get(k), tn.get(k)
    f = lambda v: ('%8.2f /%8.2f /%5d' % (mb(v[0]), mb(v[1]), v[2])) if v else '                 -'
    if (o and o[0] > 200000) or (n and n[0] > 200000) or not o or not n:
        print('%-52s %22s %22s%s' % (k, f(o), f(n), '   GONE' if not n else ('   NEW' if not o else '')))
small = [k for k in set(to) | set(tn) if k in to and k in tn and to[k][0] <= 200000 and tn[k][0] <= 200000]
print('(%d further entries, each under 0.2 MB on both sides: .NET assemblies, plist, icons ...)' % len(small))
# Python pieces of the old archive
def is_py(rel):
    r = rel
    return (r.startswith(('py/', 'pyext/', 'shaderengine/', 'engine/', 'scripts/')) or r.endswith(('.py', '.pyc', '.fwork')) or r.startswith('dm/tools/')
            or 'Python.framework' in r or r.startswith('Frameworks/') and r.split('/')[1].startswith(('Python', '_', 'lib-dynload'))
            or r.startswith('dm/gfx/tools/') or r.startswith('dm/ios/'))
def is_stdlib_fw(rel):
    p = rel.split('/')
    return p[0] == 'Frameworks' and len(p) > 1 and p[1].endswith('.framework') and p[1][:-10] not in ('GameApp.r2r', 'OpenSSL', 'libcoreclr') and not p[1].startswith('libSystem') and p[1] != 'Python.framework'
py = {r: v for r, v in of.items() if is_py(r) or is_stdlib_fw(r)}
cat = collections.OrderedDict([('py/ (the interpreter home: stdlib .py, lib-dynload)', lambda r: r.startswith('py/')),
    ('Frameworks/Python.framework', lambda r: 'Python.framework' in r),
    ("Frameworks/<module>.framework (the interpreter's C modules)", is_stdlib_fw),
    ('pyext/ (capstone, lz4, zbic Python halves)', lambda r: r.startswith('pyext/')),
    ('dm/tools, dm/gfx/tools, dm/ios .py (the engine scripts)', lambda r: r.startswith(('dm/tools/', 'dm/gfx/tools/', 'dm/ios/')) or (r.startswith('dm/games/') and r.endswith('.py'))),
    ('shaderengine/ + engine/ (the shader scripts)', lambda r: r.startswith(('shaderengine/', 'engine/'))),
    ('top-level .py (gameapp_pipeline, shader_device, mods, assemble, ...)', lambda r: '/' not in r and r.endswith('.py')),
    ('scripts/ (pack_nup.py)', lambda r: r.startswith('scripts/'))])
print('\nthe Python pieces of the OLD archive (a file is counted in the first category that matches):')
seen = set(); tu = tc = 0
for name, pred in cat.items():
    sel = {r: v for r, v in of.items() if r not in seen and pred(r)}
    seen |= set(sel)
    u = sum(v[0] for v in sel.values()); c = sum(v[1] for v in sel.values()); tu += u; tc += c
    print('  %-66s %5d files %8.2f MB  (%.2f MB compressed)' % (name, len(sel), mb(u), mb(c)))
print('  %-66s %5d files %8.2f MB  (%.2f MB compressed)' % ('TOTAL Python pieces', len(seen), mb(tu), mb(tc)))
# only in one side
gone = sorted(r for r in of if r not in nf); new = sorted(r for r in nf if r not in of)
print('\nfiles only in the OLD archive: %d (%.2f MB); only in the NEW one: %d (%.2f MB)' % (len(gone), mb(sum(of[r][0] for r in gone)), len(new), mb(sum(nf[r][0] for r in new))))
def group(rs, files):
    g = collections.OrderedDict()
    for r in rs:
        k = '/'.join(r.split('/')[:2]) if r.count('/') >= 2 and r.split('/')[0] in ('Frameworks', 'dm') else r.split('/')[0]
        a = g.setdefault(k, [0, 0]); a[0] += 1; a[1] += files[r][0]
    return g
print('\nonly in the NEW archive (by folder, files / MB):')
for k, (n, u) in group(new, nf).items(): print('  %-60s %5d  %8.2f' % (k, n, mb(u)))
print('\nin both, size changed by more than 100 KB (old MB -> new MB):')
for r in sorted(set(of) & set(nf)):
    if abs(of[r][0] - nf[r][0]) > 100000: print('  %-60s %8.2f -> %8.2f' % (r, mb(of[r][0]), mb(nf[r][0])))
PY
