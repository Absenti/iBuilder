"""Oracle for the live path: the phone's own shader_device.live_translate on real Grizzy shaders (taken out of the split's packages as (key, ctl, code)), with the engine's real tools,
plus bad entries (wrong key, short control section, unknown stage). Writes the entries as <key>.ctl/<key>.code (for the Rust CLI), the result as result.json and the files as files/<name>.
usage: S=$OLD python run_live.py --engine DIR --out DIR"""
import argparse, json, os, struct, subprocess as real_subprocess, sys, tempfile, shutil
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
S = os.environ['S'] + '/shader'
ap = argparse.ArgumentParser(); ap.add_argument('--engine', required=True); ap.add_argument('--out', required=True); ap.add_argument('--chain', default='6c53446391cc'); ap.add_argument('--n', type=int, default=4)
a = ap.parse_args()
import shader_device as sd
tl = os.path.join(a.engine, 'dm/gfx/tools')
XB, SB = os.path.join(tl, 'dmsh_xlate_bin/dmsh_xlate'), os.path.join(tl, 'spirv_cross_bin/spirv-cross')
def run(b, args):
    p = real_subprocess.run([b, *args], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr
tools = {'xlate': lambda x: run(XB, x), 'spirv_cross': lambda x: run(SB, x)}
CTL = bytes.fromhex('34127698')
pkg = S + '/split-out/pkg'
rows = [l.rstrip('\n').split('\t') for l in open(S + '/profile/0100DFA027504000/shader_map.tsv') if l.strip() and not l.startswith('#')]
keys = [l.strip() for l in open(S + '/keys-s300.txt') if l.strip()]
byk = {r[3]: r for r in rows if len(r) > 4 and r[0].endswith('.bin')}
def record(name, key):
    d = open(os.path.join(pkg, name), 'rb').read()
    o = d.find(b'BTCDBTCD' + bytes.fromhex(key))
    e = d.find(b'BTCDBTCD', o + 8)
    return d[o:e if e >= 0 else len(d)]
picked, per = [], {}
for k in keys:
    st = byk[k][0][:2]
    if per.get(st, 0) < a.n and st in ('ps', 'vs', 'cs', 'gs'):
        per[st] = per.get(st, 0) + 1; picked.append(k)
entries = []
for k in picked:
    rec = record(byk[k][0], k); ci = rec.find(CTL, 0x38)
    entries.append((k, rec[ci:], rec[0x38:ci]))
bad_key = entries[0][0][:-1] + ('0' if entries[0][0][-1] != '0' else '1')
entries += [(bad_key, entries[0][1], entries[0][2]), ('1234567890abcdef', b'\0' * 0x40, b'\0' * 0x20), ('zz', entries[1][1], entries[1][2])]
out = a.out; shutil.rmtree(out, ignore_errors=True); os.makedirs(out + '/entries')
for k, ctl, code in entries:
    open(f'{out}/entries/{k}.ctl', 'wb').write(ctl); open(f'{out}/entries/{k}.code', 'wb').write(code)
open(out + '/order.txt', 'w').write(''.join(k + '\n' for k, _, _ in entries))
logs = []
work = out + '/work'
ok, bad, files = sd.live_translate(entries, S + '/profile/0100DFA027504000', S + '/profile/0100DFA027504000', '0100DFA027504000', work, a.chain, tools, log=logs.append, engine=a.engine, cas=None)
os.makedirs(out + '/files')
for n, d in files.items(): open(out + '/files/' + n, 'wb').write(d)
json.dump({'ok': ok, 'bad': bad, 'files': sorted(files)}, open(out + '/result.json', 'w'), indent=1, sort_keys=True)
print('live:', len(entries), 'entries,', len(ok), 'ok,', len(bad), 'bad,', len(files), 'files; image', work + '/out/b-' + a.chain + '.img', os.path.exists(work + '/out/b-' + a.chain + '.img'))
