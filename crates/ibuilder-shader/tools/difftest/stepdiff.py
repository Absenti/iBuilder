#!/usr/bin/env python3
"""The Python half of the per-step differential test (docs/shaders/SHADER-RUST.md, section 3).

    PYTHONDONTWRITEBYTECODE=1 python -I stepdiff.py <engine copy (the dir holding dm/)> <cases.tsv> <out.tsv> [--jobs N] [--only STEP] [--dump DIR ID...]

cases.tsv: `id<TAB>step<TAB>path of the input MSL<TAB>args json`. Every case applies ONE step function of the engine (dmsh_rewrite / dmsh_entry / dmsh_wide /
dmsh_atomic / dmsh_reflect / dmsh_side / dmsh_geometry / dmsh_build.vs_tf, the functions themselves, not a re-statement) to the text and writes
`id status same len crc32 meta msg` (status OK or ERR: a ValueError or a SystemExit; same: text unchanged; meta: JSON of what else the function returns, sorted keys).
`step_diff` (Rust) writes the same format; compare.py compares the two."""
import ast, json, os, re, signal, sys, zlib

eng = os.path.abspath(sys.argv[1])
sys.path.insert(0, os.path.join(eng, 'dm/gfx/tools')); sys.path.insert(0, os.path.join(eng, 'dm/tools'))
import dmsh_rewrite as rw, dmsh_entry as en, dmsh_wide, dmsh_atomic, dmsh_reflect, dmsh_side, dmsh_geometry  # noqa: E402
import dmsh_tess, dmsh_meshstage as dms, dmsh_mesh  # noqa: E402  (round 3: tools/tessmesh/)
import zlib as _z

def _extract(names):   # functions / constants of dmsh_build.py by name, executed alone (the module itself runs a whole build at import)
    src = open(os.path.join(eng, 'dm/gfx/tools/dmsh_build.py')).read()
    tree = ast.parse(src); ns = {'re': re, 'sys': sys, 'struct': __import__('struct')}
    for node in tree.body:
        nm = node.targets[0].id if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name) else getattr(node, 'name', None)
        if nm in names: exec(compile(ast.Module([node], []), 'dmsh_build.py', 'exec'), ns)
    return ns
BUILD = _extract({'TF_SLOT', 'vs_tf', 'tf_layout'})

def J(local=None): return local

def none_or(r): return ('' if r is None else r), {'none': r is None}

def first_store(m, a):
    t, stores = rw.first_store(m, a.get('local')); return t, {'stores': [list(s) for s in stores]}

def gather(m, a): return rw.gather_offsets(m), {}


def vs_pad_py(src, partner):   # dmsh_build.vs_pad with the partner locations given (partner_inputs read them from the pixel shaders' finished files)
    miss = sorted(partner - {int(x) for x in re.findall(r'\[\[user\(locn(\d+)\)', src)})
    i = src.find('struct main0_out\n{\n'); j = src.find('\n};', i); k = src.find('main0_out out = {};')
    if not miss or i < 0 or j < 0 or k < 0: return src, []
    src = src[:j] + ''.join(f'\n    float4 dm_pad{l} [[user(locn{l})]];' for l in miss) + src[j:]
    k = src.find('main0_out out = {};') + len('main0_out out = {};')
    return src[:k] + ''.join(f'\n    out.dm_pad{l} = float4(0.0, 0.0, 0.0, 1.0);' for l in miss) + src[k:], miss

def crc(t): return None if t is None else _z.crc32(t.encode())

def read_only_of(res):   # dmsh_build.finish(): the MSL s<n> numbers (binding order) of the storage buffers the translator does not write, the raw pool table left out
    import dmsh_abi as abi
    return {i for i, b in enumerate(sorted(res['sbufs'], key=lambda b: b['binding'])) if not b['flags'] & 1 and b['cb_slot'] != abi.GMEM_MAP_CB_SLOT}

