/* ibuilder.h -- the C ABI of the Rust chain driver (crate ibuilder-ffi). Hand-written; keep it in step with src/lib.rs.
 *
 * It replaces gameapp_run_chain() and the CPython host of gameapp/gameapp_shim.c: the app links libibuilder_ffi.a (iOS: aarch64-apple-ios) and calls ibuilder_run_chain() from its
 * worker thread instead of starting CPython.  See docs/chain/PIPELINE-RUST.md ("How the C# app switches") for the P/Invoke declarations.
 *
 * Rules of the ABI
 *   * Plain C: opaque handle, integer / pointer / struct arguments, no C++ and no Rust types.  All strings are NUL-terminated UTF-8.
 *   * No panic crosses the boundary: every entry point catches panics (catch_unwind; release builds use panic=abort, where a panic hook logs the message first, then the process aborts).
 *     A caught panic makes the call return IBUILDER_E_PANIC (or NULL / -1) and ibuilder_last_error() says what happened.
 *   * Callbacks may be called from any thread the chain uses (the log and progress ones from the thread running ibuilder_run_chain and from stage 3's worker threads, the tool ones from
 *     the translation threads, the sign one from the chain thread).  They must not call back into ibuilder_run_chain / ibuilder_destroy on the same handle.
 *   * Buffers returned to the caller are filled into caller-owned memory (buf, cap): the functions return the length the full text needs, so a result longer than cap is detected by
 *     `needed >= cap` (the text is truncated and NUL-terminated).
 *   * Thread safety: ibuilder_cancel() and ibuilder_request_stop() may be called from any thread at any time; everything else on a handle is for one thread at a time.  One chain run per process is the
 *     iOS reality (the process is the unit that frees memory); a second ibuilder_run_chain on a handle that is running returns IBUILDER_E_BUSY.
 */
#ifndef IBUILDER_H
#define IBUILDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define IBUILDER_ABI_VERSION 1u

/* ---- result codes of ibuilder_run_chain (the chain's own: what gameapp_run_chain returned) and ABI errors ---------------------------------- */
#define IBUILDER_RC_OK             0    /* the chain ran and no stage failed ("result=ok"; "shaders=none (...)" is a game with nothing to build, not a failure) */
#define IBUILDER_RC_FAILED         1    /* a stage that must succeed failed (NUP read, profile, rewrite), or a boot-only launch whose boot did not run (dlopen failed, no entry point) */
#define IBUILDER_RC_NO_EXTRACT_OUT 2    /* no extract_out in the config */
#define IBUILDER_RC_NOTHING_TO_BOOT 3   /* boot-only had nothing to boot */
#define IBUILDER_RC_NO_SHADER_LIBRARY 4 /* the game has no shader library and would draw nothing: stage 3 had to build one and could not ("shaders=failed (no library: ...)" in
                                         * ibuilder_chain_status(), "result=failed (shaders)").  A build went on to the assembly but booted nothing and wrote no framework stamp (the
                                         * game reads as not built); a boot-only launch refused the boot and consumed its request (the framework stays: a later boot tries the library
                                         * again).  "shaders=none (...)" (nothing to build) is not this: it keeps IBUILDER_RC_OK. */
#define IBUILDER_RC_NO_FRAMEWORK   5    /* no framework to play: stage 4 failed, the assembly or the signature ("assemble=failed" in ibuilder_chain_status(); the signer's rc 10-14 for a
                                         * missing or unreadable certificate, ...; with sign_adhoc set, 10, 12 and 13 sign ad hoc instead: see ibuilder_options).  Nothing was stamped or booted.  It wins over IBUILDER_RC_NO_SHADER_LIBRARY when stage 3 failed too
                                         * (the status names both).  It was IBUILDER_RC_OK, as in Python. */
