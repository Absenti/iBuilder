/* Host test (macOS) of app/c/nsz_decompress.c: crafted NSZ inputs, and the largest malloc each one asks for.
 * Build and run: app/c/test/run.sh (it compiles zstd's decoder from $ZSTD_SRC, this file, and runs it). */
#include <CommonCrypto/CommonCrypto.h>
#include <errno.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include "zstd.h"

/* allocation wrapper: records the largest request and can fail the Nth one */
static size_t g_max; static int g_count, g_fail_at;
static void *t_malloc(size_t n) { g_count++; if (n > g_max) g_max = n; if (g_fail_at && g_count == g_fail_at) return NULL; return malloc(n); }
static void *t_calloc(size_t a, size_t b) { g_count++; if (a * b > g_max) g_max = a * b; if (g_fail_at && g_count == g_fail_at) return NULL; return calloc(a, b); }
#define malloc t_malloc
#define calloc t_calloc
#include "../nsz_decompress.c"
#undef malloc
#undef calloc

static int g_bad;
#define CHECK(c, ...) do { if (!(c)) { g_bad++; printf("  FAIL: "); printf(__VA_ARGS__); printf("\n"); } } while (0)

static void p64(uint8_t *p, uint64_t v) { for (int i = 0; i < 8; i++) p[i] = (uint8_t) (v >> (8 * i)); }
static void p32(uint8_t *p, uint32_t v) { for (int i = 0; i < 4; i++) p[i] = (uint8_t) (v >> (8 * i)); }

/* An NSZ: PFS0 with one entry "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz.ncz" (not a content id: the hash check is skipped) holding
 * head(0x4000) NCZSECTN count {sections} body. */
static size_t build(uint8_t *f, uint64_t count, const uint64_t (*sec)[2], const uint8_t *body, size_t blen)
{
    const char *name = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz.ncz";
    size_t ssz = strlen(name) + 1, hdr = 16 + 0x18 + ssz;
    uint8_t *e = f + hdr;                                   /* the entry's data */
    memset(e, 0x5a, 0x4000);
    memcpy(e + 0x4000, "NCZSECTN", 8); p64(e + 0x4008, count);
    size_t o = 0x4010;
    for (uint64_t i = 0; i < count && sec; i++, o += 0x40) { memset(e + o, 0, 0x40); p64(e + o, sec[i][0]); p64(e + o + 8, sec[i][1]); /* crypto 0 */ }
    memcpy(e + o, body, blen);
    size_t esz = o + blen;
    memcpy(f, "PFS0", 4); p32(f + 4, 1); p32(f + 8, (uint32_t) ssz); p32(f + 12, 0);
    p64(f + 16, 0); p64(f + 24, esz); p32(f + 32, 0); p32(f + 36, 0);
    memcpy(f + 40, name, ssz);
    return hdr + esz;
}

static int run(const uint8_t *f, size_t n, char *err, size_t *out_size)
{
    const char *src = "/tmp/nsz_test_in.nsz", *dst = "/tmp/nsz_test_out.nsp";
    FILE *fp = fopen(src, "wb"); fwrite(f, 1, n, fp); fclose(fp);
    unlink(dst); err[0] = 0; g_max = 0; g_count = 0;
    int rc = gameapp_nsz_to_nsp(src, dst, err, 512, NULL);
    struct stat st; *out_size = stat(dst, &st) == 0 ? (size_t) st.st_size : 0;
    return rc;
}

