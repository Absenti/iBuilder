#!/usr/bin/env python3
"""The engine's own vk_shaders.py / gl_shaders.py / idtech4_programs.py run as they are, except their last step: instead of compiling the translated MSL with
`xcrun metal` / `metallib`, `metallib()` copies every .metal file it is given to <out dir>/msl/ (the name the Python gave it). The table is the Python's own.
This is the oracle of crates/ibuilder-glvk (tests/fixtures/*/expected/ were made with it).

Usage:  NXNATIVE=<a checkout of the engine> python3 -I -B oracle.py vk <SPIR-V dir | NSO>... <out dir>
        NXNATIVE=<...> python3 -I -B oracle.py gl <input path>... <out dir> [--fixed-function]
        NXNATIVE=<...> python3 -I -B oracle.py idtech4 <main NSO> <RomFS> <job file>
The tools are the engine's: $SPIRV_CROSS (default dm/gfx/tools/spirv_cross_bin/spirv-cross), glslangValidator on PATH. -I keeps this folder's files out of the import path,
-B writes no __pycache__ into the engine checkout."""
import os
import shutil
import sys

root = os.environ.get('NXNATIVE')
if not root or not os.path.isdir(os.path.join(root, 'dm', 'gfx', 'tools')):
    sys.exit('NXNATIVE must name a checkout of the engine (dm/gfx/tools/vk_shaders.py)')
tools = os.path.join(root, 'dm', 'gfx', 'tools')
sys.path.insert(0, tools)
sys.path.insert(0, os.path.join(root, 'dm', 'tools'))
if len(sys.argv) < 2 or sys.argv[1] not in ('vk', 'gl', 'idtech4'): sys.exit(__doc__)
kind = sys.argv[1]
mod = __import__({'vk': 'vk_shaders', 'gl': 'gl_shaders', 'idtech4': 'idtech4_programs'}[kind])


def metallib(msls, out, sdk, extra):
    d = os.path.join(out, 'msl')
    os.makedirs(d, exist_ok=True)
    for m in msls: shutil.copyfile(m, os.path.join(d, os.path.basename(m)))
    with open(os.path.join(out, 'msl.order'), 'w') as f: f.write(''.join(os.path.basename(m) + '\n' for m in msls))
    return os.path.join(out, f'{kind}_shaders.metallib')


if kind != 'idtech4': mod.metallib = metallib
sys.argv = [mod.__file__] + sys.argv[2:]
if kind == 'idtech4':
    import runpy
    runpy.run_path(mod.__file__, run_name='__main__')
else:
    import nso
    try: mod.main()
    except nso.Error as e: mod.red(str(e))
