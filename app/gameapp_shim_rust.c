/* app/gameapp_shim_rust.c -- the native half of iBuilder with NO CPython: a drop-in for gameapp/gameapp_shim.c.
 *
 * The C# app (GameAppMain.cs, GameAppExternal.cs, ... unchanged) imports the same `gameapp_*` symbols with [DllImport("__Internal")]. This file
 * exports exactly those and delegates the chain, the memory and address-space numbers, the guest boot and the live-shader queue to the Rust
 * library (libibuilder_ffi.a, include/ibuilder.h). What stays here is what only Apple frameworks, C# or third-party code can do:
 *
 *   * the signing identity (Keychain item / protected file) and the zsign call, offered to the chain as the ibuilder_sign_fn callback;
 *   * gameapp_nsz (nsz_decompress.c), the touch-pad and TV entries (dlsym on the loaded host, through ibuilder_host_symbol);
 *   * the managed shader translator's entry (gameapp_set_xlate) and SPIRV-Cross (spvc_cli.c), as the two ibuilder_tool_fn callbacks;
 *   * the "GameAppLiveShader" notification (ibuilder_live_notify_fn).
 *
 * What is gone with the interpreter: Py_BytesMain, the inittab modules (_dmgameapp, _capstone, lz4, zbic), choose_gil, the signal logger, the
 * guest-output redirect, the boot's time limit, the address-space reservation, the live queue and the memory numbers (all in Rust now).
 *
 * Sources of the parts kept as they were: the Keychain code and device_identity_sign come from gameapp/gameapp_shim.c of the iOSBuilder worktree
 * (app/SNAPSHOT.tsv names it in docs/APP-RUST.md); the pad entries keep its log lines and return codes.
 */
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <signal.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>

#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>
#include <objc/message.h>
#include <objc/runtime.h>
#include <TargetConditionals.h>

#include "ibuilder.h"
#include "inapp_shim.h"   /* sign/inapp_shim.h -- the zsign adapter */

/* ---- one line into Documents/gameapp.chain.log (one open per line: usable at any point of a launch) ----------------------------------------- */

static void documents_dir(char *out, size_t n)
{
    const char *home = getenv("HOME");
    snprintf(out, n, "%s/Documents", home ? home : ".");
}

static void logln(const char *fmt, ...)
{
    char line[2048];
    va_list ap; va_start(ap, fmt); vsnprintf(line, sizeof line, fmt, ap); va_end(ap);
    char docs[PATH_MAX], path[PATH_MAX + 32];
    documents_dir(docs, sizeof docs);
    snprintf(path, sizeof path, "%s/gameapp.chain.log", docs);
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (fd >= 0) { dprintf(fd, "%s\n", line); close(fd); }
    fprintf(stderr, "%s\n", line);
}

/* ---- memory and address space: the kernel's numbers, read by the Rust library ---------------------------------------------------------------- */

int gameapp_memline(char *buf, int n)
{
    char docs[PATH_MAX];
    documents_dir(docs, sizeof docs);
    return ibuilder_mem_line(docs, buf, n);
}

/* -1 where there is no ceiling (the simulator, a Mac): the Rust side maps os_proc_available_memory() == 0 to -1 too. */
long long gameapp_available(void) { return ibuilder_available_bytes(); }

void gameapp_address_space(long long *free_total, long long *largest)
{
    ibuilder_address_space a;
    if (ibuilder_address_space_info(&a) != 0) { if (free_total) *free_total = 0; if (largest) *largest = 0; return; }
    if (free_total) *free_total = (long long) a.free_total;
    if (largest) *largest = (long long) a.largest;
}

int gameapp_extended_va(void)
{
    ibuilder_address_space a;
    return ibuilder_address_space_info(&a) == 0 ? a.extended : 0;
}

/* The RAM entitlement read from this app's own signature: 1 / 0, or -1 when the signature could not be read. */
int gameapp_increased_memory(void)
{
    ibuilder_address_space a;
    return ibuilder_address_space_info(&a) == 0 ? a.increased_memory : -1;
}

/* get-task-allow: 1 a development signature, 0 a distribution one, -1 unreadable -- and -1 on the simulator, as before (its build carries no entitlements). */
int gameapp_get_task_allow(void)
{
#if TARGET_OS_SIMULATOR
    return -1;
#else
    ibuilder_address_space a;
    return ibuilder_address_space_info(&a) == 0 ? a.get_task_allow : -1;
#endif
}

/* ---- where the signing identity lives: this app's Keychain, or (only when the Keychain refuses) a protected file -----------------------------
 * Unchanged from gameapp/gameapp_shim.c. Some installers sign the app without a Keychain access group (errSecMissingEntitlement, -34018): the
 * identity then goes into Library/Application Support/gameapp.identity (mode 0600, data protection class A, excluded from backups). */
#ifndef F_SETPROTECTIONCLASS
#define F_SETPROTECTIONCLASS 64
#endif

static int identity_file_path(char *out, size_t n)
{
    const char *home = getenv("HOME");
    if (home == NULL || home[0] == 0) return -1;
    char dir[PATH_MAX];
    snprintf(dir, sizeof dir, "%s/Library/Application Support", home);
    mkdir(dir, 0700);
    snprintf(out, n, "%s/gameapp.identity", dir);
    return 0;
}

