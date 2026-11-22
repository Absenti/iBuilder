#!/bin/bash
# app/sim/verify-chain.sh <app Documents folder> <reference run folder> -- did the chain that ran INSIDE the app write the same outputs as the Mac CLI run (scripts/e2e-grizzy.sh)?
#
# <reference run folder> is a `ibuilder --out` folder of the CLI on the same Grizzy inputs whose outputs were compared with the Python oracle by the earlier packages
# (docs/chain/E2E-RUST.md: 21 of 21 rewrite files identical to out-py/pyrewrite/out, the library identical to the Python's but for three stamped files). The oracle folders themselves
# (out-py, out/ref-img-grizzy) were deleted from the iOSBuilder checkout on 2026-11-19 ~12:09, so the comparison here is in-app output against that earlier verified CLI output.
#   (2) the 21 rewrite files, cmp one by one;  (3) the shader library image, libdiff (file by file inside the DMROMFS1 image);  (4) the signed framework: codesign on the Mach-O alone,
#   its platform and identifier;  (5) the markers and the stage lines of the run.
#   LIBDIFF  the libdiff binary (ibuilder-shaderbuild, features host-tools,f2-finish)  [required for (3)]
set -uo pipefail
D="${1:?usage: verify-chain.sh <app Documents> <reference run folder>}"; R="${2:?usage: verify-chain.sh <app Documents> <reference run folder>}"
TID=0100DFA027504000; CHAIN="${CHAIN:-b24b25240cad}"; bad=0
n=0; d=0
for m in main sdk subsdk0; do for e in patched.bin fixups.tsv sites.tsv gates.tsv cuts.tsv obs.tsv changed.tsv; do
  if cmp "$D/extract/rewrite/$m/$m.$e" "$R/extract/rewrite/$m/$m.$e"; then n=$((n+1)); else d=$((d+1)); fi
done; done
echo "(2) rewrite files identical to the reference run: $n of 21, different: $d"; [ "$d" = 0 ] && [ "$n" = 21 ] || bad=1
lib="$D/shaders/$TID/b-$CHAIN.img"; ref="$R/shaders/$TID/b-$CHAIN.img"
if [ -f "$lib" ] && [ -f "$ref" ] && [ -x "${LIBDIFF:-/nonexistent}" ]; then
  echo "(3) libdiff, the library built in the app vs the reference run's:"
  "$LIBDIFF" "$lib" "$ref" --ignore-first-line c1_full.tsv --ignore-first-line cb_extent.tsv; echo "    exit $?"
else echo "(3) skipped or missing: lib=$([ -f "$lib" ] && echo yes || echo NO) ref=$([ -f "$ref" ] && echo yes || echo NO) LIBDIFF=${LIBDIFF:-unset}"; bad=1; fi
for t in shader_facts.tsv shader_map.tsv shader_pairs.tsv pso_pairs.tsv bf_want.tsv pso_manifest.tsv; do
  if cmp -s "$D/shaders/$TID/$t" "$R/shaders/$TID/$t"; then echo "    table $t identical"; else echo "    table $t DIFFERS or absent"; fi
done
fw="$D/DmGameProbe.framework/DmGameProbe"
if [ -f "$fw" ]; then
  # the framework BUNDLE, as the app's zsign signed it (Info.plist sealed, _CodeSignature/CodeResources written): stronger than the Mach-O alone that ibuilder-sign's ad hoc signature needs
  echo "(4) the framework: $(stat -f %z "$fw") bytes; $(otool -l "$fw" | grep -A4 LC_BUILD_VERSION | awk '/ platform /{print "platform " $2}')"
  codesign --verify --strict --verbose=2 "$D/DmGameProbe.framework" 2>&1 | sed "s|$D/||" | sed 's/^/    /'; [ "${PIPESTATUS[0]}" = 0 ] || bad=1
  codesign -dvv "$D/DmGameProbe.framework" 2>&1 | grep -E 'Identifier|Format|CodeDirectory|Signature|Info.plist|Sealed|TeamIdentifier' | sed 's/^/    /'
else echo "(4) no framework at $fw"; bad=1; fi
echo "(5) the run's own lines:"
for f in gameapp.stage gameapp.framework build_times.tsv; do [ -f "$D/$f" ] && { echo "  -- $f"; sed 's/^/    /' "$D/$f" | head -12; }; done
exit $bad
