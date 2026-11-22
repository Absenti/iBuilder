# shellcheck shell=bash
# scripts/lib-hash.sh -- content fingerprints shared by scripts/build-app.sh and scripts/build-ios-ffi.sh (sourced, never run). One xargs pass per tree instead of three processes per file
# (review r4 §6.3: find|while read|shasum|cut was 8.4x slower for the crates, 19x for a plain folder). The values are IDENTICAL to the ones the per-file loops produced.
#
#   hash_listing [<hex digits kept, default 64>]   stdin: one path per line (relative to the current folder); stdout: `<sha256 prefix> <path>` per line, in input order
#   rust_tree_hashes <repo root>                   one line per crate: `<crate>TAB<sha256 of its src/, include/, Cargo.toml, build.rs>`
#   dir_tree_hash <folder>                         sha256 of the sorted `<16 hex> <path>` listing of every file of the folder (.DS_Store and ._* left out)
#   crate_tree_hash <crate folder>                 the value rust_tree_hashes prints for one crate
#   framework_id_listing <repo root> <engine> <host dylib> <shader chain> <cargo features>
#                                                  the FRAMEWORK ID's inputs, one `name TAB value` line each (see the block above the function); framework_id_of turns the listing into the id

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

crate_tree_hash() {
  (cd "$1" && { find src include -type f 2>/dev/null; ls Cargo.toml build.rs 2>/dev/null; } | LC_ALL=C sort | hash_listing) | shasum -a 256 | cut -d' ' -f1
}

