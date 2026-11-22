/* test_abi.c -- proves the C ABI of libibuilder_ffi (include/ibuilder.h) from plain C: build a handle, run the whole chain on stand-in stage workers, read the callbacks,
 * the status, the memory and launch-state functions, cancel, and the argument checks.  No Rust types are visible here.
 *
 *   cargo build --release -p ibuilder-ffi --features mock-stages
 *   clang -std=c11 -Wall -Wextra -Icrates/ibuilder-ffi/include crates/ibuilder-ffi/tests/c/test_abi.c \
 *         <target>/release/libibuilder_ffi.a -o test_abi $(rustc --print native-static-libs ...)
 *   ./test_abi <a prepared Documents folder>
 *
 * The folder is prepared by tests/c_abi.rs (extract/ExeFS/{main,subsdk0,sdk}, host-prebuilt.dylib and a gameapp.config); argv[2], when given, is a second folder for the launch-state checks.
 */
#include "ibuilder.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int failures = 0;
#define CHECK(cond, msg) do { if (cond) printf("PASS %s\n", msg); else { printf("FAIL %s  (%s:%d)\n", msg, __FILE__, __LINE__); failures++; } } while (0)

struct seen {
    int log_lines[2];
    int stage_lines;
    int last_k, last_n;
    double last_fraction;
    char last_text[256];
    int mem_lines, rewrite_line, sign_calls, xlate_calls, spirv_calls, tools_line;
    int live_busy_events;
    int stop_line;
};

/* a stop the test raises from inside the run, as the UI thread would: when the log says `g_stop_line`, kind 1 / 2 go to ibuilder_request_stop, kind 99 to ibuilder_cancel(g_handle) */
static const char *g_stop_line = NULL;
static int g_stop_kind = 0;
static ibuilder_t *g_handle = NULL;

static void on_log(void *user, int stream, const char *line)
{
    struct seen *s = user;
    if (stream == 0 || stream == 1) s->log_lines[stream]++;
    if (strstr(line, "[mem:")) s->mem_lines++;
    if (strstr(line, "stage 2 rewrite:")) s->rewrite_line = 1;
    if (strstr(line, "tools: xlate rc=3 out='dmsh_xlate job.json outdir'")) s->tools_line = 1;
    if (strstr(line, "[stop] the chain was ")) s->stop_line = 1;
    if (g_stop_line != NULL && strstr(line, g_stop_line)) {
        g_stop_line = NULL;
        if (g_stop_kind == 99) ibuilder_cancel(g_handle);
        else ibuilder_request_stop(g_stop_kind);
    }
}

static void on_progress(void *user, int k, int n, const char *text, double fraction)
{
    struct seen *s = user;
    s->stage_lines++;
    s->last_k = k; s->last_n = n; s->last_fraction = fraction;
    snprintf(s->last_text, sizeof s->last_text, "%s", text);
}

static int g_sign_rc;   /* the next signature's result: 12 = no certificate in this app's Keychain */
static int on_sign(void *user, const char *framework_dir, const char *exe, char *mode, int mode_cap, char *ident, int ident_cap, char *err, int err_cap)
{
    struct seen *s = user;
    s->sign_calls++;
    if (g_sign_rc != 0) { snprintf(mode, (size_t) mode_cap, "none"); snprintf(ident, (size_t) ident_cap, "-"); snprintf(err, (size_t) err_cap, "no certificate (test)"); return g_sign_rc; }
    snprintf(mode, (size_t) mode_cap, "device(zsign, in-memory .p12)");
    snprintf(ident, (size_t) ident_cap, "dev.ibuilder.app");
    err[0] = 0; (void) err_cap;
    /* the caller's signer does its work on the framework: here, append a marker the test looks for */
    char path[1024];
    snprintf(path, sizeof path, "%s/%s", framework_dir, exe);
    FILE *f = fopen(path, "ab");
    if (f) { fputs("C-SIGNED\n", f); fclose(f); }
    return 0;
}

