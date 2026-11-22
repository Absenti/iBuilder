# shellcheck shell=bash
# scripts/lib-hash.sh -- content fingerprints shared by scripts/build-app.sh and scripts/build-ios-ffi.sh (sourced, never run). One xargs pass per tree instead of three processes per file
# (review r4 §6.3: find|while read|shasum|cut was 8.4x slower for the crates, 19x for a plain folder). The values are IDENTICAL to the ones the per-file loops produced.
#
#   hash_listing [<hex digits kept, default 64>]   stdin: one path per line (relative to the current folder); stdout: `<sha256 prefix> <path>` per line, in input order
#   rust_tree_hashes <repo root>                   one line per crate: `<crate>TAB<sha256 of its src/, include/, Cargo.toml, build.rs>`
#   dir_tree_hash <folder>                         sha256 of the sorted `<16 hex> <path>` listing of every file of the folder (.DS_Store and ._* left out)

hash_listing() {
  local w="${1:-64}" list
  list="$(cat)"                                   # a small string; an empty list must not start `shasum` on stdin
  [ -n "$list" ] || return 0
  printf '%s\n' "$list" | tr '\n' '\0' | xargs -0 shasum -a 256 | awk -v w="$w" '{
      # shasum escapes a name holding a backslash or a newline: the line starts with "\" and the name has \\ for \ and \n for a newline (review r5 L3); undo it, so the value is the one the per-file loops gave
      h = $1; p = substr($0, 67)
      if (substr(h, 1, 1) == "\\") { h = substr(h, 2); p = substr($0, 68); gsub(/\\\\/, "\001", p); gsub(/\\n/, "\n", p); gsub(/\001/, "\\", p) }
      print substr(h, 1, w), p }'
}

rust_tree_hashes() {
  local root="$1" c n h
  for c in "$root"/crates/*/; do
    n="$(basename "$c")"
    h="$( (cd "$c" && { find src include -type f 2>/dev/null; ls Cargo.toml build.rs 2>/dev/null; } | LC_ALL=C sort | hash_listing) | shasum -a 256 | cut -d' ' -f1)"
    printf '%s\t%s\n' "$n" "$h"
  done
}

dir_tree_hash() {
  ( cd "$1" && find . -type f ! -name .DS_Store ! -name '._*' | LC_ALL=C sort | hash_listing 16 ) | shasum -a 256 | cut -d' ' -f1
}