def head_rewrites(msl, stage, read_only, mode, local, mesh_kind):
    """dmsh_build.finish() from check_msl to the raw / mesh helpers' rewrites (upstream 033793f0, dmsh_build.py l.880-937), without a mesh stage's to_stage (dmsh_meshstage)"""
    src = dmsh_atomic.check_msl(msl)
    if stage == 'ps': src = rw.per_vertex(src)
    src = rw.precise(src)
    src = rw.bit0_mask(src)
    src = rw.select_guards(src)
    src = rw.mul_extended(src)
    src = rw.float_add_msl(src)
    src = rw.gather_offsets(src)
    src = rw.hmma_msl(src)
    src = rw.shared_store_msl(src)
    src = dmsh_reflect.pool_arrays(src)
    src = dmsh_wide.wide_loads(src)
    src = dmsh_wide.constant_reads(src, read_only, mode)
    src = rw.tg_fence(src)
    src = rw.border_emu(src)
    if stage == 'ps': src = rw.frag_z_snap(src)[0]
    src = rw.uniform_tests(src)
    blend0 = 0
    if stage == 'ps': src, blend0 = rw.blend_zero(src)
    first, spill, loop_sites = [], 0, []
    if not mesh_kind and stage == 'cs':
        src, first = rw.first_store(src, local)
        src, spill = rw.tg_spill(src)
        src, loop_sites = rw.compute_loops(src)
        src = rw.max_threads(src, local)
    if 'dmsh_raw_find' in src: src = rw.raw_noinline(src, mesh_kind or stage)
    if mesh_kind == 'ms': src = rw.mesh_subgroup_noinline(src)
    return src, blend0, first, spill, loop_sites

def rewrites(msl, a):   # the `rewrites` step: args {stage, local, read_only, mode, mesh_kind}
    src, blend0, first, spill, sites = head_rewrites(msl, a['stage'], set(a.get('read_only') or ()), a.get('mode', 'fp'), a.get('local'), a.get('mesh_kind'))
    return src, {'blend0': blend0, 'first': [list(f) for f in first], 'spill': spill, 'loop_sites': sites}

def chain(msl, a):
    """dmsh_build.finish() from check_msl to the file it writes, for ps / vs / cs (upstream 033793f0, dmsh_build.py l.880-970), on the engine's own functions"""
    stage = a['stage']; local = a.get('local'); tf = a.get('tf'); partner = a.get('partner')
    res = json.load(open(a['res'])) if a.get('res') else {'notes': [], 'sbufs': []}
    src, blend0, first, spill, loop_sites = head_rewrites(msl, stage, read_only_of(res), a.get('mode', 'fp'), local, None)
    pad = []; lay = None; discard = False; fu = fd = None; shared = None; analysis = None; plain_len = None; viewport_mask = False
    if stage == 'cs':
        out = src
    elif stage == 'vs':
        src = en.vs_base_instance(en.vs_layer(en.vs_init(src)))
        if partner is not None: src, pad = vs_pad_py(src, set(partner))
        src, unculled = en.vs_viewport_mask(src)
        viewport_mask = src != unculled
        capture = dmsh_geometry.capture(unculled)
        src, full = en.vs_depth(src)
        layer_source = en.vs_depth(unculled)[1] if viewport_mask else full
        full += capture
        discard = en.vs_writes(src, dmsh_reflect.reflect('k', 'vs', src, res))
        if discard: full += en.vs_discard(unculled)
        if tf: full += BUILD['vs_tf'](unculled, tf)
        plain_len = len(full)
        full, lay = en.vs_layer_entry(full, layer_source)
        out = full; analysis = src
    else:
        fu = en.fs_uint(src)
        fd = en.fs_dual(src) if a.get('dual_want') else None
        shared = dmsh_side.shared_body(src) if fu or fd else None
        if shared: out = shared + (fu or '') + (fd or ''); analysis = src
        else: out = src; fu = fd = None
    meta = {'blend0': blend0, 'fu': bool(fu), 'fd': bool(fd), 'shared': bool(shared), 'pad': pad, 'discard': discard, 'lay': lay, 'first': [list(f) for f in first], 'spill': spill,
            'analysis_crc': crc(analysis), 'plain_len': plain_len, 'loop_sites': loop_sites, 'viewport_mask': viewport_mask}
    return out, meta

def tf_layout_step(m, a):   # the control section as hex text; the layout as json, the `[RED]` line the function prints in the meta
    import io, contextlib
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf): r = BUILD['tf_layout']('k', bytes.fromhex(m.strip()))
    return json.dumps(r, sort_keys=True, separators=(',', ':')), {'red': buf.getvalue().strip() or None}