/* The identity blob (a binary plist {p12, passphrase}) from the Keychain, else from the file; NULL when neither. *where: 1 Keychain, 2 file. */
static CFDataRef identity_blob_copy(int *where)
{
    if (where) *where = 0;
    const void *svcKeys[] = { kSecClass, kSecAttrService, kSecAttrAccount, kSecReturnData, kSecMatchLimit };
    const void *svcVals[] = { kSecClassGenericPassword, CFSTR("dev.nxpocket.certificate"),
                              CFSTR("signing.identity"), kCFBooleanTrue, kSecMatchLimitOne };
    CFDictionaryRef query = CFDictionaryCreate(NULL, svcKeys, svcVals, 5,
                                               &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFTypeRef item = NULL;
    OSStatus status = query ? SecItemCopyMatching(query, &item) : errSecParam;
    if (query) CFRelease(query);
    if (status == errSecSuccess && item != NULL && CFGetTypeID(item) == CFDataGetTypeID()) {
        if (where) *where = 1;
        return (CFDataRef) item;
    }
    if (item) CFRelease(item);
    char path[PATH_MAX];
    if (identity_file_path(path, sizeof path) != 0) return NULL;
    int fd = open(path, O_RDONLY);
    if (fd < 0) return NULL;
    struct stat st;
    CFDataRef data = NULL;
    if (fstat(fd, &st) == 0 && st.st_size > 0 && st.st_size < (16 << 20)) {
        UInt8 *buf = malloc((size_t) st.st_size);
        if (buf && read(fd, buf, (size_t) st.st_size) == st.st_size) data = CFDataCreate(NULL, buf, st.st_size);
        if (buf) { memset(buf, 0, (size_t) st.st_size); free(buf); }
    }
    close(fd);
    if (data && where) *where = 2;
    return data;
}

static int identity_file_write(CFDataRef blob)
{
    char path[PATH_MAX], tmp[PATH_MAX + 8];
    if (identity_file_path(path, sizeof path) != 0) return -1;
    snprintf(tmp, sizeof tmp, "%s.part", path);
    int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) return -1;
    /* class A: complete protection, unreadable while locked. Set BEFORE the first byte goes in; its result used to be dropped and is logged now. When it is refused the file is KEPT (it is mode 0600): an install whose
     * Keychain refused the identity (the reason for this file) may have no data protection either, and then there would be no identity at all. */
    if (fcntl(fd, F_SETPROTECTIONCLASS, 1) != 0)
        logln("[gameapp] identity: complete data protection is not available on this install (errno %d): the identity file is kept at mode 0600 without it", errno);
    CFIndex len = CFDataGetLength(blob);
    int ok = write(fd, CFDataGetBytePtr(blob), (size_t) len) == len && fsync(fd) == 0;
    close(fd);
    if (!ok || rename(tmp, path) != 0) { unlink(tmp); return -1; }
    unsigned char one = 1;
    setxattr(path, "com.apple.MobileBackup", &one, 1, 0, 0);   /* not in an iCloud or computer backup */
    return 0;
}

/* Where the identity is kept now: 0 nowhere, 1 this app's Keychain, 2 the protected file (the Keychain refused). */
int gameapp_identity_where(void)
{
    int where = 0;
    CFDataRef blob = identity_blob_copy(&where);
    if (blob) CFRelease(blob);
    return where;
}

/* embedded.mobileprovision's path inside this bundle, or "" */
static void provision_path(char *out, size_t n)
{
    out[0] = 0;
    CFBundleRef bundle = CFBundleGetMainBundle();
    CFURLRef bundleURL = bundle ? CFBundleCopyBundleURL(bundle) : NULL;
    if (bundleURL) {
        CFURLRef profURL = CFURLCreateCopyAppendingPathComponent(NULL, bundleURL, CFSTR("embedded.mobileprovision"), false);
        if (profURL) { CFURLGetFileSystemRepresentation(profURL, true, (UInt8 *) out, (CFIndex) n); CFRelease(profURL); }
        CFRelease(bundleURL);
    }
}

/* The device identity path: embedded.mobileprovision + the Keychain .p12, both in memory (no secret file). rc as the header says: 10 no profile,
 * 11 unreadable, 12 no certificate, 13 the item carries no .p12, 14 the passphrase does not fit the buffer or is not a string, 20 zsign refused. */
