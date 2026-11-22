"""BIG_TEXT layout (text > ~100 MB: the patch area sits before the base): sites at the start and at the end of a 101 MB text."""
import os, sys, subprocess, shutil, random
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import difftest
from synth import build_nso
rng = random.Random(5)
f0, f1 = difftest.make_tcase(3)[0], difftest.make_tcase(4)[0]
nso = build_nso([f0, f1], text_min=0, pad_before_last=0x6100000 // 4 - 800, bss=0x1000000)[0]
d = f'{difftest.S}/rw-fuzz/bigtext'; shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
open(f'{d}/in.nso', 'wb').write(nso); print('nso bytes', len(nso), flush=True)
env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', DM_REWRITE_J='1')
import time; t = time.time()
r = subprocess.run([difftest.RUST, '--threads', '4', f'{d}/in.nso', f'{d}/rs', 'm'], capture_output=True, text=True); print('rust', r.returncode, r.stdout.strip()[-300:], r.stderr.strip()[-200:], round(time.time() - t, 1), 's', flush=True)
t = time.time()
p = subprocess.run([difftest.PY, difftest.PATCH, f'{d}/in.nso', f'{d}/py', 'm'], capture_output=True, text=True, env=env); print('python', p.returncode, p.stdout.strip()[-300:], p.stderr.strip()[-200:], round(time.time() - t, 1), 's', flush=True)
if p.returncode == r.returncode == 0:
    bad = [f for f in os.listdir(f'{d}/py') if f != 'build_src.sha256' and open(f'{d}/py/{f}', 'rb').read() != open(f'{d}/rs/{f}', 'rb').read()]
    print('BIGTEXT', 'SAME' if not bad and p.stdout == r.stdout else f'DIFF {bad}')
    print(open(f'{d}/py/m.fixups.tsv').read()[:200])
