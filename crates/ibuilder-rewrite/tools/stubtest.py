#!/usr/bin/env python3
"""Differential test of the ported per-game stub functions (src/stubs_games/) against the REAL Python (`patch_nso.py` of the engine, run in the reference venv).

For every code-valued entry of the engine's `dm/games/*/patches.py` (FRAME_STEP / STUBS by build ID, MATCH_FRAME_STEP / MATCH_STUBS by rule):
  1. the entry's own stub function is run once with a recording context to learn the sites it asserts (offset, original bytes) and the data addresses it reads;
  2. a synthetic NSO is built (stubsynth.py) with the registered build ID (or, for a matched rule, functions of the rule's sizes at fixed offsets: nops and a ret, with
     the original bytes at the sites; their fingerprints are computed by the engine's own `nso_match.Image` and put in a scratch copy of `patches.py` in place of the
     registered ones, since a real function's fingerprint cannot be made without the game), and run through `patch_nso.py` (a scratch copy of the engine's tools);
  3. the cases: `ok` (everything in place), `mismatch` (one site's original bytes changed: both must stop with the same message), for matched rules `miss` (no function
     has the fingerprint), `dup` (two functions do: "2 matching functions"), `miss-other<i>` (an `others` function missing), `far` (a patch area before the base: a
     128 MB bss);
  4. every file `patch_nso.py` wrote and what it printed is compared with the Rust rewriter (`ibuilder-rewrite` CLI) for the entries found by build ID; for matched
     rules (which need the substituted fingerprints) `--export` writes the NSO fixtures and the SHA-256 of the Python's outputs for `cargo test -p ibuilder-rewrite stub_ports`.

usage: stubtest.py --engine <dm dir> --python <venv python> --rust <ibuilder-rewrite binary> [--work DIR] [--only PROFILE] [--jobs N] [--export DIR]
The engine copy is copied (tools + the games' patches.py) per worker; the engine is never written.
"""
import argparse, hashlib, importlib.util, os, shutil, struct, subprocess, sys, threading
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stubsynth
from stubsynth import NOP, RET

FAKE_PAGE = 0x2000000


def fake_address(off):
    """the data address the recorder hands `ctx.address(off)`, and that the synthetic adrp / ldr pair at `off` makes"""
    return FAKE_PAGE + ((off >> 12) & 0xfff) * 0x1000 + 0x100


class Rec:
    """the ctx of a stub function (patch_nso.py's `fs` / `st` namespace) that records instead of writing"""
    def __init__(self, base=0, others=None, found=None, state=0x6000000):
        self.base, self.others, self.found, self.state = base, others or {}, found, state
        self.words, self.sites, self.puts, self.addrs = [], [], [], []
        self.capabilities, self.render_scale, self.step_max, self.fps_ready = set(), 0, 0, False
        self._here = 0x7000000

    def here(self): return self._here + 4 * len(self.words)
    def emit(self, w): self.words.append(w)
    def site(self, off, orig): self.sites.append((off, orig))
    def put(self, off, words, why): self.puts.append((off, list(words), why))

    def address(self, off, lo_off=None):
        self.addrs.append((off, off + 4 if lo_off is None else lo_off))
        return fake_address(off)


def load_patch_file(eng, profile):
    spec = importlib.util.spec_from_file_location('p_' + profile, os.path.join(eng, 'games', profile, 'patches.py'))
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    return m


def entries(eng, only=None):
    out = []
    for prof in sorted(os.listdir(os.path.join(eng, 'games'))):
        path = os.path.join(eng, 'games', prof, 'patches.py')
        if not os.path.isfile(path) or (only and prof != only): continue
        m = load_patch_file(eng, prof)
        for k in ('FRAME_STEP', 'STUBS'):
            for bid, fn in sorted(getattr(m, k, {}).items()): out.append(dict(kind=k, profile=prof, key=bid, fn=fn, mod=m))
        for k in ('MATCH_FRAME_STEP', 'MATCH_STUBS'):
            for mod, rules in getattr(m, k, {}).items():
                for i, r in enumerate(rules): out.append(dict(kind=k, profile=prof, module=mod, index=i, rule=r, fn=r['stub']))
    return out