static int device_identity_sign(const char *framework, const char *exe, const char *identifier)
{
    char profPath[4096];
    provision_path(profPath, sizeof profPath);
    struct stat pst;
    if (profPath[0] == 0 || stat(profPath, &pst) != 0) return 10;

    FILE *pf = fopen(profPath, "rb");
    uint8_t *prov = NULL; size_t provLen = 0;
    if (pf) { prov = malloc((size_t) pst.st_size); if (prov) provLen = fread(prov, 1, (size_t) pst.st_size, pf); fclose(pf); }
    if (prov == NULL || provLen == 0) { free(prov); return 11; }
    logln("[gameapp] identity: embedded.mobileprovision %zu bytes, in memory (never a file)", provLen);

    int where = 0;
    CFTypeRef item = identity_blob_copy(&where);
    if (item == NULL) {
        logln("[gameapp] identity: no certificate in the Keychain (nor in the protected file)");
        free(prov);
        return 12;
    }
    if (where == 2) logln("[gameapp] identity: read from the protected file (this install's Keychain refused it)");
    CFDataRef data = (CFDataRef) item;
    CFIndex blobLen = CFDataGetLength(data);
    UInt8 *blob = malloc((size_t) blobLen);
    if (blob == NULL) {
        logln("[gameapp] identity: no memory to read the certificate item (%ld bytes)", (long) blobLen);
        CFRelease(item);
        free(prov);
        return 11;
    }
    CFDataGetBytes(data, CFRangeMake(0, blobLen), blob);
    CFRelease(item);
    CFDataRef blobData = CFDataCreate(NULL, blob, blobLen);
    if (blobData == NULL) {
        logln("[gameapp] identity: no memory to read the certificate item (%ld bytes)", (long) blobLen);
        memset(blob, 0, (size_t) blobLen);
        free(blob); free(prov);
        return 11;
    }
    CFPropertyListRef pl = CFPropertyListCreateWithData(NULL, blobData, 0, NULL, NULL);
    CFRelease(blobData);
    const uint8_t *p12 = NULL; size_t p12Len = 0; char pass[512] = {0};
    if (pl && CFGetTypeID(pl) == CFDictionaryGetTypeID()) {
        CFDictionaryRef d = (CFDictionaryRef) pl;
        CFDataRef p12d = (CFDataRef) CFDictionaryGetValue(d, CFSTR("p12"));
        CFStringRef passs = (CFStringRef) CFDictionaryGetValue(d, CFSTR("passphrase"));
        if (p12d && CFGetTypeID(p12d) == CFDataGetTypeID()) { p12Len = (size_t) CFDataGetLength(p12d); p12 = CFDataGetBytePtr(p12d); }   /* a "p12" that is not data is no .p12: 13 */
        /* A "passphrase" that is not a string (a damaged item) is refused like one that does not fit: CFStringGetCString below would read garbage. */
        if (passs && CFGetTypeID(passs) != CFStringGetTypeID()) {
            logln("[gameapp] identity: the .p12's passphrase in the item is not a string: not signing with a wrong one");
            memset(pass, 0, sizeof pass);
            CFRelease(pl);
            free(blob); free(prov);
            return 14;
        }
        /* A passphrase that does not fit the buffer (511 bytes of UTF-8) used to be handed over cut or empty, and zsign then said "wrong password" with no hint why: refuse, and say so. */
        if (passs && !CFStringGetCString(passs, pass, sizeof pass, kCFStringEncodingUTF8)) {
            logln("[gameapp] identity: the .p12's passphrase is longer than %zu bytes of UTF-8 (or cannot be converted): not signing with a wrong one", sizeof pass - 1);
            memset(pass, 0, sizeof pass);
            CFRelease(pl);
            free(blob); free(prov);
            return 14;
        }
    }
    if (p12 == NULL || p12Len == 0) {
        if (pl) CFRelease(pl);
        free(blob); free(prov);
        return 13;
    }
    logln("[gameapp] identity: .p12 read from the Keychain, %zu bytes, in memory only", p12Len);

    char err[1024] = {0};
    int rc = nxpocket_zsign_sign_memory(p12, (int) p12Len, pass, prov, (int) provLen,
                                        NULL, 0, framework, exe, identifier, err, (int) sizeof err);
    logln("[gameapp] nxpocket_zsign_sign_memory -> %d%s%s", rc, err[0] ? ": " : "", err);
    if (pl) CFRelease(pl);
    free(blob);
    free(prov);
    memset(pass, 0, sizeof pass);
    return rc == 0 ? 0 : 20;
}

/* 0 = no item, 1 = an item holding a .p12 (+ passphrase). *p12Len, when asked for, is the .p12's size: the certificate itself is never read out here. */
int gameapp_identity_status(size_t *p12Len)
{
    if (p12Len) *p12Len = 0;
    CFTypeRef item = identity_blob_copy(NULL);
    if (item == NULL) return 0;

    int holding = 0;
    CFDataRef blobData = (CFDataRef) item;
    CFPropertyListRef pl = CFPropertyListCreateWithData(NULL, blobData, 0, NULL, NULL);
    if (pl && CFGetTypeID(pl) == CFDictionaryGetTypeID()) {
        CFDataRef p12 = (CFDataRef) CFDictionaryGetValue((CFDictionaryRef) pl, CFSTR("p12"));
        if (p12 && CFGetTypeID(p12) == CFDataGetTypeID() && CFDataGetLength(p12) > 0) {
            holding = 1;
            if (p12Len) *p12Len = (size_t) CFDataGetLength(p12);
        }
    }
    if (pl) CFRelease(pl);
    CFRelease(item);
    return holding;
}

/* Store {p12, passphrase} as a binary plist in this app's own Keychain, unlocked-this-device-only. Neither the bytes nor the password are written
 * to disk or logged. 0 = stored; 210..216 = the bytes could not be read into the plist; otherwise the Security framework's own status. */
