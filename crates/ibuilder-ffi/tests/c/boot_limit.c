/* boot_limit.c -- the boot's time limit, end to end through the ABI.  A boot-only request is on disk and the framework is a host that runs for 5 seconds; boot_seconds=1 in the config.
 * The child process runs ibuilder_run_chain(); the library dlopens the host, starts its boot and arms the limit; after 1 s the library logs the cause, consumes the request and sends
 * SIGTERM to its own process -- the designed end, which `GameAppUi.ClassifyPreviousRun` reads from the log at the next launch.  The parent checks all of that.
 *
 *   boot_limit <documents folder>      (prepared by tests/c_abi.rs: gameapp.config, gameapp.boot-only, extract/flat, the host dylib as the framework)
 */
#include "ibuilder.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static char *slurp(const char *path)
{
    static char buf[16384];
    FILE *f = fopen(path, "r");
    size_t n = f ? fread(buf, 1, sizeof buf - 1, f) : 0;
    if (f) fclose(f);
    buf[n] = 0;
    return buf;
}

int main(int argc, char **argv)
{
    if (argc < 2) { fprintf(stderr, "usage: boot_limit <documents>\n"); return 2; }
    char path[1024];
    struct timespec a, b;
    clock_gettime(CLOCK_MONOTONIC, &a);
    pid_t pid = fork();
    if (pid == 0) {
        ibuilder_options o; memset(&o, 0, sizeof o);
        o.struct_size = sizeof o; o.documents_dir = argv[1]; o.bundle_dir = argv[1];
        o.flags = IBUILDER_FLAG_BOOT_IN_PROCESS;
        int err = 0;
        ibuilder_t *h = ibuilder_create(&o, &err);
        if (!h) _exit(3);
        ibuilder_run_chain(h, NULL);
        _exit(0);                       /* reached only if the boot returned: it must not */
    }
    int st = 0;
    waitpid(pid, &st, 0);
    clock_gettime(CLOCK_MONOTONIC, &b);
    double took = (b.tv_sec - a.tv_sec) + (b.tv_nsec - a.tv_nsec) / 1e9;
    int termed = WIFSIGNALED(st) && WTERMSIG(st) == SIGTERM;
    snprintf(path, sizeof path, "%s/gameapp.chain.log", argv[1]);
    char *chain = slurp(path);
    int limit_line = strstr(chain, "boot time limit 1 s reached: sending SIGTERM to self") != NULL;
    int consumed_line = strstr(chain, "gameapp.boot-only consumed: the boot ran to its time limit, the designed end") != NULL;
    snprintf(path, sizeof path, "%s/gameapp.boot-only", argv[1]);
    struct stat sb;
    int request_gone = stat(path, &sb) != 0;
    snprintf(path, sizeof path, "%s/gameapp.guest.log", argv[1]);
    char *guest = slurp(path);
    int guest_ran = strstr(guest, "[dm] >>> nnMain") != NULL && strstr(guest, "[dm] graceful exit done") == NULL;
    printf("%s the process ended by SIGTERM (its own limit), not by the host returning\n", termed ? "PASS" : "FAIL");
    printf("%s ... after about the limit, not the host's 5 seconds (%.1f s)\n", (took > 0.9 && took < 4.0) ? "PASS" : "FAIL", took);
    printf("%s the chain log names the cause (\"boot time limit 1 s reached: sending SIGTERM to self\")\n", limit_line ? "PASS" : "FAIL");
    printf("%s the boot request was consumed before the signal (a boot that ran to its limit was honoured)\n", (consumed_line && request_gone) ? "PASS" : "FAIL");
    printf("%s the guest's own output reached gameapp.guest.log, ending mid-boot\n", guest_ran ? "PASS" : "FAIL");
    return (termed && took > 0.9 && took < 4.0 && limit_line && consumed_line && request_gone && guest_ran) ? 0 : 1;
}