/* zstd -19 --no-check frames of gen_data(4096) and gen_data(4196) (one Compressed_Block each), made once with the zstd CLI (review r5 A2) */
static const unsigned char blk0_zst[] = {
  0x28, 0xb5, 0x2f, 0xfd, 0x60, 0x00, 0x0f, 0x65, 0x06, 0x00, 0x02, 0x0a,
  0x25, 0x20, 0x10, 0xb3, 0x07, 0xff, 0x6f, 0x40, 0xdb, 0xb6, 0x49, 0x92,
  0x24, 0x49, 0x92, 0x14, 0x40, 0x3d, 0x28, 0x54, 0x55, 0x55, 0x3d, 0x00,
  0x00, 0xd8, 0x6d, 0xdb, 0x3e, 0x00, 0x00, 0x00, 0x28, 0x15, 0x5b, 0xf6,
  0x70, 0x57, 0x85, 0x90, 0xb8, 0x38, 0xc5, 0xbb, 0x5e, 0xd4, 0x6f, 0xf2,
  0x5b, 0x94, 0x02, 0xc2, 0xea, 0x35, 0x6b, 0x6a, 0xd5, 0xaf, 0x4d, 0xaf,
  0xe1, 0x4f, 0x57, 0x79, 0xab, 0xf3, 0xad, 0x2c, 0xcf, 0x60, 0xcf, 0x36,
  0xc7, 0xab, 0x2e, 0x86, 0x05, 0xb6, 0xd4, 0xed, 0x58, 0xe4, 0x98, 0xee,
  0x7c, 0x94, 0xe5, 0xa0, 0xa0, 0xfe, 0xdd, 0xfa, 0xb3, 0xfb, 0xeb, 0x1a,
  0x6c, 0x8a, 0x00, 0x1a, 0x13, 0x86, 0xfc, 0xc7, 0x09, 0x7a, 0x7c, 0x93,
  0x49, 0x90, 0x00, 0x43, 0xca, 0xb1, 0x7b, 0x28, 0xc5, 0xed, 0x5d, 0x1d,
  0x4d, 0x04, 0x8f, 0x48, 0x4b, 0xdf, 0x67, 0x24, 0x71, 0x47, 0x73, 0xf7,
  0x31, 0x1a, 0xb4, 0xe7, 0xe7, 0x12, 0x9a, 0x99, 0xab, 0x98, 0xf3, 0x99,
  0x0d, 0x92, 0x0c, 0x1c, 0x01, 0x49, 0xa8, 0xa1, 0xe7, 0xff, 0x1e, 0xe0,
  0xa5, 0x69, 0x0d, 0x12, 0xf8, 0xbf, 0x5c, 0xf0, 0xbf, 0xfd, 0xde, 0x01,
  0xfd, 0x0d, 0x68, 0x97, 0x90, 0xd1, 0x55, 0x74, 0xd2, 0x59, 0xe4, 0x0a,
  0x35, 0x4b, 0xc4, 0x28, 0x91, 0x4b, 0x67, 0xa2, 0xab, 0x6a, 0x9c, 0xe8,
  0x54, 0xf4, 0x3a, 0x89, 0xbc, 0xde, 0xe9, 0x48, 0x3b, 0x0d
};
static const unsigned int blk0_zst_len = 214;
static const unsigned char stream_zst[] = {
  0x28, 0xb5, 0x2f, 0xfd, 0x60, 0x64, 0x0f, 0x65, 0x06, 0x00, 0x02, 0x0a,
  0x25, 0x20, 0x10, 0xb3, 0x07, 0xff, 0x6f, 0x40, 0xdb, 0xb6, 0x49, 0x92,
  0x24, 0x49, 0x92, 0x14, 0x40, 0x3d, 0x28, 0x54, 0x55, 0x55, 0x3d, 0x00,
  0x00, 0xd8, 0x6d, 0xdb, 0x3e, 0x00, 0x00, 0x00, 0x28, 0x15, 0x5b, 0xf6,
  0x70, 0x57, 0x85, 0x90, 0xb8, 0x38, 0xc5, 0xbb, 0x5e, 0xd4, 0x6f, 0xf2,
  0x5b, 0x94, 0x02, 0xc2, 0xea, 0x35, 0x6b, 0x6a, 0xd5, 0xaf, 0x4d, 0xaf,
  0xe1, 0x4f, 0x57, 0x79, 0xab, 0xf3, 0xad, 0x2c, 0xcf, 0x60, 0xcf, 0x36,
  0xc7, 0xab, 0x2e, 0x86, 0x05, 0xb6, 0xd4, 0xed, 0x58, 0xe4, 0x98, 0xee,
  0x7c, 0x94, 0xe5, 0xa0, 0xa0, 0xfe, 0xdd, 0xfa, 0xb3, 0xfb, 0xeb, 0x1a,
  0x6c, 0x8a, 0x00, 0x1a, 0x13, 0x86, 0xfc, 0xc7, 0x09, 0x7a, 0x7c, 0x93,
  0x49, 0x90, 0x00, 0x43, 0xca, 0xb1, 0x7b, 0x28, 0xc5, 0xed, 0x5d, 0x1d,
  0x4d, 0x04, 0x8f, 0x48, 0x4b, 0xdf, 0x67, 0x24, 0x71, 0x47, 0x73, 0xf7,
  0x31, 0x1a, 0xb4, 0xe7, 0xe7, 0x12, 0x9a, 0x99, 0xab, 0x98, 0xf3, 0x99,
  0x0d, 0x92, 0x0c, 0x1c, 0x01, 0x49, 0xa8, 0xa1, 0xe7, 0xff, 0x1e, 0xe0,
  0xa5, 0x69, 0x0d, 0x12, 0xf8, 0xbf, 0x5c, 0xf0, 0xbf, 0xfd, 0xde, 0x01,
  0x61, 0x0e, 0x68, 0x97, 0x90, 0xd1, 0x55, 0x74, 0xd2, 0x59, 0xe4, 0x0a,
  0x35, 0x4b, 0xc4, 0x28, 0x91, 0x4b, 0x67, 0xa2, 0xab, 0x6a, 0x9c, 0xe8,
  0x54, 0xf4, 0x3a, 0x89, 0xbc, 0xde, 0xe9, 0x48, 0x3b, 0x0d
};
static const unsigned int stream_zst_len = 214;