#define IBUILDER_RC_RESTART        75   /* the chain ended on purpose before stage 3: close the app, the next launch continues (Documents/gameapp.continue) */
#define IBUILDER_RC_CANCELLED      130  /* ibuilder_cancel(), or ibuilder_request_stop(IBUILDER_STOP_CANCEL): stopped at the next safe point; the caller removes the partial build */
#define IBUILDER_RC_PAUSED         131  /* ibuilder_request_stop(IBUILDER_STOP_PAUSE): stopped at the next safe point with everything kept (the shader cache, the rewrite cache, the split's jobs): run the chain again to continue */
#define IBUILDER_E_BAD_ARG         (-1)
#define IBUILDER_E_BUSY            (-2)
#define IBUILDER_E_NUP             (-3)  /* ibuilder_nup_icon: the file is not a readable Switch 2 package (cannot be opened, is not a NUP, or its head is damaged or truncated) */
#define IBUILDER_E_IO              (-4)  /* ibuilder_begin_run: a file it had to write could not be written; ibuilder_last_error() says which and why */
#define IBUILDER_E_PANIC           (-99)

/* ---- the handle -------------------------------------------------------------------------------------------------------------------------- */
typedef struct ibuilder ibuilder_t;   /* opaque */

/* line: one log line, no trailing newline.  stream 0 = Documents/pipeline.stages.log (the chain: stage lines, [mem:...], timings), 1 = Documents/gameapp.chain.log
 * ("[gameapp] ..." lines: the boot, its time limit, signals).  The same lines are written to those files; the callback is for a live view. */
typedef void (*ibuilder_log_fn)(void *user, int stream, const char *line);

/* A stage line was written (gameapp.stage): stage k of n, the text after "stage k/n: ", and where the progress bar stands (0..1). */
typedef void (*ibuilder_progress_fn)(void *user, int stage_k, int stage_n, const char *text, double fraction);

/* A native tool run in process, as gameapp_tool_fn in the shim: argv is the tool's command line, program name first; returns the return code and sets *out_stdout / *out_stderr to
 * malloc()ed NUL-terminated strings (or NULL); the library free()s them.  xlate is the engine's dmsh_xlate (C#), spirv_cross is SPIRV-Cross (C). */
typedef int (*ibuilder_tool_fn)(int argc, const char **argv, char **out_stdout, char **out_stderr);

/* Sign <framework_dir>/<exe> in process (zsign: ad hoc on the simulator, the device identity when the app carries embedded.mobileprovision).  Fill mode ("device(zsign, in-memory .p12)"),
 * identifier (this app's CodeDirectory identifier) and err (a message when rc != 0).  rc: 0 signed, 10 no embedded.mobileprovision, 11 unreadable (the profile, or no memory to read the Keychain item), 12 no certificate in this app's
 * Keychain, 13 the item carries no .p12 (no "p12" entry, or one that is not data), 14 the .p12's passphrase is longer than the caller can hand over (the shim reads 511 bytes) OR is not a string, 20 the signer itself refused.  The Keychain and the .p12 stay on the caller's side:
 * this library never sees a secret.
 * mode, identifier and err are buffers of the *_cap bytes given: write a NUL-TERMINATED string of at most cap-1 characters into each (like snprintf).  The library does not trust that: it forces the LAST
 * byte of each buffer to 0 after the call, so a text that fills a buffer to the end is cut to cap-1 characters instead of being read past the buffer. */
typedef int (*ibuilder_sign_fn)(void *user, const char *framework_dir, const char *exe, char *mode, int mode_cap, char *identifier, int identifier_cap, char *err, int err_cap);

/* The live-shader view hook (a "GameAppLiveShader" notification in the app): busy = 1 when the first game thread starts waiting for a translation, 0 when the last stops. */
typedef void (*ibuilder_live_notify_fn)(void *user, int busy);