STEPS = {
    'chain': chain,
    'tf_layout': tf_layout_step,
    'check_msl': lambda m, a: (dmsh_atomic.check_msl(m), {}),
    'wide_atomics': lambda m, a: (dmsh_atomic.wide_atomics(m), {}),
    'per_vertex': lambda m, a: (rw.per_vertex(m), {}),
    'precise': lambda m, a: (rw.precise(m), {}),
    'bit0_mask': lambda m, a: (rw.bit0_mask(m), {}),
    'select_guards': lambda m, a: (rw.select_guards(m), {}),
    'mul_extended': lambda m, a: (rw.mul_extended(m), {}),
    'float_add_msl': lambda m, a: (rw.float_add_msl(m), {}),
    'gather_offsets': gather,
    'hmma_msl': lambda m, a: (rw.hmma_msl(m), {}),
    'shared_store_msl': lambda m, a: (rw.shared_store_msl(m), {}),
    'pool_arrays': lambda m, a: (dmsh_reflect.pool_arrays(m), {}),
    'wide_loads': lambda m, a: (dmsh_wide.wide_loads(m), {}),
    'tg_fence': lambda m, a: (rw.tg_fence(m), {}),
    'border_emu': lambda m, a: (rw.border_emu(m), {}),
    'frag_z_snap': lambda m, a: (lambda r: (r[0], {'n': r[1]}))(rw.frag_z_snap(m)),
    'uniform_tests': lambda m, a: (rw.uniform_tests(m), {}),
    'blend_zero': lambda m, a: (lambda r: (r[0], {'k': r[1]}))(rw.blend_zero(m)),
    'first_store': first_store,
    'tg_spill': lambda m, a: (lambda r: (r[0], {'total': r[1]}))(rw.tg_spill(m)),
    'max_threads': lambda m, a: (rw.max_threads(m, a.get('local')), {}),
    'vs_init': lambda m, a: (en.vs_init(m), {}),
    'vs_layer': lambda m, a: (en.vs_layer(m), {}),
    'vs_base_instance': lambda m, a: (en.vs_base_instance(m), {}),
    'drop_point_size': lambda m, a: (en.drop_point_size(m), {}),
    'vs_depth': lambda m, a: (lambda r: (r[0], {'full': r[1]}))(en.vs_depth(m)),
    'vs_discard': lambda m, a: (en.vs_discard(m), {}),
    'vs_layer_entry': lambda m, a: (lambda r: (r[0], {'lay': r[1]}))(en.vs_layer_entry(m)),
    'capture': lambda m, a: (dmsh_geometry.capture(m), {}),
    'fs_uint': lambda m, a: none_or(en.fs_uint(m)),
    'fs_dual': lambda m, a: none_or(en.fs_dual(m)),
    'shared_body': lambda m, a: none_or(dmsh_side.shared_body(m)),
    'vs_tf': lambda m, a: (BUILD['vs_tf'](m, a['tf']), {}),
    # upstream 033793f0's new steps, and its finish() order over the rewrites (`rewrites`)
    'constant_reads': lambda m, a: (dmsh_wide.constant_reads(m, set(a.get('read_only') or ()), a.get('mode', 'fp')), {}),
    'compute_loops': lambda m, a: (lambda r: (r[0], {'sites': r[1]}))(rw.compute_loops(m)),
    'raw_noinline': lambda m, a: (rw.raw_noinline(m, a.get('stage', 'cs')), {}),
    'mesh_subgroup_noinline': lambda m, a: (rw.mesh_subgroup_noinline(m), {}),
    'rewrites': rewrites,
}

# ---- round 3 (tools/tessmesh/): tessellation, task / mesh shaders, passthrough geometry -----------------------------------------------------------------------------
def _tuples(info):   # the JSON form of an info dict -> the Python's (tuples for the component pairs)
    info = dict(info)
    for k in ('vertex', 'primitive'):
        if k in info: info[k] = [tuple(x) for x in info[k]]
    return info

def tess_hull(m, a):
    out, facts = dmsh_tess.hull(m, a['spv']); return out, {'facts': facts}

def tess_domain(m, a):
    out, facts = dmsh_tess.domain(m, a['spv'], a.get('mode')); return out, {'facts': facts}

def tess_mode(m, a):
    r = dmsh_tess.control_mode(bytes.fromhex(a['ctl']))
    return '', {'mode': r, 'bits': dmsh_tess.mode_bits(r) if r else None} if r else {'mode': None}

def mesh_info(m, a):
    i = dms.info(bytes.fromhex(a['ctl']), bytes.fromhex(a['area']))
    lay, size = dms.memory(i)   # a KeyError for a kind that is neither ts nor ms: the Python build stops
    return '', {'info': i, 'memory': lay, 'size': size, 'own_shared': dms.own_shared(bytes.fromhex(a.get('code', '')))}

def mesh_to_stage(m, a):
    i = _tuples(a['info'])
    reads = {int(k): v for k, v in a['reads'].items()} if a.get('reads') is not None else None
    out = dms.to_stage(m, i, reads)
    return out, {'spill': bool(i.get('spill'))}

def mesh_pixel_reads(m, a):
    return '', {'reads': {str(k): v for k, v in dms.pixel_reads(bytes.fromhex(a['sph'])).items()}}

