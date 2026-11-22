#!/bin/bash
# scripts/check-no-spawn.sh -- the project promises that no process is ever spawned. This checks the promise on a built executable:
# it fails if the Mach-O imports any call that starts a process (fork, vfork, forkpty, daemon, exec*, posix_spawn*, system, popen; the list is scripts/lib-spawn.sh, shared with app/verify_ipa.sh).
# Whole symbol names only: `_forkpty_x` or `_systemd` do not count.
#
# usage: scripts/check-no-spawn.sh <Mach-O executable | GameApp.app folder>
#   exit 0  no spawn symbol among the undefined symbols
#   exit 1  one or more spawn symbols (one line per hit, then a summary)
#   exit 2  bad argument, the executable's imports could not be read (or too few: nothing was measured), or a lone file that the bundle mode would allow an import in: pass the .app
set -u
# shellcheck source=lib-spawn.sh
. "$(dirname "$0")/lib-spawn.sh" || exit 2

usage() { echo "usage: check-no-spawn.sh <Mach-O executable | .app folder>" >&2; exit 2; }
[ $# -eq 1 ] || usage
ARG=$1

if [ -d "$ARG" ]; then
  case "$ARG" in
    *.app|*.app/) ;;
    *) echo "check-no-spawn: '$ARG' is a folder but not a .app bundle" >&2; exit 2 ;;
  esac
  APPDIR=${ARG%/}
  PLIST="$APPDIR/Info.plist"
  [ -f "$PLIST" ] || { echo "check-no-spawn: no Info.plist in $APPDIR" >&2; exit 2; }
  EXE_NAME=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$PLIST" 2>/dev/null) || EXE_NAME=""
  [ -n "$EXE_NAME" ] || { echo "check-no-spawn: no CFBundleExecutable in $PLIST" >&2; exit 2; }
  EXE="$APPDIR/$EXE_NAME"
  # A .app: the executable AND every other Mach-O of the bundle (the host dylib, the frameworks): review r3 §4.3. The one allowed import is listed in ALLOW.
  ALL=1
elif [ -f "$ARG" ]; then
  EXE=$ARG
else
  echo "check-no-spawn: no such file or .app folder: $ARG" >&2
  exit 2
fi
[ -f "$EXE" ] || { echo "check-no-spawn: no executable at $EXE" >&2; exit 2; }

# The list of spawn symbols and the allowed imports (each with its reason) live in scripts/lib-spawn.sh: one definition for this script and app/verify_ipa.sh.
# libSystem.Native's _fork / _execve: the .NET runtime's SystemNative_ForkAndExecProcess, used only by System.Diagnostics.Process.Start; the app's C# (app/gameapp + every patch, and the
# translator's xlate/) never names Process / ProcessStartInfo (grep, 2026-11-20), and iOS refuses to start a process anyway. host-prebuilt.dylib's _popen: the engine's frame capture.

# The executable (or the lone file): at least one undefined symbol, or nothing was measured.
spawn_scan "$EXE" || { echo "check-no-spawn: $SCAN_ERR" >&2; exit 2; }
# A .app's executable is the app (hundreds of imports; verify_ipa.sh requires 50): fewer means nm read next to nothing. A lone file may be a tiny test binary: one is enough.
MIN_EXE=1; [ -n "${ALL:-}" ] && MIN_EXE=50
if ! spawn_scan_guard "$EXE" "$MIN_EXE"; then
  echo "check-no-spawn: $SCAN_ERR" >&2
  exit 2
fi
N=$SCAN_N

if [ -z "${ALL:-}" ]; then
  # A lone file: its place in a bundle is unknown, so an allowed import (by file and name) cannot be judged here. Say so (exit 2) rather than FAIL a binary the bundle mode allows.
  HITS=0; PENDING=0; base=${EXE##*/}
  while IFS= read -r s; do
    [ -n "$s" ] || continue
    if reason=$(spawn_allow_by_name "$base" "$s"); then
      echo "$base imports $s, which the bundle mode allows ($reason): pass the .app folder so the path can be checked" >&2
      PENDING=$((PENDING + 1)); continue
    fi
    echo "spawn symbol imported: $s  ($EXE)"
    HITS=$((HITS + 1))
  done <<EOF2
$SCAN_HITS
EOF2
  if [ "$HITS" -gt 0 ]; then
    echo "FAIL: $HITS process-spawn symbol(s) among $N undefined symbols in $EXE"
    exit 1
  fi
  if [ "$PENDING" -gt 0 ]; then echo "check-no-spawn: $PENDING import(s) allowed only inside the bundle: run it on the .app folder" >&2; exit 2; fi
  echo "no spawn symbol among $N undefined symbols ($EXE)"
  exit 0
fi

# A .app: the executable's imports are never allowed (nothing in ALLOW names it).
HITS=0
while IFS= read -r s; do
  [ -n "$s" ] || continue
  echo "spawn symbol imported: $s  ($EXE)"
  HITS=$((HITS + 1))
done <<EOF2
$SCAN_HITS
EOF2
if [ "$HITS" -gt 0 ]; then
  echo "FAIL: $HITS process-spawn symbol(s) among $N undefined symbols in $EXE"
  exit 1
fi
echo "no spawn symbol among $N undefined symbols ($EXE)"
NFILES=0; OTHER=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  [ "$f" = "$EXE" ] && continue
  NFILES=$((NFILES + 1))
  rel=${f#"$APPDIR/"}
  spawn_scan "$f" || { echo "check-no-spawn: $SCAN_ERR" >&2; exit 2; }
  spawn_scan_guard "$f" 0 || { echo "check-no-spawn: $SCAN_ERR" >&2; exit 2; }
  while IFS= read -r s; do
    [ -n "$s" ] || continue
    if reason=$(spawn_allow_reason "$rel" "$s"); then echo "allowed: $rel imports $s ($reason)"; continue; fi
    echo "spawn symbol imported: $s  ($f)"
    OTHER=$((OTHER + 1))
  done <<EOF2
$SCAN_HITS
EOF2
done < <(list_macho "$APPDIR")
# fewer than 2 other Mach-O files read (a failed `file`, an empty folder): nothing was measured, which is not a pass (as app/verify_ipa.sh)
if [ "$NFILES" -lt 2 ]; then echo "check-no-spawn: only $NFILES other Mach-O file(s) found in $APPDIR: the check of the host and the frameworks measured nothing" >&2; exit 2; fi
if [ "$OTHER" -gt 0 ]; then echo "FAIL: $OTHER process-spawn symbol(s) in the bundle's other Mach-O files ($NFILES read)"; exit 1; fi
echo "no spawn symbol in the bundle's other $NFILES Mach-O files, but for the allowed imports listed above"
exit 0