#define IBUILDER_FLAG_BOOT_IN_PROCESS 0x1u  /* PIPELINE_BOOT=1: stage 5 runs in this process (the app).  Without it the chain only builds. */
#define IBUILDER_FLAG_MOCK_STAGES     0x2u  /* deterministic stand-in stage workers (only in a build with the mock-stages feature: a test aid, refused otherwise) */
#define IBUILDER_FLAG_GPU_NO_BC       0x4u  /* the device's GPU has no BC texture formats (MTLDevice.supportsBCTextureCompression is NO, asked by the caller): every boot sets DM_PROF_TEX_BC_CPU=1, so the
                                             * host decodes BC1-7 on the CPU (nvn_bcdec.c) instead of stopping at mtl_init.  Not set (a GPU with them, or not known): nothing changes. */

typedef struct ibuilder_options {
    uint32_t struct_size;               /* sizeof(ibuilder_options): lets the struct grow without breaking old callers */
    const char *documents_dir;          /* Documents: the config, the markers, the logs, extract/, shaders/ ... */
    const char *bundle_dir;             /* the .app folder: dm/games/<id>/game.tsv of the games the engine knows, host-prebuilt.dylib */
    ibuilder_log_fn log;                void *log_user;        /* may be NULL */
    ibuilder_progress_fn progress;      void *progress_user;   /* may be NULL */
    ibuilder_sign_fn sign;              void *sign_user;       /* NULL: the Rust signer (ibuilder-sign, ad hoc only) */
    ibuilder_tool_fn xlate;             /* NULL: stage 3 cannot translate shaders */
    ibuilder_tool_fn spirv_cross;
    ibuilder_live_notify_fn live_notify; void *live_notify_user; /* may be NULL */
    uint32_t flags;
    /* Stage 4b's fallback when sign returned 10, 12 or 13 (no identity on this install: no profile, no certificate, no .p12): sign the framework AD HOC (called with sign_user, same
     * buffers and rc as sign).  The build then succeeds for EXPORT ONLY: the framework stamp says "signature=adhoc-export", no boot starts from it (iOS refuses to map such a framework
     * from Documents on a device), and the app offers Export as its own app (the installer re-signs every executable of the IPA).  NULL: no fallback, rc 10-13 fail the build as before. */
    ibuilder_sign_fn sign_adhoc;
} ibuilder_options;

uint32_t ibuilder_abi_version(void);          /* IBUILDER_ABI_VERSION of the library: compare with the header you compiled against */
const char *ibuilder_version(void);           /* crate version, static string */
/* Copy the last error of this thread (a caught panic, a bad argument) into buf; returns the length needed.  "" when there is none. */
int ibuilder_last_error(char *buf, int cap);

/* Create a handle.  NULL on error with *err set (IBUILDER_E_BAD_ARG: a NULL option, a struct_size that is too small, or the mock flag in a build without it).  Installs the fatal-signal
 * logger and the panic hook (once per process). */
ibuilder_t *ibuilder_create(const ibuilder_options *opts, int *err);
void ibuilder_destroy(ibuilder_t *h);
/* Ask the running chain to stop at the next safe point (between modules, between stages, and after each shader of stage 3).  Thread-safe.  The run then returns IBUILDER_RC_CANCELLED.
 * Equal to ibuilder_request_stop(IBUILDER_STOP_CANCEL) (the run's handle is cleared when the run starts: a cancel that came before a run is forgotten). */
void ibuilder_cancel(ibuilder_t *h);

