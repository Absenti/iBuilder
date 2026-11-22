/* run_chain.c -- the smallest real client of the ABI: run the whole chain (real stage workers, a build with --features real-stages) over a Documents folder whose gameapp.config
 * and extract/ are already laid out (ibuilder --out <dir> lays them out), print the stage lines and the result, then the memory line.
 *
 *   PATH=/opt/homebrew/opt/rustup/bin:$PATH cargo build --release --target aarch64-apple-ios-sim -p ibuilder-ffi --features real-stages
 *   xcrun --sdk iphonesimulator clang -arch arm64 -target arm64-apple-ios14.0-simulator -Icrates/ibuilder-ffi/include run_chain.c libibuilder_ffi.a -framework CoreFoundation -o run_chain
 *   xcrun simctl spawn <booted simulator> ./run_chain <Documents> <bundle: the engine folder with dm/host, dm/games>
 */
#include "ibuilder.h"
#include <stdio.h>
#include <time.h>

static void on_log(void *u, int stream, const char *line) { (void) u; if (stream == 0 && (line[0] == 's' || line[0] == ' ' ) && (line[0] == 's' || line[2] == '[')) printf("  %s\n", line); }
static void on_progress(void *u, int k, int n, const char *text, double f) { (void) u; printf("progress %d/%d %.3f  %s\n", k, n, f, text); }

int main(int argc, char **argv)
{
    if (argc < 3) { fprintf(stderr, "usage: run_chain <documents> <bundle>\n"); return 2; }
    ibuilder_options o = {0};
    o.struct_size = sizeof o;
    o.documents_dir = argv[1];
    o.bundle_dir = argv[2];
    o.log = on_log;
    o.progress = on_progress;
    int err = 0;
    ibuilder_t *h = ibuilder_create(&o, &err);
    if (!h) { char e[256]; ibuilder_last_error(e, sizeof e); fprintf(stderr, "ibuilder_create failed (%d): %s\n", err, e); return 1; }
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);
    int rc = ibuilder_run_chain(h, NULL);
    clock_gettime(CLOCK_MONOTONIC, &b);
    char status[512], mem[256];
    ibuilder_chain_status(h, status, sizeof status);
    ibuilder_mem_line(argv[1], mem, sizeof mem);
    printf("ibuilder_run_chain -> %d in %.2f s\n%s%s\n", rc, (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9, status, mem);
    ibuilder_destroy(h);
    return rc;
}
