# shellcheck shell=bash
# scripts/lib-spawn.sh -- the ONE definition of "an import that starts a process", shared by scripts/check-no-spawn.sh and app/verify_ipa.sh (sourced, never run).
# Review r4 §3.2/§3.3: the two scripts had two different lists, so the same IPA could pass one and fail the other. Edit the list HERE, nowhere else.
#
# Functions (bash 3.2 compatible, macOS tools only: nm, awk, file, xargs):
#   spawn_allow_reason <path-relative-to-the-.app> <symbol>   prints the reason and returns 0 when that import is allowed in that file, returns 1 otherwise
#   spawn_allow_by_name <file-basename> <symbol>              same, matching the file by its base name only (a lone file, whose place in a bundle is unknown)
#   spawn_scan <Mach-O>   one `nm -arch all -u` (every slice of a universal file), one `awk` pass. Sets SCAN_N (undefined symbols read), SCAN_NR (those that look like `_name`), SCAN_NALL (all symbols of the file, only when SCAN_N is 0),
#                         SCAN_HITS (the spawn symbols imported, one per line, in the file's order). Returns 0, or 2 when nm failed (message in SCAN_ERR).
#   spawn_scan_guard <Mach-O> <min-undefined>   after spawn_scan: the "measured nothing" guard (F3). Returns 0 when the scan is believable, 1 (reason in SCAN_ERR) when it is not: fewer
#                         than <min-undefined> undefined symbols, or no symbol at all when <min-undefined> is 0 (an image with truly no import, such as the R2R image of the managed
#                         half, still has defined symbols; a file nm reads nothing from is not "clean", it is unmeasured).
#   filter_macho            stdin = paths (one per line), stdout = those that are Mach-O by content, same order (build-app.sh signs them)
#   list_macho <folder>   prints, one per line, every regular file (or link to one) under <folder> that IS a Mach-O by content (`file`'s magic: thin, fat, 32/64-bit), whatever its mode or name (F4: the
#                         previous `-perm -u+x -o -name '*.dylib' -o -path '*.framework/*'` filter missed a 0644 Mach-O outside a framework). One `find`, one batched `file`.

# The calls that start a process (whole symbol names, with Apple's leading underscore). _forkpty (fork + pty) and _daemon (fork + setsid) were missing from both former lists, _fexecve too.
SPAWN_SYMBOLS="_fork _vfork _forkpty _daemon _execve _fexecve _execv _execvp _execvP _execvpe _execl _execle _execlp _posix_spawn _posix_spawnp _system _popen"
SPAWN_ERE='^_(fork|vfork|forkpty|daemon|execve|fexecve|execv|execvp|execvP|execvpe|execl|execle|execlp|posix_spawn|posix_spawnp|system|popen)([$].*)?$'

# The imports allowed, each by file (relative to the .app) and symbol, with its reason (one case arm per entry; keep it that way: the reason is printed where the import is found).
spawn_allow_reason() {
  case "$1:$2" in
    host-prebuilt.dylib:_popen)
      echo "the engine's frame capture runs git describe; iOS refuses the process" ;;
    Frameworks/libSystem.Native.framework/libSystem.Native:_fork|Frameworks/libSystem.Native.framework/libSystem.Native:_execve)
      echo "the .NET runtime's Process.Start, which the app's C# never calls; iOS refuses the process" ;;
    *) return 1 ;;
  esac
}
spawn_allow_by_name() {
  case "$1" in
    host-prebuilt.dylib) spawn_allow_reason "host-prebuilt.dylib" "$2" ;;
    libSystem.Native) spawn_allow_reason "Frameworks/libSystem.Native.framework/libSystem.Native" "$2" ;;
    *) return 1 ;;
  esac
}

spawn_scan() {
  local f=$1 syms out
  SCAN_N=0; SCAN_NR=0; SCAN_NALL=0; SCAN_HITS=""; SCAN_ERR=""
  # nm -u prints bare names on Apple's nm (`_fork`); other nm builds print `U _fork`. Both are reduced to the bare name.
  # `-arch all` (review r5 P4): a universal file is read slice by slice (plain `nm -u` reads the host's slice only, so an import present only in an arm64e slice was not seen); each slice is
  # announced by a `<file> (for architecture <arch>):` line, which is not a symbol. A thin file prints as before.
  # A Darwin variant suffix (review r5 P6: `_popen$DARWIN_EXTSN`, `_fork$NOCANCEL`...) is the same call: the name is compared without what follows its first `$`. (Objective-C names such as
  # `_OBJC_CLASS_$_X` reduce to a prefix that is never in the list.)
  if ! syms=$(nm -arch all -u "$f" 2>&1); then SCAN_ERR="nm -arch all -u failed on $f: $syms"; return 2; fi
  out=$(printf '%s\n' "$syms" | awk -v list="$SPAWN_SYMBOLS" '
    BEGIN { n = split(list, a, " "); for (i = 1; i <= n; i++) spawn[a[i]] = 1 }
    / \(for architecture [^)]*\):$/ { next }
    { sub(/^[ \t]*U[ \t]+/, ""); sub(/^[ \t]+/, ""); sub(/[ \t]+$/, ""); if ($0 == "") next
      c++; if ($0 ~ /^_[A-Za-z0-9_$.]+$/) r++
      base = $0; sub(/[$].*$/, "", base)
      if (base in spawn) seen[base] = 1 }
    END { for (i = 1; i <= n; i++) if (a[i] in seen) hits = hits a[i] "\n"   # reported in the order of the list, as the per-symbol loops did
          printf "%d %d\n%s", c, r, hits }')
  SCAN_N=${out%%[ ]*}; out=${out#*[ ]}
  case "$out" in
    *$'\n'*) SCAN_NR=${out%%$'\n'*}; SCAN_HITS=${out#*$'\n'} ;;   # $(...) dropped the trailing newline: no hit line means no newline at all
    *)        SCAN_NR=$out; SCAN_HITS="" ;;
  esac
  if [ "$SCAN_N" -eq 0 ]; then SCAN_NALL=$(nm -arch all "$f" 2>/dev/null | grep -v ' (for architecture [^)]*):$' | grep -c . || true); fi
  return 0
}

spawn_scan_guard() {
  local f=$1 min=$2
  if [ "$min" -gt 0 ]; then
    if [ "$SCAN_NR" -lt "$min" ]; then SCAN_ERR="nm -u lists $SCAN_NR undefined symbols in $f (fewer than $min): the check measured nothing"; return 1; fi
  elif [ "$SCAN_N" -eq 0 ] && [ "${SCAN_NALL:-0}" -eq 0 ]; then
    SCAN_ERR="nm lists no symbol at all in $f: the check measured nothing"; return 1
  fi
  return 0
}

# filter_macho: stdin = paths, one per line; stdout = the ones that are Mach-O by content, in order, duplicates kept. One batched `file` (it was one `file | grep` per path).
filter_macho() {
  local list
  list="$(cat)"
  [ -n "$list" ] || return 0
  printf '%s\n' "$list" | tr '\n' '\0' | xargs -0 file -0 --mime-type | tr '\0' '\n' | awk '
    /: *application\/x-mach-binary$/ { if (prev != "") print prev; prev = ""; next }
    { prev = $0 }'
}
# `find -L` (review r5 P5): a symbolic link to a Mach-O is listed (by its own path) like the file itself, so an import behind a link is scanned and a linked file is signed and verified; `-L`
# reports a link loop on stderr and goes on (it never walks one twice). A dangling link is not a file: it is skipped, as before.
list_macho() { find -L "$1" -type f | filter_macho; }