int gameapp_identity_store_bytes(const uint8_t *p12, size_t len, const char *passphrase)
{
    if (p12 == NULL || len == 0) return 210;
    CFDataRef p12data = CFDataCreate(NULL, p12, (CFIndex) len);
    if (!p12data) return 211;
    CFStringRef pass = CFStringCreateWithCString(NULL, passphrase ? passphrase : "", kCFStringEncodingUTF8);
    if (!pass) { CFRelease(p12data); return 212; }

    const void *dkeys[] = { CFSTR("p12"), CFSTR("passphrase") };
    const void *dvals[] = { p12data, pass };
    CFDictionaryRef dict = CFDictionaryCreate(NULL, dkeys, dvals, 2,
                                              &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFDataRef blob = dict ? CFPropertyListCreateData(NULL, dict, kCFPropertyListBinaryFormat_v1_0, 0, NULL) : NULL;
    if (dict) CFRelease(dict);
    CFRelease(pass);
    CFRelease(p12data);
    if (!blob) return 213;

    const void *svcKeys[] = { kSecClass, kSecAttrService, kSecAttrAccount };
    const void *svcVals[] = { kSecClassGenericPassword, CFSTR("dev.nxpocket.certificate"), CFSTR("signing.identity") };
    CFDictionaryRef query = CFDictionaryCreate(NULL, svcKeys, svcVals, 3,
                                               &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (!query) { CFRelease(blob); return 214; }
    /* Update first, add only when there is nothing to update: a refused write leaves the previous item as it was. */
    const void *upKeys[] = { kSecValueData, kSecAttrAccessible };
    const void *upVals[] = { blob, kSecAttrAccessibleWhenUnlockedThisDeviceOnly };
    CFDictionaryRef changes = CFDictionaryCreate(NULL, upKeys, upVals, 2,
                                                 &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    OSStatus status = changes ? SecItemUpdate(query, changes) : errSecParam;
    if (changes) CFRelease(changes);
    if (status == errSecItemNotFound) {
        const void *addKeys[] = { kSecClass, kSecAttrService, kSecAttrAccount, kSecValueData, kSecAttrAccessible };
        const void *addVals[] = { kSecClassGenericPassword, CFSTR("dev.nxpocket.certificate"),
                                  CFSTR("signing.identity"), blob, kSecAttrAccessibleWhenUnlockedThisDeviceOnly };
        CFDictionaryRef add = CFDictionaryCreate(NULL, addKeys, addVals, 5,
                                                 &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        status = add ? SecItemAdd(add, NULL) : errSecParam;
        if (add) CFRelease(add);
    }
    CFRelease(query);
    if (status == errSecSuccess) {
        char path[PATH_MAX];
        if (identity_file_path(path, sizeof path) == 0) unlink(path);   /* the Keychain holds it now: no file left */
        CFRelease(blob);
        return 0;
    }
    /* errSecMissingEntitlement: this install has no Keychain access group. The identity goes into the protected file instead; 1 tells the app where. */
    int rc = (status == errSecMissingEntitlement && identity_file_write(blob) == 0) ? 1 : (int) status;
    CFRelease(blob);
    return rc;
}

/* ---- the chain's signing step: ibuilder_sign_fn -----------------------------------------------------------------------------------------------
 * What py_sign did. A device install carries embedded.mobileprovision and a Keychain .p12 -> zsign with the identity; the simulator has neither ->
 * zsign's ad hoc mode (the same signer, the same CodeDirectory, no CMS slot). Both in process: no codesign, no spawn. Fills mode / identifier / err. */
static void fill(char *dst, int cap, const char *src)
{
    if (dst == NULL || cap <= 0) return;
    snprintf(dst, (size_t) cap, "%s", src ? src : "");
}

static int sign_cb(void *user, const char *framework_dir, const char *exe, char *mode, int mode_cap, char *identifier, int identifier_cap,
                   char *err, int err_cap)
{
    (void) user;
    char ident[256] = {0};
    ibuilder_self_identifier(ident, (int) sizeof ident);
    if (ident[0] == 0) snprintf(ident, sizeof ident, "dev.ibuilder.app");
    fill(identifier, identifier_cap, ident);
    if (err && err_cap > 0) err[0] = 0;
    char prof[4096];
    provision_path(prof, sizeof prof);
    struct stat pst;
    if (prof[0] != 0 && stat(prof, &pst) == 0) {
        fill(mode, mode_cap, "device(zsign, in-memory .p12)");
        return device_identity_sign(framework_dir, exe, ident);
    }
    fill(mode, mode_cap, "adhoc(zsign, no certificate)");
    char e[1024] = {0};
    int rc = nxpocket_sign_adhoc(framework_dir, exe, ident, e, (int) sizeof e);
    fill(err, err_cap, e);
    return rc;
}

/* Stage 4b's fallback (ibuilder_options.sign_adhoc): the chain calls it when sign_cb found no identity on this install (rc 10 no embedded.mobileprovision, 12 no certificate,
 * 13 the item holds no .p12). The same zsign ad hoc mode as the simulator's branch above: the framework is then built for EXPORT ONLY (the stamp says signature=adhoc-export), because
 * iOS does not map an ad hoc framework from Documents on a device; an installer re-signs every executable of an exported IPA, so "Export as its own app" works without a .p12. */
static int sign_adhoc_cb(void *user, const char *framework_dir, const char *exe, char *mode, int mode_cap, char *identifier, int identifier_cap,
                         char *err, int err_cap)
{
    (void) user;
    char ident[256] = {0};
    ibuilder_self_identifier(ident, (int) sizeof ident);
    if (ident[0] == 0) snprintf(ident, sizeof ident, "dev.ibuilder.app");
    fill(identifier, identifier_cap, ident);
    fill(mode, mode_cap, "adhoc(zsign, no certificate: export only)");
    char e[1024] = {0};
    int rc = nxpocket_sign_adhoc(framework_dir, exe, ident, e, (int) sizeof e);
    fill(err, err_cap, e);
    logln("[gameapp] no signing identity here: %s/%s signed ad hoc for export only -> %d%s%s", framework_dir ? framework_dir : "?", exe ? exe : "?", rc, e[0] ? ": " : "", e);
    return rc;
}

/* "Sign again to play": a framework a build signed ad hoc because no .p12 was here (export only) is signed again with the identity the player has imported since, the way stage 4b
 * signs (device_identity_sign: embedded.mobileprovision + the Keychain .p12, in memory). No rebuild: the Mach-O, its Info.plist and its rewritten modules stay as they are. rc as
 * device_identity_sign: 0 signed, 10 no profile, 11 unreadable, 12 no certificate, 13 no .p12, 14 the passphrase does not fit, 20 zsign refused. The app rewrites the stamp on 0. */
int gameapp_sign_framework_again(const char *framework_dir, const char *exe)
{
    if (framework_dir == NULL || exe == NULL || framework_dir[0] == 0 || exe[0] == 0) return 11;
    char ident[256] = {0};
    ibuilder_self_identifier(ident, (int) sizeof ident);
    if (ident[0] == 0) snprintf(ident, sizeof ident, "dev.ibuilder.app");
    int rc = device_identity_sign(framework_dir, exe, ident);
    logln("[gameapp] sign again with this app's identity (an export-only build): %s/%s -> %d", framework_dir, exe, rc);
    return rc;
}

/* ---- the GPU: BC texture formats ---------------------------------------------------------------------------------------------------------------
 * MTLDevice.supportsBCTextureCompression of the system default device (A17 Pro and later iPhones, M1 and later iPads, every Apple silicon Mac and its simulator: yes). Without it the host
 * stops at mtl_init ("has no BC texture formats") unless the profile's tex_bc_cpu is on: then nvn_bcdec.c decodes them on the CPU. 1 yes, 0 no, -1 unknown (no Metal, no device, a
 * system without the property). Asked once, through the Objective-C runtime (this file is plain C; Metal is loaded by the app anyway). The chain gets it as IBUILDER_FLAG_GPU_NO_BC. */
static _Atomic int g_gpu_bc = -2;

int gameapp_gpu_bc_textures(void)
{
    int have = atomic_load(&g_gpu_bc);
    if (have != -2) return have;
    int v = -1;
    void *metal = dlopen("/System/Library/Frameworks/Metal.framework/Metal", RTLD_LAZY | RTLD_GLOBAL);
    typedef id (*create_fn)(void);
    create_fn create = metal != NULL ? (create_fn) dlsym(metal, "MTLCreateSystemDefaultDevice") : NULL;
    id dev = create != NULL ? create() : NULL;
    if (dev != NULL) {
        SEL bc = sel_registerName("supportsBCTextureCompression");
        if (class_respondsToSelector(object_getClass(dev), bc))
            v = ((BOOL (*)(id, SEL)) objc_msgSend)(dev, bc) ? 1 : 0;
        ((void (*)(id, SEL)) objc_msgSend)(dev, sel_registerName("release"));   /* MTLCreateSystemDefaultDevice returns +1 */
    }
    atomic_store(&g_gpu_bc, v);
    logln("[gameapp] GPU BC textures: %s", v == 1 ? "yes" : v == 0 ? "NO: every game decodes them on the CPU (slower, more memory)" : "unknown (treated as yes)");
    return v;
}

/* ---- the game view and the host (stage 5 is in Rust; these are the app's entries into the loaded host) ---------------------------------------- */

/* The game view's CAMetalLayer, from the managed side. Retained here (the view lives on the main thread; the host keeps this pointer from a
 * thread of its own while the guest runs) and handed to the Rust library, which gives it to the host's dm_front_set_layer right after the dlopen. */
void gameapp_set_game_layer(void *layer)
{
    if (layer != NULL) CFRetain(layer);
    ibuilder_set_game_layer(layer);
    logln("[gameapp] the game view's layer is %p: it goes to the host right after the framework is dlopen'd", layer);
}

/* The player's own "Quit game": the Rust library logs the cause, consumes the boot request and sends SIGTERM to this process. */
void gameapp_quit_game(void)
{
    char docs[PATH_MAX];
    documents_dir(docs, sizeof docs);
    ibuilder_quit_game(docs);
}

/* An NSZ before the extraction (gameapp/nsz_decompress.c, CommonCrypto + a standard zstd decoder): the lines go to the chain log, which the screen tails. */
int gameapp_nsz_to_nsp(const char *src, const char *dst, char *err, int errlen, void (*log)(const char *));
static void nsz_log(const char *line) { logln("%s", line); }

int gameapp_nsz(const char *src, const char *dst, char *err, int errlen)
{
    logln("[nsz] %s: rebuilding the NSP (zstd + AES-CTR, as `nsz -D`) into %s", src, dst);
    int rc = gameapp_nsz_to_nsp(src, dst, err, errlen, nsz_log);
    if (rc == 0) logln("[nsz] done: every NCA's SHA-256 matches its name");
    else logln("[nsz] FAILED: %s", err != NULL ? err : "?");
    return rc;
}

/* A Switch 2 package's cover before any build (crates/ibuilder-nup icon.rs, through ibuilder_nup_icon): the icon of the .nup at `path` and the title it is named by, read from the HEAD of the
 * file only (the table of contents and the meta block: no game file, no keys). Off the main thread like the other icon reads; one call at a time per file.
 *   Returns the icon's length in bytes: > 0 the icon (copied into buf when it fits, i.e. the result is <= cap; when it is larger nothing is written and the caller calls again with a buffer of that
 *   size), 0 the package carries no icon, < 0 not a readable package (IBUILDER_E_NUP; the reason is in the chain log) or a bad argument.
 *   *title_id (may be NULL) is the program ID the build names the game by (Documents/games/<id>/), 0 when the file does not say; it is set whenever the result is >= 0.
 * C#: [DllImport("__Internal")] static extern int gameapp_nup_icon(string path, byte[] buf, int cap, out ulong titleId); */
int gameapp_nup_icon(const char *path, uint8_t *buf, int cap, uint64_t *title_id)
{
    int rc = ibuilder_nup_icon(path, buf, cap, title_id);
    if (rc < 0) {
        char why[512] = {0};
        ibuilder_last_error(why, (int) sizeof why);
        logln("[nup] cover of %s: not read (%d): %s", path != NULL ? path : "(null)", rc, why);
    }
    return rc;
}

/* The same cover for a .nup that is not a local file (a game on the NAS, before its download: app patch 0084): the head is read through the caller's `read` (the C# reads the SFTP
 * stream at the offset asked), on this thread, inside this call; `size` is the package's length.  Same results as gameapp_nup_icon (crates/ibuilder-ffi ibuilder_nup_icon_read).
 * C#: [DllImport("__Internal")] static extern unsafe int gameapp_nup_icon_read(delegate* unmanaged<IntPtr, ulong, byte*, int, long> read, IntPtr ctx, ulong size, byte[] buf, int cap, out ulong titleId); */
int gameapp_nup_icon_read(ibuilder_nup_read_fn read, void *ctx, uint64_t size, uint8_t *buf, int cap, uint64_t *title_id)
{
    int rc = ibuilder_nup_icon_read(read, ctx, size, buf, cap, title_id);
    if (rc < 0) {
        char why[512] = {0};
        ibuilder_last_error(why, (int) sizeof why);
        logln("[nup] cover of a remote package (%llu bytes): not read (%d): %s", (unsigned long long) size, rc, why);
    }
    return rc;
}

/* The loaded host (the framework the boot dlopen'd): dm_host_probe_boot is in every host, so its absence means "not loaded yet". */
static void *host_sym(const char *name) { return ibuilder_host_symbol(name); }
static int host_loaded(void) { return host_sym("dm_host_probe_boot") != NULL; }

/* A retained UIView (the caller owns it), or NULL: the host is not loaded yet, or it is an older one with no gamepad (*why: 0 made, 1 not loaded yet, 2 no gamepad). */
void *gameapp_pad_create(double width, double height, int *why)
{
    if (!host_loaded()) { if (why) *why = 1; return NULL; }
    void *(*create)(double, double) = (void *(*)(double, double)) host_sym("dm_front_pad_create");
    if (create == NULL) {
        if (why) *why = 2;
        logln("[gameapp] this host has no dm_front_pad_create: no touch gamepad (an older host-prebuilt.dylib)");
        return NULL;
    }
    void *pad = create(width, height);
    if (why) *why = pad != NULL ? 0 : 2;
    logln("[gameapp] touch gamepad %s", pad != NULL ? "created over the game view (the engine's DmGamepad.m)" : "NOT created");
    return pad;
}

/* Lays the pad over the view and writes the picture's rectangle (x, y, w, h) into out; 0 when the host has no layout entry (out untouched). */
int gameapp_pad_layout(void *pad, double width, double height, double top, double left, double bottom, double right, double *out)
{
    void (*layout)(void *, double, double, double, double, double, double, double *) =
        (void (*)(void *, double, double, double, double, double, double, double *)) host_sym("dm_front_pad_layout");
    if (layout == NULL) return 0;
    layout(pad, width, height, top, left, bottom, right, out);
    return 1;
}

/* The phone as the controller while the picture is on an external display (load/front_tv.m). 0 when the host is not loaded or predates these entries. */
int gameapp_pad_layout_tv(void *pad, double width, double height)
{
    void (*fn)(void *, double, double) = (void (*)(void *, double, double)) host_sym("dm_front_pad_layout_tv");
    if (fn == NULL) return 0;
    fn(pad, width, height);
    return 1;
}

int gameapp_tv_attached(int on)
{
    void (*fn)(int) = (void (*)(int)) host_sym("dm_front_tv_attached");
    if (fn == NULL) return 0;
    fn(on);
    return 1;
}

/* The host's in-game menu (the gear's), a retained UIMenu for the "Playing on the TV" screen's own menu button. */
void *gameapp_game_menu(void)
{
    void *(*fn)(void) = (void *(*)(void)) host_sym("dm_game_menu");
    return fn ? fn() : NULL;
}

/* ---- the shader chain's native tools, in process: the two ibuilder_tool_fn callbacks ------------------------------------------------------------
 * The translator is C# (the engine's dmsh_xlate over Ryujinx.Graphics.Shader, compiled into this app; the managed side registers its entry here at
 * launch). Its entry takes only the arguments (dmsh_xlate <job.json> <outdir>), and the Rust library passes the full command line, program name
 * first, as gameapp_tool_fn always did: the trampoline drops argv[0]. SPIRV-Cross (gameapp/spvc_cli.c over the C API) takes argv[0] as main() does. */
typedef int (*gameapp_tool_fn)(int argc, const char **argv, char **out, char **err);
static _Atomic(gameapp_tool_fn) g_xlate_fn;
void gameapp_set_xlate(gameapp_tool_fn fn) { atomic_store(&g_xlate_fn, fn); }
int gameapp_spirv_cross(int argc, const char **argv, char **out_stdout, char **out_stderr);   /* gameapp/spvc_cli.c */

static int xlate_tool(int argc, const char **argv, char **out_stdout, char **out_stderr)
{
    gameapp_tool_fn fn = atomic_load(&g_xlate_fn);
    *out_stdout = NULL; *out_stderr = NULL;
    if (fn == NULL) { *out_stderr = strdup("the translator is not registered (the managed side did not start it)"); return 127; }
    if (argc < 1) { *out_stderr = strdup("no program name"); return 2; }
    return fn(argc - 1, argv + 1, out_stdout, out_stderr);
}

/* ---- the live-shader view hook: a "GameAppLiveShader" notification (busy 1 while the first game thread waits for a translation, then 0) ---- */
static void live_notify_cb(void *user, int busy)
{
    (void) user;
    const void *k[] = { CFSTR("busy") };
    const void *v[] = { busy ? kCFBooleanTrue : kCFBooleanFalse };
    CFDictionaryRef info = CFDictionaryCreate(NULL, k, v, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    CFNotificationCenterPostNotification(CFNotificationCenterGetLocalCenter(), CFSTR("GameAppLiveShader"), NULL, info, true);
    if (info) CFRelease(info);
}

/* ---- the .app directory (everything the process needs travels inside it) ------------------------------------------------------------------------ */
static int bundle_dir(char *out, size_t n)
{
    char exe[PATH_MAX];
    uint32_t size = sizeof exe;
    if (_NSGetExecutablePath(exe, &size) != 0) return -1;
    char real[PATH_MAX];
    if (realpath(exe, real) == NULL) return -1;
    char *slash = strrchr(real, '/');
    if (slash == NULL) return -1;
    *slash = '\0';
    snprintf(out, n, "%s", real);
    return 0;
}

/* ---- DM_PROFILE before the host is loaded ------------------------------------------------------------------------------------------------------------
 * The host reads its runtime profile lazily, at the first dm_profile() call, from DM_PROFILE; a call that comes before the variable is set loads the COMPILED generic profile for good
 * (reserve 0x100000000, alias 0, stack 0x1000000: the guest then crashed right after nnosInitialize). The Rust boot sets DM_PROFILE after the dlopen, the address-space reservation and the
 * layer hand-over, and the game view asks the host for its touch gamepad (front_pad.m, which calls dm_profile()) from the main thread as soon as the dlopen is done: when the reservation takes
 * long (the simulator: a 52 GB block, a census of 2,600 regions) the pad wins the race. Measured: an exported game's player on the simulator (docs/APP-RUST.md, "Found while testing").
 * The profile's path is in the config the app wrote; the file is final by the time any host is loaded (stage 5 is the only thing that loads one), so the variable is set here, before
 * the chain starts. The proper fix is in ibuilder-pipeline (sys/boot.rs: setenv before dlopen), which this package does not own; this keeps the shim safe either way. */
static void preset_profile_env(const char *docs, const char *cfg)
{
    char path[PATH_MAX] = {0}, line[PATH_MAX + 16];
    FILE *f = fopen(cfg, "r");
    if (f != NULL) {
        while (fgets(line, sizeof line, f) != NULL)
            if (strncmp(line, "profile=", 8) == 0) {
                snprintf(path, sizeof path, "%s", line + 8);
                path[strcspn(path, "\r\n")] = 0;
            }
        fclose(f);
    }
    if (path[0] == 0) snprintf(path, sizeof path, "%s/profile.tsv", docs);
    setenv("DM_PROFILE", path, 1);
    logln("[gameapp] chain: DM_PROFILE=%s set before any host is loaded", path);
}

/* ---- stopping a chain from another thread --------------------------------------------------------------------------------------------------------
 * The C# never asked for this (the interpreter had no way to stop a stage), so it is not among the 19 imports; it is here so that a C# patch needs nothing more than a
 * P/Invoke. Both calls are thread-safe and may be made from the UI thread while gameapp_run_chain is on the stack of the worker thread; the chain stops between modules, between
 * stages, after each shader of stage 3, and wherever a worker polls its flag (the Switch 2 package reader of stage 1, a streaming zstd of tens of GB, is one of them).
 *
 *   int gameapp_request_stop(int kind)   kind 1 = PAUSE, 2 = CANCEL, anything else withdraws the request; returns the request in effect (0, 1, 2). A cancel overrides a pause, a
 *                                        pause never overrides a cancel. The run then returns 131 (paused: everything kept, run the chain again to continue) or 130 (cancelled).
 *                                        A request is cleared when a run STARTS: raise it only while gameapp_run_chain is running, and keep a Pause pressed earlier yourself.
 *   void gameapp_cancel_chain(void)      a cancel of the run that is going (as gameapp_request_stop(2)); a no-op when no chain is running.
 *
 * gameapp_request_stop needs no handle, so it has no window between ibuilder_create and the store of g_chain: it is the one to P/Invoke. */
static _Atomic(ibuilder_t *) g_chain;
void gameapp_cancel_chain(void)
{
    ibuilder_t *h = atomic_load(&g_chain);
    if (h != NULL) ibuilder_cancel(h);
}

int gameapp_request_stop(int kind)
{
    int in_effect = ibuilder_request_stop(kind);
    logln("[gameapp] chain: stop requested (kind %d: %s) -> %d", kind, kind == IBUILDER_STOP_PAUSE ? "pause" : kind == IBUILDER_STOP_CANCEL ? "cancel" : "withdrawn", in_effect);
    return in_effect;
}

/* ---- the entry the managed app P/Invokes: gameapp_run_chain() -------------------------------------------------------------------------------------
 * The whole chain (profile -> rewrite -> shaders -> assemble -> sign -> boot) on the Rust library, on the caller's thread. Returns what
 * gameapp_run_chain returned: 0 ran, 1 NUP/profile/rewrite failed, 2 no extract_out (or no bundle, or the library could not be created), 3 boot-only had nothing to boot, 4 the game has
 * no shader library (IBUILDER_RC_NO_SHADER_LIBRARY: a build's stage 3 could not make it, so the game was assembled but neither booted nor stamped; or a boot-only launch's stage 3 could
 * not make a missing one, so the boot was refused), 5 no framework (IBUILDER_RC_NO_FRAMEWORK: the assembly or the signature failed, nothing to play), 75 ended on purpose before stage 3, 130 cancelled, 131 paused by the user (gameapp_request_stop(1)). The library's own code is
 * returned unchanged (nothing here maps it to 0, or reads the status text to decide: the library decides, review r4 §4.1). The boot runs inside this call, and the guest's own SIGTERM
 * may end the process without it returning. */
/* The C# reads 4 as "no shader library" and 5 as "no framework" (app/patches/0078, ChainResult.NoShaderLibrary / NoFramework): the header's values are pinned here, so a change of one
 * does not compile instead of silently turning that message into a generic failure. */
_Static_assert(IBUILDER_RC_NO_SHADER_LIBRARY == 4, "app/patches/0078 (ChainResult.NoShaderLibrary) reads 4: change both together");
_Static_assert(IBUILDER_RC_NO_FRAMEWORK == 5, "app/patches/0078 (ChainResult.NoFramework) reads 5: change both together");

int gameapp_run_chain(void)
{
    struct rlimit stack = { 16 * 1024 * 1024, 16 * 1024 * 1024 };   /* what the interpreter needed; harmless for Rust */
    setrlimit(RLIMIT_STACK, &stack);

    char bundle[PATH_MAX], docs[PATH_MAX], cfg[PATH_MAX + 32];
    if (bundle_dir(bundle, sizeof bundle) != 0) { fprintf(stderr, "[gameapp] no bundle dir\n"); return 2; }
    documents_dir(docs, sizeof docs);
    snprintf(cfg, sizeof cfg, "%s/gameapp.config", docs);

    ibuilder_options o;
    memset(&o, 0, sizeof o);
    o.struct_size = sizeof o;
    o.documents_dir = docs;
    o.bundle_dir = bundle;
    o.sign = sign_cb;
    o.xlate = xlate_tool;
    o.spirv_cross = gameapp_spirv_cross;
    o.live_notify = live_notify_cb;
    o.flags = IBUILDER_FLAG_BOOT_IN_PROCESS;   /* PIPELINE_BOOT=1: stage 5 runs in this process */
    o.sign_adhoc = sign_adhoc_cb;              /* no .p12 here: the framework is signed ad hoc, for export only */
    if (gameapp_gpu_bc_textures() == 0) o.flags |= IBUILDER_FLAG_GPU_NO_BC;   /* a GPU without BC textures: every boot decodes them on the CPU */

    preset_profile_env(docs, cfg);
    int err = 0;
    ibuilder_t *h = ibuilder_create(&o, &err);
    if (h == NULL) {
        char m[512] = {0};
        ibuilder_last_error(m, (int) sizeof m);
        logln("[gameapp] ibuilder_create failed (%d): %s", err, m);
        return 2;
    }
    atomic_store(&g_chain, h);
    logln("[gameapp] chain: ibuilder %s (ABI %u), bundle=%s", ibuilder_version(), ibuilder_abi_version(), bundle);
    logln("[gameapp] chain: no embedded interpreter; documents=%s config=%s", docs, cfg);
    int rc = ibuilder_run_chain(h, cfg);
    char st[1024] = {0};
    ibuilder_chain_status(h, st, (int) sizeof st);
    for (char *p = st; *p; p++) if (*p == '\n') *p = ' ';
    logln("[gameapp] chain: ibuilder_run_chain -> %d (%s)", rc, st);
    atomic_store(&g_chain, NULL);
    ibuilder_destroy(h);
    return rc;
}
