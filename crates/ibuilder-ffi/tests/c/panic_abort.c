/* panic_abort.c -- the release profile of this workspace is panic = "abort": a Rust panic cannot be caught, so the library installs a panic hook that writes the message to
 * Documents/gameapp.chain.log first, and then the process aborts.  This program forks a child that makes the library panic, and checks both halves: the child died of SIGABRT (nothing
 * crossed the C boundary as an exception, nothing was swallowed) and the cause is in the log.  In an unwinding build (cargo test) the same call returns IBUILDER_E_PANIC instead:
 * that half is a Rust unit test (a_panic_never_crosses_the_boundary).
 *
 *   panic_abort <documents folder>
 */
#define IBUILDER_TESTING 1
#include "ibuilder.h"

#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

int main(int argc, char **argv)
{
    if (argc < 2) { fprintf(stderr, "usage: panic_abort <documents>\n"); return 2; }
    char logpath[1024];
    snprintf(logpath, sizeof logpath, "%s/gameapp.chain.log", argv[1]);
    pid_t pid = fork();
    if (pid == 0) {
        ibuilder_options o; memset(&o, 0, sizeof o);
        o.struct_size = sizeof o; o.documents_dir = argv[1]; o.bundle_dir = argv[1];
        int err = 0;
        ibuilder_t *h = ibuilder_create(&o, &err);
        if (!h) _exit(3);
        ibuilder_test_panic(h);      /* does not return in a panic=abort build */
        _exit(0);                    /* reached only if the panic was caught: an unwinding build */
    }
    int st = 0;
    waitpid(pid, &st, 0);
    int aborted = WIFSIGNALED(st) && WTERMSIG(st) == SIGABRT;
    FILE *f = fopen(logpath, "r");
    char buf[8192] = {0};
    size_t n = f ? fread(buf, 1, sizeof buf - 1, f) : 0;
    if (f) fclose(f);
    int logged = n > 0 && strstr(buf, "[ibuilder] PANIC: ") != NULL && strstr(buf, "test panic requested") != NULL;
    printf("%s the child aborted (SIGABRT) instead of letting a panic cross the boundary\n", aborted ? "PASS" : "FAIL");
    printf("%s the panic message is in gameapp.chain.log\n", logged ? "PASS" : "FAIL");
    return (aborted && logged) ? 0 : 1;
}