rust_tree_hashes() {
  local root="$1" c
  for c in "$root"/crates/*/; do
    printf '%s\t%s\n' "$(basename "$c")" "$(crate_tree_hash "$c")"
  done
}

# ---- the FRAMEWORK ID ------------------------------------------------------------------------------------------------------------------------------------------------------
# The app's build id (GameAppBuild.Id, <yyyymmdd-hhmmss>-rust<hash>) changes with every build; a built game's framework stamp (Documents/gameapp.framework) compared with it made every
# game "Needs setup" after every app update. The framework id (GameAppBuild.FrameworkId, the config's framework_id=, stamped as framework_id=) is a hash of ONLY what shapes a built
# framework (DmGameProbe.framework, extract/flat, profile.tsv) and its use at the boot: an app whose framework id is the stamp's boots the game with no rebuild; any other id rebuilds.
# The inputs, one line each, in this order (a missing file or folder hashes as "-", so a moved input changes the id rather than vanishing from it):
#   framework-epoch      FRAMEWORK_EPOCH below: raise it by hand when something NOT listed here changes what a build makes or how a built game boots (a config key the C# app hands the
#                        chain at the build, say): every game then rebuilds once
#   host                 sha256 of the prebuilt loadable host the bundle carries (dist/host/host-prebuilt-loadable-{ios,sim}.dylib, pinned in app/inputs.tsv): the game is grafted onto it
#   shader-chain         the host's translation chain (dmsh_chain.h); the shader library has its own versioning besides (chain + split version), this is the host's contract
#   cargo-features       the ibuilder-ffi features of the build (real-stages, ibuilder-cli/nup)
#   crate:<name>         crate_tree_hash of each stage crate that makes framework bytes or reads the game: ibuilder-nso ibuilder-a64 ibuilder-rewrite ibuilder-macho ibuilder-sign
#                        ibuilder-profile ibuilder-mods ibuilder-nup
#   ibuilder-pipeline    its Cargo.toml and src/, EXCEPT the files that never touch a framework or its boot (FRAMEWORK_PIPELINE_EXCLUDED: stage 3's shaders, the logs, the
#                        memory/thermal gates, the stop request, the launch state machine and the stamp itself); a new file of the crate counts until it is excluded by name
#   ibuilder-cli-adapters  crates/ibuilder-cli/src/adapters.rs and Cargo.toml (the real stage workers the app runs, and their features)
#   cargo-lock           sha256 of Cargo.lock (the dependencies' versions)
#   app-game-tables      dir_tree_hash of app/game-tables (this repository's tables, which replace the engine's of the same name in the bundle)
#   engine-settings      sha256 of <engine>/dm/games/settings.tsv (the settings the profile and the launcher render)
#   engine-game-tables   the hash listing of every <engine>/dm/games/<id>/game.tsv (the registered games' tables the profile is derived from)
#   engine-host-sources  the hash listing of <engine>/dm/host/*.c, dm/gfx/*.c, dm/gfx/nvn/*.c, dm/gfx/vk/*.m (the hook set the profile reads from the host's sources)
#   extractor            sha256 of <engine>/dm/tools/nx_extract/Program.cs and app/patches/nx_extract_merged_update.py (stage 1: what the game's code is read from)
# NOT inputs: the C# app (app/gameapp, app/patches/*.patch), the build time, ibuilder-shader / -shaderbuild / -shadersplit (the shader library: its own versioning), ibuilder-ffi,
# ibuilder-alloc, ibuilder-pack, ibuilder-testkit, the shim, zsign / OpenSSL (a framework already signed stays signed), the docs. The id is the first 16 hex digits of the sha256 of the
# listing. Same tree, same id: scripts/framework-id.sh prints it without building anything.
FRAMEWORK_EPOCH=1
FRAMEWORK_CRATES="ibuilder-nso ibuilder-a64 ibuilder-rewrite ibuilder-macho ibuilder-sign ibuilder-profile ibuilder-mods ibuilder-nup"
FRAMEWORK_PIPELINE_EXCLUDED="src/shaders.rs src/shaderpack.rs src/live.rs src/gate.rs src/thermal.rs src/memory.rs src/log.rs src/logwindow.rs src/diag.rs src/timings.rs src/stop.rs src/state.rs src/stage.rs src/markers.rs src/mock.rs src/testutil.rs src/sys/mem.rs src/sys/qos.rs src/sys/signals.rs src/sys/redirect.rs"

file_sha_or_dash() { if [ -f "$1" ]; then shasum -a 256 < "$1" | cut -d' ' -f1; else echo -; fi; }

framework_id_listing() {
  local root="$1" engine="$2" host="$3" chain="$4" features="$5" c ex
  printf 'framework-epoch\t%s\n' "$FRAMEWORK_EPOCH"
  printf 'host\t%s\n' "$(file_sha_or_dash "$host")"
  printf 'shader-chain\t%s\n' "${chain:--}"
  printf 'cargo-features\t%s\n' "${features:--}"
  for c in $FRAMEWORK_CRATES; do
    if [ -d "$root/crates/$c" ]; then printf 'crate:%s\t%s\n' "$c" "$(crate_tree_hash "$root/crates/$c")"; else printf 'crate:%s\t-\n' "$c"; fi
  done
  ex="$(printf '%s\n' $FRAMEWORK_PIPELINE_EXCLUDED)"
  printf 'ibuilder-pipeline\t%s\n' "$( (cd "$root/crates/ibuilder-pipeline" && { find src -type f | grep -vxF "$ex"; ls Cargo.toml build.rs 2>/dev/null; } | LC_ALL=C sort | hash_listing) | shasum -a 256 | cut -d' ' -f1)"
  printf 'ibuilder-cli-adapters\t%s\n' "$( (cd "$root/crates/ibuilder-cli" && ls src/adapters.rs Cargo.toml 2>/dev/null | LC_ALL=C sort | hash_listing) | shasum -a 256 | cut -d' ' -f1)"
  printf 'cargo-lock\t%s\n' "$(file_sha_or_dash "$root/Cargo.lock")"
  if [ -d "$root/app/game-tables" ]; then printf 'app-game-tables\t%s\n' "$(dir_tree_hash "$root/app/game-tables")"; else printf 'app-game-tables\t-\n'; fi
  printf 'engine-settings\t%s\n' "$(file_sha_or_dash "$engine/dm/games/settings.tsv")"
  if [ -d "$engine/dm/games" ]; then
    printf 'engine-game-tables\t%s\n' "$( (cd "$engine/dm/games" && find . -mindepth 2 -maxdepth 2 -type f -name game.tsv | LC_ALL=C sort | hash_listing) | shasum -a 256 | cut -d' ' -f1)"
  else printf 'engine-game-tables\t-\n'; fi
  if [ -d "$engine/dm" ]; then
    printf 'engine-host-sources\t%s\n' "$( (cd "$engine/dm" && ls host/*.c gfx/*.c gfx/nvn/*.c gfx/vk/*.m 2>/dev/null | LC_ALL=C sort | hash_listing) | shasum -a 256 | cut -d' ' -f1)"
  else printf 'engine-host-sources\t-\n'; fi
  printf 'extractor\t%s %s\n' "$(file_sha_or_dash "$engine/dm/tools/nx_extract/Program.cs")" "$(file_sha_or_dash "$root/app/patches/nx_extract_merged_update.py")"
}

framework_id_of() { shasum -a 256 | cut -c1-16; }   # stdin: a framework_id_listing

dir_tree_hash() {
  ( cd "$1" && find . -type f ! -name .DS_Store ! -name '._*' | LC_ALL=C sort | hash_listing 16 ) | shasum -a 256 | cut -d' ' -f1
}