/* ---- pause and cancel of a running chain ---------------------------------------------------------------------------------------------------------
 * A request is one byte of the PROCESS (no handle): the UI thread raises it while the chain runs on a thread of its own.  Every long loop of the chain reads it at its safe points -- between stages,
 * between the modules of the rewrite, after EACH shader of stage 3 (every worker finishes the shader in hand, writes it whole into the shader cache and starts no other), in the waits for a hot phone or
 * for memory -- so a run ends within about one shader (stage 3: the translator call in hand, a fraction of a second) once it is asked to.  Stage 2's rewrite ends between its modules (one module in hand is
 * finished) and the reading of the game's shaders at the start of stage 3 ends when it is done (seconds).
 *   IBUILDER_STOP_PAUSE   the run ends with IBUILDER_RC_PAUSED and ibuilder_chain_status() says "result=paused: by the user at <where>".  Nothing is thrown away: call ibuilder_run_chain again to continue.
 *   IBUILDER_STOP_CANCEL  the run ends with IBUILDER_RC_CANCELLED ("result=cancelled").  Same stop, same state on disk; the caller removes the partial build and keeps the shader cache.
 * A cancel overrides a pause; a pause never overrides a cancel.  The chain clears the request when a run STARTS: one that comes before a run (or after the last one ended) is not for the next one, so raise it
 * only while ibuilder_run_chain is on the stack.  Every line the stop adds to Documents/pipeline.stages.log starts with "[stop] ". */
#define IBUILDER_STOP_NONE   0
#define IBUILDER_STOP_PAUSE  1
#define IBUILDER_STOP_CANCEL 2
/* kind: IBUILDER_STOP_PAUSE or IBUILDER_STOP_CANCEL; any other value (0 is the clean one) withdraws the request.  Returns the request in effect afterwards (0, 1 or 2). */
int ibuilder_request_stop(int kind);
/* The request in effect now: 0 none, 1 pause, 2 cancel. */
int ibuilder_stop_requested(void);
/* Run the chain (gameapp_run_chain): read the config, then profile -> rewrite -> shaders -> assemble -> sign -> (boot).  config_path NULL = <Documents>/gameapp.config.  Blocks until the chain
 * ends; with IBUILDER_FLAG_BOOT_IN_PROCESS the guest runs inside this call and the process may end (its own SIGTERM) without the call returning. */
int ibuilder_run_chain(ibuilder_t *h, const char *config_path);
/* "stage=status" lines of the last run ("profile=ok\nrewrite=ok\nshaders=none (...)\nassemble=ok\n") and, last, "result=<timings result>".  Returns the length needed. */
int ibuilder_chain_status(ibuilder_t *h, char *buf, int cap);

/* ---- memory: the kernel's numbers (gameapp_memline, gameapp_available, gameapp_free, py_mem) ----------------------------------------------------------- */
typedef struct ibuilder_mem {
    uint64_t resident, peak, footprint;
    uint64_t available;                 /* os_proc_available_memory(): the ceiling iOS still grants; 0 where there is none (a Mac, the simulator) */
    uint64_t free_bytes;                /* free space on the volume of Documents (or HOME when documents is NULL) */
} ibuilder_mem;
int ibuilder_mem_info(const char *documents_dir, ibuilder_mem *out);                         /* 0 ok */
int ibuilder_mem_line(const char *documents_dir, char *buf, int cap);                        /* "resident N MiB, peak ..., footprint ..., available ... | free X GiB" */
int64_t ibuilder_available_bytes(void);                                                      /* -1 where there is no ceiling */
int64_t ibuilder_free_bytes(const char *path);                                               /* -1 when unreadable */

typedef struct ibuilder_address_space {
    uint64_t free_total, largest;       /* bytes of free address space now, and the largest hole */
    int32_t extended;                   /* Extended Virtual Addressing is in this app's signature */
    int32_t increased_memory;           /* the increased-memory-limit entitlement: 1, 0, -1 unreadable */
    int32_t get_task_allow;             /* 1 development signature, 0 distribution, -1 unreadable */
} ibuilder_address_space;
int ibuilder_address_space_info(ibuilder_address_space *out);                                /* 0 ok, -1 not available on this platform */
int ibuilder_self_identifier(char *buf, int cap);                                           /* this app's CodeDirectory identifier */

