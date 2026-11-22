#!/usr/bin/env python3
"""Dev-time generator (Python is NOT needed at run time): turns the data tables of the engine's patch files into
`src/tables.rs`, the static registry the Rust rewriter uses instead of `importlib`-loading `patches.py`.

Reads (read-only) <engine dm dir>/tools/{sdk_patches,engine_patches}.py and <dm dir>/games/*/patches.py:
  CUTS, OBS, CAPABILITIES (per build ID), MATCH_CUTS (per module; per profile for games, SDK-wide / engine-wide
  for sdk_patches / engine_patches). The code-valued tables are not data, only their stub *functions* are code: they are ported
  by hand into `src/stubs_games/` (see docs/crates/REWRITE-RUST.md). Everything around them is data and is generated here: the
  build IDs of FRAME_STEP / STUBS, and the rules of MATCH_FRAME_STEP / MATCH_STUBS (name, size, fingerprints, `others`,
  the imported symbol of an anchored rule, `fps_required`) with the stub function named by its position in the rule list.
  Each patch file that has code-valued tables also gets a code digest (`code_digest`): the sha256 (16 hex digits) of the source
  of every top-level statement the code-valued tables reach (the stub functions, their helper data and imports; comments,
  blank lines, CUTS / OBS / CAPABILITIES / MATCH_CUTS / BINDING and anything else unreachable left out). The Rust port records
  the digest it was verified against (`stubs_games::PORTS`), so an engine update that changes a stub's code is caught by
  `scripts/check-patch-coverage.sh` and by the `patch_coverage` test instead of silently running a stale port.

usage: gen_patch_tables.py <engine dm dir> [--coverage <file>] [--no-overlay] > crates/ibuilder-rewrite/src/tables.rs
  The patch files listed in tools/overlay/overlay.tsv (upstream changes ported before the pinned engine has them) are read from tools/overlay/ instead of the engine,
  each checked against the sha256 of the engine file it replaces (a moved engine stops the generation); --no-overlay reads the engine's files alone.
  --coverage writes the machine-readable list of everything the patch files register (kind, profile, key, digest, game):
  CUTS / OBS / CAPABILITIES by build ID, MATCH_CUTS by rule, FRAME_STEP / STUBS by build ID, MATCH_FRAME_STEP /
  MATCH_STUBS by rule; `ibuilder-rewrite --list-ported` prints the same rows for what the Rust side can rewrite.
"""
import ast, glob, hashlib, importlib.util, io, json, os, sys, tokenize

args = sys.argv[1:]
coverage_path = None
use_overlay = '--no-overlay' not in args
args = [a for a in args if a != '--no-overlay']
if '--coverage' in args:
    i = args.index('--coverage'); coverage_path = args[i + 1]; del args[i:i + 2]
dm = args[0]
sys.path.insert(0, os.path.join(dm, 'tools'))   # the patch files import nso_asm / nso_match
SHARED = ('sdk_patches.py', 'engine_patches.py')
OVERLAY = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'overlay')
REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', '..'))


def overlay_files():
    """{engine-relative path (dm/...): overlay file} of tools/overlay/overlay.tsv, each checked against the engine file it replaces"""
    out = {}
    if not use_overlay: return out
    for row in open(os.path.join(OVERLAY, 'overlay.tsv'), encoding='utf-8').read().splitlines():
        if not row.strip() or row.startswith('#'): continue
        rel, base = row.split('\t')[:2]
        mine, theirs = os.path.join(OVERLAY, rel), os.path.join(os.path.dirname(dm), rel)
        have = hashlib.sha256(open(theirs, 'rb').read()).hexdigest() if os.path.exists(theirs) else '-'
        if have != base and have != hashlib.sha256(open(mine, 'rb').read()).hexdigest():
            sys.exit(f'[RED] tools/overlay/{rel}: the engine\'s file is not the one this copy was ported onto (sha256 {have}, expected {base}): redo the port (overlay.tsv)')
        out[rel] = mine
    return out


over = overlay_files()
games = {os.path.basename(os.path.dirname(p)): p for p in glob.glob(os.path.join(glob.escape(dm), 'games', '*', 'patches.py'))}
for rel, mine in over.items():
    parts = rel.split('/')
    if parts[1] == 'games': games[parts[2]] = mine
files = [over.get(f'dm/tools/{f}', os.path.join(dm, 'tools', f)) for f in SHARED] + [games[g] for g in sorted(games)]


def s(x):
    r = json.dumps(x, ensure_ascii=False)
    assert '\\u' not in r, r
    return r


def load(p):
    spec = importlib.util.spec_from_file_location('p_' + os.path.basename(os.path.dirname(p)), p)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def fn_rule(fn):
    cuts = ', '.join(f'Cut {{ off: {o}, old: {s(old)}, new: {s(new)}, why: {s(why)} }}' for o, old, new, why in fn['cuts'])
    fps = ', '.join(s(x) for x in fn['fingerprints'])
    return f'FnRule {{ size: {fn["size"]}, fingerprints: &[{fps}], cuts: &[{cuts}] }}'


