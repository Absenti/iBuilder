#!/usr/bin/env bash
# scripts/check-forbidden-files.sh [<root>] -- fails, with the list, when the tree holds a secret or a piece of game data.
#
# This project ships no game, no firmware, no key and no certificate (README.md, CONTRIBUTING.md). This is the mechanical check of that promise; CI runs it on every push and pull
# request (job `forbidden-files`), and it can be run by hand before a commit:  scripts/check-forbidden-files.sh
#
# <root> defaults to the repository root. A sub-folder of the repository works too (the allowlist is matched on the path relative to the repository); a folder outside it is checked as if it were a
# copy of the repository (the allowlist paths are relative to <root>).
# The files looked at are the ones a commit would contain:
#   * inside a git work tree:  `git ls-files --cached --others --exclude-standard` (tracked files, plus new files that are not ignored);
#   * otherwise, with git:     the same list from a scratch index outside the tree (.gitignore applies as on a push)
#   * otherwise:               every file under <root>, except the folders .git and target.
# What is refused (rule name in brackets):
#   [keys]        prod.keys, title.keys, any *.keys                      [certificate]  *.p12, *.pfx, *.p8, *.jks, *.keystore
#   [profile]     *.mobileprovision, *.provisionprofile                  [local-config] local.env (the gitignored scripts/local.env)
#   [private-key] a text file holding `-----BEGIN ... PRIVATE KEY-----`; a *.pem or *.key that is binary (a DER key cannot be inspected)
#   [game-data]   *.nsp *.xci *.nsz *.nup *.nca *.nxgame (also a folder named *.nxgame), RomFS.img, *.dmsb, and by CONTENT, whatever the name:
#                 NSO0 at offset 0 (a Switch code module), NRO0 at 0x10, PFS0 / HFS0 at 0, HEAD at 0x100 (an XCI), NCA2 / NCA3 at 0x200
#   [binary]      *.ipa, *.dylib, a *.a larger than 1 MB
#   [size]        any file larger than 10 MB
#   [unreadable]  a file that cannot be read (it could not be inspected)
#   [key-line]    a text file with an assignment line of the prod.keys format: master_key_NN, header_key, titlekek_NN or key_area_key_<kind>_NN, then `=`
# Exit status: 0 nothing found, 1 one or more hits (printed, one line each), 2 bad argument or a tool is missing.
#
# Nothing here proves a file is legal to publish: it catches the usual accidents (a file copied from the dump folder, a key pasted into a note). The pre-publication audit is a human one.
set -u