/* ---- the guest ------------------------------------------------------------------------------------------------------------------------------- */
/* Before the boot: the game view's CAMetalLayer (the caller keeps it alive: CFRetain it first).  Handed to the host's dm_front_set_layer right after the dlopen. */
void ibuilder_set_game_layer(void *layer);
/* dlsym on the loaded host (dm_front_pad_create, dm_front_pad_layout, ...); NULL before the framework is loaded or when the host lacks the symbol. */
void *ibuilder_host_symbol(const char *name);
/* The player's "Quit game": logs the cause, consumes the boot request, SIGTERM to self (the host answers with its graceful exit). */
void ibuilder_quit_game(const char *documents_dir);
/* A Mach-O initializer calls this first thing in a launch (iOS: before the managed runtime), to take the guest's address-space block early (DECISIONS traps 22-27).  Only on iOS. */
void ibuilder_early_init(const char *documents_dir);
/* The host's callback for a shader its library lacks (nvn_live_set): int handler(key, ctl, nctl, code, ncode, stage, cap).  Exported for callers that register it themselves.
 * key is NUL-terminated; ctl / code are valid for nctl / ncode bytes; stage is writable for cap bytes and receives a NUL-TERMINATED stage name (cut to cap-1 characters) for a result of 1 or 2.
 * Returns -1 refused / off / failed, 0 not translated yet (ask again), 1 translated by this ask, 2 translated by an earlier one.  Like every entry point it never lets a panic out: a failure of the
 * library itself answers -1 and leaves its message in ibuilder_last_error() (on the calling thread). */
int ibuilder_live_handler(const char *key, const void *ctl, uint32_t nctl, const void *code, uint32_t ncode, char *stage, int cap);

/* ---- the launch state machine (what GameAppMain.cs / GameAppUi.cs decided) ---------------------------------------------------------------------- */
#define IBUILDER_BOOT_REQUEST_NONE      0
#define IBUILDER_BOOT_REQUEST_ARMED     1
#define IBUILDER_BOOT_REQUEST_ATTEMPTED 2
int ibuilder_classify_boot_request(int exists, const char *content);                         /* content NULL = unreadable (reads as attempted) */
#define IBUILDER_FRAMEWORK_NONE  0
#define IBUILDER_FRAMEWORK_FRESH 1
#define IBUILDER_FRAMEWORK_STALE 2
int ibuilder_classify_framework(int on_disk, const char *stamp_content, const char *current_build);
#define IBUILDER_PREV_DESIGNED_END 0
#define IBUILDER_PREV_PLAYER_END   1
#define IBUILDER_PREV_DESIGNED_END_NO_CLEAN_EXIT 2
#define IBUILDER_PREV_FAULT_IN_GUEST 3
#define IBUILDER_PREV_KILLED_OR_UNKNOWN 4
int ibuilder_classify_previous_run(const char *stage_line, const char *chain_last, const char *guest_last);   /* any may be NULL */

typedef struct ibuilder_stage {
    int32_t valid, step, of;
    double fraction;                    /* where the bar stands when the step starts */
    char text[256];
} ibuilder_stage;
int ibuilder_parse_stage(const char *line, ibuilder_stage *out);                             /* GameAppUi.ParseStage; returns out->valid */
int ibuilder_is_boot_stage(const char *line);

/* Decide what a launch does (ReportInterruptedRunIfAny + the auto-run branches of ViewDidLoad) from the files in Documents.  Performs their file effects (marker removed, killed note,
 * one-shot args renamed to .done, withdrawn boot requests) and returns JSON:
 *   {"interrupted":bool, "action":{"kind":"nothing"|"run"|"continue"|"boot_only", ...}, "notes":["line for the screen's log", ...]}
 * "run" carries "request":"chain-only"|"boot-only"|"extraction" and, for an extraction, keys/base/update/out/flag; "continue" carries game, stage, why. */
