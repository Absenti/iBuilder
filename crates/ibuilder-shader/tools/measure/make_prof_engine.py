"""Copy the engine tree and instrument the COPY of dmsh_build.py (scratch folder only): wall + thread-CPU timers around the Python steps,
optional cProfile of every fin() call. Never touches the original."""
import os, shutil
S = os.environ['S'] + '/shader'
src, dst = S + '/engine', S + '/' + os.environ.get('DST', 'engine-prof')
shutil.rmtree(dst, ignore_errors=True)
shutil.copytree(src, dst, ignore=shutil.ignore_patterns('__pycache__'))
p = dst + '/dm/gfx/tools/dmsh_build.py'
t = open(p).read()
INSTR = r'''
# ======================= INSTRUMENTATION (scratch copy only) =======================
import time as _t, threading as _th, cProfile as _cp, pstats as _ps
_IT = collections.defaultdict(lambda: [0, 0, 0, 0]); _ITL = _th.Lock(); _PR = []
_CPROF = os.environ.get('DMSH_CPROFILE') == '1'
_CAP = os.environ.get('DMSH_CAPTURE')
_TL = _th.local()
_STEPS = {'atomic.check_msl', 'rewrite.per_vertex', 'rewrite.precise', 'rewrite.bit0_mask', 'rewrite.select_guards', 'rewrite.mul_extended', 'rewrite.float_add_msl', 'rewrite.gather_offsets', 'rewrite.hmma_msl', 'rewrite.shared_store_msl', 'reflect.pool_arrays', 'wide.wide_loads', 'wide.constant_reads', 'rewrite.tg_fence', 'rewrite.border_emu', 'rewrite.frag_z_snap', 'rewrite.uniform_tests', 'rewrite.blend_zero', 'rewrite.first_store', 'rewrite.tg_spill', 'rewrite.compute_loops', 'rewrite.max_threads', 'entry.vs_init', 'entry.vs_layer', 'entry.vs_base_instance', 'entry.vs_depth', 'entry.vs_discard', 'entry.vs_layer_entry', 'entry.fs_uint', 'entry.fs_dual', 'side.shared_body'}
def _timed(name, fn):
    short = name.replace('dmsh_', '', 1)
    def w(*a, **k):
        t0 = _t.perf_counter_ns(); c0 = _t.thread_time_ns()
        try: r = fn(*a, **k)
        finally:
            t1 = _t.perf_counter_ns(); c1 = _t.thread_time_ns()
        ch = 0
        if a and isinstance(a[0], str):
            o = r[0] if isinstance(r, tuple) and r else r
            if isinstance(o, str): ch = 1 if o != a[0] else 0
            elif o is None: ch = 0
        if _CAP and short == 'atomic.check_msl' and getattr(_TL, 'key', None) and not getattr(_TL, 'captured', True) and a and isinstance(a[0], str):
            import shutil as _sh
            d = os.path.join(_CAP, _TL.key); os.makedirs(d, exist_ok=True)
            open(d + '/raw.metal', 'w').write(a[0]); _sh.copyfile(os.path.join(_TL.wdir, _TL.key + '.res.json'), d + '/res.json')
            json.dump({k: v for k, v in _TL.job.items() if k in ('stage', 'local', 'tf')}, open(d + '/job.json', 'w')); _TL.captured = True
        with _ITL:
            q = _IT[name]; q[0] += 1; q[1] += t1 - t0; q[2] += c1 - c0; q[3] += ch
        if name == 'build.finish': _TL.key = None
        if _CAP and os.environ.get('DMSH_CAPTURE_STEPS') == '1' and short in _STEPS and getattr(_TL, 'key', None) and a and isinstance(a[0], str):
            d = os.path.join(_CAP, _TL.key); os.makedirs(d, exist_ok=True); _TL.seq = getattr(_TL, 'seq', 0) + 1
            o = r[0] if isinstance(r, tuple) and r else r
            if isinstance(o, str):
                with open(os.path.join(d, '%02d_%s.log' % (_TL.seq, short)), 'w') as f: f.write('changed' if o != a[0] else 'same')
                if o != a[0]:
                    open(os.path.join(d, '%02d_%s.in' % (_TL.seq, short)), 'w').write(a[0]); open(os.path.join(d, '%02d_%s.out' % (_TL.seq, short)), 'w').write(o)
        return r
    w.__name__ = getattr(fn, '__name__', name); return w
def _keyed(fn):
    def w(key, wdir, j, c1):
        _TL.key = key; _TL.seq = 0; _TL.wdir = wdir; _TL.job = j; _TL.captured = False
        return fn(key, wdir, j, c1)
    w.__name__ = fn.__name__; return w
def _profiled(fn):
    def w(*a, **k):
        pr = _cp.Profile(); pr.enable()
        try: return fn(*a, **k)
        finally: pr.disable(); _PR.append(pr)
    w.__name__ = getattr(fn, '__name__', 'fn'); return w
import dmsh_fetch as _f, dmsh_side as _s, dmsh_atomic as _a, dmsh_wide as _w, dmsh_geometry as _g, dmsh_entry as _e, dmsh_reflect as _r, dmsh_rewrite as _rw, dmsh_vin as _vi, dmsh_pull as _pu, dmsh_tess as _te, dmsh_meshstage as _ms
for _mod in (_f, _s, _a, _w, _g, _e, _r, _rw, _vi, _pu, _te, _ms):
    for _n in dir(_mod):
        _o = getattr(_mod, _n)
        if callable(_o) and not _n.startswith('_') and getattr(_o, '__module__', None) == _mod.__name__ and not isinstance(_o, type):
            setattr(_mod, _n, _timed(_mod.__name__ + '.' + _n, _o))
for _n in ('job_for', 'finish', 'fin', 'restored', 'rec_of', 'spv_single_lane', 'vs_pad', 'ps_libraries', 'vin_finish', 'pull_finish', 'write_refl', 'publish_cache', 'commit', 'restore_cache',
           'cache_lookup', 'translate', 'shader_key', 'gs_slice', 'geometry_finish', 'clone', 'atomic_json', 'publish_library', 'partner_inputs', 'vs_tf'):
    globals()[_n] = _timed('build.' + _n, _profiled(globals()[_n]) if (_n == 'fin' and _CPROF) else globals()[_n])
finish = _keyed(finish)
'''
marker = 'SERVED = shader_cache.Served(STORE, PROCS) if PROCS and keys else None\n'
assert t.count(marker) == 1
t = t.replace(marker, INSTR + marker)
t += r'''
# ---- dump
_out = os.environ.get('DMSH_IT_OUT')
if _out:
    json.dump({k: v for k, v in _IT.items()}, open(_out, 'w'), indent=1)
    if _PR:
        _st = _ps.Stats(_PR[0])
        for _p in _PR[1:]: _st.add(_p)
        _st.dump_stats(_out + '.pstats')
'''
open(p, 'w').write(t)
print('instrumented', p)
