/* gameapp/nsz_decompress.c -- an NSZ (a compressed NSP) back into the NSP it was made from, on the phone.
 *
 * The engine's extractor (nx_extract, LibHac) reads NSP and XCI; an NSZ it can only list (docs/GAME-APP.md §8.5). On a
 * Mac the engine's NSP script runs the `nsz -D` tool first; there is no such tool on iOS, so this is the same
 * conversion, written from the NCZ layout that tool defines:
 *
 *   PFS0 container, entries as in the NSP, except that a compressed NCA is stored as <content id>.ncz:
 *     0x0000  the NCA's first 0x4000 bytes, exactly as they were (header + section headers, still encrypted)
 *     0x4000  "NCZSECTN", u64 count, count x { u64 offset, u64 size, u64 crypto type, u64 pad, key[16], counter[16] }
 *     then    either one zstd stream, or "NCZBLOCK" { u8 version, u8 type, u8 unused, u8 block size exponent,
 *             u32 blocks, u64 decompressed size, u32 compressed size[blocks] } and the blocks (a block whose
 *             compressed size is not below its decompressed size is stored as it is)
 *   The decompressed bytes are the NCA from 0x4000 on, DECRYPTED: a section of crypto type 3 (AES-CTR) or 4 (BKTR)
 *   is encrypted again with its key and the counter nonce[0:8] || big-endian (offset >> 4), offset in the NCA.
 *
 * The result is checked the way the content system checks it: an NCA's file name is the first 16 bytes of its
 * SHA-256, so each rebuilt NCA is hashed and compared with its name. A mismatch is a failed conversion, not a warning.
 * zstd comes from the zstd the app already links (out-pydev's libpyext_static.a); AES and SHA-256 from CommonCrypto. */
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

typedef void (*nsz_log_fn)(const char *line);

typedef struct { uint64_t offset, size; uint32_t name_off; char *name; int ncz; } pfs_entry;
typedef struct { uint64_t offset, size, crypto; uint8_t key[16], counter[16]; } ncz_section;

typedef struct {
    FILE *out;
    ncz_section *sec; int nsec;
    uint64_t pos;                /* the NCA offset of the next byte written */
    CC_SHA256_CTX sha;
    CCCryptorRef ecb; int ecb_sec;   /* the AES-ECB context of the section whose key is loaded, -1 none */
    uint8_t *ks; size_t ks_cap;  /* keystream scratch */
    char *err; int errlen;
} nca_sink;

static int fail(char *err, int errlen, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt);
    if (err != NULL && errlen > 0) vsnprintf(err, (size_t) errlen, fmt, ap);
    va_end(ap);
    return -1;
}

static uint32_t le32(const uint8_t *p) { return (uint32_t) p[0] | (uint32_t) p[1] << 8 | (uint32_t) p[2] << 16 | (uint32_t) p[3] << 24; }
static uint64_t le64(const uint8_t *p) { return (uint64_t) le32(p) | (uint64_t) le32(p + 4) << 32; }
static void put32(uint8_t *p, uint32_t v) { for (int i = 0; i < 4; i++) p[i] = (uint8_t) (v >> (8 * i)); }
static void put64(uint8_t *p, uint64_t v) { for (int i = 0; i < 8; i++) p[i] = (uint8_t) (v >> (8 * i)); }

static int read_at(FILE *f, uint64_t off, void *buf, size_t n)
{
    if (fseeko(f, (off_t) off, SEEK_SET) != 0) return -1;
    return fread(buf, 1, n, f) == n ? 0 : -1;
}