# ==================================================================================================================================================================
# ALLOWLIST: the ONLY exceptions. Every entry is a path pattern (shell glob, `*` also matches `/`, relative to <root>) plus a size cap, and says why. Add nothing without a reason.
# ==================================================================================================================================================================
# Used for the files the NSO0 magic, the NRO0 magic or the *.nso name flags. Return 0 = allowed. $1 = path relative to <root>, $2 = size in bytes.
nso_fixture_allowed() {
  case "$1" in
    # 1. The small synthetic Switch modules of the crates' tests (crates/<crate>/tests/data/**, crates/<crate>/tests/fixtures/**): for example the 14 `main` files of the shader splitter's
    #    games/ corpus (4,909 bytes each). Made by the repository's own generators, no game code in them. Cap 64 KB.
    crates/*/tests/data/*|crates/*/tests/fixtures/*)
      [ "$2" -le 65536 ] && return 0 ;;
  esac
  case "$1" in
    # 2. The rewriter's per-game stub fixtures, built by crates/ibuilder-rewrite/tools/stubsynth.py ("No game code is in it: a function is nops and a ret", docs/crates/REWRITE-RUST.md,
    #    "Verification of the ports"). The biggest is 244,616 bytes, hence the larger cap (256 KB) for this one folder.
    crates/ibuilder-rewrite/tests/fixtures/stubs/*.nso)
      [ "$2" -le 262144 ] && return 0 ;;
  esac
  case "$1" in
    # 3. The relayout fixtures, built by crates/ibuilder-rewrite/tools/layoutcases.py: the synthetic images of upstream's dm/tools/tests/test_mod_layout.py (an NRO of the Math.nro shape, an
    #    uncompressed NSO with MOD0 in text and DT_RELR, the .eh_frame / LSDA variant, a module that already fits; "no game code"). The NSO is 78,080 bytes, hence 128 KB.
    crates/ibuilder-rewrite/tests/fixtures/layout/base.*.bin)
      [ "$2" -le 131072 ] && return 0 ;;
  esac
  return 1
}
# Text files allowed to contain a line the [key-line] / [private-key] scan matches (documentation of a format, never a value). Return 0 = allowed. $1 = path relative to <root>.
text_hit_allowed() {
  # None today. An entry would be a case clause, for example:  case "$1" in docs/some-format-note.md) return 0 ;; esac
  return 1
}
# ==================================================================================================================================================================

MAX_FILE_BYTES=10485760        # [size]   10 MB
MAX_STATIC_LIB_BYTES=1048576   # [binary] 1 MB for *.a

ROOT_ARG="${1:-}"
case "$ROOT_ARG" in -h|--help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;; esac
if [ -z "$ROOT_ARG" ]; then
  ROOT="$(cd "$(dirname "$0")/.." && pwd)"
else
  ROOT="$(cd "$ROOT_ARG" 2>/dev/null && pwd)" || { echo "check-forbidden-files: not a folder: $ROOT_ARG" >&2; exit 2; }
fi
# F5 (review r4): the allowlist patterns are relative to the REPOSITORY root. When <root> is a folder inside this repository (crates/ibuilder-rewrite, say) the paths below are relative to
# <root>, so the allowlist is matched against PREFIX + path (PREFIX = <root> relative to the repository, with a trailing slash); the messages keep paths relative to <root>.
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX=""
case "$ROOT/" in
  "$REPO_ROOT"/?*) PREFIX="${ROOT#"$REPO_ROOT"/}/" ;;
esac
for tool in find grep wc tr od sort xargs; do
  command -v "$tool" >/dev/null 2>&1 || { echo "check-forbidden-files: '$tool' is not installed" >&2; exit 2; }
done

TMP="$(mktemp -d "${TMPDIR:-/tmp}/forbidden-files.XXXXXX")" || exit 2
trap 'rm -rf "${TMP:?}"' EXIT
LIST="$TMP/list"         # NUL-separated paths, relative to ROOT
HITS="$TMP/hits"         # rule <TAB> path <TAB> detail
SCAN="$TMP/scan"         # NUL-separated regular files small enough for the text scans
: > "$HITS"; : > "$SCAN"

cd "$ROOT" || exit 2
MODE=find
if command -v git >/dev/null 2>&1 && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  MODE=git
  git ls-files -z --cached --others --exclude-standard > "$LIST" || { echo "check-forbidden-files: git ls-files failed" >&2; exit 2; }
elif command -v git >/dev/null 2>&1 && [ -f .gitignore ]; then
  # Not a git work tree (the project lives without one before its first push): git's own ignore engine on a throw-away index OUTSIDE the tree (GIT_DIR in $TMP), so the list is
  # what a first `git add -A` would take: .gitignore applies exactly as it will on the push (dist/, target/, scripts/local.env are ignored there). Review r3 §5.14: the plain walk below
  # listed those ignored outputs too (the .ipa and dylibs of dist/, the machine-local env file) and reported them as forbidden.
  MODE=gitignore
  GIT_DIR="$TMP/ignore-index" git --work-tree=. init -q >/dev/null 2>&1 \
    && GIT_DIR="$TMP/ignore-index" git --work-tree=. ls-files -z --others --exclude-standard > "$LIST" \
    || { echo "check-forbidden-files: git ls-files on a scratch index failed" >&2; exit 2; }
else
  find . \( -name .git -o -name target \) -prune -o \( -type f -o -type l \) -print0 > "$LIST" \
    || { echo "check-forbidden-files: could not list $ROOT" >&2; exit 2; }
fi

hit() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$HITS"; }
# hex of the first 516 bytes (enough for every magic below), lower case, no spaces or newlines. One process per file: `xxd -p -c 516` prints a single line. The previous version used a bash
# substitution (`${h// /}`) on the `od` output, which is quadratic: 495.8 ms per call under fr_FR.UTF-8 (55.4 ms under LC_ALL=C); `od | tr -d ' \n'` is the portable fallback when xxd is missing.
if [ "$(printf 'AB' | xxd -p -c 516 -l 516 2>/dev/null)" = 4142 ]; then   # probe: an xxd that rejects -c 516 must not silently blind the magic checks
  head_hex() { xxd -p -c 516 -l 516 "$1" 2>/dev/null; }
else
  head_hex() { od -An -tx1 -v -N 516 "$1" 2>/dev/null | tr -d ' \n'; }
fi
shopt -s nocasematch                             # file names are matched without regard to case (PROD.KEYS, Game.NSP): `case` obeys this

NFILES=0; NALLOWED=0
while IFS= read -r -d '' f; do
  f="${f#./}"                                    # find prints ./path, git prints path
  [ -e "$f" ] || [ -L "$f" ] || continue         # listed by git but deleted in the work tree
  NFILES=$((NFILES + 1))
  base="${f##*/}"

  # ---- by name -------------------------------------------------------------------------------------------------------------------------------------------------
  case "$base" in
    *.keys)                                      hit keys "$f" "a key file ($base)" ;;
    *.p12|*.pfx|*.p8|*.jks|*.keystore)           hit certificate "$f" "a certificate / signing key container" ;;
    *.mobileprovision|*.provisionprofile)        hit profile "$f" "a provisioning profile" ;;
    local.env)                                   hit local-config "$f" "a machine-local settings file, never committed" ;;
    *.nsp|*.xci|*.nsz|*.nup|*.nca|*.nxgame)      hit game-data "$f" "a game container (.${base##*.})" ;;
    romfs.img)                                   hit game-data "$f" "a RomFS image" ;;
    *.dmsb)                                      hit game-data "$f" "a shader library built from a game" ;;
    *.ipa)                                       hit binary "$f" "an .ipa (releases carry those, not the tree)" ;;
    *.dylib)                                     hit binary "$f" "a .dylib (a built host library)" ;;
  esac
  case "$f" in
    *.nxgame/*)                                  hit game-data "$f" "inside a *.nxgame folder" ;;
  esac

  [ -L "$f" ] && continue                        # a symbolic link: the name rules above are all that apply
  [ -f "$f" ] || continue
  # a file that cannot be read cannot be inspected: that is a finding, never a silent pass (review r5 M8)
  if [ ! -r "$f" ]; then hit unreadable "$f" "cannot be read (permissions): not inspected"; continue; fi
  size="$(wc -c < "$f")"; size="${size// /}"

  # ---- by size -------------------------------------------------------------------------------------------------------------------------------------------------
  [ "$size" -gt "$MAX_FILE_BYTES" ] && hit size "$f" "$size bytes (limit $MAX_FILE_BYTES)"
  case "$base" in
    *.a) [ "$size" -gt "$MAX_STATIC_LIB_BYTES" ] && hit binary "$f" "static library of $size bytes (limit $MAX_STATIC_LIB_BYTES)" ;;
  esac

  # ---- by content: the magic of a game container or a Switch module, whatever the file is called -------------------------------------------------------------
  nso=0
  case "$base" in *.nso) nso=1 ;; esac
  if [ "$size" -ge 4 ]; then
    hex="$(head_hex "$f")"
    case "$hex" in 4e534f30*) nso=1 ;; esac                                  # "NSO0" at 0
    if [ "${hex:32:8}" = 4e524f30 ]; then
      if nso_fixture_allowed "$PREFIX$f" "$size"; then NALLOWED=$((NALLOWED + 1)); else hit game-data "$f" "an NRO module (NRO0 at 0x10)"; fi
    fi
    case "$hex" in 50465330*) hit game-data "$f" "a PFS0 partition (an NSP)" ;; 48465330*) hit game-data "$f" "an HFS0 partition (an XCI)" ;; esac
    [ "${hex:512:8}" = 48454144 ] && hit game-data "$f" "an XCI header (HEAD at 0x100)"
    case "${hex:1024:8}" in 4e434133|4e434132) hit game-data "$f" "an NCA (NCA2/NCA3 at 0x200)" ;; esac
  fi
  if [ "$nso" = 1 ]; then
    if nso_fixture_allowed "$PREFIX$f" "$size"; then NALLOWED=$((NALLOWED + 1)); else hit game-data "$f" "a Switch code module (NSO) of $size bytes, not in the allowlist"; fi
  fi

  # ---- by content: private keys that are not text --------------------------------------------------------------------------------------------------------------
  case "$base" in
    *.pem|*.key) [ "$size" -gt 0 ] && ! grep -Iq '' "$f" 2>/dev/null && hit private-key "$f" "binary .pem/.key: cannot be inspected" ;;
  esac

  [ "$size" -le "$MAX_FILE_BYTES" ] && printf '%s\0' "$f" >> "$SCAN"
done < "$LIST"

# ---- by content: the text scans (binary files are skipped by grep -I) ------------------------------------------------------------------------------------------
scan() {   # scan <rule> <detail> <ERE>
  [ -s "$SCAN" ] || return 0
  xargs -0 grep -IlE -e "$3" -- < "$SCAN" 2>/dev/null | while IFS= read -r p; do
    text_hit_allowed "$PREFIX$p" || hit "$1" "$p" "$2"
  done
}
scan private-key 'holds a private key block' 'BEGIN [A-Z0-9 ]*PRIVATE KEY'
scan key-line    'has a master_key line (prod.keys format)'        'master_key_[0-9a-f]{2}[[:space:]]*='
scan key-line    'has a header_key line (prod.keys format)'        'header_key[[:space:]]*='
scan key-line    'has a titlekek line (prod.keys format)'          'titlekek_[0-9a-f]{2}[[:space:]]*='
scan key-line    'has a key_area_key line (prod.keys format)'      'key_area_key_[a-z]+_[0-9a-f]{2}[[:space:]]*='

# ---- report ----------------------------------------------------------------------------------------------------------------------------------------------------
if [ -s "$HITS" ]; then
  N="$(sort -u "$HITS" | wc -l | tr -d ' ')"
  echo "check-forbidden-files: $N forbidden file(s) or line(s) under $ROOT ($MODE mode, $NFILES files looked at):" >&2
  sort -u "$HITS" | awk -F'\t' '{ printf "  [%s] %s: %s\n", $1, $2, $3 }' >&2
  cat >&2 <<'MSG'

Remove them from the tree. If one was ever committed or pushed, removing the file is not enough: a secret must be revoked and replaced, and the history rewritten.
The allowlist (synthetic test fixtures only) is at the top of scripts/check-forbidden-files.sh.
MSG
  exit 1
fi
echo "check-forbidden-files: OK ($NFILES files looked at in $MODE mode under $ROOT; $NALLOWED synthetic NSO fixture(s) allowlisted)"
exit 0