def mesh_compose(m, a):
    return dmsh_mesh.compose(m, open(a['gs'], encoding='utf-8', newline='').read(), a['reads']), {}

def geo_lean(m, a):
    out, reads = dmsh_geometry.lean(m, a['stride']); return out, {'reads': reads}

STEPS.update({'tess_hull': tess_hull, 'tess_domain': tess_domain, 'tess_mode': tess_mode, 'mesh_info': mesh_info, 'mesh_to_stage': mesh_to_stage, 'mesh_pixel_reads': mesh_pixel_reads,
              'mesh_compose': mesh_compose, 'geo_lean': geo_lean, 'mesh_out_bytes': lambda m, a: ('', {'n': dmsh_mesh.out_bytes(m)}), 'mesh_prims_for': lambda m, a: ('', {'p': dmsh_mesh.prims_for(a['n'])})})

def _alarm(signum, frame): raise TimeoutError('timeout')
signal.signal(signal.SIGALRM, _alarm)

def one(row, text):
    cid, step, path, args = row
    fn = STEPS.get(step)
    if fn is None: return f'{cid}\tUNKNOWN\t0\t0\t0\t{{}}\t""', None
    try:
        signal.setitimer(signal.ITIMER_REAL, 60 if step in ('chain', 'rewrites') else float(os.environ.get('STEPDIFF_TIMEOUT', '1')))   # a few of the engine's functions loop without end on texts no real shader has (drop_point_size): that is an ERR here
        try: out, meta = fn(text, args)
        finally: signal.setitimer(signal.ITIMER_REAL, 0)
    except TimeoutError as e: return f'{cid}\tERR\t0\t0\t0\tnull\t"timeout"', None
    except ValueError as e: return f'{cid}\tERR\t0\t0\t0\tnull\t{json.dumps(str(e), ensure_ascii=False)}', None
    except SystemExit as e: return f'{cid}\tERR\t0\t0\t0\tnull\t{json.dumps(str(e.code), ensure_ascii=False)}', None
    except Exception as e: return f'{cid}\tERR\t0\t0\t0\tnull\t{json.dumps(type(e).__name__ + ": " + str(e), ensure_ascii=False)}', None   # an exception finish() does not catch (IndexError, RecursionError, ...): the build stops there; ERR
    b = out.encode()
    return f'{cid}\tOK\t{int(out == text)}\t{len(b)}\t{zlib.crc32(b)}\t{json.dumps(meta, sort_keys=True, separators=(",", ":"), ensure_ascii=False)}\t""', out

_cache = {}
def run_chunk(rows):
    res = []
    for row in rows:
        p = row[2]
        if p not in _cache:
            if len(_cache) > 64: _cache.clear()
            _cache[p] = open(p, encoding='utf-8', newline='').read()
        res.append(one(row, _cache[p]))
    return res

def main():
    cases, out = sys.argv[2], sys.argv[3]
    args = sys.argv[4:]
    jobs = int(args[args.index('--jobs') + 1]) if '--jobs' in args else 6
    only = args[args.index('--only') + 1] if '--only' in args else None
    dump_dir, dump = None, []
    if '--dump' in args: i = args.index('--dump'); dump_dir, dump = args[i + 1], args[i + 2:]
    rows = []
    for l in open(cases, encoding='utf-8').read().split('\n'):
        if not l: continue
        p = l.split('\t', 3)
        if only and p[1] != only: continue
        rows.append((p[0], p[1], p[2], json.loads(p[3]) if len(p) > 3 and p[3] else None))
    rows.sort(key=lambda r: r[2])   # cases on one file together (each worker caches its files)
    order = {r[0]: i for i, r in enumerate(rows)}
    chunks = [rows[i::1] for i in range(1)]
    if jobs > 1:
        import multiprocessing as mp
        n = max(1, len(rows) // (jobs * 8))
        chunks = [rows[i:i + n] for i in range(0, len(rows), n)]
        with mp.get_context('fork').Pool(jobs) as pool: parts = pool.map(run_chunk, chunks)
        results = [r for part in parts for r in part]
    else:
        results = run_chunk(rows)
    lines = {}
    for (line, text), row in zip(results, rows):
        lines[row[0]] = line
        if dump_dir and row[0] in dump and text is not None: open(f'{dump_dir}/{row[0]}.{row[1]}.out', 'w', newline='').write(text)
    with open(out, 'w') as f:
        for row in sorted(rows, key=lambda r: int(r[0]) if r[0].isdigit() else r[0]): f.write(lines[row[0]] + '\n')
    print(len(rows), 'cases', file=sys.stderr)

if __name__ == '__main__': main()
