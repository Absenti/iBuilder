#!/bin/bash
# app/verify_ipa.sh -- read the .ipa back from the archive and measure it, entry by entry. Adapted from gameapp/verify_ipa.sh of the iOSBuilder worktree for the app whose chain is Rust.
#
# The artifact handed to a sideloader is the ZIP, not the bundle on this Mac, so everything below is measured from `unzip` output and a fresh extraction. Same discipline as the original
# (Payload/ only, no Symbols/, no identity: SideStore re-signs), the same checks of the Info.plist keys, the entitlements, the host and the managed half, and what changed with the chain:
#   * NO Python: no Python.framework, py/, pyext/, lib-dynload, .py, .pyc, .fwork, shaderengine; the executable does not link or mention the interpreter;
#   * the Rust library is linked (its C ABI entry points and its own log strings are in the executable), the shim exports every gameapp_* the C# imports, the bundle id is the expected one (dev.ibuilder.app),
#     the signature is ad hoc / unsigned (no identity, no provisioning profile) and carries the RAM entitlement.
# The checks of the original that read gameapp_pipeline.py / mods.py / shader_device.py / pack_nup.py in the bundle are replaced by checks of the Rust chain's own log strings in the executable.
#
# usage: app/verify_ipa.sh <file.ipa> [expected-bundle-id]      env: GAMEAPP_RAM_ENTITLEMENT=0 GAMEAPP_EVA_ENTITLEMENT=1 SHADER_CHAIN=<id> FRAMEWORK_ID=<16 hex>
set -uo pipefail
# shellcheck source=../scripts/lib-spawn.sh
. "$(cd "$(dirname "$0")" && pwd)/../scripts/lib-spawn.sh" || exit 1
IPA="${1:?usage: verify_ipa.sh <file.ipa> [bundle-id]}"
EXPECT_ID="${2:-dev.ibuilder.app}"
say() { printf '\033[1m%s\033[0m\n' "$*" >&2; }
# Every PASS / FAIL line is one check, counted (review r5 A5): the summary before the verdict says how many, so "N checks" has one meaning (the summary and the verdict lines are not checks).
pass() { printf '  \033[32mPASS\033[0m %s\n' "$*" >&2; NPASS=$((NPASS + 1)); }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$*" >&2; BAD=1; NFAIL=$((NFAIL + 1)); }
BAD=0; NPASS=0; NFAIL=0

[ -f "$IPA" ] || { echo "no such .ipa: $IPA" >&2; exit 1; }
say "the archive"
echo "  $IPA" >&2
echo "  $(stat -f %z "$IPA") bytes  sha256 $(shasum -a 256 "$IPA" | awk '{print $1}')" >&2

ENTRIES=$(unzip -Z1 "$IPA")
COUNT=$(printf '%s\n' "$ENTRIES" | wc -l | tr -d ' ')
APPS=$(printf '%s\n' "$ENTRIES" | grep -oE '^Payload/[^/]+\.app' | sort -u)
NAPP=$(printf '%s\n' "$APPS" | grep -c . || true)
say "the layout"
echo "  $COUNT entries; $NAPP .app under Payload/" >&2
[ "$NAPP" = 1 ] || fail "expected exactly one Payload/*.app, found $NAPP"
BADOUT=$(printf '%s\n' "$ENTRIES" | grep -vE '^Payload/[^/]+\.app/' || true)
[ -z "$BADOUT" ] && pass "Payload/<Name>.app only (no symbols, no __MACOSX, no top-level files)" || fail "entries outside Payload/<Name>.app/: $(printf '%s ' $BADOUT)"
if grep -Eq '__MACOSX|/\.DS_Store$|/\._' <<< "$ENTRIES"; then fail "macOS noise present"; else pass "no .DS_Store / __MACOSX / ._ files"; fi
if grep -qE '^Symbols/' <<< "$ENTRIES"; then fail "the archive carries Symbols/"; else pass "no Symbols/ directory"; fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
unzip -q "$IPA" -d "$WORK" || { echo "unzip failed" >&2; exit 1; }
APP="$WORK/Payload/$(basename "${APPS%%$'\n'*}" | sed 's|^Payload/||')"
[ -d "$APP" ] || { echo "no app extracted at $APP" >&2; exit 1; }

say "the bundle inside it"
ID=$(plutil -extract CFBundleIdentifier raw "$APP/Info.plist" 2>/dev/null)
EXE=$(plutil -extract CFBundleExecutable raw "$APP/Info.plist" 2>/dev/null)
echo "  $(basename "$APP")  id=$ID  executable=$EXE" >&2
[ "$ID" = "$EXPECT_ID" ] && pass "bundle id is $EXPECT_ID (a NEW app since 2026-11-22: it does not replace dev.nxpocket.gameapp)" || fail "bundle id is '$ID', expected '$EXPECT_ID'"
[ -n "$EXE" ] && [ -f "$APP/$EXE" ] && pass "CFBundleExecutable ($EXE) is in the bundle" || fail "CFBundleExecutable missing"
F=$(file -b "$APP/$EXE")
echo "  file: $F" >&2
case "$F" in *arm64*) pass "the executable is arm64" ;; *) fail "not arm64: $F" ;; esac
PLATFORM=$(otool -l "$APP/$EXE" | grep -A4 LC_BUILD_VERSION | awk '/ platform /{print $2}')
MINOS=$(otool -l "$APP/$EXE" | grep -A4 LC_BUILD_VERSION | awk '/ minos /{print $2}')
echo "  LC_BUILD_VERSION platform $PLATFORM  minos $MINOS" >&2
[ "$PLATFORM" = 2 ] && pass "platform 2 (device, not the simulator's 7)" || fail "platform $PLATFORM is not the device's 2"

