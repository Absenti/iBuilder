"""The Python side of the differential harness: the same operations as crates/ibuilder-mods/src/bin/mods_diff.rs, through gameapp/mods.py (a copy)."""
import json, os, struct, sys
sys.dont_write_bytecode = True
import importlib.util
import lz4.block
# MODS_PY: the gameapp/mods.py under test (a COPY, run with PYTHONDONTWRITEBYTECODE=1 and python -I from a venv that has lz4)
_spec = importlib.util.spec_from_file_location('mods', os.environ['MODS_PY'])
mods = importlib.util.module_from_spec(_spec)
sys.modules['mods'] = mods
_spec.loader.exec_module(mods)

BIG = 1 << 100
def cl(v): return max(-BIG, min(BIG, v))
def hexs(b): return bytes(b).hex()
def unhex(s): return bytes.fromhex(s)

def err(e):
    n = type(e).__name__
    if isinstance(e, OSError): n = 'OSError'
    if isinstance(e, struct.error): n = 'error'
    msg = str(e) if isinstance(e, (mods.ModError, OSError)) else ''
    return {'exc': n, 'msg': msg}

def ok(v): return {'ok': v}
def writes_json(w): return [[str(cl(o)), hexs(d)] for o, d in w]
def patch_json(p): return {'mod': p.mod, 'source': p.source, 'name': p.name, 'bid': p.bid, 'on': p.on, 'writes': writes_json(p.writes)}
def patch_from(v): return mods.Patch(v['mod'], v['source'], v['name'], v['bid'], v['on'], [(int(o), unhex(d)) for o, d in v['writes']])
def label(p): return '%s/%s [%s]' % (p.mod, p.source, p.name)

def scenario(op):
    docs, tid, exe = op['docs'], op['tid'], op['exe']
    modules = op.get('modules', [])
    logs = []
    steps = []
    plan = None
    for st in op['steps']:
        k = st['k']
        try:
            if k == 'plan':
                plan = mods.plan(docs, tid, exe, modules, logs.append, st.get('quiet', False))
                r = ok([[m, d, [patch_json(p) for p in ps]] for m, (d, ps) in plan.items()])
            elif k == 'apply':
                if plan is None:
                    steps.append({'skip': True}); continue
                res = []
                for m, (d, ps) in plan.items():
                    res.append([m, mods.apply_module(docs, tid, exe, m, ps, logs.append)])
                r = ok(res)
            elif k == 'remove':
                mods.remove_patched(docs, tid); r = ok(None)
            elif k == 'mark':
                mods.mark_applied(docs, tid, logs.append); r = ok(None)
            elif k == 'boot':
                env = dict(st.get('env', {}))
                out = mods.apply_boot(docs, tid, logs.append, env)
                r = ok({'set': [[a, b] for a, b in out.items()], 'env': env})
            elif k == 'cheats':
                r = ok(mods.write_cheats_folder(docs, tid, logs.append))
            elif k == 'signature':
                r = ok(mods.exefs_signature(docs, tid))
            elif k == 'state':
                s = mods.read_state(os.path.join(docs, 'games', tid, 'mods.tsv'))
                r = ok({'mod': sorted([k2, v] for k2, v in s['mod'].items()),
                        'patch': sorted([a, b, v] for (a, b), v in s['patch'].items()),
                        'cheat': sorted([a, b, v] for (a, b), v in s['cheat'].items())})
            elif k == 'list':
                try: en = mods.enabled_mods(docs, tid)
                except Exception as e: en = err(e)
                r = ok({'mods': mods.list_mods(docs, tid), 'enabled': en})
            elif k == 'record':
                mods.record_build_ids(exe, os.path.join(docs, 'games', tid, 'build_ids.txt'), logs.append); r = ok(None)
            elif k == 'patches':
                ps, sk = mods.exefs_patches(docs, tid)
                r = ok({'patches': [patch_json(p) for p in ps], 'skipped': [list(x) for x in sk]})
            else:
                r = {'bad': k}
        except Exception as e:
            r = err(e)
            if k == 'apply': plan = plan
        steps.append(r)
    return {'steps': steps, 'logs': logs}

def run(op):
    o = op['op']
    try:
        if o == 'lz4':
            try: return ok(hexs(mods.lz4_block(unhex(op['hex']), op['size'])))
            except lz4.block.LZ4BlockError: return {'exc': 'LZ4BlockError', 'msg': ''}
        if o == 'ips': return ok(writes_json(mods.parse_ips(unhex(op['hex']))))
        if o == 'pch': return ok([patch_json(p) for p in mods.parse_pchtxt(op['text'], 'm', 'f.pchtxt')])
        if o == 'bidm': return ok(mods.bid_matches(op['wanted'], unhex(op['bid'])))
        if o == 'stem': return ok(mods.stem_bid(op['name']))
        if o == 'sections': return ok(mods.cheat_sections(op['text']))
        if o == 'cheatbid': return ok(mods.cheat_file_bid(op['name']))
        if o == 'repr': return ok(repr(op['text']))
        if o == 'nso':
            new, applied, notes = mods.patch_nso(unhex(op['hex']), [patch_from(p) for p in op['patches']])
            return ok(None if new is None else {'data': hexs(new), 'applied': [label(p) for p in applied], 'notes': [[label(p), n] for p, n in notes]})
        if o == 'image':
            h, segs = mods.nso_image(unhex(op['hex']))
            return ok({'header': hexs(h), 'segs': [[m, hexs(b)] for m, b in segs]})
        if o == 'scenario': return scenario(op)
        return {'bad': o}
    except Exception as e:
        return err(e)

if __name__ == '__main__':
    ops = json.load(open(sys.argv[1]))
    json.dump([run(op) for op in ops], open(sys.argv[2], 'w'))
