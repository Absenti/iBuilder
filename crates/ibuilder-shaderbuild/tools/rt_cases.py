#!/usr/bin/env python3
"""Records what the shipped engine's `dmsh_rt_build.make_job` answers for synthetic ray-tracing modules, as the fixture of `tests/rt_build.rs`.

usage: rt_cases.py <engine>/dm/gfx <out.json>      (run with the reference venv's python3 -B -I-free: it imports the engine's own tools and its test builders)

Every case is one `make_job(records)` call: the records (key, control, code as hex) and either its answer (the manifest, the translator job `rt`, the linked code's SHA-256 and size,
`local_memory_bytes`, the modules' entries) or the text of the `InvalidModule` it raised. The modules are the ones the engine's own `tests/test_dmsh_rt_build.py` builds (`record()`): small
serialized RTCore metadata with chosen entries / resources / fingerprints, plus mutations of their control section; no game file is involved.
"""
import hashlib, json, random, struct, sys
from pathlib import Path

gfx = Path(sys.argv[1])
sys.path.insert(0, str(gfx / 'tools'))
sys.path.insert(0, str(gfx / 'tests'))
from dmsh_rt import InvalidModule
from dmsh_rt_build import make_job
from test_dmsh_rt_build import record
from test_dmsh_rt import replace

cases = []


def hexed(r):
    return dict(key=r['key'], control=bytes(r['control']).hex(), code=bytes(r['code']).hex())


def add(name, records):
    entry = dict(name=name, records=[hexed(r) if not isinstance(r['key'], bool) else None for r in records])
    try:
        job = make_job(records)
    except InvalidModule as e:
        entry['error'] = str(e)
    else:
        entry['ok'] = dict(manifest=job.manifest, rt=job.rt, code_sha256=hashlib.sha256(job.code).hexdigest(), code_len=len(job.code),
                           local_memory_bytes=job.local_memory_bytes,
                           module_entries={f'{k:016x}': [dict(e.job_record(), entry_index=e.entry_index, region_index=e.region_index) for e in v] for k, v in job.module_entries.items()})
    cases.append(entry)


ray, _ = record(1)
miss, _ = record(3, entries=((('unrelated_miss_name',), 8, 16),))
hit, _ = record(9, entries=((('not_an_engine_group',), 7, 64),))
add('universal', [hit, ray, miss])
add('one', [ray])
add('duplicate-same', [ray, dict(ray, key='%016x' % 1)])
add('duplicate-conflict', [ray, dict(ray, code=bytes(len(ray['code'])))])
add('no-records', [])
add('multi-entry', [record(1, entries=((('a', 'b'), 0, 32), (('c',), 0, 64)))[0]])
add('layout-order', [record(resources=(('descriptorBaseArray', 0x1430, 0x10, 0x240, b''), ('coreParams', 0x140, 0x40, 0x80, b'')))[0]])
add('initializer', [record(resources=(('coreParams', 0x140, 0x40, 0, b'abc'), ('descriptorBaseArray', 0x1430, 0x10, 0x140, b'xyz')))[0]])
add('fingerprint-differs', [ray, record(2, fingerprint=bytes(16))[0]])
add('layout-differs', [ray, record(2, resources=(('coreParams', 0x140, 0x40, 0, b'a'), ('descriptorBaseArray', 0x1430, 0x10, 0x140, b'')))[0]])
for i, resources in enumerate(((('coreParams', 0x140, 0x40, 0, b''),),
                               (('unknown', 0x140, 0x40, 0, b''), ('descriptorBaseArray', 0x1430, 0x10, 0x140, b'')),
                               (('coreParams', 0x180, 0x40, 0, b''), ('descriptorBaseArray', 0x1430, 0x10, 0x180, b'')),
                               (('coreParams', 0x140, 0x40, 0, b''), ('descriptorBaseArray', 0x1430, 0x10, 0x100, b'')))):
    add(f'bad-resources-{i}', [record(resources=resources)[0]])
for i, entries in enumerate(((  (('hit', 'unsupported_resume'), 7, 16),), (( ('not_raygen',), 8, 0),), ((('ray',), 0, 0x10000), (('hit',), 7, 32)), ((('ray',), 0, 0x10001),))):
    add(f'bad-entries-{i}', [record(entries=entries)[0]])
item, marks = record()
for i, (offset, fmt, value) in enumerate(((marks['entry0'] + 4, 'I', 16), (marks['fixed'] + 8, 'I', 32), (marks['selector'], 'i', 0), (marks['reloc'] + 8, 'Q', 1))):
    add(f'native-{i}', [dict(item, control=replace(item['control'], offset, fmt, value))])
for i, control in enumerate((item['control'][:0x67], replace(item['control'], 0, 'I', 0), replace(item['control'], 4, 'I', 1), replace(item['control'], 0x56, 'B', 5),
                             replace(item['control'], 0x5c, 'I', 0), replace(item['control'], 0x60, 'I', 0x60), replace(item['control'], 0x60, 'I', 0xe1),
                             replace(item['control'], 0x5c, 'I', len(item['control'])), replace(item['control'], 0x64, 'I', 16), replace(item['control'], 0x44, 'I', 16))):
    add(f'bad-control-{i}', [dict(item, control=control)])
# the logical-ID exhaustion case (10,923 modules of 2 + 1 regions) is not stored: the record is the same bytes under 10,923 keys; the answer is recorded, the test rebuilds the input
big = record(entries=((('a', 'b'), 0, 32), (('c',), 0, 64)))[0]
try:
    make_job(dict(big, key=k) for k in range(10923))
    big_answer = dict(ok=True)
except InvalidModule as e:
    big_answer = dict(error=str(e))
cases.append(dict(name='id-exhaustion', many=dict(record=hexed(big), count=10923), **big_answer))

# random: the entries (1-4, 1-3 parts each, kinds 0 / 7 / 8, frames), resources in either order, 1-5 modules under random keys
rng = random.Random(0x5254)
for n in range(60):
    mods = []
    ents_kinds = [0, 7, 8]
    layout = (0x40 * rng.randint(0, 3), 0x200 + 0x10 * rng.randint(0, 3), rng.random() < 0.5)
    for m in range(rng.randint(1, 5)):
        entries = []
        for e in range(rng.randint(1, 4)):
            kind = 0 if (m == 0 and e == 0 and rng.random() < 0.9) else rng.choice(ents_kinds)   # most cases have a ray-generation entry
            parts = tuple(f'p{m}_{e}_{k}' for k in range(rng.randint(1, 3 if kind == 0 else 1)))
            entries.append((parts, kind, rng.choice([0, 16, 32, 96, 160, 0x1000, 0x7000, 0x7ff0])))
        a, b = layout[0], layout[1] + (0x10 if rng.random() < 0.05 else 0)   # now and then one module disagrees with the others
        r1 = ('coreParams', 0x140, 0x40, a, b'')
        r2 = ('descriptorBaseArray', 0x1430, 0x10, b, b'')
        mods.append(record(rng.getrandbits(64), entries=tuple(entries), resources=(r1, r2) if layout[2] else (r2, r1))[0])
    add(f'random-{n}', mods)

Path(sys.argv[2]).write_text(json.dumps(cases, separators=(',', ':')))
ok = sum(1 for c in cases if 'ok' in c)
print(f'{len(cases)} cases: {ok} answers, {len(cases) - ok} errors; {Path(sys.argv[2]).stat().st_size} bytes')