class Layout:
    """where the synthetic functions go (matched rules) and what is poked"""
    def __init__(self): self.functions, self.pokes, self.imports, self.names = [], {}, [], {}

    def poke(self, off, w):
        assert off % 4 == 0 and self.pokes.get(off, w) == w, f'poke conflict at {off:#x}'
        self.pokes[off] = w & 0xFFFFFFFF


def le(hexs): return [struct.unpack('<I', bytes.fromhex(hexs[i:i + 8]))[0] for i in range(0, len(hexs), 8)]


def poke_recording(lay, rec):
    for off, orig in rec.sites:
        for i, w in enumerate(le(orig)): lay.poke(off + 4 * i, w)
    for off, lo in rec.addrs:
        addr = fake_address(off)
        imm = ((addr & ~0xfff) - (off & ~0xfff)) >> 12
        lay.poke(off, 0x90000009 | (imm & 3) << 29 | ((imm >> 2) & 0x7ffff) << 5)       # adrp x9
        lay.poke(lo, 0xf9400129 | ((addr & 0xfff) // 8) << 10)                          # ldr x9,[x9,#lo]


def plan(entry):
    """(layout, recording, build id, module, text size) of an entry's `ok` case; the matched rules' functions are placed and their rule data kept"""
    lay = Layout()
    if 'key' in entry:
        bid = bytes.fromhex(entry['key'])
        rec = Rec()
        entry['fn'](rec)
        poke_recording(lay, rec)
        # the build's registered cuts and observation gates assert their original bytes too
        extra = []
        for off, old, new, why in getattr(entry['mod'], 'CUTS', {}).get(entry['key'], []):
            for i, w in enumerate(le(old)): lay.poke(off + 4 * i, w)
            extra.append(off + len(old) // 2)
        for off, number, word, why in getattr(entry['mod'], 'OBS', {}).get(entry['key'], []):
            lay.poke(off, int(word, 16) if not isinstance(word, bytes) else struct.unpack('<I', word)[0]); extra.append(off + 4)
        top = max([o + 4 * len(le(h)) for o, h in rec.sites] + [lo + 4 for _, lo in rec.addrs] + extra + [0x1000])
        lay.functions.append((0x100, 0x20))
        return lay, rec, bid, 'main', (top + 0x1000 + 0xfff) & ~0xfff
    rule = entry['rule']
    pos = 0x1000
    def place(label, size):
        nonlocal pos
        start = pos; pos = (pos + size + 0x100 + 0xff) & ~0xff
        lay.functions.append((start, size)); lay.names[label] = start
        return start
    if 'calls' in rule:
        start = place('main', 0x400)
        plt = pos; pos += 0x40
        lay.imports.append((rule['calls'], plt))
        # what Hades II's find reviewed: one call to the anchor, then the one movz / movk pair (reviewed site +0x190, x8)
        lay.poke(start + 0x100, 0x94000000 | (((plt - (start + 0x100)) >> 2) & 0x3ffffff))
        off, rd = rule['reviewed'][0]
        lay.poke(start + off, 0xd280a000 | rd); lay.poke(start + off + 4, 0xf2c05a00 | rd)
        rec = Rec(base=start, found=(off, rd), others={})
    else:
        start = place('main', rule['size'])
        others = {}
        for label, fn in rule.get('others', {}).items(): others[label] = place(label, fn['size'])
        rec = Rec(base=start, others=others)
    entry['fn'](rec)
    poke_recording(lay, rec)
    return lay, rec, bytes(16), 'main', (pos + 0x1000 + 0xfff) & ~0xfff


def mark_functions(lay, nso_funcs):
    """a marker word in each function that tells functions of one size apart (the nops of two functions would have one fingerprint)"""
    for n, (start, size) in enumerate(nso_funcs):
        i = size // 4 - 2
        while (start + 4 * i) in lay.pokes and i > 0: i -= 1
        lay.pokes[start + 4 * i] = 0x52800000 | (n + 1) << 5      # mov w0,#n+1
        # the poked words of a function are part of its fingerprint: keep the marker out of site words


def compress(nso):
    import lz4.block
    h = bytearray(nso[:0x100]); segs = []
    for i in range(3):
        fo, mo, sz = struct.unpack_from('<III', h, 0x10 + 0x10 * i)
        segs.append((mo, sz, nso[fo:fo + sz]))
    body = b''; flags = 0
    for i, (mo, sz, raw) in enumerate(segs):
        c = lz4.block.compress(raw, store_size=False)
        fo = 0x100 + len(body); body += c
        struct.pack_into('<III', h, 0x10 + 0x10 * i, fo, mo, sz)
        struct.pack_into('<I', h, 0x60 + 4 * i, len(c))
        flags |= 1 << i
    struct.pack_into('<I', h, 0xc, flags)
    return bytes(h) + body


def sha(b): return hashlib.sha256(b).hexdigest()


class Worker:
    def __init__(self, args, n):
        self.root = os.path.join(args.work, f'w{n}')
        shutil.rmtree(self.root, ignore_errors=True)
        os.makedirs(self.root)
        shutil.copytree(os.path.join(args.engine, 'tools'), os.path.join(self.root, 'dm', 'tools'), ignore=shutil.ignore_patterns('__pycache__', 'tests', 'fuzz', 'ghidra'))
        for prof in os.listdir(os.path.join(args.engine, 'games')):
            src = os.path.join(args.engine, 'games', prof, 'patches.py')
            if os.path.isfile(src):
                os.makedirs(os.path.join(self.root, 'dm', 'games', prof), exist_ok=True)
                shutil.copy(src, os.path.join(self.root, 'dm', 'games', prof, 'patches.py'))
        self.args, self.n = args, n
        self.tools = os.path.join(self.root, 'dm', 'tools')
        sys.path.insert(0, self.tools)

    def patches_path(self, prof): return os.path.join(self.root, 'dm', 'games', prof, 'patches.py')

    def fingerprints(self, nso_bytes, funcs):
        """the engine's own fingerprint of each synthetic function (by start)"""
        path = os.path.join(self.root, 'fp.nso')
        open(path, 'wb').write(nso_bytes)
        import subprocess as sp
        code = ('import sys,json; sys.path.insert(0, %r); import nso, nso_match; m = nso.read(%r); im = nso_match.Image(m.image, m.text_end); '
                'print(json.dumps({str(s): im.fingerprint(s) for s in %r}))' % (self.tools, path, [s for s, _ in funcs]))
        r = sp.run([self.args.python, '-B', '-c', code], capture_output=True, text=True, env=dict(os.environ, PYTHONDONTWRITEBYTECODE='1'), cwd=self.root)
        assert r.returncode == 0, r.stderr
        import json
        return {int(k): v for k, v in json.loads(r.stdout).items()}

    def run_python(self, nso_bytes, prof, module, subst):
        path = self.patches_path(prof)
        orig = open(path, encoding='utf-8').read()
        text = orig
        for a, b in subst.items():
            assert a in text, a
            text = text.replace(a, b)
        open(path, 'w', encoding='utf-8').write(text)
        try:
            nso = os.path.join(self.root, 'in.nso'); out = os.path.join(self.root, 'py')
            open(nso, 'wb').write(nso_bytes)
            shutil.rmtree(out, ignore_errors=True)
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1', DM_GAME=prof)
            env.pop('DM_GAMES_DIR', None)
            p = subprocess.run([self.args.python, '-B', 'patch_nso.py', nso, out, module], capture_output=True, text=True, env=env, cwd=self.tools)
        finally:
            open(path, 'w', encoding='utf-8').write(orig)
        files = {}
        if os.path.isdir(out):
            for f in sorted(os.listdir(out)):
                if f != 'build_src.sha256': files[f.split('.', 1)[1]] = open(os.path.join(out, f), 'rb').read()
        err = (p.stderr.strip().splitlines() or [''])[-1]
        if err.startswith('AssertionError: '): err = err[len('AssertionError: '):]
        return dict(rc=p.returncode, stdout=p.stdout, err=err, files=files)

    def run_rust(self, nso_bytes, prof, module):
        nso = os.path.join(self.root, 'in-rs.nso'); out = os.path.join(self.root, 'rs')
        open(nso, 'wb').write(nso_bytes)
        shutil.rmtree(out, ignore_errors=True)
        env = dict(os.environ); env.pop('DM_GAMES_DIR', None)
        p = subprocess.run([self.args.rust, '--threads', '2', '--game', prof, nso, out, module], capture_output=True, text=True, env=env)
        files = {}
        if os.path.isdir(out):
            for f in sorted(os.listdir(out)):
                if f != 'build_src.sha256': files[f.split('.', 1)[1]] = open(os.path.join(out, f), 'rb').read()
        return dict(rc=p.returncode, stdout=p.stdout, err=p.stderr.strip().splitlines()[-1] if p.stderr.strip() else '', files=files)


def cases_of(entry):
    base = ['ok', 'mismatch']
    if 'rule' in entry:
        rule = entry['rule']
        base += ['miss', 'dup'] if 'calls' not in rule else ['miss', 'nosize', 'usesx16']
        base += [f'miss-other{i}' for i in range(len(rule.get('others', {})))]
    if entry['kind'] == 'STUBS': base.append('sdkname')   # a build ID's STUBS apply to any module that carries it (the module name is not asked)
    base.append('far')
    return base


def run_case(w, entry, case, export):
    lay, rec, bid, module, text_size = plan(entry)
    if case == 'sdkname': module = 'sdk'
    subst = {}
    funcs = list(lay.functions)
    bss = 0x1000
    pokes_extra = {}
    if case == 'far': bss = 0x8000000
    if 'rule' in entry and 'calls' in entry['rule']:
        main_start = lay.names['main']
        if case == 'miss': lay.pokes[main_start + 0x100] = NOP                       # no call of the anchor
        if case == 'nosize': lay.pokes[main_start + entry['rule']['reviewed'][0][0] + 4] = NOP   # a movz without its movk
        if case == 'usesx16': lay.pokes[main_start + 0x200] = 0xd2800030            # mov x16,#1
    if case == 'mismatch' and rec.sites:
        off, orig = rec.sites[0]
        lay.pokes[off] ^= 0x1      # one bit of the first site word
    if 'rule' in entry:
        mark_functions(lay, funcs)
        if case == 'dup' and 'calls' not in entry['rule']:
            main_start, main_size = funcs[0]
            copy_start = (text_size + 0xff) & ~0xff
            text_size = (copy_start + main_size + 0x1000 + 0xfff) & ~0xfff
            funcs.append((copy_start, main_size))
            for o in [o for o in list(lay.pokes) if main_start <= o < main_start + main_size]: lay.pokes[copy_start + (o - main_start)] = lay.pokes[o]
    nso, info = stubsynth.build(funcs, lay.pokes, text_size, bid, bss=bss, imports=lay.imports)
    fpmap = {}
    if 'rule' in entry and 'calls' not in entry['rule']:
        fp = w.fingerprints(nso, funcs)
        rule = entry['rule']
        main_start = lay.names['main']
        if case != 'miss': subst[rule['fingerprints'][0]] = fp[main_start]; fpmap[rule['fingerprints'][0]] = fp[main_start]
        for i, (label, fn) in enumerate(rule.get('others', {}).items()):
            if case == f'miss-other{i}': continue
            subst[fn['fingerprints'][0]] = fp[lay.names[label]]; fpmap[fn['fingerprints'][0]] = fp[lay.names[label]]
    elif 'rule' in entry and case == 'miss':
        pass
    py = w.run_python(nso, entry['profile'], module, subst)
    name = f"{entry['profile']}-{entry['kind'].lower()}-{entry.get('key', '')[:8] or entry['module'] + str(entry['index'])}-{case}"
    res = dict(name=name, entry=entry, case=case, module=module, py=py, nso=nso, fpmap=fpmap)
    if 'key' in entry:
        res['rs'] = w.run_rust(nso, entry['profile'], module)
    return res


def compare(a, b):
    if a['rc'] != 0 and b['rc'] != 0:
        # a bare `assert` has no message in the Python: only the fact that both stop is comparable then
        return 'SAME-FAIL' if a['err'] in ('', 'AssertionError') or a['err'] == b['err'] else f"MISMATCH both fail: py {a['err'][:90]!r} | rs {b['err'][:90]!r}"
    if a['rc'] != 0 or b['rc'] != 0:
        return f"MISMATCH py rc={a['rc']} {a['err'][:80]!r} | rs rc={b['rc']} {b['err'][:80]!r}"
    bad = [f for f in sorted(set(a['files']) | set(b['files'])) if a['files'].get(f) != b['files'].get(f)]
    if a['stdout'] != b['stdout']: bad.append('stdout')
    return 'SAME' if not bad else f'DIFF {bad}'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--engine', required=True); ap.add_argument('--python', required=True); ap.add_argument('--rust', required=True)
    ap.add_argument('--work', required=True, help='scratch folder (copies of the engine tools are made in it)'); ap.add_argument('--only'); ap.add_argument('--jobs', type=int, default=4); ap.add_argument('--export')
    ap.add_argument('--cases', help='comma-separated list of cases to run (default: all)')
    args = ap.parse_args()
    sys.path.insert(0, os.path.join(args.engine, 'tools'))
    os.makedirs(args.work, exist_ok=True)
    todo = [(e, c) for e in entries(args.engine, args.only) for c in cases_of(e) if not args.cases or c in args.cases.split(',')]
    workers = [Worker(args, n) for n in range(args.jobs)]
    free = list(range(args.jobs)); lock = threading.Lock()
    results = []
    def job(ec):
        with lock: n = free.pop()
        try: return run_case(workers[n], *ec, args.export)
        finally:
            with lock: free.append(n)
    tally = {}
    with ThreadPoolExecutor(args.jobs) as ex:
        for r in ex.map(job, todo):
            results.append(r)
            if 'rs' in r:
                verdict = compare(r['py'], r['rs'])
            else:
                verdict = f"py rc={r['py']['rc']} (matched rule: run `cargo test` on the exported fixtures)"
            if 'rs' in r and verdict.startswith('SAME') and r['py']['rc'] == 0 and 'mismatch' in r['case']: verdict = 'MISMATCH (case mismatch passed)'
            if r['case'] == 'mismatch' and r['py']['rc'] == 0: verdict += ' [!] py did not fail'
            key = verdict.split(' ')[0]
            tally[key] = tally.get(key, 0) + 1
            print(f"{r['name']:70s} {verdict}  ({len(r['py']['files'].get('cuts.tsv', b'').splitlines())} cut rows)", flush=True)
            if r['py']['rc'] != 0 and (os.environ.get('STUBTEST_VERBOSE') or r['case'] not in ('mismatch',)): print(f"    py: {r['py']['err'][:200]}" + (f"\n    rs: {r['rs']['err'][:200]}" if 'rs' in r else ''))
    print('TALLY', tally)
    if args.export:
        os.makedirs(args.export, exist_ok=True)
        with open(os.path.join(args.export, 'manifest.tsv'), 'w') as m:
            m.write('# case\tkind\tname\tvalue: meta rows (profile, module, fpmap = registered fingerprint=synthetic one, result, error) and, for a case the Python finished, the SHA-256 of what it wrote and printed\n')
            for r in results:
                c = r['name']
                open(os.path.join(args.export, c + '.nso'), 'wb').write(compress(r['nso']))
                m.write(f"{c}\tmeta\tprofile\t{r['entry']['profile']}\n{c}\tmeta\tmodule\t{r['module']}\n")
                m.write(f"{c}\tmeta\tfpmap\t{','.join(a + '=' + b for a, b in r['fpmap'].items()) or '-'}\n")
                py = r['py']
                if py['rc'] == 0:
                    m.write(f"{c}\tmeta\tresult\tok\n{c}\tstdout\t-\t{sha(py['stdout'].encode())}\n")
                    for f, data in py['files'].items(): m.write(f"{c}\tfile\t{f}\t{sha(data)}\n")
                else:
                    m.write(f"{c}\tmeta\tresult\tfail\n{c}\tmeta\terror\t{py['err']}\n")
    bad = [r for r in results if 'rs' in r and not compare(r['py'], r['rs']).startswith('SAME')]
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
