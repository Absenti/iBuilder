import sys, os, time, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import shader_device as sd
S = os.environ['S']
IOS = os.environ['IOSBUILDER']   # the original iOSBuilder checkout (read only)
t0 = time.time()
n = sd.split(IOS + '/out/ref-img-grizzy/RomFS.img', IOS + '/out/ref-out-grizzy/ExeFS/main',
             S + '/shader/split-out/pkg', S + '/shader/split-out/tables', log=lambda l: print(l, flush=True), jobs=6,
             profile_dir=S + '/shader/prof-game', game_id='0100DFA027504000', engine=S + '/shader/engine')
print('shaders', n, 'wall', round(time.time() - t0, 1), 's')
