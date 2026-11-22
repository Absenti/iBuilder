"""The ONE writer of crates/ibuilder-rewrite/tests/fixtures/synth/manifest.tsv (tools/export_fixtures.py builds the fixtures and calls write() below; run alone, this
rewrites the manifest of the fixtures already in the folder). For every case it records what the SHIPPED engine's patch_nso.py does with it: the SHA-256 of every file
it writes and of what it prints, or, when it stops, its message. tests/synthetic.rs reads it, requires the Rust to do the same for every case, and checks the shipped
engine's stamp against the one written here.

Since R6 the Rust ports the shipped engine's jump solver (dm/tools/jump_solver.py), so every case is a parity case: the older engine and the per-case statuses
(`python-stops`, `shipped-differs`) of R5 are gone.

Format (tab-separated; tests/synthetic.rs is its reader):
  #! shipped-engine <source revision: first line of stamp.txt> <sha256 of stamp.txt>
  # ...                                                                             (comments)
  <case> <module name> <file kind> <sha256> shipped                                 (the shipped Python finished: one row per file it wrote, and `stdout`)
  <case> <module name> error <message> shipped                                     (the shipped Python stopped: the last line of its traceback without the
                                                                                    exception's type, `AssertionError: `; the Rust stops with this message)

usage: synth_manifest.py [fixtures dir]      (default: the repository's tests/fixtures/synth; run with the reference venv's python, from a scratch copy of the engine:
                                              never from the original tree; the case list is read from the folder's manifest.tsv, any earlier format)
environment (all required):
  IBR_PY                 the reference venv's python (capstone 5.0.7, lz4)
  IBR_PATCH_NSO          the shipped engine's dm/tools/patch_nso.py (a scratch copy of $IOSBUILDER_WORKTREE/out-stage/out-engine/engine/dm)
  IBR_ENGINE_STAMP       the shipped engine's out-stage/out-engine/stamp.txt (its first line is the source revision; the file's sha256 is the pin)
  IBR_WORK               scratch folder for the oracle runs"""
import hashlib, os, re, shutil, subprocess, sys

FX = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'tests', 'fixtures', 'synth'))
KEYS = ('IBR_PY', 'IBR_PATCH_NSO', 'IBR_ENGINE_STAMP', 'IBR_WORK')


def config():
    """the four settings of the environment (see the docstring), or exit naming the first one missing"""
    return tuple(os.environ.get(k) or sys.exit(f'set {k} (see the docstring of tools/synth_manifest.py)') for k in KEYS)


def sha(b): return hashlib.sha256(b).hexdigest()


def message(stderr, rc):
    """what the Rust's error must say: the traceback's last line without its exception type (`AssertionError: jump table 40: ...` -> `jump table 40: ...`)"""
    last = (stderr.strip().splitlines() or ['exit %d' % rc])[-1]
    m = re.match(r'[A-Za-z_][\w.]*(Error|Exception): (.*)$', last)
    return m.group(2) if m else last


def oracle(py, patch, work, fx, case, mod):
    """[(file kind, sha256 or message)]: stdout first and every file, or [('error', message)] when the Python stops"""
    d = f'{work}/{case}'; shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1'); env.pop('DM_GAME', None); env.pop('DM_GAMES_DIR', None)
    p = subprocess.run([py, patch, f'{fx}/{case}.nso', f'{d}/py', mod], capture_output=True, text=True, env=env, cwd=work)
    if p.returncode:
        msg = message(p.stderr, p.returncode)
        assert '\t' not in msg and '\n' not in msg, msg
        return [('error', msg)]
    rows = [('stdout', sha(p.stdout.encode()))]
    for f in sorted(os.listdir(f'{d}/py')):
        if f != 'build_src.sha256': rows.append((f.split('.', 1)[1], sha(open(f'{d}/py/{f}', 'rb').read())))
    return rows


def cases_of(fx):
    """[(case, module)] in the order of the folder's manifest.tsv (any format: the first two columns of each data row)"""
    cases, seen = [], set()
    for l in open(f'{fx}/manifest.tsv'):
        if l.startswith('#') or not l.strip(): continue
        c, m = l.split('\t')[:2]
        if c not in seen: seen.add(c); cases.append((c, m))
    return cases


def write(cases, fx=FX):
    """Run the shipped engine on `<fx>/<case>.nso` for every (case, module) and write `<fx>/manifest.tsv`"""
    py, patch, stamp_path, work = config()
    os.makedirs(work, exist_ok=True)
    stamp = open(stamp_path, 'rb').read()
    out = ['#! shipped-engine\t%s\t%s\n' % (stamp.decode().splitlines()[0], sha(stamp)),
           '# shipped-engine: the source revision of the engine the app ships (first line of out-stage/out-engine/stamp.txt) and the sha256 of that stamp.txt (the pin tests/synthetic.rs checks)\n',
           '# every case is a parity case: the Rust must write these files and print this stdout, or stop with the `error` row\'s message\n',
           '# written by tools/synth_manifest.py (also called by tools/export_fixtures.py); do not edit by hand\n',
           '# case\tmodule name\tfile (stdout = what patch_nso.py printed; error = it stopped)\tsha256 of the shipped Python\'s output, or its message\toracle\n']
    for case, mod in cases:
        rows = oracle(py, patch, work, fx, case, mod)
        out += [f'{case}\t{mod}\t{f}\t{h}\tshipped\n' for f, h in rows]
        print(case, 'stops: ' + rows[0][1] if rows[0][0] == 'error' else 'rewrites')
    with open(f'{fx}/manifest.tsv', 'w') as m: m.writelines(out)


if __name__ == '__main__':
    fx = sys.argv[1] if len(sys.argv) > 1 else FX
    write(cases_of(fx), fx)
