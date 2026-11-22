"""Stand-in for dmsh_xlate in the end-to-end test (docs/shaders/SHADER-TESS-MESH-RUST.md 3.3): `fake_xlate.py <jobs.json> <work dir>`.
A job whose key has a prepared answer in $TM_FAKE (<key>.spv, <key>.res.json, <key>.reader.spv) gets it copied into the work folder; every other job goes to the real translator
($TM_REAL_XLATE, one call for all of them). The Python build and the Rust build run the same script, so both see the same answers."""
import json, os, shutil, subprocess, sys
jobs = json.load(open(sys.argv[1])); work = sys.argv[2]
fake, real = os.environ['TM_FAKE'], os.environ['TM_REAL_XLATE']
rest = []
log = os.environ.get('TM_JOBLOG')   # a folder: every job is logged there as json.dumps of the job (its `code` path cut to the file name) and the sha256 of the code file it names
if log:
    import hashlib
    os.makedirs(log, exist_ok=True)
    for j in jobs:
        k = j['key']
        try: sha = hashlib.sha256(open(j['code'], 'rb').read()).hexdigest()
        except OSError: sha = 'unreadable'
        open(f'{log}/{k}.job.json', 'w').write(json.dumps({**j, 'code': os.path.basename(j['code'])}) + '\n' + sha + '\n')
for j in jobs:
    k = j['key']
    if os.path.exists(f'{fake}/{k}.spv'):
        for ext in ('.spv', '.res.json', '.reader.spv'):
            if os.path.exists(f'{fake}/{k}{ext}'): shutil.copy(f'{fake}/{k}{ext}', f'{work}/{k}{ext}')
    else: rest.append(j)
if rest:
    tmp = f'{work}/_real_{os.getpid()}.json'
    json.dump(rest, open(tmp, 'w'))
    r = subprocess.run([real, tmp, work], capture_output=True, text=True)
    sys.stdout.write(r.stdout); sys.stderr.write(r.stderr)
    os.unlink(tmp)
    sys.exit(r.returncode)