/* Encrypt (CTR is symmetric) len bytes at NCA offset pos with section s's key. */
static int ctr_apply(nca_sink *k, int s, uint8_t *data, size_t len, uint64_t pos)
{
    if (k->ecb_sec != s) {
        if (k->ecb != NULL) CCCryptorRelease(k->ecb);
        k->ecb = NULL;
        if (CCCryptorCreate(kCCEncrypt, kCCAlgorithmAES, kCCOptionECBMode, k->sec[s].key, 16, NULL, &k->ecb) != kCCSuccess)
            return fail(k->err, k->errlen, "AES context refused");
        k->ecb_sec = s;
    }
    size_t skip = (size_t) (pos & 15), blocks = (skip + len + 15) / 16, need = blocks * 16;
    if (need > k->ks_cap) {
        uint8_t *n = realloc(k->ks, need);
        if (n == NULL) return fail(k->err, k->errlen, "no memory for the keystream");
        k->ks = n; k->ks_cap = need;
    }
    uint64_t ctr = pos >> 4;
    for (size_t b = 0; b < blocks; b++) {
        uint8_t *c = k->ks + 16 * b;
        memcpy(c, k->sec[s].counter, 8);
        uint64_t v = ctr + b;
        for (int i = 0; i < 8; i++) c[15 - i] = (uint8_t) (v >> (8 * i));
    }
    size_t moved = 0;
    if (CCCryptorUpdate(k->ecb, k->ks, need, k->ks, need, &moved) != kCCSuccess || moved != need)
        return fail(k->err, k->errlen, "AES-ECB keystream failed");
    for (size_t i = 0; i < len; i++) data[i] ^= k->ks[skip + i];
    return 0;
}

/* Decompressed (decrypted) NCA bytes from k->pos on: encrypt what lies in an encrypted section, write, hash. */
static int sink_put(nca_sink *k, uint8_t *data, size_t len)
{
    size_t done = 0;
    while (done < len) {
        uint64_t pos = k->pos;
        int s = -1; uint64_t end = UINT64_MAX;
        for (int i = 0; i < k->nsec; i++) {
            if (pos >= k->sec[i].offset && pos < k->sec[i].offset + k->sec[i].size) { s = i; end = k->sec[i].offset + k->sec[i].size; break; }
            if (k->sec[i].offset > pos && k->sec[i].offset < end) end = k->sec[i].offset;   /* a gap: plain up to the next section */
        }
        size_t n = len - done;
        if (end != UINT64_MAX && end - pos < n) n = (size_t) (end - pos);
        if (s >= 0 && (k->sec[s].crypto == 3 || k->sec[s].crypto == 4))
            if (ctr_apply(k, s, data + done, n, pos) != 0) return -1;
        if (fwrite(data + done, 1, n, k->out) != n) return fail(k->err, k->errlen, "write failed (%s): is the disk full?", strerror(errno));
        CC_SHA256_Update(&k->sha, data + done, (CC_LONG) n);
        k->pos += n; done += n;
    }
    return 0;
}

static int sec_cmp(const void *a, const void *b)
{
    const ncz_section *x = a, *y = b;
    return x->offset < y->offset ? -1 : x->offset > y->offset;
}

/* One .ncz entry (at base, csize bytes in the source) -> its NCA, appended to out. nca_size gets the NCA's size and
 * digest its SHA-256. */