/* stage 4b's fallback with no identity (rc 10/12/13): an ad hoc signature, for export only */
static int g_adhoc_calls;
static int on_sign_adhoc(void *user, const char *framework_dir, const char *exe, char *mode, int mode_cap, char *ident, int ident_cap, char *err, int err_cap)
{
    (void) user; (void) framework_dir; (void) exe; (void) err_cap;
    g_adhoc_calls++;
    snprintf(mode, (size_t) mode_cap, "adhoc(zsign, no certificate: export only)");
    snprintf(ident, (size_t) ident_cap, "dev.ibuilder.app");
    err[0] = 0;
    return 0;
}

static int file_has(const char *docs, const char *rel, const char *text)
{
    char p[2048], buf[4096];
    snprintf(p, sizeof p, "%s/%s", docs, rel);
    FILE *f = fopen(p, "rb");
    if (!f) return 0;
    size_t n = fread(buf, 1, sizeof buf - 1, f);
    fclose(f);
    buf[n] = 0;
    return strstr(buf, text) != NULL;
}

static struct seen *g_tool_seen;
static int tool(int argc, const char **argv, char **out, char **err, int rc, int *counter)
{
    size_t n = 1;
    for (int i = 0; i < argc; i++) n += strlen(argv[i]) + 1;
    char *s = malloc(n);
    s[0] = 0;
    for (int i = 0; i < argc; i++) { if (i) strcat(s, " "); strcat(s, argv[i]); }
    *out = s;           /* malloc()ed: the library free()s it */
    *err = NULL;
    (*counter)++;
    return rc;
}
static int on_xlate(int argc, const char **argv, char **out, char **err) { return tool(argc, argv, out, err, 3, &g_tool_seen->xlate_calls); }
static int on_spirv(int argc, const char **argv, char **out, char **err) { return tool(argc, argv, out, err, 0, &g_tool_seen->spirv_calls); }

static void on_live(void *user, int busy) { ((struct seen *) user)->live_busy_events += busy ? 1 : 100; }

static int exists(const char *dir, const char *name)
{
    char p[1024]; struct stat st;
    snprintf(p, sizeof p, "%s/%s", dir, name);
    return stat(p, &st) == 0;
}

