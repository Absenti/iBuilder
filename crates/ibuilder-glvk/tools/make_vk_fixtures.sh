#!/usr/bin/env bash
# tools/make_vk_fixtures.sh: tests/fixtures/vk/{in,in2} from tests/fixtures/vk/src (glslangValidator -V, spirv-as, src/material.py), then tests/fixtures/vk/expected from
# the engine's own vk_shaders.py through tools/oracle.py. Needs NXNATIVE (a checkout of the engine), glslangValidator and spirv-as on PATH; SPIRV_CROSS defaults to the
# engine's pinned dm/gfx/tools/spirv_cross_bin/spirv-cross.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
F="$HERE/tests/fixtures/vk"
: "${NXNATIVE:?set NXNATIVE to a checkout of the engine}"
export SPIRV_CROSS="${SPIRV_CROSS:-$NXNATIVE/dm/gfx/tools/spirv_cross_bin/spirv-cross}"
rm -rf "$F/in" "$F/in2" "$F/expected"
mkdir -p "$F/in/sub" "$F/in2"
glslangValidator -V -o "$F/in/a.vert" "$F/src/a.vert" >/dev/null
glslangValidator -V -o "$F/in/b.frag" "$F/src/b.frag" >/dev/null
glslangValidator -V -o "$F/in/sub/c" "$F/src/c.comp" >/dev/null
glslangValidator -V -o "$F/in/sub/d.frag" "$F/src/d.frag" >/dev/null
glslangValidator -V -o "$F/in/sub/k.frag" "$F/src/k.frag" >/dev/null
glslangValidator -V -o "$F/in/h.geom" "$F/src/h.geom" >/dev/null
spirv-as -o "$F/in/e.geom" "$F/src/e.geom.spvasm"
spirv-as -o "$F/in/sub/f.vert" "$F/src/f.vert.spvasm"
glslangValidator -V -o "$F/g.spv" "$F/src/g.vert" >/dev/null
python3 -I "$F/src/material.py" "$F/g.spv" "$F/in/sub/g.material.bin"
rm "$F/g.spv"
printf 'not a shader\n' > "$F/in/readme.txt"
cp "$F/in/a.vert" "$F/in2/again.vert"   # the same module in a second root: listed once, under its first path
python3 -I -B "$HERE/tools/oracle.py" vk "$F/in" "$F/in2" "$F/expected"
rm -f "$F/expected/vk/vk_shaders.metallib"