static int ncz_to_nca(FILE *in, uint64_t base, uint64_t csize, FILE *out, uint64_t *nca_size, uint8_t digest[32],
                      char *err, int errlen)
{
    uint8_t *head = malloc(0x4000);
    if (head == NULL) return fail(err, errlen, "no memory");
    nca_sink k = { .out = out, .ecb_sec = -1, .err = err, .errlen = errlen };
    ZSTD_DStream *zd = NULL; uint8_t *ibuf = NULL, *obuf = NULL; int rc = -1;
    CC_SHA256_Init(&k.sha);
    if (read_at(in, base, head, 0x4000) != 0) { fail(err, errlen, "short NCZ header"); goto out; }
    if (fwrite(head, 1, 0x4000, out) != 0x4000) { fail(err, errlen, "write failed (%s)", strerror(errno)); goto out; }
    CC_SHA256_Update(&k.sha, head, 0x4000);
    k.pos = 0x4000;

    uint8_t m[16];
    if (read_at(in, base + 0x4000, m, 16) != 0 || memcmp(m, "NCZSECTN", 8) != 0) { fail(err, errlen, "no NCZSECTN: not an NCZ"); goto out; }
    uint64_t count = le64(m + 8);
    if (count == 0 || count > 100000) {   /* an update's BKTR sections come one per counter: 709 measured */ fail(err, errlen, "NCZ with %llu sections", (unsigned long long) count); goto out; }
    k.sec = calloc((size_t) count, sizeof *k.sec); k.nsec = (int) count;
    if (k.sec == NULL) { fail(err, errlen, "no memory"); goto out; }
    uint64_t total = 0x4000;
    for (uint64_t i = 0; i < count; i++) {
        uint8_t e[0x40];
        if (read_at(in, base + 0x4010 + 0x40 * i, e, 0x40) != 0) { fail(err, errlen, "short NCZ section table"); goto out; }
        k.sec[i].offset = le64(e); k.sec[i].size = le64(e + 8); k.sec[i].crypto = le64(e + 16);
        memcpy(k.sec[i].key, e + 0x20, 16); memcpy(k.sec[i].counter, e + 0x30, 16);
        /* offset and size are the file's words: their sum is checked before it is used here (total) and in sink_put/ctr_apply */
        if (k.sec[i].size > UINT64_MAX - k.sec[i].offset) { fail(err, errlen, "NCZ section %llu: offset + size overflows", (unsigned long long) i); goto out; }
        if (k.sec[i].offset + k.sec[i].size > total) total = k.sec[i].offset + k.sec[i].size;
    }
    qsort(k.sec, (size_t) count, sizeof *k.sec, sec_cmp);
    uint64_t data = base + 0x4010 + 0x40 * count, data_end = base + csize;
    uint8_t bm[24];
    if (read_at(in, data, bm, 8) != 0) { fail(err, errlen, "short NCZ body"); goto out; }

    if (memcmp(bm, "NCZBLOCK", 8) == 0) {
        if (read_at(in, data, bm, 24) != 0) { fail(err, errlen, "short NCZBLOCK header"); goto out; }
        unsigned exp = bm[11]; uint32_t nblocks = le32(bm + 12); uint64_t dsize = le64(bm + 16);
        if (exp < 12 || exp > 28 || nblocks == 0) { fail(err, errlen, "NCZBLOCK block size 2^%u, %u blocks", exp, nblocks); goto out; }
        size_t bsize = (size_t) 1 << exp;
        /* Everything the header says is checked against the entry's own length and against itself BEFORE anything is
         * allocated: the size list must fit in the entry, the blocks must be as many as the decompressed size needs, and the
         * buffers are sized by the blocks actually present, not by 2^exp. */
        uint64_t avail = data_end > data ? data_end - data : 0;
        if (avail < 24 || (uint64_t) nblocks > (avail - 24) / 4) { fail(err, errlen, "NCZBLOCK: %u block sizes do not fit in the entry", nblocks); goto out; }
        if (dsize < (uint64_t) bsize * (nblocks - 1) || dsize - (uint64_t) bsize * (nblocks - 1) > bsize) { fail(err, errlen, "NCZBLOCK: %u blocks of 2^%u do not make %llu bytes", nblocks, exp, (unsigned long long) dsize); goto out; }
        uint32_t *sizes = malloc(4 * (size_t) nblocks);
        uint8_t *raw = malloc(4 * (size_t) nblocks);
        if (sizes == NULL || raw == NULL || read_at(in, data + 24, raw, 4 * (size_t) nblocks) != 0) {
            free(sizes); free(raw); fail(err, errlen, "short NCZBLOCK size list"); goto out;
        }
        uint64_t stored = 0; size_t maxin = 0; int any_compressed = 0;
        for (uint32_t i = 0; i < nblocks; i++) {
            sizes[i] = le32(raw + 4 * i);
            size_t w = i + 1 < nblocks ? bsize : (size_t) (dsize - (uint64_t) bsize * (nblocks - 1));
            if (sizes[i] > bsize) { free(sizes); free(raw); fail(err, errlen, "NCZBLOCK block %u out of range", i); goto out; }
            if (sizes[i] > maxin) maxin = sizes[i];
            if (sizes[i] < w) any_compressed = 1;
            stored += sizes[i];
        }
        free(raw);
        if (stored > avail - 24 - 4 * (uint64_t) nblocks) { free(sizes); fail(err, errlen, "NCZBLOCK blocks run past the end of the entry"); goto out; }
        size_t isize = maxin > 0 ? maxin : 1;
        size_t osize = any_compressed ? (dsize < bsize ? (size_t) dsize : bsize) : 1;
        ibuf = malloc(isize); obuf = malloc(osize);
        if (ibuf == NULL || obuf == NULL) { free(sizes); fail(err, errlen, "no memory for the blocks"); goto out; }
        uint64_t at = data + 24 + 4 * (uint64_t) nblocks;
        for (uint32_t i = 0; i < nblocks; i++) {
            size_t want = i + 1 < nblocks ? bsize : (size_t) (dsize - (uint64_t) bsize * (nblocks - 1));
            if (want > bsize || sizes[i] > bsize) { free(sizes); fail(err, errlen, "NCZBLOCK block %u out of range", i); goto out; }
            if (read_at(in, at, ibuf, sizes[i]) != 0) { free(sizes); fail(err, errlen, "short NCZ block %u", i); goto out; }
            at += sizes[i];
            uint8_t *src = ibuf;
            if (sizes[i] < want) {
                size_t got = ZSTD_decompress(obuf, osize, ibuf, sizes[i]);
                if (ZSTD_isError(got) || got != want) { free(sizes); fail(err, errlen, "zstd block %u: %s", i, ZSTD_isError(got) ? ZSTD_getErrorName(got) : "wrong size"); goto out; }
                src = obuf;
            }
            if (sink_put(&k, src, want) != 0) { free(sizes); goto out; }
        }
        free(sizes);
    } else {
        zd = ZSTD_createDStream();
        size_t isz = ZSTD_DStreamInSize(), osz = ZSTD_DStreamOutSize();
        ibuf = malloc(isz); obuf = malloc(osz);
        if (zd == NULL || ibuf == NULL || obuf == NULL) { fail(err, errlen, "no memory for the zstd stream"); goto out; }
        ZSTD_initDStream(zd);
        uint64_t at = data;
        if (fseeko(in, (off_t) at, SEEK_SET) != 0) { fail(err, errlen, "seek failed"); goto out; }
        size_t last = 1;
        while (at < data_end && k.pos < total) {
            size_t want = (size_t) (data_end - at < isz ? data_end - at : isz);
            size_t got = fread(ibuf, 1, want, in);
            if (got == 0) { fail(err, errlen, "short NCZ stream"); goto out; }
            at += got;
            ZSTD_inBuffer zin = { ibuf, got, 0 };
            while (zin.pos < zin.size) {
                ZSTD_outBuffer zout = { obuf, osz, 0 };
                last = ZSTD_decompressStream(zd, &zout, &zin);
                if (ZSTD_isError(last)) { fail(err, errlen, "zstd: %s", ZSTD_getErrorName(last)); goto out; }
                if (zout.pos > 0 && sink_put(&k, obuf, zout.pos) != 0) goto out;
            }
        }
        for (;;) {   /* flush what the decoder still holds */
            ZSTD_inBuffer zin = { NULL, 0, 0 };
            ZSTD_outBuffer zout = { obuf, osz, 0 };
            last = ZSTD_decompressStream(zd, &zout, &zin);
            if (ZSTD_isError(last)) { fail(err, errlen, "zstd: %s", ZSTD_getErrorName(last)); goto out; }
            if (zout.pos == 0) break;
            if (sink_put(&k, obuf, zout.pos) != 0) goto out;
        }
    }
    if (k.pos != total) { fail(err, errlen, "NCA rebuilt to %llu bytes, its sections say %llu", (unsigned long long) k.pos, (unsigned long long) total); goto out; }
    CC_SHA256_Final(digest, &k.sha);
    *nca_size = total;
    rc = 0;
out:
    if (zd != NULL) ZSTD_freeDStream(zd);
    if (k.ecb != NULL) CCCryptorRelease(k.ecb);
    free(k.ks); free(k.sec); free(ibuf); free(obuf); free(head);
    return rc;
}

