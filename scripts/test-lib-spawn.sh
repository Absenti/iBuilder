#!/bin/bash
# scripts/test-lib-spawn.sh -- self-test of scripts/lib-spawn.sh and scripts/check-no-spawn.sh on tiny Mach-O files made here with clang and lipo (macOS host, no IPA needed).
# Review r5 P4/P5/P6: a universal file whose spawn import is only in its arm64e slice, a Mach-O reached through a symbolic link, a `$`-suffixed Darwin variant (`_popen$DARWIN_EXTSN`).
#
# usage: scripts/test-lib-spawn.sh [scratch-folder]   (default: a new folder under ${TMPDIR:-/tmp}; removed at the end unless given)
#   exit 0  every case behaves; 1 a case failed (one line per failure); 2 the tools (clang, lipo, nm) are missing
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib-spawn.sh
. "$HERE/lib-spawn.sh" || exit 2
for t in clang lipo nm file; do command -v "$t" >/dev/null 2>&1 || { echo "test-lib-spawn: $t not found" >&2; exit 2; }; done
if [ $# -ge 1 ]; then mkdir -p "$1" && W=$(cd "$1" && pwd) || exit 2; KEEP=1; else W=$(mktemp -d "${TMPDIR:-/tmp}/test-lib-spawn.XXXXXX") || exit 2; KEEP=; fi
BAD=0
ok()   { echo "ok    $*"; }
bad()  { echo "FAIL  $*"; BAD=$((BAD + 1)); }

printf '#include <stdio.h>\nint main(void){puts("x");return 0;}\n' > "$W/clean.c"
printf '#include <stdio.h>\nint main(void){FILE*f=popen("x","r");return f!=0;}\n' > "$W/evil.c"
# an import of the Darwin variant name itself (no SDK header declares it for arm64: the name is given with an asm label and left undefined at link time)
printf '#include <stdio.h>\nextern FILE *p2(const char*,const char*) __asm("_popen$DARWIN_EXTSN");\nint main(void){return p2("x","r")!=0;}\n' > "$W/suffix.c"
clang -arch arm64 "$W/clean.c" -o "$W/clean_arm64" 2>"$W/cc.log" \
  && clang -arch arm64e "$W/evil.c" -o "$W/evil_arm64e" 2>>"$W/cc.log" \
  && clang -arch arm64 "$W/suffix.c" -Wl,-U,'_popen$DARWIN_EXTSN' -o "$W/suffix_arm64" 2>>"$W/cc.log" \
  && lipo -create "$W/clean_arm64" "$W/evil_arm64e" -output "$W/fat" \
  || { echo "test-lib-spawn: could not build the test files:"; cat "$W/cc.log"; exit 2; }

# P4: the fat file's arm64e slice imports _popen; the thin clean file imports none
spawn_scan "$W/fat"
case "$SCAN_HITS" in *_popen*) ok "P4 universal file: _popen of the arm64e slice is seen ($SCAN_N undefined symbols read)" ;; *) bad "P4 universal file: _popen of the arm64e slice not seen (hits: '$SCAN_HITS')" ;; esac
case "$(printf '%s' "$SCAN_HITS")" in *architecture*) bad "P4 a slice header was read as a symbol" ;; esac
spawn_scan "$W/clean_arm64"
if [ -z "$SCAN_HITS" ] && [ "$SCAN_N" -ge 1 ]; then ok "P4 thin clean file: no hit, $SCAN_N undefined symbol(s) read"; else bad "P4 thin clean file: hits '$SCAN_HITS', $SCAN_N read"; fi
"$HERE/check-no-spawn.sh" "$W/fat" >/dev/null 2>&1; rc=$?
[ "$rc" = 1 ] && ok "P4 check-no-spawn.sh fails the universal file (exit 1)" || bad "P4 check-no-spawn.sh on the universal file: exit $rc (want 1)"
"$HERE/check-no-spawn.sh" "$W/clean_arm64" >/dev/null 2>&1; rc=$?
[ "$rc" = 0 ] && ok "P4 check-no-spawn.sh passes the thin clean file (exit 0)" || bad "P4 check-no-spawn.sh on the thin clean file: exit $rc (want 0)"

# P6: `_popen$DARWIN_EXTSN` is _popen
spawn_scan "$W/suffix_arm64"
if nm -u "$W/suffix_arm64" | grep -qx '_popen$DARWIN_EXTSN'; then
  case "$SCAN_HITS" in *_popen*) ok "P6 _popen\$DARWIN_EXTSN is counted as _popen" ;; *) bad "P6 _popen\$DARWIN_EXTSN not seen (hits: '$SCAN_HITS')" ;; esac
else bad "P6 the test file does not import _popen\$DARWIN_EXTSN (nm -u: $(nm -u "$W/suffix_arm64" | tr '\n' ' '))"; fi
spawn_scan "$W/clean_arm64"; [ -z "$SCAN_HITS" ] || bad "P6 a clean file got a hit"

# P5: a bundle with two real Mach-O files and a link to a third one outside the folder
B="$W/Bundle.app"; rm -rf "$B"; mkdir -p "$B/Frameworks" "$W/outside"
cp "$W/clean_arm64" "$B/one"; cp "$W/clean_arm64" "$B/Frameworks/two"; cp "$W/evil_arm64e" "$W/outside/evil"
ln -s "$W/outside/evil" "$B/Frameworks/linked"
ln -s "../Frameworks" "$B/Frameworks/loop"   # a link loop: reported, never walked twice
echo text > "$B/readme.txt"; ln -s "$W/nowhere" "$B/dangling"
N=$(list_macho "$B" 2>/dev/null | grep -c .)
L=$(list_macho "$B" 2>/dev/null | grep -c '/Frameworks/linked$')
if [ "$N" = 3 ] && [ "$L" = 1 ]; then ok "P5 list_macho lists the 2 Mach-O files and the linked one (3), once each, through a loop and a dangling link"
else bad "P5 list_macho listed $N file(s), the linked one $L time(s): $(list_macho "$B" 2>/dev/null | tr '\n' ' ')"; fi
HIT=
while IFS= read -r f; do spawn_scan "$f"; [ -n "$SCAN_HITS" ] && HIT="$HIT ${f#"$B/"}"; done < <(list_macho "$B" 2>/dev/null)
[ "$HIT" = " Frameworks/linked" ] && ok "P5 the linked Mach-O's _popen is seen" || bad "P5 hits in the bundle: '$HIT' (want ' Frameworks/linked')"

[ -n "$KEEP" ] || rm -rf "$W"
if [ "$BAD" -gt 0 ]; then echo "test-lib-spawn: $BAD failure(s)"; exit 1; fi
echo "test-lib-spawn: all cases pass"
exit 0