rm -f "$WORK/strings.txt"; strings -a "$APP/$EXE" > "$WORK/strings.txt"
nm -gU "$APP/$EXE" > "$WORK/nm.txt" 2>&1 || true
has_exe() { grep -qF -- "$1" "$WORK/strings.txt"; }
# The build machine's home directory must not be anywhere in the bundle (review r5 H1: the check used to read the executable only, while host-prebuilt.dylib carried object paths in its symbol table
# and Microsoft.iOS.dll its .pdb path): a shared IPA would say who built it. Every file is searched, whatever its kind; the names of the files are printed.
# (scripts/scrub-build-paths.py blanks them before the signature; this is the gate.) /Users/$USER/ and $HOME/ are the two spellings; a HOME that is empty or "/" is not searched (it would match everything).
HOMEHITS=""
for needle in "/Users/${USER:-__nouser__}/" "${HOME:-/}/"; do
  case "$needle" in "//"|"/") continue ;; esac
  HOMEHITS="$HOMEHITS
$(/usr/bin/grep -rFl -- "$needle" "$APP" 2>/dev/null || true)"
done
HOMEHITS="$(printf '%s\n' "$HOMEHITS" | grep . | sort -u | sed "s|^$APP/||")"
if [ -n "$HOMEHITS" ]; then
  fail "the bundle carries the build machine's home directory in $(printf '%s\n' "$HOMEHITS" | grep -c .) file(s): $(printf '%s' "$HOMEHITS" | tr '\n' ' ') (a shared IPA would say who built it; scripts/scrub-build-paths.py)"
else pass "no file of the bundle carries the build machine's home directory ($(find "$APP" -type f | wc -l | tr -d ' ') files searched)"; fi

say "no Python anywhere"
NPYFILES=$(printf '%s\n' "$ENTRIES" | grep -cE '\.py$|\.pyc$|\.fwork$|/py/|/pyext/|lib-dynload|/shaderengine/|Python\.framework|libpyext|/_sysconfigdata' || true)
[ "$NPYFILES" = 0 ] && pass "no .py / .pyc / .fwork, no py/ pyext/ lib-dynload/ shaderengine/, no Python.framework in the archive ($COUNT entries)" || fail "$NPYFILES archive entries belong to the interpreter"
if otool -L "$APP/$EXE" | grep -qi python; then fail "the executable links a Python library"; else pass "the executable links no Python library (otool -L)"; fi
NPYSYM=$(grep -cE ' [TU] _(Py_BytesMain|Py_Initialize|PyImport_AppendInittab|PyInit__capstone|PyInit_zbic|PyInit__dmgameapp)$' "$WORK/nm.txt" || true)
NPYSTR=$(grep -cE '^(PYTHONHOME|PYTHONPATH|PYTHONDONTWRITEBYTECODE|gameapp_pipeline\.py|_dmgameapp)$|registered \(inittab, no \.so\)|Py_BytesMain' "$WORK/strings.txt" || true)
[ "$NPYSYM" = 0 ] && [ "$NPYSTR" = 0 ] && pass "no interpreter symbol, no PYTHONHOME / inittab / gameapp_pipeline.py string in the executable" || fail "the executable still names the interpreter ($NPYSYM symbols, $NPYSTR strings)"
[ ! -d "$APP/Frameworks/Python.framework" ] && pass "no Frameworks/Python.framework (the Python build carried it, ~4.8 MB + ~19 MB of py/)" || fail "Frameworks/Python.framework is embedded"
[ -f "$APP/Frameworks/OpenSSL.framework/OpenSSL" ] && pass "Frameworks/OpenSSL.framework stays (zsign needs it)" || fail "Frameworks/OpenSSL.framework missing"
[ -f "$APP/host-prebuilt.dylib" ] && pass "the prebuilt host travels inside the bundle" || fail "host-prebuilt.dylib missing"

say "no process is spawned"
# The chain logs "process spawns: none" as a literal (crates/ibuilder-pipeline/src/chain.rs); this is the measurement (review r2, N5; scripts/check-no-spawn.sh is the same rule as a standalone command): the executable
# imports none of the calls that start a process. A symbol table that could not be read (or that lists next to nothing) is a failure: the check would pass on nothing.
# Apple's `nm -u` prints bare symbol names (`_fork`); another nm prints `U _fork`: both are reduced to the bare name, so the pattern below cannot miss one by its format (a first version of this
# check expected a leading space and matched nothing, which is exactly a check that cannot fail: the same run counts the names it read and refuses to pass on a handful)
# The list of spawn symbols and the allowed imports come from scripts/lib-spawn.sh, the same file scripts/check-no-spawn.sh reads (review r4 §3.2: two lists used to drift).
spawn_scan "$APP/$EXE" || true
NUNDEF=$SCAN_NR
NSPAWN=$(printf '%s\n' "$SCAN_HITS" | grep -c . || true)
if [ "${NUNDEF:-0}" -lt 50 ]; then fail "could not read the executable's imports (nm -u lists $NUNDEF undefined symbols): the no-spawn check measured nothing"
elif [ "$NSPAWN" = 0 ]; then pass "the executable imports none of fork / vfork / exec* / posix_spawn* / system / popen ($NUNDEF imports read, nm -u)"
else fail "the executable imports a process-spawning call ($NSPAWN of fork / vfork / exec* / posix_spawn* / system / popen): $(printf '%s\n' "$SCAN_HITS" | tr '\n' ' ')"; fi