static int has_suffix(const char *s, const char *suf)
{
    size_t a = strlen(s), b = strlen(suf);
    return a >= b && strcasecmp(s + a - b, suf) == 0;
}

static int hexval(char c) { return c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1; }

/* 1: the NCA's name is the first 16 bytes of the digest; 0: it is not; -1: the name is not a content id. */
static int name_matches(const char *name, const uint8_t digest[32])
{
    for (int i = 0; i < 16; i++) {
        int h = hexval(name[2 * i]), l = hexval(name[2 * i + 1]);
        if (h < 0 || l < 0) return -1;
        if (((h << 4) | l) != digest[i]) return 0;
    }
    return 1;
}

/* src: an .nsz; dst: the .nsp to write (written to dst + ".part" and renamed on success). Returns 0, or -1 with err. */
int gameapp_nsz_to_nsp(const char *src, const char *dst, char *err, int errlen, nsz_log_fn log)
{
    char line[512], part[4096] = "";
    FILE *in = fopen(src, "rb"), *out = NULL;
    pfs_entry *e = NULL; char *strs = NULL; uint8_t *hdr = NULL; int rc = -1; uint32_t n = 0;
    if (in == NULL) return fail(err, errlen, "cannot open %s: %s", src, strerror(errno));
    uint8_t h[16];
    if (read_at(in, 0, h, 16) != 0 || memcmp(h, "PFS0", 4) != 0) { fail(err, errlen, "not a PFS0 container (an NSZ is one)"); goto done; }
    n = le32(h + 4); uint32_t ssz = le32(h + 8);
    if (n == 0 || n > 4096 || ssz > (1u << 20)) { fail(err, errlen, "PFS0 with %u entries, %u bytes of names", n, ssz); goto done; }
    e = calloc(n, sizeof *e); strs = calloc(1, ssz + 1);
    uint8_t *raw = malloc(0x18 * (size_t) n);
    if (e == NULL || strs == NULL || raw == NULL || read_at(in, 16, raw, 0x18 * (size_t) n) != 0 || read_at(in, 16 + 0x18 * (uint64_t) n, strs, ssz) != 0) {
        free(raw); fail(err, errlen, "short PFS0 header"); goto done;
    }
    for (uint32_t i = 0; i < n; i++) {
        e[i].offset = le64(raw + 0x18 * i); e[i].size = le64(raw + 0x18 * i + 8); e[i].name_off = le32(raw + 0x18 * i + 16);
        if (e[i].name_off >= ssz) { free(raw); fail(err, errlen, "PFS0 entry %u has no name", i); goto done; }
        e[i].name = strdup(strs + e[i].name_off);
        e[i].ncz = has_suffix(e[i].name, ".ncz");
        if (e[i].ncz) memcpy(e[i].name + strlen(e[i].name) - 4, ".nca", 4);
    }
    free(raw);
    uint64_t data0 = 16 + 0x18 * (uint64_t) n + ssz;
    /* Every entry's offset and size are the file's words: they are compared with the file's own length BEFORE anything is allocated for them (review r5 L2: a forged size of 2^40 made the
     * NCZBLOCK path ask for 8 GiB, on a phone that jetsams at ~3.3 GiB). An entry that runs past the end of the file is a damaged container. */
    uint64_t flen = 0;
    if (fseeko(in, 0, SEEK_END) != 0 || ftello(in) < 0) { fail(err, errlen, "cannot get the length of %s", src); goto done; }
    flen = (uint64_t) ftello(in);
    for (uint32_t i = 0; i < n; i++) {
        if (e[i].offset > UINT64_MAX - data0 || e[i].size > UINT64_MAX - (data0 + e[i].offset) || data0 + e[i].offset + e[i].size > flen) {
            fail(err, errlen, "PFS0 entry %u (%s) runs past the end of the file", i, e[i].name); goto done;
        }
    }

    /* The output header: same entries, .ncz renamed .nca, names padded so the data starts on 0x20. Sizes and offsets are
     * written once every NCA has been rebuilt (they are only known then). */
    size_t names = 0;
    for (uint32_t i = 0; i < n; i++) names += strlen(e[i].name) + 1;
    size_t hsize = 16 + 0x18 * (size_t) n + names;
    size_t padded = (hsize + 0x1f) & ~(size_t) 0x1f;
    hdr = calloc(1, padded);
    if (hdr == NULL) { fail(err, errlen, "no memory"); goto done; }
    snprintf(part, sizeof part, "%s.part", dst);
    out = fopen(part, "wb");
    if (out == NULL) { fail(err, errlen, "cannot write %s: %s", part, strerror(errno)); goto done; }
    if (fwrite(hdr, 1, padded, out) != padded) { fail(err, errlen, "write failed (%s)", strerror(errno)); goto done; }

    uint64_t at = 0;
    for (uint32_t i = 0; i < n; i++) {
        uint64_t size = 0;
        if (e[i].ncz) {
            uint8_t digest[32];
            if (ncz_to_nca(in, data0 + e[i].offset, e[i].size, out, &size, digest, err, errlen) != 0) goto done;
            int ok = name_matches(e[i].name, digest);
            if (ok == 0) { fail(err, errlen, "%s: the rebuilt NCA's SHA-256 does not match its name", e[i].name); goto done; }
            if (log != NULL) {
                snprintf(line, sizeof line, "[nsz] %s: %.1f MB -> %.1f MB, SHA-256 %s", e[i].name, e[i].size / 1048576.0, size / 1048576.0,
                         ok == 1 ? "matches its name" : "not checked (the name is not a content id)");
                log(line);
            }
        } else {   /* stored as it was (the meta NCA, the ticket, the certificate) */
            uint8_t *buf = malloc(1 << 20);
            if (buf == NULL) { fail(err, errlen, "no memory"); goto done; }
            uint64_t left = e[i].size, from = data0 + e[i].offset;
            if (fseeko(in, (off_t) from, SEEK_SET) != 0) { free(buf); fail(err, errlen, "seek failed"); goto done; }
            while (left > 0) {
                size_t want = left < (1 << 20) ? (size_t) left : (1 << 20);
                if (fread(buf, 1, want, in) != want) { free(buf); fail(err, errlen, "short entry %s", e[i].name); goto done; }
                if (fwrite(buf, 1, want, out) != want) { free(buf); fail(err, errlen, "write failed (%s)", strerror(errno)); goto done; }
                left -= want;
            }
            free(buf);
            size = e[i].size;
        }
        e[i].offset = at; e[i].size = size; at += size;
    }

    memcpy(hdr, "PFS0", 4); put32(hdr + 4, n); put32(hdr + 8, (uint32_t) (names + (padded - hsize)));
    size_t no = 0;
    for (uint32_t i = 0; i < n; i++) {
        uint8_t *p = hdr + 16 + 0x18 * i;
        put64(p, e[i].offset); put64(p + 8, e[i].size); put32(p + 16, (uint32_t) no); put32(p + 20, 0);
        memcpy(hdr + 16 + 0x18 * (size_t) n + no, e[i].name, strlen(e[i].name) + 1);
        no += strlen(e[i].name) + 1;
    }
    if (fseeko(out, 0, SEEK_SET) != 0 || fwrite(hdr, 1, padded, out) != padded) { fail(err, errlen, "header write failed (%s)", strerror(errno)); goto done; }
    if (fclose(out) != 0) { out = NULL; fail(err, errlen, "close failed (%s)", strerror(errno)); goto done; }
    out = NULL;
    if (rename(part, dst) != 0) { fail(err, errlen, "rename failed (%s)", strerror(errno)); goto done; }
    rc = 0;
done:
    if (out != NULL) { fclose(out); unlink(part); }
    if (rc != 0 && part[0] != '\0') unlink(part);
    if (e != NULL) for (uint32_t i = 0; i < n; i++) free(e[i].name);
    free(e); free(strs); free(hdr);
    fclose(in);
    return rc;
}

#ifdef NSZ_MAIN   /* a Mac command line: cc -DNSZ_MAIN -I<zstd>/lib nsz_decompress.c <zstd>/lib/libzstd.a; nsz_decompress <in.nsz> <out.nsp> */
static void print_line(const char *line) { puts(line); }
int main(int argc, char **argv)
{
    if (argc != 3) { fprintf(stderr, "usage: %s <in.nsz> <out.nsp>\n", argv[0]); return 2; }
    char err[512] = "";
    if (gameapp_nsz_to_nsp(argv[1], argv[2], err, sizeof err, print_line) != 0) { fprintf(stderr, "FAILED: %s\n", err); return 1; }
    puts("ok");
    return 0;
}
#endif
