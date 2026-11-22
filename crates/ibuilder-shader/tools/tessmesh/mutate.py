"""Does the differential harness have teeth? `mutate.py <corpus dir with res/py.tsv> [--only N]`: for each mutation of the Rust source (one edit, listed below) rebuild `step_diff`, run it over the corpus's cases
of the steps the mutated function belongs to and count the cases whose result differs from the Python's stored result; the source is restored after every mutation (and at the end, whatever happens).
usage: MUT_ROOT=<private workspace copy> CARGO_TARGET_DIR=<dir> python3 mutate.py <corpus> ; prints a table (also as json on the last line)."""
import json, os, shutil, subprocess, sys

REPO = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', '..', '..'))   # this repository
ROOT = os.environ.get('MUT_ROOT')   # a PRIVATE copy of the workspace (ibuilder-shader + ibuilder-testkit): the sources are edited in place, so never the shared repo (other builds read it)
if not ROOT or os.path.realpath(ROOT) == os.path.realpath(REPO): raise SystemExit('set MUT_ROOT to a private copy of the workspace (see the docstring); the repo itself must not be mutated')
SRC = ROOT + '/crates/ibuilder-shader/src/'
corp = sys.argv[1]
TARGET = os.environ['CARGO_TARGET_DIR']
MUTS = [   # (name, file, old, new, steps)
    ('hull: in_bytes += 16 -> 15', 'finish/tess.rs', 'in_bytes += 16;', 'in_bytes += 15;', ['tess_hull']),
    ('location(): l < 32 -> l <= 32', 'finish/tess.rs', 'Some(l) if v.input == input && v.patch == patch && l < 32 => Ok(l),', 'Some(l) if v.input == input && v.patch == patch && l <= 32 => Ok(l),', ['tess_hull', 'tess_domain']),
    ('mode_bits: winding bit 3 -> 2', 'finish/tess.rs', '((m.winding == "cw") as u32) << 3', '((m.winding == "cw") as u32) << 2', ['tess_domain', 'tess_mode']),
    ('domain prologue: dm_i < 32u -> 31u', 'finish/tess.rs', 'dm_i < dm_ncp && dm_i < 32u', 'dm_i < dm_ncp && dm_i < 31u', ['tess_domain']),
    ('hull: patch slots struct skipped for main0_patchOut', 'finish/tess.rs', '[("main0_out", CP_SLOTS, false), ("main0_patchOut", PATCH_SLOTS, true)]', '[("main0_out", CP_SLOTS, false), ("main0_patchOut", PATCH_SLOTS - 1, true)]', ['tess_hull']),
    ('meshstage memory: payload rounding 15 -> 16', 'finish/meshstage.rs', 'let out = INPUT + ((i.payload + 15) & !15);', 'let out = INPUT + ((i.payload + 16) & !15);', ['mesh_info', 'mesh_to_stage']),
    ('to_stage: spill at SPILL_MIN <= words * 4 -> <', 'finish/meshstage.rs', 'SPILL_MIN <= words * 4', 'SPILL_MIN < words * 4', ['mesh_to_stage']),
    ('_width: component 3 test & 8 -> & 4', 'finish/meshstage.rs', 'if (read.unwrap_or(0xf) & 8) != 0', 'if (read.unwrap_or(0xf) & 4) != 0', ['mesh_to_stage']),
    ('info: own_shared opcode 0x388 dropped', 'finish/meshstage.rs', '0x984 | 0x388 | 0x988', '0x984 | 0x988', ['mesh_info']),
    ('pixel_reads: bits of a component 2 * c -> c', 'finish/meshstage.rs', '(0..4).filter(|c| b >> (2 * c) & 3 != 0)', '(0..4).filter(|c| b >> c & 3 != 0)', ['mesh_pixel_reads']),
    ('prims_for: + 4 * p -> + 5 * p', 'finish/mesh.rs', '3 * p * vbytes + 4 * p <= MESH_BYTES', '3 * p * vbytes + 5 * p <= MESH_BYTES', ['mesh_prims_for', 'mesh_compose']),
    ('compose: run slots < RUN_SLOTS -> <=', 'finish/mesh.rs', 'if s < RUN_SLOTS { Some', 'if s <= RUN_SLOTS { Some', ['mesh_compose']),
    ('lean: MASK_WORD 5 -> 6', 'finish/lean.rs', 'pub const MASK_WORD: i64 = 5;', 'pub const MASK_WORD: i64 = 6;', ['geo_lean', 'mesh_compose']),
    ('lean: dead-code use count -= 1 -> -= 2', 'finish/lean.rs', '*u -= 1;', '*u -= 2;', ['geo_lean']),
    ('lean: an array used at a computed index is dropped from `arrays` -> kept', 'finish/lean.rs', 'if used { arrays.remove(&a); }', 'if used && false { arrays.remove(&a); }', ['geo_lean']),
    ('pycompat: splitlines treats \\r as no boundary', 'finish/pycompat.rs', "matches!(c, '\\n' | '\\r' | '\\u{0b}'", "matches!(c, '\\n' | '\\u{0b}'", ['tess_hull', 'tess_domain']),
]
cases = {}
for l in open(corp + '/cases.tsv', encoding='utf-8'):
    p = l.rstrip('\n').split('\t', 3)
    if len(p) >= 3: cases[p[0]] = p[1]
py = {}
for l in open(corp + '/res/py.tsv', encoding='utf-8'):
    c = l.rstrip('\n').split('\t')
    if c and c[0]: py[c[0]] = c


def build():
    r = subprocess.run(['cargo', 'build', '--release', '-p', 'ibuilder-shader', '--bin', 'step_diff'], cwd=ROOT, capture_output=True, text=True)
    if r.returncode: print(r.stderr[-1500:]); raise SystemExit('build failed')


def run_steps(steps):
    sub = f'{corp}/res/mut_cases.tsv'
    with open(sub, 'w') as f:
        for l in open(corp + '/cases.tsv', encoding='utf-8'):
            p = l.split('\t', 3)
            if p[1] in steps: f.write(l)
    out = f'{corp}/res/mut_rs.tsv'
    r = subprocess.run([TARGET + '/release/step_diff', '--cases', sub, '--out', out], capture_output=True, text=True)
    if r.returncode: raise SystemExit('step_diff failed: ' + r.stderr[-500:])
    rs = {}
    for l in open(out, encoding='utf-8'):
        c = l.rstrip('\n').split('\t')
        if c and c[0]: rs[c[0]] = c
    n = d = 0
    for cid, c in rs.items():
        n += 1
        if c[:6] != py[cid][:6]: d += 1
    return n, d


results = []
only = int(sys.argv[sys.argv.index('--only') + 1]) if '--only' in sys.argv else None
try:
    for i, (name, f, old, new, steps) in enumerate(MUTS):
        if only is not None and i != only: continue
        path = SRC + f
        text = open(path).read()
        if text.count(old) != 1: print(f'SKIP {name}: the text to mutate occurs {text.count(old)} times'); continue
        shutil.copy(path, path + '.orig')
        try:
            open(path, 'w').write(text.replace(old, new))
            build()
            n, d = run_steps(set(steps))
        finally:
            shutil.move(path + '.orig', path)
        results.append({'mutation': name, 'steps': steps, 'cases': n, 'differ': d})
        print(f'{name:75} {n:6} cases, {d:5} differ', flush=True)
finally:
    build()   # the original again
print(json.dumps(results))
