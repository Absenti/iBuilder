/* gameapp/stdzstd.c: a STANDARD zstd stream decoder, beside the app's Nintendo one.
 *
 * The zstd the app links (libpyext_static.a's zbic, compiled with ZSTD_ZBIC_SUPPORT=1) reads Nintendo's ZBIC format:
 * another magic number and other entropy tables. It refuses a standard zstd frame ("Unknown frame descriptor"), which
 * is what a .nup container (Switch 2 games, gameapp/nup_zstd.py) holds. This file and zstd's own decoder sources are
 * linked into ONE relocatable object whose only external symbols are the three below (tools/build_stdzstd.sh: ld -r
 * with an export list), so the two zstd's never meet at link time. */
#include <stdlib.h>
#include <string.h>
#include "zstd.h"

typedef struct { ZSTD_DStream *ds; int eof; } stdzstd_stream;

void *gameapp_stdzstd_new(void)
{
    stdzstd_stream *s = calloc(1, sizeof *s);
    if (!s) return NULL;
    s->ds = ZSTD_createDStream();
    if (!s->ds || ZSTD_isError(ZSTD_initDStream(s->ds))) { if (s->ds) ZSTD_freeDStream(s->ds); free(s); return NULL; }
    return s;
}

void gameapp_stdzstd_free(void *p)
{
    stdzstd_stream *s = p;
    if (!s) return;
    ZSTD_freeDStream(s->ds);
    free(s);
}

/* Decompresses `in` (len bytes) into *out (malloc'ed, the caller frees), *out_len its size; *eof 1 once the frame ended.
 * Returns NULL, or the zstd error's name. */
const char *gameapp_stdzstd_feed(void *p, const void *in, size_t len, char **out, size_t *out_len, int *eof)
{
    stdzstd_stream *s = p;
    size_t step = ZSTD_DStreamOutSize(), cap = len * 4 + step, used = 0;
    char *buf = malloc(cap);
    *out = NULL; *out_len = 0; *eof = s->eof;
    if (!buf) return "zstd: no memory";
    ZSTD_inBuffer zin = { in, len, 0 };
    while (!s->eof) {
        if (cap - used < step) {
            char *nb = realloc(buf, cap * 2);
            if (!nb) { free(buf); return "zstd: no memory"; }
            buf = nb; cap *= 2;
        }
        ZSTD_outBuffer zout = { buf + used, cap - used, 0 };
        size_t r = ZSTD_decompressStream(s->ds, &zout, &zin);
        if (ZSTD_isError(r)) { free(buf); return ZSTD_getErrorName(r); }
        used += zout.pos;
        if (r == 0) { s->eof = 1; break; }
        if (zin.pos == zin.size && zout.pos < zout.size) break;
    }
    *out = buf; *out_len = used; *eof = s->eof;
    return NULL;
}