int main(int argc, char **argv)
{
    if (argc < 2) { fprintf(stderr, "usage: test_abi <documents folder>\n"); return 2; }
    const char *docs = argv[1];
    struct seen seen; memset(&seen, 0, sizeof seen);
    g_tool_seen = &seen;

    /* ---- the library and the header agree ---- */
    CHECK(ibuilder_abi_version() == IBUILDER_ABI_VERSION, "the library's ABI version is the header's");
    CHECK(ibuilder_version() != NULL && ibuilder_version()[0] != 0, "ibuilder_version() is a string");

    /* ---- argument checks: errors, not crashes ---- */
    int err = 0;
    CHECK(ibuilder_create(NULL, &err) == NULL && err == IBUILDER_E_BAD_ARG, "ibuilder_create(NULL) fails with E_BAD_ARG");
    ibuilder_options bad; memset(&bad, 0, sizeof bad);
    bad.struct_size = 4;
    CHECK(ibuilder_create(&bad, &err) == NULL && err == IBUILDER_E_BAD_ARG, "a struct_size from an older header is refused");
    char msg[256];
    CHECK(ibuilder_last_error(msg, sizeof msg) > 0 && strstr(msg, "struct_size") != NULL, "ibuilder_last_error says why");
    CHECK(ibuilder_run_chain(NULL, NULL) == IBUILDER_E_BAD_ARG, "ibuilder_run_chain(NULL) fails with E_BAD_ARG");
    ibuilder_destroy(NULL);     /* a no-op */
    ibuilder_cancel(NULL);      /* a no-op */

    /* ---- one run of the whole chain on the stand-in workers ---- */
    ibuilder_options o; memset(&o, 0, sizeof o);
    o.struct_size = sizeof o;
    o.documents_dir = docs;
    o.bundle_dir = docs;
    o.log = on_log;                 o.log_user = &seen;
    o.progress = on_progress;       o.progress_user = &seen;
    o.sign = on_sign;               o.sign_user = &seen;
    o.xlate = on_xlate;             o.spirv_cross = on_spirv;
    o.live_notify = on_live;        o.live_notify_user = &seen;
    o.flags = IBUILDER_FLAG_MOCK_STAGES;
    ibuilder_t *h = ibuilder_create(&o, &err);
    CHECK(h != NULL, "ibuilder_create with the callbacks");
    if (!h) return 1;

    ibuilder_cancel(h);             /* a cancel before the run is forgotten when the run starts (a new run, a new decision) */
    int rc = ibuilder_run_chain(h, NULL);
    CHECK(rc == IBUILDER_RC_OK, "the chain ran: rc 0");
    /* the app's Finish(): without IBUILDER_FLAG_BOOT_IN_PROCESS the chain leaves the interrupted-run marker to the caller (the boot is what clears it) */
    CHECK(exists(docs, "gameapp.running"), "the run's marker is still there after a build-only chain");
    ibuilder_finish_run(docs);
    CHECK(!exists(docs, "gameapp.running"), "ibuilder_finish_run removes it");

    char status[512];
    int need = ibuilder_chain_status(h, status, sizeof status);
    CHECK(need > 0 && need < (int) sizeof status, "ibuilder_chain_status fits its buffer");
    CHECK(strstr(status, "profile=ok") && strstr(status, "rewrite=ok") && strstr(status, "shaders=ok") && strstr(status, "assemble=ok") && strstr(status, "result=ok"),
          "every stage is ok in the status text");
    char tiny[8];
    CHECK(ibuilder_chain_status(h, tiny, sizeof tiny) == need && strlen(tiny) == 7, "a short buffer is truncated and the needed length is returned");

    CHECK(seen.log_lines[0] > 20 && seen.log_lines[1] > 0, "the log callback got both streams (chain, [gameapp])");
    CHECK(seen.mem_lines >= 6, "the [mem:...] lines went through the callback");
    CHECK(seen.rewrite_line, "the stage 2 line went through the callback");
    CHECK(seen.stage_lines >= 8 && seen.last_k == 4 && seen.last_n == 5 && strstr(seen.last_text, "signing the framework") != NULL, "the progress callback saw the stages, the last one the signature");
    CHECK(seen.last_fraction > 0.87 && seen.last_fraction < 0.89, "the progress fraction is where the bar stands when stage 4 starts (0.03 + 0.45 + 0.40)");
    CHECK(seen.sign_calls == 1, "the C signer was called once");
    CHECK(seen.xlate_calls >= 1 && seen.spirv_calls >= 1 && seen.tools_line, "the C tool callbacks were called by stage 3 and their malloc()ed strings freed");
    CHECK(exists(docs, "DmGameProbe.framework/DmGameProbe") && exists(docs, "gameapp.framework") && exists(docs, "extract/flat/main.patched.bin"), "the framework, its stamp and flat/ are on disk");
    {
        char p[1024]; snprintf(p, sizeof p, "%s/DmGameProbe.framework/DmGameProbe", docs);
        FILE *f = fopen(p, "rb"); char buf[4096] = {0}; size_t n = f ? fread(buf, 1, sizeof buf - 1, f) : 0; if (f) fclose(f);
        CHECK(n > 0 && strstr(buf, "MOCK-FRAMEWORK") && strstr(buf, "C-SIGNED"), "the framework carries the C signer's mark");
    }
    CHECK(exists(docs, "pipeline.stages.log") && exists(docs, "gameapp.chain.log") && exists(docs, "build_times.tsv"), "the logs and the build-times row are written");

    /* a second run on the same handle is fine once the first ended; with a pending cancel the chain stops at its first safe point */
    ibuilder_cancel(h);
    CHECK(ibuilder_run_chain(h, NULL) == IBUILDER_RC_OK, "a cancel before a run is cleared by the run (the flag is per run)");

    /* ---- pause and cancel: ibuilder_request_stop / ibuilder_stop_requested ---- */
    CHECK(IBUILDER_RC_PAUSED == 131 && IBUILDER_RC_CANCELLED == 130, "the result codes of a pause and of a cancel");
    CHECK(ibuilder_stop_requested() == IBUILDER_STOP_NONE, "no request is pending after a run");
    CHECK(ibuilder_request_stop(IBUILDER_STOP_PAUSE) == 1 && ibuilder_stop_requested() == 1, "a pause is a request");
    CHECK(ibuilder_request_stop(IBUILDER_STOP_CANCEL) == 2, "a cancel overrides a pause");
    CHECK(ibuilder_request_stop(IBUILDER_STOP_PAUSE) == 2 && ibuilder_stop_requested() == 2, "a pause does not override a cancel");
    CHECK(ibuilder_request_stop(0) == 0 && ibuilder_request_stop(IBUILDER_STOP_PAUSE) == 1 && ibuilder_request_stop(7) == 0 && ibuilder_request_stop(-3) == 0 && ibuilder_stop_requested() == 0,
          "any other value (0, 7, -3) withdraws the request");
    ibuilder_request_stop(IBUILDER_STOP_PAUSE);
    CHECK(ibuilder_run_chain(h, NULL) == IBUILDER_RC_OK && ibuilder_stop_requested() == 0, "a request that came before a run is cleared when the run starts");

    g_handle = h;
    g_stop_line = "stage 2 rewrite:"; g_stop_kind = IBUILDER_STOP_PAUSE;
    seen.stop_line = 0;
    rc = ibuilder_run_chain(h, NULL);
    CHECK(rc == IBUILDER_RC_PAUSED, "a pause asked for while the chain runs ends it with IBUILDER_RC_PAUSED (131)");
    ibuilder_chain_status(h, status, sizeof status);
    CHECK(strstr(status, "result=paused: by the user at stage 2") != NULL && seen.stop_line, "the status says paused and the log carries the [stop] line");
    CHECK(ibuilder_stop_requested() == IBUILDER_STOP_PAUSE, "the chain leaves the request as it was (the next run clears it)");

    g_stop_line = "stage 2 rewrite:"; g_stop_kind = IBUILDER_STOP_CANCEL;
    rc = ibuilder_run_chain(h, NULL);
    CHECK(rc == IBUILDER_RC_CANCELLED, "a cancel asked for while the chain runs ends it with IBUILDER_RC_CANCELLED (130)");
    ibuilder_chain_status(h, status, sizeof status);
    CHECK(strstr(status, "result=cancelled") != NULL, "the status says cancelled");

    g_stop_line = "stage 2 rewrite:"; g_stop_kind = 99;    /* the handle's own ibuilder_cancel stops the run like a cancel request */
    CHECK(ibuilder_run_chain(h, NULL) == IBUILDER_RC_CANCELLED, "ibuilder_cancel(handle) during a run still ends it with 130");

    rc = ibuilder_run_chain(h, NULL);
    CHECK(rc == IBUILDER_RC_OK && ibuilder_stop_requested() == 0, "the run after a pause and a cancel runs to its end (rc 0)");

    /* ---- a stage 3 that cannot build the library (another chain, too little free space): IBUILDER_RC_NO_SHADER_LIBRARY, no framework stamp, the status says why (review r4 §4.1) ---- */
    CHECK(IBUILDER_RC_NO_SHADER_LIBRARY == 4, "the result code of a game left with no shader library");
    {
        char src[1024], dst[1024], text[8192] = {0};
        snprintf(src, sizeof src, "%s/gameapp.config", docs);
        snprintf(dst, sizeof dst, "%s/noshaders.config", docs);
        FILE *in = fopen(src, "rb"); size_t n = in ? fread(text, 1, sizeof text - 1, in) : 0; if (in) fclose(in);
        FILE *out = fopen(dst, "wb");
        if (out) { fwrite(text, 1, n, out); fputs("\nshader_chain=ffffffffffff\nshader_free_gib=1000000\n", out); fclose(out); }
        CHECK(n > 0 && out != NULL, "a config whose stage 3 cannot build");
        /* the game's code as extracted again (the first build dropped extract/ExeFS: the later runs above assemble nothing), so the assembly runs and stage 3 fails on the free space */
        char exefs[1024]; snprintf(exefs, sizeof exefs, "%s/extract/ExeFS", docs);
        mkdir(exefs, 0755);
        const char *mods[3] = { "main", "subsdk0", "sdk" };
        for (int i = 0; i < 3; i++) {
            char p[1200]; snprintf(p, sizeof p, "%s/%s", exefs, mods[i]);
            FILE *m = fopen(p, "wb"); if (m) { fprintf(m, "NSO:%s", mods[i]); fclose(m); }
        }
        rc = ibuilder_run_chain(h, dst);
        CHECK(rc == IBUILDER_RC_NO_SHADER_LIBRARY, "a build whose shader library could not be made returns IBUILDER_RC_NO_SHADER_LIBRARY (4), not 0");
        char why[4096];   /* the reason names a path: longer than the status buffer above */
        int whyn = ibuilder_chain_status(h, why, sizeof why);
        int said = whyn > 0 && whyn < (int) sizeof why && strstr(why, "shaders=failed (no library: ") != NULL && strstr(why, " free") != NULL
                   && strstr(why, "assemble=ok") != NULL && strstr(why, "result=failed (shaders)") != NULL;
        if (!said) printf("  status (%d bytes): %s\n", whyn, why);
        CHECK(said, "the status says stage 3 failed and why");
        CHECK(exists(docs, "DmGameProbe.framework/DmGameProbe") && !exists(docs, "gameapp.framework"), "the chain still assembled, but the framework is not stamped (the game is not ready)");
        ibuilder_finish_run(docs);
    }

    /* ---- a signature that fails: IBUILDER_RC_NO_FRAMEWORK, the status says assemble=failed, nothing stamped (review r4 follow-up: it was 0, "Built: ready to play.") ---- */
    CHECK(IBUILDER_RC_NO_FRAMEWORK == 5, "the result code of a build that left no framework");
    g_sign_rc = 12;
    rc = ibuilder_run_chain(h, NULL);
    g_sign_rc = 0;
    CHECK(rc == IBUILDER_RC_NO_FRAMEWORK, "a build whose signature failed returns IBUILDER_RC_NO_FRAMEWORK (5), not 0");
    {
        char st5[4096];
        int n5 = ibuilder_chain_status(h, st5, sizeof st5);
        CHECK(n5 > 0 && n5 < (int) sizeof st5 && strstr(st5, "assemble=failed") != NULL && strstr(st5, "result=failed (assemble)") != NULL && !exists(docs, "gameapp.framework"),
              "the status says the assembly failed, and no framework is stamped");
    }
    ibuilder_finish_run(docs);
    CHECK(ibuilder_run_chain(h, NULL) == IBUILDER_RC_OK, "and the next build, signed, is 0 again");
    CHECK(!file_has(docs, "gameapp.framework", "signature="), "a framework signed with the identity has no signature= line in its stamp");
    ibuilder_finish_run(docs);
    ibuilder_destroy(h);

    /* ---- no identity, with the ad hoc fallback: the build succeeds for export only (stamped), not booted ---- */
    {
        ibuilder_options oa = o;
        oa.sign_adhoc = on_sign_adhoc;
        ibuilder_t *ha = ibuilder_create(&oa, &err);
        CHECK(ha != NULL, "ibuilder_create with sign_adhoc");
        if (ha) {
            g_sign_rc = 12; g_adhoc_calls = 0;
            rc = ibuilder_run_chain(ha, NULL);
            g_sign_rc = 0;
            CHECK(rc == IBUILDER_RC_OK && g_adhoc_calls == 1, "no certificate (rc 12) with sign_adhoc: signed ad hoc once, the build is 0");
            CHECK(file_has(docs, "gameapp.framework", "signature=adhoc-export"), "the stamp says the framework is signed ad hoc for export only");
            ibuilder_finish_run(docs);
            g_sign_rc = 14; g_adhoc_calls = 0;
            rc = ibuilder_run_chain(ha, NULL);
            g_sign_rc = 0;
            CHECK(rc == IBUILDER_RC_NO_FRAMEWORK && g_adhoc_calls == 0, "a .p12 that cannot be used (rc 14) still fails: no ad hoc fallback for a broken certificate");
            ibuilder_finish_run(docs);
            CHECK(ibuilder_run_chain(ha, NULL) == IBUILDER_RC_OK && g_adhoc_calls == 0 && !file_has(docs, "gameapp.framework", "signature="),
                  "with the identity the fallback is never called and the stamp is as before");
            ibuilder_finish_run(docs);
            ibuilder_destroy(ha);
        }
    }

    /* ---- memory: the kernel's numbers ---- */
    ibuilder_mem m; memset(&m, 0, sizeof m);
    CHECK(ibuilder_mem_info(docs, &m) == 0 && m.resident > (1u << 20) && m.peak >= m.resident && m.free_bytes > 0, "ibuilder_mem_info: resident, peak, free");
#if defined(__APPLE__)
    CHECK(m.footprint > 0, "phys_footprint from task_info");
#endif
    char line[256];
    CHECK(ibuilder_mem_line(docs, line, sizeof line) > 20 && strstr(line, "resident") && strstr(line, "available"), "ibuilder_mem_line");
    int64_t avail = ibuilder_available_bytes();
    CHECK(avail == -1 || avail > 0, "ibuilder_available_bytes: -1 where there is no jetsam ceiling, else the ceiling");
    CHECK(ibuilder_free_bytes(docs) > 0 && ibuilder_free_bytes("/no/such/folder/at/all") == -1, "ibuilder_free_bytes");
    ibuilder_address_space as;
    int asrc = ibuilder_address_space_info(&as);
#if defined(__APPLE__)
    CHECK(asrc == 0 && as.largest > 0 && as.free_total >= as.largest, "the address-space census (vm_region_recurse_64)");
#else
    (void) asrc;
#endif
    char ident[256];
    CHECK(ibuilder_self_identifier(ident, sizeof ident) > 0, "ibuilder_self_identifier");

    /* ---- the launch state machine ---- */
    CHECK(ibuilder_classify_boot_request(1, "attempted pid 5") == IBUILDER_BOOT_REQUEST_ATTEMPTED && ibuilder_classify_boot_request(1, "boot it") == IBUILDER_BOOT_REQUEST_ARMED &&
          ibuilder_classify_boot_request(0, NULL) == IBUILDER_BOOT_REQUEST_NONE, "ibuilder_classify_boot_request");
    CHECK(ibuilder_classify_framework(1, "app_build=B1\ngame=G\n", "B1") == IBUILDER_FRAMEWORK_FRESH && ibuilder_classify_framework(1, "app_build=B1\n", "B2") == IBUILDER_FRAMEWORK_STALE &&
          ibuilder_classify_framework(0, NULL, "B1") == IBUILDER_FRAMEWORK_NONE, "ibuilder_classify_framework");
    CHECK(ibuilder_classify_framework_id(1, "app_build=B1\nframework_id=F1\ngame=G\n", "B2", "F1") == IBUILDER_FRAMEWORK_FRESH &&
          ibuilder_classify_framework_id(1, "app_build=B1\nframework_id=F1\n", "B1", "F2") == IBUILDER_FRAMEWORK_STALE &&
          ibuilder_classify_framework_id(1, "app_build=B1\ngame=G\n", "B1", "F1") == IBUILDER_FRAMEWORK_FRESH &&
          ibuilder_classify_framework_id(1, "app_build=B1\ngame=G\n", "B2", "F1") == IBUILDER_FRAMEWORK_STALE &&
          ibuilder_classify_framework_id(1, "app_build=B1\nframework_id=F1\n", "B1", NULL) == IBUILDER_FRAMEWORK_FRESH &&
          ibuilder_classify_framework_id(0, NULL, "B1", "F1") == IBUILDER_FRAMEWORK_NONE &&
          ibuilder_classify_framework(1, "app_build=B1\nframework_id=F1\n", "B2") == IBUILDER_FRAMEWORK_STALE, "ibuilder_classify_framework_id: the framework id decides, an older stamp falls back to the build id");
    CHECK(ibuilder_classify_previous_run("stage 5/5: booting the guest (x)", "[gameapp] the player ended the game: sending SIGTERM to self", "[dm] graceful exit done") == IBUILDER_PREV_PLAYER_END &&
          ibuilder_classify_previous_run("stage 2/5: rewriting", "[gameapp] dlopen OK", NULL) == IBUILDER_PREV_KILLED_OR_UNKNOWN, "ibuilder_classify_previous_run");
    ibuilder_stage st;
    CHECK(ibuilder_parse_stage("stage 2/5: rewriting the modules (2/3: subsdk0)", &st) == 1 && st.step == 2 && st.of == 5 && st.fraction > 0.4 && strstr(st.text, "subsdk0"), "ibuilder_parse_stage");
    CHECK(ibuilder_parse_stage("stage 1/5 profile: derived in 5.2s", &st) == 0 && ibuilder_is_boot_stage("stage 5/5: booting the guest") == 1 && ibuilder_is_boot_stage("stage 4/5: signing") == 0, "stage line validity");

    const char *launch = argc > 2 ? argv[2] : docs;
    char json[4096];
    {
        /* a launch with a one-shot request and no interruption starts it once and consumes it */
        char p[1024]; snprintf(p, sizeof p, "%s/gameapp.args", launch);
        FILE *f = fopen(p, "w"); if (f) { fputs("chain-only\n", f); fclose(f); }
        int n = ibuilder_decide_launch(launch, "B1", 0, NULL, NULL, json, sizeof json);
        CHECK(n > 0 && strstr(json, "\"kind\":\"run\"") && strstr(json, "chain-only") && strstr(json, "\"interrupted\":false"), "ibuilder_decide_launch: a one-shot request starts a run");
        CHECK(!exists(launch, "gameapp.args") && exists(launch, "gameapp.args.done"), "  and is consumed (renamed to .done) where it is honoured");
        CHECK(ibuilder_begin_run(launch, "Grizzy.nsp", 0) == 0 && exists(launch, "gameapp.running"), "ibuilder_begin_run writes the interrupted-run marker");
        f = fopen(p, "w"); if (f) { fputs("chain-only\n", f); fclose(f); }
        n = ibuilder_decide_launch(launch, "B1", 0, NULL, NULL, json, sizeof json);
        CHECK(n > 0 && strstr(json, "\"interrupted\":true") && strstr(json, "\"kind\":\"nothing\"") && strstr(json, "PREVIOUS RUN WAS INTERRUPTED"),
              "a relaunch after a run that never finished reports it and starts NOTHING (one run per explicit action)");
        CHECK(!exists(launch, "gameapp.running") && !exists(launch, "gameapp.args"), "  the marker is reported once and the stale request dropped");
    }

    /* ---- log rotation ---- */
    CHECK(ibuilder_logs_mark(docs, 1795000000) == 0 && exists(docs, "gameapp.logmarks"), "ibuilder_logs_mark");

    printf("\n%d failure(s)\n", failures);
    return failures ? 1 : 0;
}