def rule(r):
    if 'functions' in r:
        return f'Rule {{ name: {s(r["name"])}, multi: true, functions: &[{", ".join(fn_rule(f) for f in r["functions"])}] }}'
    return f'Rule {{ name: {s(r["name"])}, multi: false, functions: &[{fn_rule(r)}] }}'


def rules(d):
    return ', '.join(f'({s(mod)}, &[{", ".join(rule(r) for r in rs)}])' for mod, rs in d.items())

CODE_NAMES = ('FRAME_STEP', 'STUBS', 'MATCH_FRAME_STEP', 'MATCH_STUBS')


def code_digest(path):
    """sha256 (16 hex digits) of the source of the top-level statements the code-valued tables reach, comments and blank lines removed"""
    src = open(path, encoding='utf-8').read()
    lines = src.split('\n')
    for tok in tokenize.generate_tokens(io.StringIO(src).readline):
        if tok.type == tokenize.COMMENT:
            row, col = tok.start
            lines[row - 1] = lines[row - 1][:col]
    tree = ast.parse(src)
    defines = {}   # top-level name -> statements binding it
    for node in tree.body:
        names = []
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)): names = [node.name]
        elif isinstance(node, (ast.Import, ast.ImportFrom)): names = [(a.asname or a.name).split('.')[0] for a in node.names]
        elif isinstance(node, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for t in targets:
                names += [n.id for n in ast.walk(t) if isinstance(n, ast.Name)]
        for n in names: defines.setdefault(n, []).append(node)
    chosen, todo = set(), [n for name in CODE_NAMES for n in defines.get(name, [])]
    while todo:
        node = todo.pop()
        if id(node) in chosen: continue
        chosen.add(id(node))
        for n in ast.walk(node):
            if isinstance(n, ast.Name): todo += [d for d in defines.get(n.id, []) if id(d) not in chosen]
    text = []
    for node in tree.body:
        if id(node) in chosen:
            for line in lines[node.lineno - 1:node.end_lineno]:
                if line.strip(): text.append(line.rstrip())
    return hashlib.sha256('\n'.join(text).encode('utf-8')).hexdigest()[:16]


def game_name(profile):
    # the engine's profile, else this repository's (app/game-tables/<id>/game.tsv: a game only the overlay registers)
    for path in (os.path.join(dm, 'games', profile, 'game.tsv'), os.path.join(REPO, 'app', 'game-tables', profile, 'game.tsv')):
        try:
            for row in open(path, encoding='utf-8').read().splitlines():
                c = row.split('\t')
                if c[0] == 'name' and len(c) > 1: return c[1].strip()
        except OSError: pass
    return profile


def match_rule(r):
    """a MATCH_FRAME_STEP / MATCH_STUBS rule as a Rust MatchRule: the data around its stub function"""
    others = ', '.join(f'({s(label)}, {fn["size"]}, &[{", ".join(s(x) for x in fn["fingerprints"])}])' for label, fn in r.get('others', {}).items())
    if 'calls' in r:
        assert set(r) == {'name', 'calls', 'find', 'stub', 'reviewed'}, r
        return f'MatchRule {{ name: {s(r["name"])}, size: 0, fingerprints: &[], calls: Some({s(r["calls"])}), others: &[], fps_required: false }}'
    fps = ', '.join(s(x) for x in r['fingerprints'])
    return (f'MatchRule {{ name: {s(r["name"])}, size: {r["size"]}, fingerprints: &[{fps}], calls: None, others: &[{others}], '
            f'fps_required: {"true" if r.get("fps_required", False) else "false"} }}')


out = ['// @generated by tools/gen_patch_tables.py from the engine\'s patch files; do not edit.',
       '#![allow(clippy::all)]',
       'use crate::patches::{Cut, FnRule, MatchRule, Obs, Rule};', '']
cuts, obs, caps, sdk_rules, prof_rules = {}, {}, {}, [], []
code = {k: [] for k in CODE_NAMES}      # table -> [(who, [keys])]  (FRAME_STEP / STUBS: build IDs; MATCH_*: module names)
match_rules = {'MATCH_FRAME_STEP': [], 'MATCH_STUBS': []}   # table -> [(profile, module, [rule dicts])]
digests, games, cov = {}, {}, []
for p in files:
    m = load(p)
    base = os.path.basename(p)
    prof = os.path.basename(os.path.dirname(p))
    for bid, v in getattr(m, 'CUTS', {}).items():
        assert bid not in cuts, bid
        cuts[bid] = (base if base in SHARED else prof, v)
        cov.append(('CUTS', base if base in SHARED else prof, bid, '-'))
    for bid, v in getattr(m, 'OBS', {}).items():
        assert bid not in obs, bid
        obs[bid] = v
        cov.append(('OBS', '-', bid, '-'))
    for bid, v in getattr(m, 'CAPABILITIES', {}).items():
        assert bid not in caps, bid
        caps[bid] = v
        cov.append(('CAPABILITIES', '-', bid, '-'))
    mc = getattr(m, 'MATCH_CUTS', {})
    if mc:
        if base in SHARED: sdk_rules.append((base, mc))
        else: prof_rules.append((prof, mc))
        for mod, rs in mc.items():
            for r in rs: cov.append(('MATCH_CUTS', 'sdk' if base in SHARED else prof, f'{mod}:{r["name"]}', '-'))
    digest = None
    for k in CODE_NAMES:
        tbl = getattr(m, k, None)
        if not tbl: continue
        assert base not in SHARED, (base, k)
        digest = digest or code_digest(p)
        digests[prof] = digest
        games[prof] = game_name(prof)
        if k in ('FRAME_STEP', 'STUBS'):
            code[k].append((prof, sorted(tbl)))
            for bid in sorted(tbl): cov.append((k, prof, bid, digest))
        else:
            code[k].append((prof, sorted(tbl)))
            for mod, rs in tbl.items():
                match_rules[k].append((prof, mod, rs))
                for i, r in enumerate(rs): cov.append((k, prof, f'{mod}#{i}:{r["name"]}', digest))

out.append('/// CUTS: (build ID, patch file that registers it, cuts)')
out.append('pub static CUTS: &[(&str, &str, &[Cut])] = &[')
for bid, (where, v) in cuts.items():
    out.append(f'    ({s(bid)}, {s(where)}, &[' + ', '.join(f'Cut {{ off: {o}, old: {s(a)}, new: {s(b)}, why: {s(w)} }}' for o, a, b, w in v) + ']),')
out.append('];')
out.append('')
out.append('/// OBS: (build ID, observation gates)')
out.append('pub static OBS: &[(&str, &[Obs])] = &[')
for bid, v in obs.items():
    out.append(f'    ({s(bid)}, &[' + ', '.join(f'Obs {{ off: {o}, number: {n}, word: {s(w)}, why: {s(why)} }}' for o, n, w, why in v) + ']),')
out.append('];')
out.append('')
out.append('pub static CAPABILITIES: &[(&str, &[&str])] = &[')
for bid, v in caps.items():
    out.append(f'    ({s(bid)}, &[' + ', '.join(s(x) for x in v) + ']),')
out.append('];')
out.append('')
out.append('/// SDK-wide and engine-wide MATCH_CUTS in the order the files load (sdk_patches, then engine_patches): (module, rules)')
out.append('pub static SDK_MATCH_CUTS: &[(&str, &[Rule])] = &[')
for base, mc in sdk_rules:
    for mod, rs in mc.items():
        out.append(f'    ({s(mod)}, &[{", ".join(rule(r) for r in rs)}]),')
out.append('];')
out.append('')
out.append('/// per-game MATCH_CUTS: (profile, [(module, rules)])')
out.append('pub static PROFILE_MATCH_CUTS: &[(&str, &[(&str, &[Rule])])] = &[')
for prof, mc in prof_rules:
    out.append(f'    ({s(prof)}, &[{rules(mc)}]),')
out.append('];')
out.append('')
out.append('/// FRAME_STEP of the patch files: (build ID, profile). The stub functions are code: `stubs_games`')
out.append('pub static FRAME_STEP: &[(&str, &str)] = &[')
for prof, keys in code['FRAME_STEP']:
    for k in keys: out.append(f'    ({s(k)}, {s(prof)}),')
out.append('];')
out.append('')
out.append('/// STUBS of the patch files: (build ID, profile)')
out.append('pub static STUBS: &[(&str, &str)] = &[')
for prof, keys in code['STUBS']:
    for k in keys: out.append(f'    ({s(k)}, {s(prof)}),')
out.append('];')
out.append('')
for k in ('MATCH_FRAME_STEP', 'MATCH_STUBS'):
    out.append(f'/// {k}: (profile, module, rules in the file\'s order; a rule\'s stub function is the one at its position in `stubs_games`)')
    out.append(f'pub static {k}: &[(&str, &str, &[MatchRule])] = &[')
    for prof, mod, rs in match_rules[k]:
        out.append(f'    ({s(prof)}, {s(mod)}, &[')
        for r in rs: out.append(f'        {match_rule(r)},')
        out.append('    ]),')
    out.append('];')
    out.append('')
out.append('/// code digest of each patch file that has code-valued tables (see the header of tools/gen_patch_tables.py): (profile, game name, digest)')
out.append('pub static CODE_DIGESTS: &[(&str, &str, &str)] = &[')
for prof in sorted(digests):
    out.append(f'    ({s(prof)}, {s(games[prof])}, {s(digests[prof])}),')
out.append('];')
print('\n'.join(out))

if coverage_path:
    with open(coverage_path, 'w', encoding='utf-8') as f:
        f.write('# kind\tprofile or file\tkey\tcode digest\tgame (generated by tools/gen_patch_tables.py; `ibuilder-rewrite --list-ported` prints the same rows)\n')
        for kind, who, key, digest in sorted(cov, key=lambda r: (r[0], r[1], r[2])):
            f.write(f'{kind}\t{who}\t{key}\t{digest}\t{games.get(who) or (game_name(who) if len(who) == 16 else "-")}\n')