# Every other Mach-O of the bundle (review r3 §4.3: the check above read the executable only, and host-prebuilt.dylib imports _popen). The imports allowed, by name and file only, are in
# scripts/lib-spawn.sh with their reasons: host-prebuilt.dylib: _popen (the engine's frame capture); Frameworks/libSystem.Native.framework/libSystem.Native: _fork, _execve (the .NET runtime's
# Process.Start, which the app's C# never names: grep of app/gameapp, every patch and xlate/, 2026-11-20). Any other spawn import, or one of these in another file, fails.
# The Mach-O files are found by their magic (list_macho), not by mode or name (review r4 F4), and each one must have been READ: a file nm reads nothing from is unmeasured, not clean (F3).
NOTHER=0; OTHERHITS=""
while IFS= read -r f; do
  [ -n "$f" ] || continue
  [ "$f" = "$APP/$EXE" ] && continue
  NOTHER=$((NOTHER + 1))
  rel=${f#"$APP/"}
  if ! spawn_scan "$f" || ! spawn_scan_guard "$f" 0; then OTHERHITS="$OTHERHITS $rel:unread"; continue; fi
  while IFS= read -r s; do
    [ -n "$s" ] || continue
    if reason=$(spawn_allow_reason "$rel" "$s"); then echo "  allowed: $rel imports $s ($reason)" >&2
    else OTHERHITS="$OTHERHITS $rel:$s"; fi
  done <<EOF2
$SCAN_HITS
EOF2
done < <(list_macho "$APP")
if [ "$NOTHER" -lt 2 ]; then fail "the bundle's other Mach-O files were not found ($NOTHER): the no-spawn check of the dylibs and frameworks measured nothing"
elif [ -z "$OTHERHITS" ]; then pass "no other Mach-O of the bundle ($NOTHER files: the host, the frameworks) imports a process-spawning call, but for the allowed imports named above"
else fail "a Mach-O of the bundle imports a process-spawning call (or could not be read, ':unread'):$OTHERHITS"; fi

say "the Rust chain is linked"
NIB=$(grep -c ' T _ibuilder_' "$WORK/nm.txt" || true)
echo "  ibuilder_* global functions in the executable's symbol table: $NIB (the publish strips the local symbol table and the link localizes what no import needs: the string literals below are the check)" >&2
for marker in "ibuilder_run_chain: a run is already in progress on this handle" "ibuilder_create: documents_dir is required" "[ibuilder] chain: start" "[ibuilder] PANIC: " \
              "[gameapp] chain: ibuilder_run_chain -> %d (%s)"; do
  has_exe "$marker" && pass "the Rust library's own text is in the executable: '$marker'" || fail "missing in the executable: '$marker'"
done
for pair in "the chain driver:ibuilder-pipeline" "the rewriter:ibuilder-rewrite" "the profile deriver:ibuilder-profile" "the assembler:ibuilder-macho" "the shader split:ibuilder-shadersplit" "the shader build:ibuilder-shaderbuild" "the shader finisher:ibuilder-shader"; do
  label=${pair%%:*}; marker=${pair#*:}
  if has_exe "crates/$marker/" || has_exe "$marker/src" || has_exe "$marker"; then pass "$label ($marker) is in the executable"; else fail "$label ($marker) is not in the executable: the stage would be Unwired"; fi
done
if has_exe "is not wired into this build"; then
  # the message exists in the source of every adapter that can be Unwired; it is present when the code is, so it is not a failure by itself
  echo "  note: the executable carries the 'not wired into this build' texts of the adapters (present whether or not a stage is wired)" >&2
fi
# the chain's own lines the C# and the next launch read, now produced by Rust
for marker in "Documents/gameapp.boot-only consumed" "rewrite taken from the cache" "ended by iOS" "build times: " "stage 3 shaders: " "framework stamp: " \
              "does not exist on a device" "DM_NVN_SHMAP" "DM_MSL_DIR" "DM_ROMFS_OVERLAY" "DM_LIVE_SHADERS" "DM_PSO_BINARY_ARCHIVE" "DM_METALFX" "game env: " \
              "cooling down: the iPhone is" "came from the earlier run" "launcher settings:"; do
  has_exe "$marker" && pass "the chain's own text is in the executable: '$marker'" || fail "missing in the executable: '$marker' (a line the app or the next launch reads)"
done

# the game's settings sheet rows: dm/games/settings.py in Rust (crates/ibuilder-ffi/src/launcher.rs), checked against the real settings.py by tests/launcher_oracle.rs
has_exe "needs names a variable no row sets" && has_exe " in the game profile header (game.py header)" && pass "the settings rows (settings.py in Rust, ibuilder-ffi launcher) are linked: launcher.json is written by the chain" || fail "no launcher (settings.py) code in the executable: the settings sheet would show only its four legacy fields"
# the Switch 2 package reader (crates/ibuilder-nup, another package of this round) is reported, never required: until it lands a .nup fails at stage 1 and the build says so
if has_exe "crates/ibuilder-nup/" || has_exe "ibuilder_nup"; then pass "the Switch 2 package reader (ibuilder-nup) is linked: a .nup can be read in stage 1"
else echo "  INFO the Switch 2 package reader (ibuilder-nup) is NOT linked in this build: a .nup game stops at stage 1 ('no NUP reader in this build'); NSP / NSZ / XCI games are unaffected" >&2; fi

say "the shim and the app's own native pieces"
for sym in gameapp_run_chain gameapp_memline gameapp_available gameapp_extended_va gameapp_increased_memory gameapp_get_task_allow gameapp_address_space gameapp_identity_status \
           gameapp_identity_store_bytes gameapp_identity_where gameapp_set_game_layer gameapp_quit_game gameapp_nsz gameapp_pad_create gameapp_pad_layout gameapp_pad_layout_tv \
           gameapp_tv_attached gameapp_game_menu gameapp_set_xlate; do
  grep -qE " T _$sym\$" "$WORK/nm.txt" && pass "$sym is a global T (the P/Invoke entry the managed half calls)" || fail "$sym is not in the executable"
done
for pair in "signer path:adhoc(zsign, no certificate)" "device signer path:device(zsign, in-memory .p12)" "zsign:Can't find TeamId" "Keychain identity:dev.nxpocket.certificate" \
            "NSZ conversion:NCZSECTN" "SPIRV-Cross wrapper:Usage: spirv-cross [SPIR-V file] [options]"; do
  label=${pair%%:*}; marker=${pair#*:}
  has_exe "$marker" && pass "$label is linked in (its own string literal is in the image)" || fail "$label is not in the executable"
done
has_exe "the translator is not registered" && pass "the translator callback (gameapp_set_xlate's trampoline) is in the executable" || fail "no translator trampoline"
has_exe "SHA-256 matches its name" && pass "an .nsz is rebuilt into its NSP on the phone, each NCA checked against its name" || fail "no NSZ verification text"
has_exe "touch gamepad %s" && has_exe "dm_front_pad_layout" && pass "the shim makes the gamepad from the host it dlopen'd and lays it out" || fail "no gamepad entry points"
has_exe "the game view's layer is" && pass "the shim receives the game view's layer from the managed side" || fail "no gameapp_set_game_layer text"
# stage 5 and the launch reservation are Rust now: their own lines
has_exe "as-reserve: took" && has_exe "as-reserve: skipped" && has_exe "as-reserve: deferred" && has_exe "DM_AS_AT" \
  && pass "the launch-time address-space reservation is in the executable (Rust initializer: took / skipped / deferred, DM_AS_AT)" || fail "no as-reserve lines: the launch-time reservation is gone"
has_exe "the boot returned first, no SIGTERM is sent" && has_exe "the boot ran to its time limit" && has_exe "the designed end" \
  && pass "the boot's time limit stands down when the boot returns first, and a boot that runs to its limit consumes its request" || fail "no boot time-limit lines"
has_exe "was handed to the host" && has_exe "dm_front_set_layer" && pass "the layer is handed to the host right after the dlopen (dm_front_set_layer)" || fail "no layer hand-over"
has_exe "the player ended the game" && pass "Quit game ends the guest with its own line" || fail "no 'the player ended the game' line"
has_exe "gameapp.framework" && pass "a launch with a framework on disk may boot: the early initializer looks at the stamp as well as the request" || fail "the stamp is never looked at"
has_exe "nvn_live_set" && has_exe "GAMEAPP_LIVE_THREAD" && pass "the host's live-shader hook is wired by the Rust boot (nvn_live_set)" || fail "no live-shader wiring"
# the iOS 26 link turns __DATA,__mod_init_func into __TEXT,__init_offsets: count either. The Rust initializer is the one that reads IBUILDER_EARLY_INIT_FORCE.
INIT=$(otool -l "$APP/$EXE" | grep -cE 'sectname (__mod_init_func|__init_offsets)' || true)
[ "$INIT" -ge 1 ] && has_exe "IBUILDER_EARLY_INIT_FORCE" && pass "the executable has an initializer section and the Rust early initializer's own literal (it runs before the managed runtime: the guest block is taken at launch)" || fail "no initializer / no IBUILDER_EARLY_INIT_FORCE: the guest block would not be taken at launch"

# the host's strings and global symbols, dumped once to files
HS="$WORK/host-strings.txt"; HSYMS="$WORK/host-syms.txt"; : > "$HS"; : > "$HSYMS"
if [ -f "$APP/host-prebuilt.dylib" ]; then strings -a "$APP/host-prebuilt.dylib" > "$HS" 2>/dev/null || true; nm -gU "$APP/host-prebuilt.dylib" > "$HSYMS" 2>/dev/null || true; fi
has_host() { grep -qF -- "$1" "$HS"; }
say "the host (third-party engine code, prebuilt, grafted onto by the Rust assembler)"
if [ -f "$APP/host-prebuilt.dylib" ]; then
  HP=$(otool -l "$APP/host-prebuilt.dylib" | grep -A4 LC_BUILD_VERSION | awk '/ platform /{print $2}')
  [ "$HP" = 2 ] && pass "the host is platform 2" || fail "the host's platform is $HP"
  for sym in dm_host_probe_boot dm_front_set_layer dm_front_pad_create dm_front_pad_layout dm_front_pad_layout_tv dm_front_tv_attached dm_game_menu nvn_live_set nvn_live_ready dm_cheats_init mtl_low_latency_set front_user_select dm_ios_output_channels dm_audio_input_open front_text_input front_screen_max_fps; do
    grep -qE " T _$sym\$" "$HSYMS" && pass "the host defines $sym" || fail "$sym is missing in the host"
  done
  has_host "DM_AS_AT" && has_host "not the address reserved at launch" && pass "the host maps the guest block where the app reserved it, and refuses a mismatch" || fail "the host lacks the DM_AS_AT check"
  has_host "did not compile on this device" && has_host "not tried again this run" && pass "the host compiles a library shipped as Metal source, and does not retry a failing one" || fail "no source-library path in the host"
  has_host "the touch gamepad is up" && has_host "Controller layout" && pass "the host carries the engine's touch gamepad" || fail "no touch gamepad in the host"
  has_host "touch layout editor opened" && has_host "Edit controls" && pass "the host has the touch-control editor" || fail "no touch-control editor"
  has_host "players %d and %d exchanged" && pass "the host has the players menu and button remapping" || fail "no players / remap hooks"
  has_host "Keep the last 30 s ready" && has_host "Screenshot saved to Photos" && pass "the host can take screenshots and clips into Photos" || fail "no capture items"
  has_host "Storage is full, so progress was not saved" && pass "the host has the front end's hooks (software keyboard, save failure, thermal state)" || fail "no front-end hooks"
  has_host "the physical input profile reports input" && pass "a lone Joy-Con is read (paches/engine/0103)" || fail "no lone-Joy-Con patch"
  has_host "in-game menu: %s=%s" && pass "the in-game menu changes its settings live (paches/engine/0206)" || fail "no live in-game menu settings"
  has_host "translated while the game ran" && has_host "DM_NVN_SHMAP_LEARNED" && has_host "DM_MSL_LEARNED" && pass "the host translates a shader missing from the library while the game runs (paches/0008)" || fail "no live shader translation in the host"
  has_host "pipeline binary archive: written" && has_host "DM_PSO_ARCHIVE_MB" && pass "pipelines are kept in a bounded per-game binary archive (paches/0009)" || fail "no pipeline archive in the host"
  has_host "DM_PSO_PREPARE_PERCENT" && has_host "DM_PSO_PREPARE_TAIL" && pass "the host can prepare only the first part of the prewarm list before the first frame (engine-patches/0412)" || fail "no head/tail prepare in the host (0412)"
  has_host "present upscaling (DM_PRESENT_UPSCALE)" && pass "MetalFX upscaling is in the host" || fail "no MetalFX present upscaler"
  has_host "DM_LOW_MEMORY: upload staging blocks 8 MiB" && has_host "MetalFX's frame interpolator makes a frame between each two presents" && pass "Low memory and frame generation are in the host (paches/engine/0202, 0205)" || fail "no DM_LOW_MEMORY / DM_FRAMEGEN"
  grep -qE ' T _dm_cheats_init$' "$HSYMS" && has_host "DM_ROMFS_OVERLAY" && has_host "[cheats] menu: %s -> %s" && pass "the host has the cheats VM and the RomFS overlay (mods, DECISIONS 89/90)" || fail "no cheats / overlay in the host"
  if grep -qE ' _dav1d_open$' "$HSYMS" && grep -qE ' _vpx_codec_vp9_dx$' "$HSYMS"; then pass "the host decodes the games' AV1 and VP8/VP9 movies"; else fail "no movie decoders in the host"; fi
  has_host "archive admission:" && pass "the Metal 4 pipeline archives are admitted only within the address space" || fail "no archive admission check"
  if [ -n "${SHADER_CHAIN:-}" ]; then
    grep -qx -- "$SHADER_CHAIN" "$HS" && pass "the host's shader chain is $SHADER_CHAIN (the app's stage 3 builds b-$SHADER_CHAIN.img)" || fail "the host does not carry the shader chain $SHADER_CHAIN"
    python3 -I -c 'import sys; sys.exit(0 if sys.argv[2].encode("utf-16-le") in open(sys.argv[1], "rb").read() else 1)' "$APP/GameApp.dll" "$SHADER_CHAIN" \
      && pass "and the app's own constant (GameAppBuild.ShaderChain, GameApp.dll) says the same" || fail "GameApp.dll does not carry the shader chain $SHADER_CHAIN"
  fi
  if [ -n "${FRAMEWORK_ID:-}" ]; then
    python3 -I -c 'import sys; sys.exit(0 if sys.argv[2].encode("utf-16-le") in open(sys.argv[1], "rb").read() else 1)' "$APP/GameApp.dll" "$FRAMEWORK_ID" \
      && pass "the app carries its framework id $FRAMEWORK_ID (GameAppBuild.FrameworkId, GameApp.dll: a game built by an app with this id boots without a rebuild)" || fail "GameApp.dll does not carry the framework id $FRAMEWORK_ID"
  fi
fi
for f in dm/games/settings.tsv; do [ -f "$APP/$f" ] && pass "the bundle carries $f (read by the Rust chain for the launcher catalogue)" || fail "the bundle has no $f"; done
NT=$(find "$APP/dm/games" -name game.tsv | wc -l | tr -d ' ')
[ "$NT" -gt 100 ] && pass "the bundle carries $NT registered games' game.tsv (the Rust profile deriver renders a registered game from them)" || fail "only $NT game tables in the bundle"
[ -d "$APP/dm/host" ] && [ "$(ls "$APP/dm/host"/*.c 2>/dev/null | wc -l | tr -d ' ')" -gt 5 ] && pass "dm/host sources are there (the hook set of the profile deriver)" || fail "no dm/host sources"
grep -qF -- "120=120 fps?DM_GAME_FPS_MAX==120	" "$APP/dm/games/settings.tsv" 2>/dev/null && pass "120 fps is offered on the iPhone for a game whose profile allows it" || fail "the settings list keeps 120 fps Mac-only"
grep -qF -- "DM_LOW_MEMORY	toggle" "$APP/dm/games/settings.tsv" 2>/dev/null && grep -qF -- "DM_FRAMEGEN	toggle" "$APP/dm/games/settings.tsv" 2>/dev/null && pass "Low memory and frame generation are offered per game" || fail "no DM_LOW_MEMORY / DM_FRAMEGEN rows in settings.tsv"
grep -qx "System.GC.Concurrent" "$WORK/strings.txt" && pass "the app runs non-concurrent GC (the parallel shader translator)" || fail "no System.GC.Concurrent in the executable"
if ls "$APP"/Renci.SshNet.dll >/dev/null 2>&1 || has_exe "Renci.SshNet"; then pass "SSH.NET is in the app (the NAS)"; else fail "no SSH.NET in the app"; fi
if [ "$(plutil -extract CFBundleDisplayName raw "$APP/Info.plist" 2>/dev/null)" = "iBuilder" ] && [ -f "$APP/Assets.car" ] && plutil -extract CFBundleIcons raw "$APP/Info.plist" >/dev/null 2>&1; then
  pass "the app is named iBuilder and carries its icon (Assets.car, CFBundleIcons)"
else fail "the app's name or icon is missing"; fi

say "the managed half (the app's own behaviour, by its own string literals) -- the C# is the worktree's, unchanged"
if [ -f "$APP/GameApp.dll" ]; then
  python3 -I - "$APP/GameApp.dll" > "$WORK/managed.txt" <<'PY'
import sys
green, red, reset = '\033[32m', '\033[31m', '\033[0m'
dll = open(sys.argv[1], 'rb').read()
want = [('the boot-only launch path', 'gameapp.boot-only present'),
        ('a tap that boots instead of rebuilding', 'no rebuild: this tap boots the framework this version of the app'),
        ('a framework another build made is never booted', 'was not built by this version of the app'),
        ('the checklist says when the framework is stale', 'built by an older version of the app'),
        ('the config hands the pipeline the build id', 'app_build='),
        ('the config hands the pipeline the framework id, stamped with the framework (0099)', 'framework_id='),
        ('a launch does not restart a boot that never came back', 'a boot from it was started and never came back'),
        ('a boot that ran to its time limit is one line, not an interruption', 'ended at its boot time limit'),
        ('and a time limit whose exit never finished is still reported', 'not `graceful exit done`'),
        ('the main button offers the pending boot', 'Boot again (no rebuild)'),
        ('and a way to rebuild instead', 'Rebuild everything instead'),
        ('the checklist says what is missing', 'missing — import it'),
        ('the three-way interrupted report', 'that is a fault INSIDE the guest'),
        ('the guest log travels with Export logs', 'gameapp.guest.log'),
        ('the memory floor the chain refuses under', 'boot_floor_mib'),
        ('the game is put on screen at the boot', 'game view on screen'),
        ('the app hands the game view\'s layer to the shim (P/Invoke)', 'gameapp_set_game_layer', 'ascii'),
        ('and has a way out that is the player\'s own', 'Quit game'),
        ('whose button reaches the shim\'s own end (P/Invoke)', 'gameapp_quit_game', 'ascii'),
        ('the game view takes the engine\'s gamepad (P/Invoke)', 'gameapp_pad_create', 'ascii'),
        ('an .nsz goes through the NSP rebuild before the extractor (P/Invoke)', 'gameapp_nsz', 'ascii'),
        ('the config names the host\'s shader chain', 'shader_chain='),
        ('the shader translator is compiled into the app (dmsh_xlate)', 'SwshXlate', 'ascii'),
        ('and registered with the shim at launch (P/Invoke)', 'gameapp_set_xlate', 'ascii'),
        ('a game the player quit is one quiet line', 'ended when you quit the game'),
        ('the game has no time limit unless the caller asks for one', 'boot_seconds='),
        ('a built game can be exported as its own .ipa', 'Export as .ipa…'),
        ('an exported game boots from its bundle (player mode)', 'shader_packs='),
        ('the certificate goes to a protected file when the Keychain refuses (P/Invoke)', 'gameapp_identity_where', 'ascii'),
        ('a distribution certificate is read from the signature (P/Invoke)', 'gameapp_get_task_allow', 'ascii'),
        ('a game\'s saves: copies, restore, JKSV in and out', 'Export for JKSV (Switch)…'),
        ('a NAS over SFTP keeps exported games and save copies', 'Send to the NAS'),
        ('a game\'s update and DLC are shown, added and built', 'Add an update or DLC…'),
        ('the library\'s carousel and the build checklist', 'Watch the build'),
        ('the build screen lists each step with its time', 'Translate the shaders'),
        ('the log is a timeline with an errors-only switch', 'Errors and warnings only'),
        ('cloud sources: the NAS folder and Files folders', 'OneDrive… (whatever the Files app shows).'),
        ('a running build can be paused, and a cancel offers to keep or free the extracted game', 'Cancel and keep the extracted game (next build starts faster)'),
        ('the pause says what it is waiting for', 'Pausing… (finishes the current shader)'),
        ('any game can take a custom cover', 'Remove custom cover'),
        ('a game already on the phone is added by a clone, not a copy', 'Add from a folder without copying'),
        ('a folder picker that can be switched to the older API, with a hint for clouds that stay grey', 'Add a folder (older picker)'),
        ('the game file is removed after a successful build (setting)', 'Remove the game file after a successful build'),
        ('a game can be shared to iBuilder from the Files app', 'How to add from Files'),
        ('a build that leaves no shader library is a failed build, never Ready (0078)', 'The shader library could not be built: the game would draw nothing.'),
        ('a build that could not be assembled or signed is a failed build, never ready (0078)', 'The game could not be assembled or signed: see the log.'),
        ('the redesign: actions follow the certificate (no get-task-allow means export only) (0080)', 'This certificate has no get-task-allow: a game cannot run inside iBuilder. Export it and install it as its own app.'),
        ('the redesign: a build ends on Ready, Play starts the game (no autorun) (0080)', 'It ends on Ready: Play starts the game.'),
        ('the redesign: Settings tiles for the memory and the address space (0080)', 'What iOS lets a game use'),
        ('the redesign: cloud games in the library can be hidden (0080)', 'Hide from the library'),
        ('the iPad layout: sidebar, hero and inspector (0081)', 'layout: iPad (sidebar, hero, inspector)'),
        ('the Build tab: compact rows, a game not built has no settings (0082)', 'Not built yet: its settings come once it is built'),
        ('the Build tab: one short line under the running build (0082)', 'Continues if you leave the app.'),
        ('a stopped Play is not a paused build (0083)', 'Play was stopped before the game started'),
        ('an update or DLC attached before the build (0084)', 'Add an update or DLC'),
        ('attached files are matched by title id (0084)', 'it belongs to another game'),
        ('Atmosphere cheats imported before the first build (0084)', 'A cheat file (.txt)'),
        ('NAS covers read before the download, with a reason when none (0084)', 'needs prod.keys'),
        ('the build keeps going when iOS ends the background task in a step that cannot pause (0085)', 'the build cannot pause in this step'),
        ('a pause iOS asked for is withdrawn when the person comes back (0085)', 'back in the app: the pause iOS asked for'),
        ('a game can be queued while another builds (0085)', ' waits in the queue: it starts after the current build.'),
        ('nothing periodic on the main thread while a game is on screen (0085)', 'game on screen: app timers and cloud work paused'),
        ('downloads show speed and time left (0085)', 'estimating'),
        ('free space counts what iOS can purge (0085)', 'free for important use'),
        ('the cloud is not listed again at every return (0085)', 'cloud: not listed again now'),
        ('a finished shader preparation says so (0086)', 'shader warm-up: done, '),
        ('a built, up-to-date game is never re-queued; an outdated one gets Queue a rebuild (0086)', 'Queue a rebuild'),
        ('deleting a game keeps its cover while it is in the cloud (0087)', 'Its cover stays: the game is still in the cloud.'),
        ('a paused partial build can be discarded (0088)', 'Discard the partial build'),
        ('discarding says what it frees (0088)', 'partial build discarded: '),
        ('main-thread stalls are logged while a game runs (0089)', 'main thread stalled '),
        ('NAS covers read with full reads and a block cache (0090)', '): read in '),
        ('updates and DLC without their game are grouped (0091)', 'Updates and DLC without their game'),
        ('an update or DLC arriving is attached to its game (0091)', ': rebuild to include it'),
        ('older updates can be removed (0091)', 'Older update v'),
        ('free space that cannot be read is unknown, never 0 (0093)', 'free space unknown: '),
        ('a paused build can be queued to resume (0093)', 'Queue the resume'),
        ('discarding a paused rebuild keeps the finished build (0093)', 'Its finished build stays: it still plays.'),
        ('a remote file that gives nothing stops with a clear error (0093)', 'the remote file gave nothing at byte '),
        ('the update and DLC of a base game on the NAS are offered for download (0094)', 'Download its update and DLC ('),
        ('the first build fetches the update and DLC before it starts (0094)', ', fetched before the build)'),
        ('the NAS listing classifies updates and DLC (0094)', ' classified by content)'),
        ('a build without a .p12 is signed ad hoc for export (0095)', 'Built for export: import your .p12 to play inside iBuilder, or export the IPA.'),
        ('a game built for export can be signed again to play (0095)', 'Sign again to play'),
        ('the certificate is needed only to play inside iBuilder (0095)', 'needed only to Play inside iBuilder'),
        ('a GPU without BC textures is shown and handled (0096)', 'This GPU has no BC textures'),
        ('an export above 4 GB warns about signing tools (0097)', 'This IPA is larger than 4 GB: some signing tools'),
        ('the exported Info.plist is checked after writing (0097)', 'export: Info.plist checked: '),
        ('the person chooses which game prepares its shaders first (0098)', 'Prepare this one now'),
        ('a game can be left unprepared and queued again (0098)', 'Queue its preparation again'),
        ('a game with no readable shader says it is not supported yet (0100)', 'Not supported yet: iBuilder found no shader it can read in this game'),
        ('playing a game that will draw nothing asks first (0100)', 'Play anyway (sound and controls)'),
        ('a game that closed itself is said on the next launch (0100)', 'The game closed itself'),
        ('after a full export the build can be deleted', 'Delete the build?'),
        ('a build queue, one game after another', 'Add to the build queue'),
        ('a build iOS ended is offered back on the library', 'iOS ended the last build (memory). Resume'),
        ('the chain ends on purpose before stage 3 when it would not fit, and says so', 'the chain ended on purpose before stage 3'),
        ('a build keeps running in the background', 'the build continues if you leave the app'),
        ('a TV or monitor takes the picture, the phone is the controller', 'the game picture is on the display, the phone is the controller'),
        ('  the host entries for it (through the shim)', 'gameapp_pad_layout_tv', 'ascii'),
        ('the mods sheet says when a rebuild is needed', 'Rebuild to apply: the patches of the game'),
        ('the shaders are prepared before the first session', 'Preparing the shaders')]
bad = 0
for label, marker, *how in want:
    ascii_name = how == ['ascii']
    ok = (marker.encode('ascii') if ascii_name else marker.encode('utf-16-le')) in dll
    print('  %s %s (GameApp.dll, %s)' % ((green + 'PASS' + reset) if ok else (red + 'FAIL' + reset), label, 'metadata name' if ascii_name else 'UTF-16'))
    bad |= 0 if ok else 1
sys.exit(bad)
PY
  MRC=$?
  cat "$WORK/managed.txt" >&2   # its PASS / FAIL lines are checks too: counted here
  NPASS=$((NPASS + $(grep -c 'PASS' "$WORK/managed.txt" || true))); NFAIL=$((NFAIL + $(grep -c 'FAIL' "$WORK/managed.txt" || true)))
  [ "$MRC" = 0 ] || fail "the managed half does not carry the behaviour this build claims"
else fail "GameApp.dll is not in the bundle"; fi

say "the Info.plist keys the user needs"
for k in UIFileSharingEnabled LSSupportsOpeningDocumentsInPlace; do
  V=$(plutil -extract "$k" raw "$APP/Info.plist" 2>/dev/null)
  [ "$V" = "true" ] && pass "$k = true (Documents reachable / a file handed over in place)" || fail "$k is '$V'"
done
ORI=$(plutil -extract UISupportedInterfaceOrientations json -o - "$APP/Info.plist" 2>/dev/null)
if echo "$ORI" | grep -q 'LandscapeRight' && echo "$ORI" | grep -q 'Portrait'; then pass "portrait and landscape are both allowed"; else fail "UISupportedInterfaceOrientations is '$ORI'"; fi
plutil -extract UIApplicationSceneManifest.UISceneConfigurations.UIWindowSceneSessionRoleApplication.0.UISceneDelegateClassName raw "$APP/Info.plist" >/dev/null 2>&1 \
  && pass "the scene manifest names a scene delegate (iOS 27 requires it)" || fail "no UIApplicationSceneManifest scene configuration"
plutil -extract UIApplicationSceneManifest.UISceneConfigurations.UIWindowSceneSessionRoleExternalDisplayNonInteractive.0.UISceneDelegateClassName raw "$APP/Info.plist" 2>/dev/null | grep -qx GameAppExternalSceneDelegate \
  && pass "the external display's scene is there" || fail "no external display scene"
/usr/libexec/PlistBuddy -c 'Print :CADisableMinimumFrameDurationOnPhone' "$APP/Info.plist" 2>/dev/null | grep -qx true && pass "a display link may run above 60 Hz" || fail "no CADisableMinimumFrameDurationOnPhone"
FAM=$(plutil -extract UIDeviceFamily json -o - "$APP/Info.plist" 2>/dev/null)
if echo "$FAM" | grep -q '1' && echo "$FAM" | grep -q '2'; then pass "Info.plist is universal: UIDeviceFamily $FAM"; else fail "UIDeviceFamily is '$FAM'"; fi
[ "$(plutil -extract UIRequiresFullScreen raw "$APP/Info.plist" 2>/dev/null)" = "false" ] && pass "Info.plist does not require full screen (iPad multitasking)" || fail "UIRequiresFullScreen is not false"
plutil -extract NSPhotoLibraryAddUsageDescription raw "$APP/Info.plist" >/dev/null 2>&1 && pass "NSPhotoLibraryAddUsageDescription is there (the host's screenshots)" || fail "no NSPhotoLibraryAddUsageDescription"
# APP_BUILD_NUMBER (what scripts/build-app.sh passed as ApplicationVersion) is the expected CFBundleVersion; unset, it is 1 as every earlier delivery's. APP_VERSION, when set, is the expected CFBundleShortVersionString.
EXPECT_BUILD="${APP_BUILD_NUMBER:-1}"
GOT_BUILD="$(plutil -extract CFBundleVersion raw "$APP/Info.plist" 2>/dev/null)"
[ "$GOT_BUILD" = "$EXPECT_BUILD" ] && pass "CFBundleVersion is $EXPECT_BUILD (${APP_BUILD_NUMBER:+the build number this build was given}${APP_BUILD_NUMBER:-as every earlier delivery: an installer replaces the existing app})" || fail "CFBundleVersion is '$GOT_BUILD', expected $EXPECT_BUILD"
if [ -n "${APP_VERSION:-}" ]; then
  GOT_VER="$(plutil -extract CFBundleShortVersionString raw "$APP/Info.plist" 2>/dev/null)"
  [ "$GOT_VER" = "$APP_VERSION" ] && pass "CFBundleShortVersionString is $APP_VERSION, as asked" || fail "CFBundleShortVersionString is '$GOT_VER', expected $APP_VERSION"
fi
[ ! -f "$APP/embedded.mobileprovision" ] && pass "no embedded.mobileprovision (the installer adds its own)" || fail "the archive carries a provisioning profile"

say "the signature"
SIG=$(codesign -dv "$APP/$EXE" 2>&1 | grep -E 'Signature|flags|TeamIdentifier|Authority' | tr '\n' ' ')
echo "  $SIG" >&2
if codesign -dv "$APP/$EXE" 2>&1 | grep -q 'Authority='; then fail "the executable carries a signing authority (a real identity); the .ipa must be unsigned / ad hoc"; else pass "no signing authority: ad hoc or unsigned (SideStore/AltStore re-sign on install)"; fi
if codesign -dv "$APP/$EXE" 2>&1 | grep -qE 'TeamIdentifier=[A-Z0-9]{10}'; then fail "the executable carries a team identifier"; else pass "no team identifier"; fi
# The signatures must be consistent with the bytes (review r5 M3): the scrub of the home directory and the build's edits of the frameworks invalidate one, and an installer or the loader then refuses
# the file. The executable and every other Mach-O (found by magic) are verified; unsigned or modified counts as a failure.
if codesign --verify --strict "$APP/$EXE" 2>"$WORK/codesign-exe.txt"; then pass "the executable's signature verifies (codesign --verify --strict)"; else fail "the executable's signature does not verify: $(head -1 "$WORK/codesign-exe.txt")"; fi
NSIGBAD=0; NSIGN=0; SIGBAD=""
while IFS= read -r f; do
  [ -n "$f" ] || continue
  [ "$f" = "$APP/$EXE" ] && continue
  NSIGN=$((NSIGN + 1))
  if ! codesign --verify --strict "$f" >/dev/null 2>&1; then NSIGBAD=$((NSIGBAD + 1)); SIGBAD="$SIGBAD ${f#"$APP/"}"; fi
done < <(list_macho "$APP")
if [ "$NSIGBAD" = 0 ] && [ "$NSIGN" -ge 2 ]; then pass "the signatures of the $NSIGN other Mach-O files (host, frameworks) verify"
elif [ "$NSIGN" -lt 2 ]; then fail "only $NSIGN other Mach-O file(s) found to verify the signature of"
else fail "$NSIGBAD Mach-O file(s) with a signature that does not verify:$SIGBAD"; fi
ENTS=$(codesign -d --entitlements :- "$APP/$EXE" 2>/dev/null)
if [ "${GAMEAPP_RAM_ENTITLEMENT:-1}" = 0 ]; then
  if echo "$ENTS" | grep -qE 'increased-memory-limit|extended-virtual-addressing'; then fail "GAMEAPP_RAM_ENTITLEMENT=0 and the executable still carries a memory entitlement"; else pass "the fallback build: no entitlement, as asked"; fi
elif echo "$ENTS" | grep -q 'com.apple.developer.kernel.increased-memory-limit'; then
  pass "the executable carries com.apple.developer.kernel.increased-memory-limit (an installer that honours the app's entitlements asks for it)"
  echo "$ENTS" | grep -A1 'com.apple.developer.kernel.increased-memory-limit' | grep -q '<true/>' && pass "and it is true" || fail "the increased-memory-limit entitlement is present but not true"
  if [ "${GAMEAPP_EVA_ENTITLEMENT:-0}" = 1 ]; then
    echo "$ENTS" | grep -A1 'extended-virtual-addressing' | grep -q '<true/>' && pass "and extended-virtual-addressing = true" || fail "GAMEAPP_EVA_ENTITLEMENT=1 and no extended-virtual-addressing"
  elif echo "$ENTS" | grep -q 'extended-virtual-addressing'; then fail "the executable declares extended-virtual-addressing (a free Apple ID cannot carry it)"
  else pass "and no extended-virtual-addressing (a free Apple ID through SideStore cannot carry it)"; fi
else fail "the executable carries no increased-memory-limit entitlement"; fi

say "verdict"
echo "  $((NPASS + NFAIL)) checks: $NPASS passed, $NFAIL failed (one check = one PASS or FAIL line above; this line and the next are not checks)" >&2
if [ "$BAD" = 0 ]; then echo "  ALL CHECKS PASSED  ($IPA)" >&2; else echo "  SOME CHECKS FAILED  ($IPA)" >&2; fi
exit "$BAD"