/* the plaintext those frames hold: compressible, not a single repeated byte */
static void gen_data(uint8_t *d, size_t n) { static const char pat[] = "NCZ synthetic block "; for (size_t i = 0; i < n; i++) d[i] = (uint8_t) (pat[i % 20] ^ ((i / 64) & 7)); }

/* The output .nsp's one NCA, after its PFS0 header: head (0x5a x 0x4000) then the decompressed body must equal `want` (crypto type 0: nothing encrypted again). */
static int nca_body_is(const uint8_t *want, size_t n)
{
    FILE *fp = fopen("/tmp/nsz_test_out.nsp", "rb"); if (!fp) return 0;
    static uint8_t o[1 << 16]; size_t got = fread(o, 1, sizeof o, fp); fclose(fp);
    if (got < 0x4000 + n) return 0;
    size_t nca = got - (0x4000 + n);                       /* the NCA is the last entry: header, then head, then body */
    for (size_t i = 0; i < 0x4000; i++) if (o[nca + i] != 0x5a) return 0;
    return memcmp(o + nca + 0x4000, want, n) == 0;
}

static uint8_t *F; static char err[512];
int main(void)
{
    F = calloc(1, 1 << 20); size_t out;
    uint8_t body[1024];

    puts("1. valid NCZBLOCK, one stored block (2^28 declared, 10 bytes of data): accepted, small allocations");
    { uint64_t sec[1][2] = { { 0x4000, 10 } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[8] = 2; body[9] = 1; body[11] = 28; p32(body + 12, 1); p64(body + 16, 10); p32(body + 24, 10);
      memcpy(body + 28, "0123456789", 10);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 38);
      int rc = run(F, n, err, &out);
      CHECK(rc == 0, "rc %d (%s)", rc, err); CHECK(g_max < (1u << 20), "largest malloc %zu", g_max);
      printf("  rc %d, largest malloc %zu bytes\n", rc, g_max);
      FILE *fp = fopen("/tmp/nsz_test_out.nsp", "rb"); uint8_t o[8192]; size_t got = fp ? fread(o, 1, sizeof o, fp) : 0; if (fp) fclose(fp);
      /* header: 16 + 24 + names padded to 0x20; the NCA follows: head (0x5a x 0x4000) then the 10 bytes */
      size_t total = out; CHECK(total >= 0x4000 + 10, "output %zu", total); (void) got; }

    puts("2. section offset + size overflows u64: refused");
    { uint64_t sec[1][2] = { { 0xFFFFFFFFFFFFFFF0ull, 0x20 } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[11] = 12; p32(body + 12, 1); p64(body + 16, 4); p32(body + 24, 4); memcpy(body + 28, "abcd", 4);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 32);
      int rc = run(F, n, err, &out);
      CHECK(rc != 0 && strstr(err, "overflows"), "rc %d (%s)", rc, err); CHECK(out == 0, "a file was left behind");
      printf("  rc %d: %s\n", rc, err); }

    puts("3. section ending exactly at u64 max, not overflowing (offset 2^64-0x20, size 0x1f): not refused as an overflow");
    { uint64_t sec[1][2] = { { 0xFFFFFFFFFFFFFFE0ull, 0x1F } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[11] = 12; p32(body + 12, 1); p64(body + 16, 4); p32(body + 24, 4); memcpy(body + 28, "abcd", 4);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 32);
      int rc = run(F, n, err, &out);
      CHECK(rc != 0 && !strstr(err, "overflows"), "rc %d (%s)", rc, err);   /* it fails later: the body is 4 bytes, not 2^64 */
      printf("  rc %d: %s\n", rc, err); }

    puts("4. NCZBLOCK 2^28, 0x7fffffff blocks, a tiny entry: refused before any large allocation");
    { uint64_t sec[1][2] = { { 0x4000, 10 } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[11] = 28; p32(body + 12, 0x7fffffffu); p64(body + 16, 10);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 40);
      int rc = run(F, n, err, &out);
      CHECK(rc != 0, "accepted"); CHECK(g_max < (1u << 20), "largest malloc %zu", g_max);
      printf("  rc %d: %s; largest malloc %zu\n", rc, err, g_max); }

    puts("5. NCZBLOCK dsize inconsistent with the block count: refused before allocation");
    { uint64_t sec[1][2] = { { 0x4000, 10 } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[11] = 12; p32(body + 12, 1); p64(body + 16, 1ull << 40); p32(body + 24, 4);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 32);
      int rc = run(F, n, err, &out);
      CHECK(rc != 0 && g_max < (1u << 20), "rc %d, largest malloc %zu", rc, g_max);
      printf("  rc %d: %s; largest malloc %zu\n", rc, err, g_max); }

    puts("6. NCZBLOCK block sizes that run past the entry: refused before allocation");
    { uint64_t sec[1][2] = { { 0x4000, 10 } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[11] = 28; p32(body + 12, 1); p64(body + 16, 1u << 27); p32(body + 24, 1u << 27);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 32);
      int rc = run(F, n, err, &out);
      CHECK(rc != 0 && g_max < (1u << 20), "rc %d, largest malloc %zu", rc, g_max);
      printf("  rc %d: %s; largest malloc %zu\n", rc, err, g_max); }

    puts("7. allocation failure at every malloc/calloc of the valid case: a clean refusal each time (no crash, no output file)");
    { uint64_t sec[1][2] = { { 0x4000, 10 } };
      memset(body, 0, sizeof body); memcpy(body, "NCZBLOCK", 8); body[8] = 2; body[9] = 1; body[11] = 28; p32(body + 12, 1); p64(body + 16, 10); p32(body + 24, 10);
      memcpy(body + 28, "0123456789", 10);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, body, 38);
      run(F, n, err, &out); int total = g_count; printf("  %d allocations in the valid run\n", total);
      for (int k = 1; k <= total; k++) {
          g_fail_at = k; int rc = run(F, n, err, &out); g_fail_at = 0;
          CHECK(rc != 0 && out == 0, "failing allocation %d: rc %d out %zu (%s)", k, rc, out, err);
      } }

    puts("8. NCZBLOCK with a COMPRESSED block (review r5 A2): 2^12 blocks, block 0 a zstd frame (214 -> 4096 bytes), block 1 stored (100 bytes): rc 0, the body exact, small allocations");
    { static uint8_t plain[4196]; gen_data(plain, sizeof plain);
      uint64_t sec[1][2] = { { 0x4000, sizeof plain } };
      static uint8_t b[8192]; memset(b, 0, sizeof b);
      memcpy(b, "NCZBLOCK", 8); b[8] = 2; b[9] = 1; b[11] = 12; p32(b + 12, 2); p64(b + 16, sizeof plain);
      p32(b + 24, blk0_zst_len); p32(b + 28, 100);
      memcpy(b + 32, blk0_zst, blk0_zst_len); memcpy(b + 32 + blk0_zst_len, plain + 4096, 100);
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, b, 32 + blk0_zst_len + 100);
      int rc = run(F, n, err, &out);
      CHECK(rc == 0, "rc %d (%s)", rc, err); CHECK(nca_body_is(plain, sizeof plain), "the decompressed body differs");
      CHECK(g_max <= (1u << 14), "largest malloc %zu", g_max);
      printf("  rc %d, output %zu bytes, largest malloc %zu\n", rc, out, g_max);
      /* one byte of the frame damaged: refused, no output left behind */
      b[32 + 40] ^= 0xff; n = build(F, 1, (const uint64_t (*)[2]) sec, b, 32 + blk0_zst_len + 100);
      rc = run(F, n, err, &out);
      CHECK(rc != 0 && out == 0, "a damaged frame: rc %d, output %zu", rc, out); printf("  damaged frame: rc %d: %s\n", rc, err); }

    puts("9. one zstd stream (no NCZBLOCK): 214 -> 4196 bytes, rc 0, the body exact");
    { static uint8_t plain[4196]; gen_data(plain, sizeof plain);
      uint64_t sec[1][2] = { { 0x4000, sizeof plain } };
      size_t n = build(F, 1, (const uint64_t (*)[2]) sec, stream_zst, stream_zst_len);
      int rc = run(F, n, err, &out);
      CHECK(rc == 0, "rc %d (%s)", rc, err); CHECK(nca_body_is(plain, sizeof plain), "the decompressed body differs");
      printf("  rc %d, output %zu bytes\n", rc, out); }

    printf("%s (%d failures)\n", g_bad ? "FAILED" : "ALL OK", g_bad);
    return g_bad != 0;
}
