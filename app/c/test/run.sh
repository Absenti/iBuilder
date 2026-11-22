#!/bin/sh
# Host tests of app/c (macOS). ZSTD_SRC = a zstd lib/ tree; SPVC_INC = SPIRV-Cross's include dir (spirv_cross_c.h); OUT = a scratch folder.
# A compile error is a failure of this script (review r5 M9): the old binaries are removed first, and nothing hides the compiler's exit status, so a stale binary can never print "ALL OK".
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
: "${ZSTD_SRC:?set ZSTD_SRC to a zstd lib/ folder}" "${SPVC_INC:?set SPVC_INC to the folder of spirv_cross_c.h}" "${OUT:?set OUT to a scratch folder}"
mkdir -p "$OUT"
rm -f "$OUT/nsz_test" "$OUT/spvc_test"
for f in "$ZSTD_SRC"/common/debug.c "$ZSTD_SRC"/common/entropy_common.c "$ZSTD_SRC"/common/error_private.c "$ZSTD_SRC"/common/fse_decompress.c \
         "$ZSTD_SRC"/common/xxhash.c "$ZSTD_SRC"/common/zstd_common.c "$ZSTD_SRC"/decompress/*.c; do
  clang -O1 -DZSTD_DISABLE_ASM=1 -DZSTD_MULTITHREAD=0 -DZSTD_LEGACY_SUPPORT=0 -I"$ZSTD_SRC" -I"$ZSTD_SRC/common" -c "$f" -o "$OUT/$(basename "$f" .c).o"
done
clang -O1 -g -Wall -I"$ZSTD_SRC" "$HERE/nsz_decompress_test.c" "$OUT"/*.o -o "$OUT/nsz_test"
clang -O1 -g -Wall -I"$SPVC_INC" "$HERE/spvc_cli_test.c" -Wl,-undefined,dynamic_lookup -o "$OUT/spvc_test"
[ -x "$OUT/nsz_test" ] && [ -x "$OUT/spvc_test" ] || { echo "run.sh: a test binary was not built" >&2; exit 1; }
# (nsz_decompress_test.c still writes /tmp/nsz_test_* by literal path: not changed here)
TMPDIR="$OUT" "$OUT/nsz_test"; TMPDIR="$OUT" "$OUT/spvc_test"