int ibuilder_decide_launch(const char *documents_dir, const char *current_build, int cannot_run_here, const char *framework_path, const char *flat_dir, char *buf, int cap);
/* The interrupted-run marker of BeginRun / Finish. */
int ibuilder_begin_run(const char *documents_dir, const char *game, int boot_only);          /* writes gameapp.running, consumes gameapp.args; 0 ok, IBUILDER_E_IO the boot-only request
                                                                                             * could not be written (ibuilder_last_error()), IBUILDER_E_BAD_ARG a NULL documents_dir */
void ibuilder_finish_run(const char *documents_dir);                                         /* removes gameapp.running */

/* ---- log rotation (LogWindow): the last 10 minutes of every log file -------------------------------------------------------------------------------- */
int ibuilder_logs_mark(const char *documents_dir, int64_t now_epoch_secs);                   /* record each log file's length (every 30 s) */
int ibuilder_log_trim(const char *documents_dir, const char *name, int64_t now_epoch_secs);  /* 1 trimmed, 0 nothing to drop */

/* ---- a Switch 2 package's cover (the head of the .nup only) ------------------------------------------------------------------------------------------ */
/* The icon of the .nup at nup_path and the title it names the game by, for a library cover before any build.  Reads ONLY the head of the file: the 0x80-byte superblock, the table of
 * contents (15 MB for the real Ocarina package's 372,587 files, which is 25.6 GB long), the application package's meta block (the NACP and the icons, some kilobytes) and the
 * NPDMs (a few kilobytes each): no game file, no zstd frame of the content.  A NUP is not encrypted: no prod.keys are needed and none is read.  The icon is the one the build writes to
 * extract/icon.jpg for that package (the icon record, else the en-US localized one, else the first localized one); the package is the base game, else the first (the build's own choice
 * for application data).  Safe to call from any thread, one call at a time per file; a few milliseconds to a few hundred (the table of contents is parsed whole).
 *
 * Returns the icon's length in bytes:
 *   > 0                  the icon.  It is copied into buf when it fits (the result is <= cap); when it is larger NOTHING is written, and the caller calls again with a buffer of that
 *                        size.  The bytes are the image as stored (a JPEG, in every package seen), not NUL-terminated.
 *   0                    the package carries no icon (the real Ocarina of Time package carries none: its meta block has the NPDM and three small records, no NACP and no icon).
 *   IBUILDER_E_NUP       not a readable package; ibuilder_last_error() says why (never a crash, whatever the damage).
 *   IBUILDER_E_BAD_ARG   a NULL path, a negative cap, or a NULL buf with a cap > 0.  (buf NULL and cap 0 asks only for the length.)
 *   IBUILDER_E_PANIC     a panic was caught (debug builds; a release build aborts).
 * *title_id (when not NULL) is set whenever the result is >= 0: the program ID the build names the game by (the title ID of Documents/games/<id>/), or 0 when the file does not say. */
int ibuilder_nup_icon(const char *nup_path, uint8_t *buf, int cap, uint64_t *title_id);

/* ibuilder_nup_icon for a package that is not a local file (a .nup on the NAS, read over SFTP: app patch 0084): the same answer, the same buffer protocol; the bytes are asked of `read`
 * (offset, buffer, length -> how many bytes were read, 0 at the end, < 0 a failure) at the offsets the head needs, on the calling thread, inside this call.  `size` is the package's length.
 * IBUILDER_E_BAD_ARG also for a NULL read. */
typedef int64_t (*ibuilder_nup_read_fn)(void *ctx, uint64_t offset, uint8_t *buf, int len);
int ibuilder_nup_icon_read(ibuilder_nup_read_fn read, void *ctx, uint64_t size, uint8_t *buf, int cap, uint64_t *title_id);

#ifdef IBUILDER_TESTING
/* Only in a build with the mock-stages feature: panic inside an entry point (see tests/c/panic_abort.c). */
int ibuilder_test_panic(ibuilder_t *h);
#endif

#ifdef __cplusplus
}
#endif
#endif /* IBUILDER_H */
