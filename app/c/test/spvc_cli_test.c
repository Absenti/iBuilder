/* Host test (macOS) of app/c/spvc_cli.c's output buffer: a failed allocation or an overflowing size must reach the return code.
 * The SPIRV-Cross symbols are never called (undefined symbols are allowed at link time). */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <sys/resource.h>
#include <unistd.h>

static int g_fail_realloc, g_fail_malloc;
static void *t_realloc(void *p, size_t n) { return g_fail_realloc ? NULL : realloc(p, n); }
static void *t_malloc(size_t n) { return g_fail_malloc ? NULL : malloc(n); }
#define realloc t_realloc
#define malloc t_malloc
#include "../spvc_cli.c"
#undef realloc
#undef malloc

static int g_bad;
#define CHECK(c, ...) do { if (!(c)) { g_bad++; printf("  FAIL: "); printf(__VA_ARGS__); printf("\n"); } } while (0)

int main(void)
{
    const char *argv_rev[] = { "spirv-cross", "--revision", NULL };
    char *o = NULL, *e = NULL;

    puts("1. normal call (--revision): rc 0, text intact");
    int rc = gameapp_spirv_cross(2, argv_rev, &o, &e);
    CHECK(rc == 0 && e && strstr(e, "vulkan-sdk"), "rc %d", rc); printf("  rc %d, stderr %zu bytes\n", rc, e ? strlen(e) : 0); free(o); free(e);

    puts("2. realloc fails: the same call must not return success with a missing text");
    g_fail_realloc = 1; o = e = NULL;
    rc = gameapp_spirv_cross(2, argv_rev, &o, &e);
    g_fail_realloc = 0;
    CHECK(rc != 0, "rc %d (a silent truncation)", rc); printf("  rc %d, stdout %p, stderr %s", rc, (void *) o, e ? e : "(null)\n"); free(o); free(e);

    puts("3. malloc of the formatted-text buffer fails (a message longer than the 1 KiB stack buffer): flagged");
    { sbuf b = { 0 }; char big[3000]; memset(big, 'x', sizeof big - 1); big[sizeof big - 1] = 0;
      g_fail_malloc = 1; sb_fmt(&b, "%s", big); g_fail_malloc = 0;
      CHECK(b.failed, "not flagged"); free(b.p); }

    puts("4. growth: a length that cannot be represented returns (no endless loop) and flags the buffer");
    { sbuf b = { 0 }; sb_str(&b, "a");
      sb_add(&b, "b", SIZE_MAX - 1);          /* n + 1 + b.n overflows */
      CHECK(b.failed, "overflow not flagged");
      sbuf c = { 0 }; c.p = malloc(8); c.cap = 8; c.n = 4; c.p[0] = 0;
      sb_add(&c, "b", SIZE_MAX / 2 + 100);    /* the doubling would pass SIZE_MAX / 2 */
      CHECK(c.failed, "doubling overflow not flagged");
      free(b.p); free(c.p); puts("  returned"); }

    puts("5. once failed, later appends do nothing");
    { sbuf b = { 0 }; b.failed = 1; sb_str(&b, "x"); CHECK(b.p == NULL && b.n == 0, "appended after a failure"); }

    puts("6. --output (review r5 P2): a file that cannot be opened, or written in full, is exit code 1, never a shorter success");
    { const char *tmp = getenv("TMPDIR"); char path[1024], dir[1024];
      snprintf(dir, sizeof dir, "%s", tmp && *tmp ? tmp : "/tmp");
      snprintf(path, sizeof path, "%s/spvc_test_out.msl", dir);
      size_t n = 64 * 1024; char *text = malloc(n + 1); memset(text, 'm', n); text[n] = 0;
      sbuf e = { 0 };
      int rc = write_output(path, text, &e);
      FILE *fp = fopen(path, "rb"); size_t got = 0; char buf[4096]; size_t k;
      while (fp && (k = fread(buf, 1, sizeof buf, fp)) > 0) got += k; if (fp) fclose(fp);
      CHECK(rc == 0 && got == n && e.n == 0, "the whole text: rc %d, %zu of %zu bytes", rc, got, n); printf("  written: rc %d, %zu bytes\n", rc, got); free(e.p);
      e = (sbuf) { 0 };
      rc = write_output(dir, text, &e);   /* a folder: fopen fails */
      CHECK(rc == 1 && e.p && strstr(e.p, "Failed to write file"), "open failure: rc %d", rc); printf("  folder: rc %d, %s", rc, e.p ? e.p : "(none)\n"); free(e.p);
      /* a full disk, simulated: the file size limit is 1 KiB (SIGXFSZ ignored, so write() fails with EFBIG) */
      struct rlimit old, lim; getrlimit(RLIMIT_FSIZE, &old); lim = old; lim.rlim_cur = 1024;
      void (*prev)(int) = signal(SIGXFSZ, SIG_IGN);
      e = (sbuf) { 0 };
      if (setrlimit(RLIMIT_FSIZE, &lim) == 0) {
          rc = write_output(path, text, &e);
          setrlimit(RLIMIT_FSIZE, &old);
          CHECK(rc == 1 && e.p && strstr(e.p, "Failed to write file"), "a short write: rc %d", rc); printf("  short write: rc %d, %s", rc, e.p ? e.p : "(none)\n");
      } else { g_bad++; puts("  FAIL: setrlimit(RLIMIT_FSIZE) refused"); }
      signal(SIGXFSZ, prev); free(e.p); unlink(path); free(text); }

    printf("%s (%d failures)\n", g_bad ? "FAILED" : "ALL OK", g_bad);
    return g_bad != 0;
}
