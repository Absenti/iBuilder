#!/usr/bin/env bash
# scripts/build-host.sh [device] [simulator] -- the engine HOST dylibs (device + simulator) from the patched engine plus engine-patches/04NN.
#
#   scripts/build-host.sh                 -> dist/host/host-prebuilt-loadable-ios.dylib, host-prebuilt-loadable-sim.dylib, HOST-MANIFEST.txt
#   HOST_WORK=<dir> HOST_OUT=<dir> scripts/build-host.sh device
#
# What it does (docs/ENGINE-UPDATE.md has the why):
#   1. APFS-clones the patched engine tree the app ships ($W/out-stage/out-engine/engine = nxnative a3663659 + 54 NX2Apple patches + iOSBuilder paches/engine/*) into $HOST_WORK/engine,
#      applies engine-patches/0[45]*.patch in name order (04NN: upstream and our own; 05NN: adapted from other MIT ports; a patch kept for a decision lives in engine-patches/optional/, never applied) with `patch -p1 --forward` (the first that does not apply stops the build and is named), then paches/0006 (hle_svc.c:
#      DM_AS_AT; tools/build_host_ios.sh applies it to the tree itself with git for the length of its compile, which a copy does not need).
#   2. checks the shader chain id of that tree (dm/gfx/nvn/dmsh_chain.h, and dmsh_chain.py --header regenerated) equals EXPECT_CHAIN: if it moved, every game's shader library would have to be
#      rebuilt, so the build STOPS (CHAIN_ANY=1 only prints it).
#   3. makes a mirror "repo" under $HOST_WORK/repo from COPIES of the worktree's tools/, load/ and paches/ (without 0006, see above) and of out-stage/out-video's decoder archives, and runs the
#      worktree's own load/prebuild_loadable.sh (-> tools/build_host_ios.sh, tools/fix_tlv.py, the engine's DmGameMarker.m / ShellGlue.m, load/front_*.m) from that copy, with
#      NXPOCKET_ENGINE and IOSBUILDER_OUT pointing into $HOST_WORK. The mirror's tools/build_video_ios.sh is a stub that fails: the decoder archives are inputs, never rebuilt here.
#   4. verifies each dylib (platform and minos, every symbol the old prebuilt exported still exported, every symbol the app's shim and boot.rs resolve, the chain id and the patch markers as
#      strings) and only then writes dist/host/: the two dylibs and HOST-MANIFEST.txt (what the tree is made of, sha256 of every patch, toolchain, chain id, sha256 of the dylibs).
#
# Reads (never written): the iOSBuilder tree with the newest work ($IOSBUILDER_WORKTREE, default $IOSBUILDER; its out-stage engine, tools/, load/, paches/, out-load-ui/ prebuilt for the export
# comparison, out-stage/out-video/) and engine-patches/. Writes only $HOST_WORK (default <repo>/target/host-work) and $HOST_OUT (default <repo>/dist/host). No git anywhere; nothing of the app is built.
# No machine path is built in: IOSBUILDER_WORKTREE (or IOSBUILDER) comes from the environment or from the optional gitignored scripts/local.env, which this script sources first
# (scripts/local.env.example documents it); when neither is set the script stops and names the variable.
# The app picks the new host with GAMEAPP_HOST=<dist/host/host-prebuilt-loadable-{ios,sim}.dylib> scripts/build-app.sh device|simulator (app/inputs.tsv will then say "DIFFERS" for the pin).
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/local.env" ] && . "$ROOT/scripts/local.env"
W="${IOSBUILDER_WORKTREE:-${IOSBUILDER:-}}"
[ -n "$W" ] || { echo "FAIL: IOSBUILDER_WORKTREE is not set (nor IOSBUILDER): point it at the iOSBuilder tree that holds out-stage/, tools/, load/ and paches/ (environment or scripts/local.env; see scripts/local.env.example)" >&2; exit 2; }
BASE_ENGINE="${NXPOCKET_ENGINE_BASE:-$W/out-stage/out-engine/engine}"
VIDEO_SRC="${HOST_VIDEO_DIR:-$W/out-stage/out-video}"
HOST_WORK="${HOST_WORK:-$ROOT/target/host-work}"
HOST_OUT="${HOST_OUT:-$ROOT/dist/host}"
EXPECT_CHAIN="${EXPECT_CHAIN:-b24b25240cad}"
PATCH_DIR="${ENGINE_PATCH_DIR:-$ROOT/engine-patches}"
KINDS=("$@"); [ ${#KINDS[@]} -gt 0 ] || KINDS=(device simulator)
export PYTHONDONTWRITEBYTECODE=1

say() { printf '\033[1m%s\033[0m\n' "$*" >&2; }
die() { printf '\033[31mFAIL: %s\033[0m\n' "$*" >&2; exit 1; }
sha()  { shasum -a 256 "$1" | cut -d' ' -f1; }
tree_fingerprint() { ( cd "$1" && find . -type f ! -name .DS_Store ! -name '*.pyc' -print0 | xargs -0 stat -f '%m %z %N' | LC_ALL=C sort | shasum -a 256 | cut -d' ' -f1 ); }

# the work folder is ours: refuse anything that is not under it or under $HOST_OUT when cleaning
case "$HOST_WORK" in ""|/|/Users|/Users/*/Documents|"$W"|"$W"/*) die "HOST_WORK=$HOST_WORK is not an acceptable work folder" ;; esac
# HOST_WORK is wiped below (engine/, repo/, logs/): it must not be, or contain, the home directory, this repository, the worktree or the output folder (review r5 L8). A relative path is made absolute first.
case "$HOST_WORK" in /*) ;; *) HOST_WORK="$PWD/$HOST_WORK" ;; esac
HOST_WORK="${HOST_WORK%/}"
for _keep in "$HOME" "$ROOT" "$W" "$HOST_OUT" "$PWD"; do
  [ -n "$_keep" ] || continue
  case "${_keep%/}/" in "$HOST_WORK"/*) die "HOST_WORK=$HOST_WORK is, or contains, $_keep: it would be wiped" ;; esac
done
[ -d "$BASE_ENGINE/dm/host" ] || die "no patched engine at $BASE_ENGINE"
[ -f "$W/load/prebuild_loadable.sh" ] || die "no worktree at $W"
ls "$PATCH_DIR"/0[45]*.patch >/dev/null 2>&1 || die "no engine-patches/04*.patch in $PATCH_DIR"
xcrun --sdk iphoneos --show-sdk-path >/dev/null 2>&1 || die "no iphoneos SDK (xcrun)"
T0=$SECONDS

# ---------------------------------------------------------------- 1. the engine tree: a clone of the shipped patched tree + engine-patches
say "1. the engine tree: clone of $BASE_ENGINE, engine-patches/04*, paches/0006"
BASE_BEFORE="$(tree_fingerprint "$BASE_ENGINE")"
rm -rf "${HOST_WORK:?}/engine" "${HOST_WORK:?}/repo" "${HOST_WORK:?}/logs"
rm -f "$HOST_WORK"/exports-*.txt "$HOST_WORK"/imports-*.txt "$HOST_WORK"/strings-*.txt "$HOST_WORK"/build-summary.tsv "$HOST_WORK"/applied-patches.tsv
mkdir -p "$HOST_WORK/logs"
ENGINE="$HOST_WORK/engine"
cp -cR "$BASE_ENGINE" "$ENGINE" 2>/dev/null || { rm -rf "${ENGINE:?}"; cp -R "$BASE_ENGINE" "$ENGINE" || die "cannot copy the engine tree"; }
rm -rf "$ENGINE/.git"; find "$ENGINE" -name __pycache__ -type d -prune -exec rm -rf {} + 2>/dev/null
APPLIED="$HOST_WORK/applied-patches.tsv"; : > "$APPLIED"
for p in "$PATCH_DIR"/0[45]*.patch; do
  n="$(basename "$p")"
  if ( cd "$ENGINE" && patch -p1 --forward --no-backup-if-mismatch < "$p" ) > "$HOST_WORK/logs/apply-$n.log" 2>&1; then
    printf '%s\t%s\t%s files\n' "$n" "$(sha "$p")" "$(grep -c '^patching file' "$HOST_WORK/logs/apply-$n.log")" >> "$APPLIED"
    say "   applied $n ($(grep -c '^patching file' "$HOST_WORK/logs/apply-$n.log") files; fuzz/offset lines: $(grep -ciE 'fuzz|offset' "$HOST_WORK/logs/apply-$n.log"))"
  else
    cat "$HOST_WORK/logs/apply-$n.log" >&2
    die "$n does not apply to the patched engine ($BASE_ENGINE + the patches before it): stopped, nothing guessed"
  fi
done
[ -z "$(find "$ENGINE" \( -name '*.rej' -o -name '*.orig' \) | head -1)" ] || die "rejected hunks left behind"
P06="$W/paches/0006-guest-as-at.patch"
( cd "$ENGINE" && patch -p1 --forward --no-backup-if-mismatch < "$P06" ) > "$HOST_WORK/logs/apply-0006.log" 2>&1 || { cat "$HOST_WORK/logs/apply-0006.log" >&2; die "paches/0006 does not apply"; }
grep -q DM_AS_AT "$ENGINE/dm/host/hle_svc.c" || die "paches/0006 applied but DM_AS_AT is not in hle_svc.c"
say "   applied paches/0006-guest-as-at.patch (hle_svc.c: DM_AS_AT)"

# ---------------------------------------------------------------- 2. the shader chain id
say "2. shader chain id"
HDR_CHAIN="$(grep -o 'DMSH_CHAIN "[0-9a-f]*"' "$ENGINE/dm/gfx/nvn/dmsh_chain.h" | cut -d'"' -f2)"
GEN_CHAIN="$( cd "$ENGINE" && python3 -I dm/gfx/tools/dmsh_chain.py --header )"
say "   dmsh_chain.h says $HDR_CHAIN, dmsh_chain.py --header regenerates $GEN_CHAIN, expected $EXPECT_CHAIN"
if [ "$HDR_CHAIN" != "$EXPECT_CHAIN" ] || [ "$GEN_CHAIN" != "$EXPECT_CHAIN" ]; then
  [ "${CHAIN_ANY:-0}" = 1 ] || die "the shader chain id moved ($EXPECT_CHAIN -> header $HDR_CHAIN / regenerated $GEN_CHAIN): every game's shader library would have to be rebuilt. Stopped (CHAIN_ANY=1 to build anyway)"
fi
CHAIN="$HDR_CHAIN"

# ---------------------------------------------------------------- 3. the mirror repo (copies of the worktree's scripts), then the worktree's own recipe
say "3. mirror repo of the worktree's tools/ load/ paches/ (copies) and the decoder archives"
MIR="$HOST_WORK/repo"
mkdir -p "$MIR/paches" "$MIR/out-video"
cp -R "$W/tools" "$MIR/tools"; cp -R "$W/load" "$MIR/load"
for p in "$W"/paches/*.patch; do [ "$(basename "$p")" = 0006-guest-as-at.patch ] || cp "$p" "$MIR/paches/"; done
for k in ios sim; do for l in libvpx libdav1d; do
  [ -f "$VIDEO_SRC/$l-$k.a" ] || die "no $VIDEO_SRC/$l-$k.a (the movie decoders are an input, they are not rebuilt here)"
  cp "$VIDEO_SRC/$l-$k.a" "$MIR/out-video/"
done; done
printf '#!/bin/sh\necho "build-host.sh: tools/build_video_ios.sh must not run here (the decoder archives are inputs)" >&2; exit 1\n' > "$MIR/tools/build_video_ios.sh"

OLDHOST_ios="$W/out-load-ui/host-prebuilt-loadable-ios.dylib"; OLDHOST_sim="$W/out-load-ui/host-prebuilt-loadable-sim.dylib"
# the symbols the app resolves from the loaded host (app/gameapp_shim_rust.c host_sym, crates/ibuilder-pipeline/src/sys/boot.rs host_symbol, load/ ShellGlue entries)
REQ_COMMON="dm_host_probe_boot dm_front_set_layer dm_front_pad_create dm_front_pad_layout dm_front_pad_layout_tv dm_front_tv_attached dm_game_menu dm_gamepad_view dm_glue_start_game dm_glue_present_game dm_glue_focus_event dm_glue_version dmgame_image_marker dm_gfx_suspend dm_audio_suspend"
REQ_ios="$REQ_COMMON nvn_live_set nvn_live_ready"
REQ_sim="$REQ_COMMON"
mkdir -p "$HOST_OUT"
SUMMARY="$HOST_WORK/build-summary.tsv"; : > "$SUMMARY"
for KIND in "${KINDS[@]}"; do
  case "$KIND" in device) PLAT=ios; WANT_PLATFORM=2; WANT_MINOS=26.0 ;; simulator) PLAT=sim; WANT_PLATFORM=7; WANT_MINOS=27.2 ;; *) die "usage: $0 [device] [simulator]" ;; esac
  say "4. $KIND: load/prebuild_loadable.sh $KIND  (engine $ENGINE, out $HOST_WORK/out-$PLAT)"
  K0=$SECONDS
  rm -rf "${HOST_WORK:?}/out-$PLAT"   # prebuild_loadable.sh compiles the host objects only when its output folder has none: a stale folder would be reused as is
  NXPOCKET_ENGINE="$ENGINE" IOSBUILDER_OUT="$HOST_WORK/out-$PLAT" bash "$MIR/load/prebuild_loadable.sh" "$KIND" > "$HOST_WORK/logs/prebuild-$PLAT.log" 2>&1
  rc=$?
  DYL="$HOST_WORK/out-$PLAT/host-prebuilt-loadable-$PLAT.dylib"
  [ $rc -eq 0 ] && [ -f "$DYL" ] || { tail -30 "$HOST_WORK/logs/prebuild-$PLAT.log" >&2; die "prebuild_loadable.sh $KIND failed (rc=$rc), log $HOST_WORK/logs/prebuild-$PLAT.log"; }
  ELAPSED=$((SECONDS - K0))
  say "   built $DYL in ${ELAPSED}s ($(stat -f %z "$DYL") bytes); $(grep -m1 'host objects:' "$HOST_WORK/logs/prebuild-$PLAT.log" | sed 's/^-- //'), compiled: $(grep -m1 'compiled OK' "$HOST_WORK/out-$PLAT/host-build-$PLAT.log" 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g')"

  # ------------------------------------------------------------ 5. verification
  GOT_PLATFORM="$(otool -l "$DYL" | grep -A4 LC_BUILD_VERSION | awk '/platform/ {print $2; exit}')"
  GOT_MINOS="$(otool -l "$DYL" | grep -A4 LC_BUILD_VERSION | awk '/minos/ {print $2; exit}')"
  [ "$GOT_PLATFORM" = "$WANT_PLATFORM" ] && [ "$GOT_MINOS" = "$WANT_MINOS" ] || die "$KIND: platform/minos is $GOT_PLATFORM/$GOT_MINOS, want $WANT_PLATFORM/$WANT_MINOS"
  nm -gU "$DYL" | awk '{print $2" "$3}' | sort > "$HOST_WORK/exports-$PLAT.new.txt"
  OLD="$(eval echo "\$OLDHOST_$PLAT")"
  if [ -f "$OLD" ]; then
    nm -gU "$OLD" | awk '{print $2" "$3}' | sort > "$HOST_WORK/exports-$PLAT.old.txt"
    comm -23 "$HOST_WORK/exports-$PLAT.old.txt" "$HOST_WORK/exports-$PLAT.new.txt" > "$HOST_WORK/exports-$PLAT.missing.txt"
    comm -13 "$HOST_WORK/exports-$PLAT.old.txt" "$HOST_WORK/exports-$PLAT.new.txt" > "$HOST_WORK/exports-$PLAT.added.txt"
    [ ! -s "$HOST_WORK/exports-$PLAT.missing.txt" ] || { head -20 "$HOST_WORK/exports-$PLAT.missing.txt" >&2; die "$KIND: exports the old prebuilt had and this one lacks (above)"; }
    say "   exports: old $(wc -l < "$HOST_WORK/exports-$PLAT.old.txt" | tr -d ' ') -> new $(wc -l < "$HOST_WORK/exports-$PLAT.new.txt" | tr -d ' ') (none lost, $(wc -l < "$HOST_WORK/exports-$PLAT.added.txt" | tr -d ' ') added)"
  fi
  if [ -f "$OLD" ]; then   # what the dylib asks of the system: a new import that is not in the system's libraries would stop the dlopen
    nm -u "$OLD" | sort -u > "$HOST_WORK/imports-$PLAT.old.txt"; nm -u "$DYL" | sort -u > "$HOST_WORK/imports-$PLAT.new.txt"
    comm -13 "$HOST_WORK/imports-$PLAT.old.txt" "$HOST_WORK/imports-$PLAT.new.txt" > "$HOST_WORK/imports-$PLAT.added.txt"
    comm -23 "$HOST_WORK/imports-$PLAT.old.txt" "$HOST_WORK/imports-$PLAT.new.txt" > "$HOST_WORK/imports-$PLAT.removed.txt"
  fi
  REQ="$(eval echo "\$REQ_$PLAT")"
  for s in $REQ; do grep -qE " _$s\$" "$HOST_WORK/exports-$PLAT.new.txt" || die "$KIND: the app resolves $s and the host does not export it"; done
  say "   every symbol the app resolves is exported ($(echo $REQ | wc -w | tr -d ' ') checked)"
  STR="$HOST_WORK/strings-$PLAT.txt"; strings -a "$DYL" > "$STR"
  CHAIN_HITS="$(grep -c "^$CHAIN\$" "$STR")"      # dmsh_chain.h's DMSH_CHAIN, a plain string next to the library directory format "b-%s"
  [ "$CHAIN_HITS" -ge 1 ] || die "$KIND: the chain id $CHAIN is not in the dylib's strings"
  # markers: paches/0006 (DM_AS_AT), 0401 (DM_AUDREN_TRACE: both hosts), and on the device host (the simulator has no nvn_mtl.m) 0202/0203 (still there) and 0404/0405
  for m in DM_AS_AT DM_AUDREN_TRACE; do grep -q "$m" "$STR" || die "$KIND: marker $m missing from the dylib"; done
  if [ "$PLAT" = ios ]; then for m in DM_PSO_NOWAIT DM_LOW_MEMORY DM_RT_SHARE DM_NVN_UPLOAD_IDLE_MS DM_NVN_UPLOAD_SRC_MB DM_PSO_PREPARE_PERCENT DM_PSO_PREPARE_TAIL DM_PSO_USER_FIRST DM_PSO_TAIL_RESUME DM_PSO_TAIL_HOT DM_NVN_TEX_MIP_SKIP DM_STALL_DUMP DM_NVN_VBUF_CLAMP DM_NVN_PRESENT_KEEP DM_NVN_RENDER_COND DM_PSO_ARCHIVE_NOFRAG; do grep -q "$m" "$STR" || die "$KIND: marker $m missing from the dylib"; done; fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%ss\n' "$KIND" "$(basename "$DYL")" "$(stat -f %z "$DYL")" "$(sha "$DYL")" "platform $GOT_PLATFORM minos $GOT_MINOS" "chain id $CHAIN x$CHAIN_HITS in strings" "$ELAPSED" >> "$SUMMARY"
  cp "$DYL" "$HOST_OUT/host-prebuilt-loadable-$PLAT.dylib.new" && mv "$HOST_OUT/host-prebuilt-loadable-$PLAT.dylib.new" "$HOST_OUT/host-prebuilt-loadable-$PLAT.dylib"
done
# ---------------------------------------------------------------- 5b. optional: the engine's own self-tests that need no game, on this Mac (HOST_TESTS=1; a few minutes)
TESTS_TXT="$HOST_OUT/HOST-TESTS.txt"; rm -f "$TESTS_TXT"
if [ "${HOST_TESTS:-0}" = 1 ]; then
  say "5b. engine self-tests on this Mac (no game files), logs in $HOST_WORK/logs/test-*.log"
  TG=010015100B514000; NJ="$(sysctl -n hw.ncpu 2>/dev/null || echo 4)"
  { echo "engine self-tests run on the built tree ($ENGINE), $(date '+%Y-%m-%d %H:%M:%S'), profile $TG, this Mac: $(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
    echo "(commands as .github/workflows/tests.yml of the engine runs them; none needs a game or a key)"; } > "$TESTS_TXT"
  run_t() { local name=$1 t0=$SECONDS; shift
    if ( cd "$ENGINE" && "$@" ) > "$HOST_WORK/logs/test-$name.log" 2>&1; then echo "PASS  $name  ($((SECONDS - t0))s)  $*"; else echo "FAIL  $name  ($((SECONDS - t0))s)  $*"; fi; }
  { run_t host-unit make -C dm/host/tests GAME=$TG -j"$NJ" run
    run_t cpu-texture make -C dm/gfx/tex/tests run
    for t in rt_recycle_policy gfx_suspend lazy_clear_order zero_colour upload_idle upload_src pso_pending cbs_fault bc_cpu warm_head pso_head pso_user_first pso_binary_archive pso_tail_resume pso_warm pso_admission present_rescue res_commit dispatch_pipeline sparse_pool_cache pso_archive present_crop dispatch_zero env_flag mip_skip_limits pool_census stall_watch submit_compile session_reset vbuf_clamp present_keep drop_reasons render_cond_switch pso_archive_nofrag vbuf pull; do run_t "gfx-$t" make -C dm/gfx/tests GAME=$TG check_t_$t; done; } | tee -a "$TESTS_TXT" >&2
  { echo "not run: dm/gfx/tests t_geometry_mesh (0408's test): t_geometry.m reads g_mdi_v, which nvn_mtl.m of the base tree (nxnative a3663659 + 54 NX2Apple patches, before any patch here) does not define: it does not compile there either;"
    echo "         with g_mdi_v renamed to g_mdi_waits in a scratch copy it passes (47 checks). dm/tools/tests/test_x18_select.py (0409): needs the Python module capstone, not installed."; } >> "$TESTS_TXT"
  ! grep -q '^FAIL' "$TESTS_TXT" || die "an engine self-test failed: $TESTS_TXT"
fi
BASE_AFTER="$(tree_fingerprint "$BASE_ENGINE")"
[ "$BASE_BEFORE" = "$BASE_AFTER" ] || die "the read-only base engine tree changed during the build"

# ---------------------------------------------------------------- 6. the old hosts are kept beside the new ones (copied once, never overwritten), then the manifest
mkdir -p "$HOST_OUT/previous"
for k in ios sim; do o="$(eval echo "\$OLDHOST_$k")"
  [ -f "$o" ] && [ ! -f "$HOST_OUT/previous/host-prebuilt-loadable-$k.dylib" ] && cp -p "$o" "$HOST_OUT/previous/host-prebuilt-loadable-$k.dylib"
done
# ---------------------------------------------------------------- 6b. the manifest
say "6. $HOST_OUT/HOST-MANIFEST.txt"
M="$HOST_OUT/HOST-MANIFEST.txt"
STAMP="$(dirname "$BASE_ENGINE")/stamp.txt"
{
  echo "HOST-MANIFEST: the engine host dylibs, built $(date '+%Y-%m-%d %H:%M:%S %z') by scripts/build-host.sh"
  echo
  echo "== source tree"
  echo "nxnative (seedds/nxnative) base commit: $(cat "$W/out-nx2apple/.upstream-ref" 2>/dev/null | tr -d '\n' || echo unknown)   (2026-11-18 18:25, the 54 NX2Apple patches rebased by hand on it)"
  echo "engine source HEAD (NX2Apple wrapper rebase): $(head -1 "$STAMP" 2>/dev/null || echo unknown)"
  echo "shipped patched tree: $BASE_ENGINE  (key $STAMP)"
  echo "base tree metadata fingerprint (find|stat|sha256, unchanged by this build): $BASE_AFTER"
  echo "built tree: a clone of it + engine-patches/0[45]* + paches/0006; shader chain id $CHAIN (dmsh_chain.h = dmsh_chain.py --header = expected $EXPECT_CHAIN)"
  echo
  echo "== patches in the shipped tree (iOSBuilder paches/engine, name \\t sha256 prefix; from its stamp)"
  tail -n +3 "$STAMP" 2>/dev/null | sed 's/^/  /'
  echo
  echo "== patches applied by this build, in order (name, sha256, files touched)"
  while IFS="$(printf '\t')" read -r n h f; do echo "  engine-patches/$n  $h  $f"; done < "$APPLIED"
  echo "  paches/0006-guest-as-at.patch  $(sha "$P06")  (dm/host/hle_svc.c, applied to the engine copy)"
  echo "== host-level patches the worktree's load/prebuild_loadable.sh applies to copies of single files (sha256)"
  for p in 0007-msl-source-at-runtime 0008-live-shaders 0009-pso-archive 0012-touch-layout-hook 0013-controllers-players-remap; do echo "  paches/$p.patch  $(sha "$W/paches/$p.patch")"; done
  echo
  echo "== recipe files (sha256) -- copies of these are run from $MIR"
  for f in load/prebuild_loadable.sh tools/build_host_ios.sh tools/host_probe_entry.c tools/front_stub.c tools/fix_tlv.py load/glue_front_standin.m load/front_pad.m load/front_pads.m load/front_edit.m load/front_capture.m load/front_tv.m; do echo "  $f  $(sha "$W/$f")"; done
  for k in ios sim; do for l in libvpx libdav1d; do echo "  out-video/$l-$k.a (from $VIDEO_SRC)  $(sha "$VIDEO_SRC/$l-$k.a")"; done; done
  echo
  echo "== toolchain"
  echo "  $(xcodebuild -version 2>/dev/null | tr '\n' ' ')"
  echo "  iphoneos SDK $(xcrun --sdk iphoneos --show-sdk-version) ; iphonesimulator SDK $(xcrun --sdk iphonesimulator --show-sdk-version)"
  echo "  $(clang --version | head -1)"
  echo "  $(ld -v 2>&1 | head -1)"
  echo "  python3 $(python3 --version 2>&1 | cut -d' ' -f2) ; $(sw_vers -productName) $(sw_vers -productVersion) ($(uname -m))"
  echo "  targets: device arm64-apple-ios26.0, simulator arm64-apple-ios27.2-simulator (prebuild_loadable.sh's own)"
  echo
  echo "== dylibs (kind, file, bytes, sha256, platform/minos, chain id string count, build seconds)"
  sed 's/^/  /' "$SUMMARY"
  # a run for one kind must not make the manifest forget the other kind's dylib that is still in $HOST_OUT (review r5 L8): it is listed from the file itself, marked as not built by this run
  for k in device simulator; do
    case " ${KINDS[*]} " in *" $k "*) continue ;; esac
    case "$k" in device) kf=ios ;; *) kf=sim ;; esac
    d="$HOST_OUT/host-prebuilt-loadable-$kf.dylib"
    [ -f "$d" ] && printf '  %s\t%s\t%s\t%s\t%s\n' "$k" "$(basename "$d")" "$(stat -f %z "$d")" "$(sha "$d")" "present in $HOST_OUT, NOT built by this run (platform, chain id and timing not re-measured)"
  done
  echo
  echo "== exports and imports against the old prebuilt (nm -gU / nm -u)"
  for k in ios sim; do
    [ -f "$HOST_WORK/exports-$k.new.txt" ] || continue
    echo "  $k: exports $(wc -l < "$HOST_WORK/exports-$k.old.txt" | tr -d ' ') -> $(wc -l < "$HOST_WORK/exports-$k.new.txt" | tr -d ' '); lost: $(wc -l < "$HOST_WORK/exports-$k.missing.txt" | tr -d ' '); added: $(awk '{printf "%s ", $2}' "$HOST_WORK/exports-$k.added.txt")"
    echo "  $k: imports $(wc -l < "$HOST_WORK/imports-$k.old.txt" | tr -d ' ') -> $(wc -l < "$HOST_WORK/imports-$k.new.txt" | tr -d ' '); new: $(awk '{printf "%s ", $NF}' "$HOST_WORK/imports-$k.added.txt")| gone: $(awk '{printf "%s ", $NF}' "$HOST_WORK/imports-$k.removed.txt")"
  done
  echo "  every symbol the app resolves from the loaded host is exported: device: $REQ_ios"
  echo "                                                                   simulator: $REQ_sim (the simulator host has no nvn_live_set / nvn_live_ready, as before: no nvn_mtl.m there)"
  if [ -f "$TESTS_TXT" ]; then echo; echo "== engine self-tests on this Mac (HOST-TESTS.txt)"; sed 's/^/  /' "$TESTS_TXT"; fi
  echo
  echo "== the old prebuilt hosts these replace (the worktree's, left in place; copies in $HOST_OUT/previous/)"
  for k in ios sim; do o="$(eval echo "\$OLDHOST_$k")"; [ -f "$o" ] && echo "  $o  $(stat -f %z "$o") bytes  $(sha "$o")"
    [ -f "$HOST_OUT/previous/host-prebuilt-loadable-$k.dylib" ] && echo "  previous/host-prebuilt-loadable-$k.dylib  $(stat -f %z "$HOST_OUT/previous/host-prebuilt-loadable-$k.dylib") bytes  $(sha "$HOST_OUT/previous/host-prebuilt-loadable-$k.dylib")"; done
  echo
  echo "Use:  GAMEAPP_HOST=$HOST_OUT/host-prebuilt-loadable-ios.dylib scripts/build-app.sh device   (simulator: ...-sim.dylib).  Roll back: leave GAMEAPP_HOST unset (the worktree's old prebuilt), or point it at $HOST_OUT/previous/host-prebuilt-loadable-{ios,sim}.dylib."
} > "$M"
say "done in $((SECONDS - T0))s: $(ls "$HOST_OUT" | tr '\n' ' ')"
