# oracle driver (test helper): GAMEAPP_BUNDLE=<engine layout> IBUILDER_ORACLE_GAMEAPP=<gameapp dir> python -B derive_driver.py <mode> ...
# oracle driver: calls profile_derive.derive(cfg) for a list of fake extractions (registered path) or one real one
import os, sys, json
sys.dont_write_bytecode = True
sys.path.insert(0, os.environ['IBUILDER_ORACLE_GAMEAPP'])  # <iOSBuilder copy>/gameapp: the Python oracle's profile_derive.py
import profile_derive as pd
mode = sys.argv[1]
if mode == 'registered':
    root, outroot = sys.argv[2], sys.argv[3]
    for tid in sorted(os.listdir(os.path.join(os.environ['GAMEAPP_BUNDLE'], 'dm', 'games'))):
        if not os.path.isfile(os.path.join(os.environ['GAMEAPP_BUNDLE'], 'dm', 'games', tid, 'game.tsv')): continue
        ex = os.path.join(root, tid, 'extract'); os.makedirs(os.path.join(ex, 'ExeFS'), exist_ok=True)
        open(os.path.join(ex, 'RomFS.img.id'), 'w').write(json.dumps({'title': tid}))
        out = os.path.join(outroot, tid + '.profile.tsv'); os.makedirs(outroot, exist_ok=True)
        cfg = {'extract_out': ex, 'exe_dir': os.path.join(ex, 'ExeFS'), 'romfs': os.path.join(ex, 'RomFS.img'), 'profile': out}
        pd.derive(cfg, log=lambda m: None)
elif mode == 'variants':
    root = sys.argv[2]
    for v in sorted(os.listdir(root)):
        ex = os.path.join(root, v, 'extract')
        cfg = {'extract_out': ex, 'exe_dir': os.path.join(ex, 'ExeFS'), 'romfs': os.path.join(ex, 'romfs_manifest.tsv'), 'profile': os.path.join(root, v, 'profile.tsv')}
        lines = []
        try:
            pd.derive(cfg, log=lines.append)
        except BaseException as e:
            lines.append('EXC %s' % type(e).__name__)
        open(os.path.join(root, v, 'log.txt'), 'w').write('\n'.join(lines) + '\n')
else:  # provisional: extract exe romfs out pdir
    ex, exe, romfs, out, pdir = sys.argv[2:7]
    cfg = {'extract_out': ex, 'exe_dir': exe, 'romfs': romfs, 'profile': out, 'profile_out': pdir}
    print(pd.derive(cfg, log=lambda m: print(m, flush=True)))
