//! # ibuilder-ffi
//!
//! The stable C ABI over `ibuilder-pipeline` (`include/ibuilder.h`): what the iOS app links instead of CPython. `gameapp_run_chain` becomes `ibuilder_run_chain`; the memory, address-space,
//! launch-state and live-shader pieces of `gameapp_shim.c` that were never CPython bridging come with it. The Keychain / identity, NSZ conversion, touch pad view and the managed translator
//! entry stay on the app's side (Apple frameworks, C#, third-party engine code): they reach this library through the callbacks in `ibuilder_options`.
//!
//! Every `extern "C"` entry point goes through [`guard`]: a panic never crosses the boundary. Release builds of this workspace use `panic = "abort"`, where `catch_unwind` cannot catch
//! anything; so a panic hook (installed by `ibuilder_create`) writes the panic to the chain log and stderr first, then the process aborts, which leaves the cause in the log the way the
//! shim's fatal-signal logger does. Debug / test builds unwind, and the guard turns a panic into `IBUILDER_E_PANIC` plus `ibuilder_last_error()`.

/// `LauncherBase` for the app: `dm/games/settings.py` in Rust (the rows of the game's settings sheet)
#[cfg(feature = "real-stages")]
pub mod launcher;

use ibuilder_pipeline::adapters::{Adapters, ShaderTools, SignOutcome, Signer, ToolFn, ToolOutput};
use ibuilder_pipeline::chain::{banner, load_config, run_chain};
use ibuilder_pipeline::ctx::{Ctx, View};
use ibuilder_pipeline::env::ProcessEnv;
use ibuilder_pipeline::layout::Docs;
use ibuilder_pipeline::log::{FileLog, Log, NullLog, TeeLog};
use ibuilder_pipeline::memory::MemSource;
use ibuilder_pipeline::stage::{is_boot_stage, parse_stage, set_stage_hook};
use ibuilder_pipeline::state::{
    self, BootRequestState, FrameworkState, LaunchAction, LaunchFacts, PreviousRun, RunRequest,
};
use ibuilder_pipeline::stop;
use ibuilder_pipeline::sys;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

/// The app's Rust allocator: blocks of 128 KiB and more are anonymous mappings that are unmapped when freed (`ibuilder-alloc`, docs/chain/MEMORY-RUST.md), so libmalloc does not keep freed big blocks in the footprint (the
/// chain's peak footprint on Grizzy went 852 -> 345 MiB). A `#[global_allocator]` of a staticlib covers the Rust code of this archive only: the C code linked beside it (capstone, zsign, SPIRV-Cross) and the managed
/// runtime keep their own `malloc`; no pointer crosses the boundary to be freed on the other side (the tool callbacks' strings are `malloc`ed by the caller and `free`d with `libc::free`, never by this allocator).
#[cfg(feature = "real-stages")]
#[global_allocator]
static GLOBAL: ibuilder_cli::alloc::BigBlocks = ibuilder_cli::alloc::BigBlocks;

pub const ABI_VERSION: u32 = 1;
pub const E_BAD_ARG: c_int = -1;
pub const E_BUSY: c_int = -2;
/// A file the call had to write could not be written (`ibuilder_begin_run`); `ibuilder_last_error()` says which and why.
pub const E_IO: c_int = -4;
pub const E_PANIC: c_int = -99;

pub const FLAG_BOOT_IN_PROCESS: u32 = 0x1;
pub const FLAG_MOCK_STAGES: u32 = 0x2;
/// The device's GPU has no BC texture formats (the app asked Metal: `supportsBCTextureCompression` is false): every boot decodes them on the CPU (`DM_PROF_TEX_BC_CPU=1`).
pub const FLAG_GPU_NO_BC: u32 = 0x4;

// ---- the C types -------------------------------------------------------------------------------------------------------------------------------------

pub type LogFn =
    Option<unsafe extern "C" fn(user: *mut c_void, stream: c_int, line: *const c_char)>;
pub type ProgressFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        k: c_int,
        n: c_int,
        text: *const c_char,
        fraction: c_double,
    ),
>;
pub type ToolCFn = Option<
    unsafe extern "C" fn(
        argc: c_int,
        argv: *const *const c_char,
        out: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int,
>;
pub type SignFn = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        framework_dir: *const c_char,
        exe: *const c_char,
        mode: *mut c_char,
        mode_cap: c_int,
        ident: *mut c_char,
        ident_cap: c_int,
        err: *mut c_char,
        err_cap: c_int,
    ) -> c_int,
>;
pub type LiveNotifyFn = Option<unsafe extern "C" fn(user: *mut c_void, busy: c_int)>;

#[repr(C)]
pub struct IbuilderOptions {
    pub struct_size: u32,
    pub documents_dir: *const c_char,
    pub bundle_dir: *const c_char,
    pub log: LogFn,
    pub log_user: *mut c_void,
    pub progress: ProgressFn,
    pub progress_user: *mut c_void,
    pub sign: SignFn,
    pub sign_user: *mut c_void,
    pub xlate: ToolCFn,
    pub spirv_cross: ToolCFn,
    pub live_notify: LiveNotifyFn,
    pub live_notify_user: *mut c_void,
    pub flags: u32,
    /// Stage 4b's fallback when `sign` found no identity (rc 10, 12, 13): sign the framework ad hoc, for export only. Called with `sign_user`. NULL: no fallback (the build fails).
    pub sign_adhoc: SignFn,
    /// Appended (ABI rule: fields are only ever appended; a caller whose `struct_size` ends before a field leaves it NULL): glslang's `glslangValidator` command line in process
    /// (the app's `gameapp_glslang`, app/c/glslang_cli.cpp), for OpenGL games' GLSL. NULL: an OpenGL game's shaders cannot be compiled (the game is not supported); nothing else changes.
    pub glslang: ToolCFn,
}

/// The smallest `struct_size` [`ibuilder_create`] takes: the struct before its appended fields (`glslang`), as the header of 2026-11-22 declared it
pub const OPTIONS_MIN_SIZE: usize = std::mem::offset_of!(IbuilderOptions, glslang);

#[repr(C)]
pub struct IbuilderMem {
    pub resident: u64,
    pub peak: u64,
    pub footprint: u64,
    pub available: u64,
    pub free_bytes: u64,
}

#[repr(C)]
pub struct IbuilderAddressSpace {
    pub free_total: u64,
    pub largest: u64,
    pub extended: i32,
    pub increased_memory: i32,
    pub get_task_allow: i32,
}

#[repr(C)]
pub struct IbuilderStage {
    pub valid: i32,
    pub step: i32,
    pub of: i32,
    pub fraction: c_double,
    pub text: [c_char; 256],
}

// ---- the handle ----------------------------------------------------------------------------------------------------------------------------------------

/// A raw user pointer the caller promises is usable from any thread (the header says callbacks may run on any chain thread).
#[derive(Clone, Copy)]
struct UserPtr(*mut c_void);
// SAFETY: the C contract (ibuilder.h) makes the callback's user data thread-agnostic.
unsafe impl Send for UserPtr {}
unsafe impl Sync for UserPtr {}
impl UserPtr {
    /// A method, so a closure captures the whole (Send + Sync) wrapper and not its raw-pointer field.
    fn get(self) -> *mut c_void {
        self.0
    }
}

pub struct Ibuilder {
    docs: PathBuf,
    bundle: PathBuf,
    log: LogFn,
    log_user: UserPtr,
    progress: ProgressFn,
    progress_user: UserPtr,
    sign: SignFn,
    sign_user: UserPtr,
    sign_adhoc: SignFn,
    xlate: ToolCFn,
    spirv_cross: ToolCFn,
    glslang: ToolCFn,
    live_notify: LiveNotifyFn,
    live_notify_user: UserPtr,
    flags: u32,
    cancel: Arc<AtomicBool>,
    running: AtomicBool,
    status: Mutex<String>,
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_error(msg: impl Into<String>) {
    let m = msg.into();
    LAST_ERROR.with(|e| *e.borrow_mut() = m);
}

/// Run `f`, turning a panic into `default` and a recorded error: no panic crosses the C boundary.
fn guard<R>(default: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic".to_string());
            set_error(format!("panic: {msg}"));
            default
        }
    }
}

/// # Safety
/// `p` is NULL or a NUL-terminated string valid for the call.
unsafe fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        // SAFETY: the caller's contract (ibuilder.h: all strings are NUL-terminated).
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Copy `s` into `buf` (cap bytes, NUL-terminated); returns the length the whole text needs.
///
/// # Safety
/// `buf` is NULL or valid for `cap` bytes.
unsafe fn fill(buf: *mut c_char, cap: c_int, s: &str) -> c_int {
    if !buf.is_null() && cap > 0 {
        let n = s.len().min(cap as usize - 1);
        // SAFETY: n + 1 <= cap bytes are written into the caller's buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
    }
    s.len() as c_int
}

fn c_string(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

thread_local! {
    /// The line handed to a C callback, kept between calls (`with_c_line`).
    static C_LINE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Call `f` with `s` as a NUL-terminated C string (a NUL inside it becomes a space, as `c_string` does) from a buffer this thread keeps: a log line costs no allocation once the buffer has
/// grown (review r4 §6.3: `c_string` made two per line). A call made while the buffer is in use (a callback that comes back into a log on this thread) gets its own `c_string`.
fn with_c_line<R>(s: &str, f: impl FnOnce(*const c_char) -> R) -> R {
    let mut f = Some(f);
    let r = C_LINE.try_with(|cell| {
        let mut buf = cell.try_borrow_mut().ok()?;
        buf.clear();
        buf.extend_from_slice(s.as_bytes());
        for b in buf.iter_mut() {
            if *b == 0 {
                *b = b' ';
            }
        }
        buf.push(0);
        let r = (f.take()?)(buf.as_ptr() as *const c_char);
        if buf.capacity() > 64 << 10 {
            *buf = Vec::new(); // one huge line does not keep its room
        }
        Some(r)
    });
    match r {
        Ok(Some(r)) => r,
        _ => {
            let c = c_string(s);
            (f.take().expect("the callback was not called"))(c.as_ptr())
        }
    }
}

// ---- callbacks as Rust traits -----------------------------------------------------------------------------------------------------------------------------

struct CallbackLog {
    f: unsafe extern "C" fn(*mut c_void, c_int, *const c_char),
    user: UserPtr,
    stream: c_int,
    progress: ProgressFn,
    progress_user: UserPtr,
}

impl Log for CallbackLog {
    fn line(&self, s: &str) {
        // SAFETY: the caller's callback, with a NUL-terminated line that lives for the call.
        with_c_line(s, |c| unsafe { (self.f)(self.user.0, self.stream, c) });
        let _ = (self.progress, self.progress_user);
    }
}

type SignCFn = unsafe extern "C" fn(
    *mut c_void,
    *const c_char,
    *const c_char,
    *mut c_char,
    c_int,
    *mut c_char,
    c_int,
    *mut c_char,
    c_int,
) -> c_int;

struct CallbackSigner {
    f: SignCFn,
    /// `sign_adhoc` of the options: stage 4b's fallback with no identity (export only); `None` when the caller gave none
    adhoc: Option<SignCFn>,
    user: UserPtr,
}

impl CallbackSigner {
    fn call(&self, f: SignCFn, framework_dir: &Path, exe: &str) -> SignOutcome {
        let (fd, ex) = (c_string(&framework_dir.to_string_lossy()), c_string(exe));
        let (mut mode, mut ident, mut err) =
            ([0 as c_char; 256], [0 as c_char; 256], [0 as c_char; 1024]);
        // SAFETY: the caller's sign callback with valid NUL-terminated inputs and writable output buffers of the stated sizes.
        let rc = unsafe {
            f(
                self.user.0,
                fd.as_ptr(),
                ex.as_ptr(),
                mode.as_mut_ptr(),
                256,
                ident.as_mut_ptr(),
                256,
                err.as_mut_ptr(),
                1024,
            )
        };
        // The callback fills its buffers up to the capacity it was given; a text that fills one to the last byte has no NUL, and `CStr::from_ptr` would read on past the array. The last byte of
        // each is forced to 0 here (a text of exactly the capacity loses its last character, as `fill` does), whatever the callback did.
        for b in [&mut mode[..], &mut ident[..], &mut err[..]] {
            if let Some(last) = b.last_mut() {
                *last = 0;
            }
        }
        let s = |b: &[c_char]| {
            unsafe { CStr::from_ptr(b.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        };
        SignOutcome {
            rc,
            mode: s(&mode),
            identifier: s(&ident),
            err: s(&err),
        }
    }
}

impl Signer for CallbackSigner {
    fn sign(&self, framework_dir: &Path, exe: &str, _v: &View<'_>) -> SignOutcome {
        self.call(self.f, framework_dir, exe)
    }
    fn sign_adhoc(&self, framework_dir: &Path, exe: &str, _v: &View<'_>) -> Option<SignOutcome> {
        self.adhoc.map(|f| self.call(f, framework_dir, exe))
    }
    fn identifier(&self) -> String {
        sys::entitlements::self_identifier()
    }
}

fn tool_fn(
    f: unsafe extern "C" fn(
        c_int,
        *const *const c_char,
        *mut *mut c_char,
        *mut *mut c_char,
    ) -> c_int,
) -> ToolFn {
    Arc::new(move |argv: &[String]| {
        let owned: Vec<CString> = argv.iter().map(|a| c_string(a)).collect();
        let ptrs: Vec<*const c_char> = owned
            .iter()
            .map(|c| c.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let (mut out, mut err): (*mut c_char, *mut c_char) =
            (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: the caller's tool callback; argv is a NULL-terminated array of NUL-terminated strings that live for the call; it sets out/err to malloc()ed strings or leaves NULL.
        let rc = unsafe { f(argv.len() as c_int, ptrs.as_ptr(), &mut out, &mut err) };
        let take = |p: *mut c_char| -> String {
            if p.is_null() {
                return String::new();
            }
            // SAFETY: a malloc()ed NUL-terminated string by contract; freed once, here.
            unsafe {
                let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                libc::free(p as *mut c_void);
                s
            }
        };
        ToolOutput {
            rc,
            stdout: take(out),
            stderr: take(err),
        }
    })
}

fn install_panic_hook(sink: Arc<Mutex<Option<Arc<dyn Log>>>>) {
    static ONCE: Once = Once::new();
    ONCE.call_once(move || {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let line = format!("[ibuilder] PANIC: {info}");
            if let Ok(g) = sink.lock()
                && let Some(l) = g.as_ref()
            {
                l.line(&line);
            }
            eprintln!("{line}");
            prev(info);
        }));
    });
}

type PanicSink = Arc<Mutex<Option<Arc<dyn Log>>>>;

fn panic_sink() -> PanicSink {
    // a process-wide sink behind an Arc so the hook closure owns a handle
    static CELL: std::sync::OnceLock<PanicSink> = std::sync::OnceLock::new();
    CELL.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

// ---- entry points: the handle -------------------------------------------------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// # Safety
/// `buf` is NULL or valid for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_last_error(buf: *mut c_char, cap: c_int) -> c_int {
    guard(0, || {
        let e = LAST_ERROR.with(|e| e.borrow().clone());
        // SAFETY: forwarded contract.
        unsafe { fill(buf, cap, &e) }
    })
}

/// # Safety
/// `opts` is NULL or points to an `IbuilderOptions` valid for the call; its strings are NUL-terminated; `err` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_create(
    opts: *const IbuilderOptions,
    err: *mut c_int,
) -> *mut Ibuilder {
    guard(std::ptr::null_mut(), || {
        let fail = |code: c_int, msg: &str| -> *mut Ibuilder {
            set_error(msg);
            if !err.is_null() {
                // SAFETY: writable by contract.
                unsafe { *err = code };
            }
            std::ptr::null_mut()
        };
        if opts.is_null() {
            return fail(E_BAD_ARG, "ibuilder_create: opts is NULL");
        }
        // SAFETY: non-NULL; struct_size is the first field of every version of the struct.
        let size = unsafe { std::ptr::addr_of!((*opts).struct_size).read() } as usize;
        if size < OPTIONS_MIN_SIZE {
            return fail(
                E_BAD_ARG,
                "ibuilder_create: struct_size is smaller than this library's ibuilder_options (ABI mismatch)",
            );
        }
        // Only the caller's struct_size bytes are read; the fields appended after them stay zero (NULL). All-zero is a valid IbuilderOptions (NULL pointers, None callbacks).
        // SAFETY: `opts` points to at least `size` readable bytes by contract, and at most size_of::<IbuilderOptions>() are copied into a zeroed struct of that size.
        let copied = unsafe {
            let mut c: IbuilderOptions = std::mem::zeroed();
            std::ptr::copy_nonoverlapping(
                opts as *const u8,
                &mut c as *mut IbuilderOptions as *mut u8,
                size.min(std::mem::size_of::<IbuilderOptions>()),
            );
            c
        };
        let o = &copied;
        // SAFETY: strings are NUL-terminated by contract.
        let (docs, bundle) = unsafe { (cstr(o.documents_dir), cstr(o.bundle_dir)) };
        let Some(docs) = docs.filter(|d| !d.is_empty()) else {
            return fail(E_BAD_ARG, "ibuilder_create: documents_dir is required");
        };
        if o.flags & FLAG_MOCK_STAGES != 0 && !cfg!(feature = "mock-stages") {
            return fail(
                E_BAD_ARG,
                "ibuilder_create: IBUILDER_FLAG_MOCK_STAGES needs a build with the mock-stages feature",
            );
        }
        let h = Ibuilder {
            bundle: PathBuf::from(bundle.unwrap_or_else(|| docs.clone())),
            docs: PathBuf::from(docs),
            log: o.log,
            log_user: UserPtr(o.log_user),
            progress: o.progress,
            progress_user: UserPtr(o.progress_user),
            sign: o.sign,
            sign_user: UserPtr(o.sign_user),
            sign_adhoc: o.sign_adhoc,
            xlate: o.xlate,
            spirv_cross: o.spirv_cross,
            glslang: o.glslang,
            live_notify: o.live_notify,
            live_notify_user: UserPtr(o.live_notify_user),
            flags: o.flags,
            cancel: Arc::new(AtomicBool::new(false)),
            running: AtomicBool::new(false),
            status: Mutex::new(String::new()),
        };
        install_panic_hook(panic_sink());
        if let Ok(f) = FileLog::append(&Docs::new(&h.docs).chain_log()) {
            *panic_sink().lock().unwrap() = Some(Arc::new(f));
        }
        Box::into_raw(Box::new(h))
    })
}

/// # Safety
/// `h` is NULL or a handle from `ibuilder_create` not used again; no run may be in progress.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_destroy(h: *mut Ibuilder) {
    guard((), || {
        if !h.is_null() {
            // SAFETY: a Box this library made, destroyed once.
            drop(unsafe { Box::from_raw(h) });
        }
    })
}

/// # Safety
/// `h` is NULL or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_cancel(h: *mut Ibuilder) {
    guard((), || {
        // SAFETY: a live handle by contract; only an atomic is touched.
        if let Some(h) = unsafe { h.as_ref() } {
            h.cancel.store(true, Ordering::SeqCst);
            // the same as `ibuilder_request_stop(2)`: the workers of stage 3 read the process-wide request, not this handle's flag
            stop::request(stop::CANCEL);
        }
    })
}

/// Ask the running chain to PAUSE (`kind` 1) or CANCEL (`kind` 2) at its next safe point; any other value withdraws a request. Process-wide, from any thread, at any time; no handle is needed.
/// A cancel overrides a pause and a pause never overrides a cancel. Returns the request in effect afterwards (0, 1 or 2). The chain clears the request when a run starts.
#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_request_stop(kind: c_int) -> c_int {
    guard(E_PANIC, || {
        stop::request(u8::try_from(kind).unwrap_or(0)) as c_int
    })
}

/// The request in effect now: 0 none, 1 pause, 2 cancel.
#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_stop_requested() -> c_int {
    guard(E_PANIC, || stop::requested() as c_int)
}

/// The build mode (the build screen's "Mode"): `on` 1 = Fast (the workers keep the chain thread's QoS: faster and hotter), 0 = Cool (utility QoS). Process-wide, from any thread, at any time:
/// a running build's shader translation follows within a shader, the other stages from the next build. Every run starts from the app setting `fast_build` (`ibuilder_run_chain`), so the app
/// saves the setting too. Timing only, never a result.
#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_set_fast_build(on: c_int) {
    guard((), || sys::qos::set_fast_build(on != 0))
}

/// For the tool callbacks the app gives the chain (translator, SPIRV-Cross): the calling build worker moves to the build mode in effect if it changed. A no-op on any other thread.
#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_qos_follow() {
    guard((), sys::qos::follow)
}

impl Ibuilder {
    fn make_logs(&self) -> (Arc<dyn Log>, Arc<dyn Log>) {
        let docs = Docs::new(&self.docs);
        let _ = std::fs::create_dir_all(docs.root());
        let wrap = |file: Option<FileLog>, stream: c_int| -> Arc<dyn Log> {
            let mut sinks: Vec<Arc<dyn Log>> = Vec::new();
            if let Some(f) = file {
                sinks.push(Arc::new(f));
            }
            if let Some(f) = self.log {
                sinks.push(Arc::new(CallbackLog {
                    f,
                    user: self.log_user,
                    stream,
                    progress: self.progress,
                    progress_user: self.progress_user,
                }));
            }
            if sinks.is_empty() {
                Arc::new(NullLog)
            } else {
                Arc::new(TeeLog(sinks))
            }
        };
        let stages = FileLog::open(&docs.stages_log(), &banner()).ok();
        let chain = FileLog::append(&docs.chain_log()).ok();
        if let Some(c) = &chain {
            #[cfg(unix)]
            sys::signals::install_signal_logger(c.raw_fd());
            c.line(&format!(
                "[gameapp] ==== chain run start, pid {} ====",
                std::process::id()
            ));
        }
        (wrap(stages, 0), wrap(chain, 1))
    }

    fn adapters(&self, live: Arc<ibuilder_pipeline::live::LiveQueue>) -> Adapters {
        #[cfg(feature = "mock-stages")]
        let mut a = if self.flags & FLAG_MOCK_STAGES != 0 {
            ibuilder_pipeline::mock::mock_adapters("0100DFA027504000")
        } else {
            Adapters::unwired()
        };
        #[cfg(not(feature = "mock-stages"))]
        let mut a = Adapters::unwired();
        #[cfg(feature = "real-stages")]
        if !cfg!(feature = "mock-stages") || self.flags & FLAG_MOCK_STAGES == 0 {
            a = ibuilder_cli::adapters::real_adapters(&sys::entitlements::self_identifier());
            // the settings sheet's rows (dm/games/settings.py): ibuilder-cli has no LauncherBase of its own
            #[cfg(feature = "real-stages")]
            if a.launcher.is_none() {
                a.launcher = Some(Arc::new(launcher::RustLauncher));
            }
        }
        if let Some(f) = self.sign {
            a.signer = Arc::new(CallbackSigner {
                f,
                adhoc: self.sign_adhoc,
                user: self.sign_user,
            });
        }
        if let (Some(x), Some(s)) = (self.xlate, self.spirv_cross) {
            a.tools = Some(ShaderTools {
                xlate: tool_fn(x),
                spirv_cross: tool_fn(s),
                glslang: self.glslang.map(tool_fn),
            });
        }
        #[cfg(feature = "mock-stages")]
        let mock = self.flags & FLAG_MOCK_STAGES != 0;
        #[cfg(not(feature = "mock-stages"))]
        let mock = false;
        if !mock {
            a.boot = Some(Arc::new(sys::boot::DlopenBoot::new(live)));
        }
        a
    }
}

/// # Safety
/// `h` is a live handle; `config_path` is NULL or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_run_chain(h: *mut Ibuilder, config_path: *const c_char) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: a live handle by contract.
        let Some(h) = (unsafe { h.as_ref() }) else {
            set_error("ibuilder_run_chain: NULL handle");
            return E_BAD_ARG;
        };
        if h.running.swap(true, Ordering::SeqCst) {
            set_error("ibuilder_run_chain: a run is already in progress on this handle");
            return E_BUSY;
        }
        struct Done<'a>(&'a Ibuilder);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.running.store(false, Ordering::SeqCst);
                set_stage_hook(None);
            }
        }
        let _done = Done(h);
        h.cancel.store(false, Ordering::SeqCst);
        // SAFETY: NUL-terminated by contract.
        let cfg_path = unsafe { cstr(config_path) }.map(PathBuf::from);
        let (log, chain_log) = h.make_logs();
        *panic_sink().lock().unwrap() = Some(chain_log.clone());
        let docs = Docs::new(&h.docs);
        let mem: Arc<dyn MemSource> = Arc::new(sys::mem::SystemMem::new(&h.docs));
        let env = Arc::new(ProcessEnv::new());
        let mut ctx: Ctx = Ctx::builder(&h.docs)
            .bundle(&h.bundle)
            .env(env)
            .log(log)
            .chain_log(chain_log)
            .mem(mem)
            .boot_enabled(h.flags & FLAG_BOOT_IN_PROCESS != 0)
            .gpu_without_bc(h.flags & FLAG_GPU_NO_BC != 0)
            .build();
        // the app's "Fast build" setting (`fast_build 1` in gameapp.settings.tsv, default off), read at every run: the build's workers keep this thread's QoS instead of utility (sys/qos.rs)
        sys::qos::set_fast_build(ibuilder_pipeline::shaders::fast_build_setting(&ctx));
        sys::qos::record_caller_class();
        // speed-up B9: the app's "Optimized translator (experimental)" setting (`xlate_optimized 1`, default off): the translator runs without its DebugMode for this run's builds
        let optimized = ibuilder_pipeline::shaders::app_setting(&ctx, "xlate_optimized")
            .is_some_and(|v| {
                matches!(
                    v.trim().to_lowercase().as_str(),
                    "1" | "on" | "yes" | "true"
                )
            });
        sys::qos::set_xlate_optimized(optimized);
        if optimized {
            ctx.log("chain: the optimized translator is on (experimental: the translator runs without its DebugMode; a game built so may show graphics errors)");
        }
        ctx.cancel = h.cancel.clone();
        ctx.adapters = h.adapters(ctx.live.clone());
        if let Some(f) = h.live_notify {
            let user = h.live_notify_user;
            // SAFETY: the caller's callback, user data thread-agnostic by contract.
            ctx.live.set_notify(Arc::new(move |busy| unsafe {
                f(user.get(), busy as c_int)
            }));
        }
        if let Some(f) = h.progress {
            let user = h.progress_user;
            set_stage_hook(Some(Arc::new(move |k, n, text| {
                let p = parse_stage(&format!("stage {k}/{n}: {text}"));
                // SAFETY: the caller's callback with a NUL-terminated string that lives for the call.
                with_c_line(text, |c| unsafe {
                    f(user.get(), k as c_int, n as c_int, c, p.fraction)
                });
            })));
        }
        ctx.log("[ibuilder] chain: start");
        ctx.chain_log.line(&format!(
            "[gameapp] chain: documents={} bundle={} (ibuilder-ffi {})",
            docs.root().display(),
            h.bundle.display(),
            env!("CARGO_PKG_VERSION")
        ));
        load_config(&mut ctx, cfg_path.as_deref());
        let rep = run_chain(&mut ctx);
        ctx.chain_log.line(&format!(
            "[gameapp] chain: ibuilder_run_chain -> {}, the chain's result {}",
            rep.rc, rep.result
        ));
        let mut s = String::new();
        for (k, v) in &rep.status {
            s.push_str(&format!("{k}={v}\n"));
        }
        s.push_str(&format!("result={}\n", rep.result));
        *h.status.lock().unwrap() = s;
        rep.rc
    })
}

/// # Safety
/// `h` is a live handle; `buf` is NULL or valid for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_chain_status(
    h: *mut Ibuilder,
    buf: *mut c_char,
    cap: c_int,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: a live handle by contract.
        let Some(h) = (unsafe { h.as_ref() }) else {
            return E_BAD_ARG;
        };
        let s = h.status.lock().unwrap().clone();
        // SAFETY: forwarded.
        unsafe { fill(buf, cap, &s) }
    })
}

/// Test aid (feature `mock-stages`): panic inside an entry point. In an unwinding build the guard returns `IBUILDER_E_PANIC`; in the release profile (`panic = "abort"`) the panic hook
/// writes `[ibuilder] PANIC: ...` to `gameapp.chain.log` and the process aborts: `tests/c/panic_abort.c` checks exactly that.
///
/// # Safety
/// `h` is NULL or a live handle.
#[cfg(feature = "mock-stages")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_test_panic(h: *mut Ibuilder) -> c_int {
    guard(E_PANIC, || {
        let _ = h;
        panic!("test panic requested through ibuilder_test_panic")
    })
}

// ---- memory ---------------------------------------------------------------------------------------------------------------------------------------------------------

fn mem_source(documents: *const c_char) -> sys::mem::SystemMem {
    // SAFETY: NUL-terminated by contract.
    let d = unsafe { cstr(documents) }
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join("Documents"))
                .unwrap_or_else(|| PathBuf::from("."))
        });
    sys::mem::SystemMem::new(d)
}

/// # Safety
/// `out` is writable; `documents_dir` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_mem_info(
    documents_dir: *const c_char,
    out: *mut IbuilderMem,
) -> c_int {
    guard(E_PANIC, || {
        if out.is_null() {
            return E_BAD_ARG;
        }
        let m = mem_source(documents_dir).mem();
        // SAFETY: writable by contract.
        unsafe {
            *out = IbuilderMem {
                resident: m.resident,
                peak: m.peak,
                footprint: m.footprint,
                available: m.available,
                free_bytes: m.free,
            }
        };
        0
    })
}

/// # Safety
/// `buf` is NULL or valid for `cap` bytes; `documents_dir` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_mem_line(
    documents_dir: *const c_char,
    buf: *mut c_char,
    cap: c_int,
) -> c_int {
    guard(E_PANIC, || {
        let s = mem_source(documents_dir).memline();
        // SAFETY: forwarded.
        unsafe { fill(buf, cap, &s) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_available_bytes() -> i64 {
    guard(-1, || {
        let a = sys::mem::available_bytes();
        if a == 0 { -1 } else { a as i64 }
    })
}

/// # Safety
/// `path` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_free_bytes(path: *const c_char) -> i64 {
    guard(-1, || {
        // SAFETY: NUL-terminated by contract.
        match unsafe { cstr(path) } {
            Some(p) => sys::free_bytes(Path::new(&p)),
            None => -1,
        }
    })
}

/// # Safety
/// `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_address_space_info(out: *mut IbuilderAddressSpace) -> c_int {
    guard(E_PANIC, || {
        if out.is_null() {
            return E_BAD_ARG;
        }
        let Some((free, largest)) = sys::vm::address_space() else {
            return -1;
        };
        let e = sys::entitlements::own_entitlements();
        // SAFETY: writable by contract.
        unsafe {
            *out = IbuilderAddressSpace {
                free_total: free,
                largest,
                extended: e.as_ref().map(|e| e.extended_va as i32).unwrap_or(0),
                increased_memory: e.as_ref().map(|e| e.increased_memory as i32).unwrap_or(-1),
                get_task_allow: e.as_ref().map(|e| e.get_task_allow as i32).unwrap_or(-1),
            }
        };
        0
    })
}

/// # Safety
/// `buf` is NULL or valid for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_self_identifier(buf: *mut c_char, cap: c_int) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: forwarded.
        unsafe { fill(buf, cap, &sys::entitlements::self_identifier()) }
    })
}

// ---- the guest ----------------------------------------------------------------------------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn ibuilder_set_game_layer(layer: *mut c_void) {
    guard((), || sys::boot::set_game_layer(layer))
}

/// # Safety
/// `name` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_host_symbol(name: *const c_char) -> *mut c_void {
    guard(std::ptr::null_mut(), || {
        // SAFETY: NUL-terminated by contract.
        match unsafe { cstr(name) } {
            Some(n) => sys::boot::host_symbol(&n),
            None => std::ptr::null_mut(),
        }
    })
}

/// # Safety
/// `documents_dir` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_quit_game(documents_dir: *const c_char) {
    guard((), || {
        // SAFETY: NUL-terminated by contract.
        let Some(d) = (unsafe { cstr(documents_dir) }) else {
            return;
        };
        let log = FileLog::append(&Docs::new(&d).chain_log());
        match log {
            Ok(l) => sys::boot::quit_game(Path::new(&d), &l),
            Err(_) => sys::boot::quit_game(Path::new(&d), &NullLog),
        }
    })
}

/// # Safety
/// `documents_dir` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_early_init(documents_dir: *const c_char) {
    guard((), || {
        // SAFETY: NUL-terminated by contract.
        if let Some(d) = unsafe { cstr(documents_dir) } {
            sys::boot::early_init(Path::new(&d), false);
        }
    })
}

/// # Safety
/// The host's call: `key` NUL-terminated, `ctl`/`code` valid for their lengths, `stage` writable for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_live_handler(
    key: *const c_char,
    ctl: *const c_void,
    nctl: u32,
    code: *const c_void,
    ncode: u32,
    stage: *mut c_char,
    cap: c_int,
) -> c_int {
    // like every entry point: no panic crosses the boundary, and the message of one is kept for `ibuilder_last_error()` (the handler's own `catch_unwind` would swallow it)
    guard(-1, || {
        // SAFETY: the host's contract above, passed on.
        unsafe { sys::boot::live_handler_unguarded(key, ctl, nctl, code, ncode, stage, cap) }
    })
}

// ---- the launch state machine --------------------------------------------------------------------------------------------------------------------------------------------

/// # Safety
/// `content` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_classify_boot_request(
    exists: c_int,
    content: *const c_char,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        match state::classify_boot_request(exists != 0, unsafe { cstr(content) }.as_deref()) {
            BootRequestState::None => 0,
            BootRequestState::Armed => 1,
            BootRequestState::Attempted => 2,
        }
    })
}

/// # Safety
/// Strings are NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_classify_framework(
    on_disk: c_int,
    stamp_content: *const c_char,
    current_build: *const c_char,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (s, b) = unsafe { (cstr(stamp_content), cstr(current_build)) };
        match state::classify_framework(on_disk != 0, s.as_deref(), b.as_deref().unwrap_or("")) {
            FrameworkState::None => 0,
            FrameworkState::Fresh => 1,
            FrameworkState::Stale => 2,
        }
    })
}

/// `ibuilder_classify_framework` with this app's framework id (`GameAppBuild.FrameworkId`, NULL or "" for none): FRESH when the stamp's `framework_id` is the same, or, for a stamp
/// from before framework ids, when its `app_build` is (`state::framework_matches`). The older entry point keeps its answer (build ids only).
///
/// # Safety
/// Strings are NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_classify_framework_id(
    on_disk: c_int,
    stamp_content: *const c_char,
    current_build: *const c_char,
    current_framework_id: *const c_char,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (s, b, f) = unsafe {
            (
                cstr(stamp_content),
                cstr(current_build),
                cstr(current_framework_id),
            )
        };
        match state::classify_framework_id(
            on_disk != 0,
            s.as_deref(),
            b.as_deref().unwrap_or(""),
            f.as_deref().unwrap_or(""),
        ) {
            FrameworkState::None => 0,
            FrameworkState::Fresh => 1,
            FrameworkState::Stale => 2,
        }
    })
}

/// # Safety
/// Strings are NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_classify_previous_run(
    stage_line: *const c_char,
    chain_last: *const c_char,
    guest_last: *const c_char,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (s, c, g) = unsafe { (cstr(stage_line), cstr(chain_last), cstr(guest_last)) };
        previous_run_code(state::classify_previous_run(
            s.as_deref(),
            c.as_deref(),
            g.as_deref(),
        ))
    })
}

/// The C codes of `PreviousRun` (`IBUILDER_PREV_*`): 0-4 as they always were, 5 the game's own exit (svcExitProcess).
fn previous_run_code(p: PreviousRun) -> c_int {
    match p {
        PreviousRun::DesignedEnd => 0,
        PreviousRun::PlayerEnd => 1,
        PreviousRun::DesignedEndWithoutCleanExit => 2,
        PreviousRun::FaultInGuest => 3,
        PreviousRun::KilledOrUnknown => 4,
        PreviousRun::GuestExit => 5,
    }
}

/// The same with the guest log's last exit-cause line (`[dm] received <cause>: graceful exit`, `state::last_exit_cause`): a game that called svcExitProcess is
/// `IBUILDER_PREV_GUEST_EXIT`.
///
/// # Safety
/// Strings are NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_classify_previous_run_exit(
    stage_line: *const c_char,
    chain_last: *const c_char,
    guest_last: *const c_char,
    exit_cause: *const c_char,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (s, c, g, x) = unsafe {
            (
                cstr(stage_line),
                cstr(chain_last),
                cstr(guest_last),
                cstr(exit_cause),
            )
        };
        previous_run_code(state::classify_previous_run_exit(
            s.as_deref(),
            c.as_deref(),
            g.as_deref(),
            x.as_deref(),
        ))
    })
}

/// # Safety
/// `line` is NULL or NUL-terminated; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_parse_stage(
    line: *const c_char,
    out: *mut IbuilderStage,
) -> c_int {
    guard(0, || {
        if out.is_null() {
            return 0;
        }
        // SAFETY: NUL-terminated by contract.
        let p = parse_stage(&unsafe { cstr(line) }.unwrap_or_default());
        let mut text = [0 as c_char; 256];
        for (i, b) in p.text.bytes().take(255).enumerate() {
            text[i] = b as c_char;
        }
        // SAFETY: writable by contract.
        unsafe {
            *out = IbuilderStage {
                valid: p.valid as i32,
                step: p.step as i32,
                of: p.of as i32,
                fraction: p.fraction,
                text,
            }
        };
        p.valid as c_int
    })
}

/// # Safety
/// `line` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_is_boot_stage(line: *const c_char) -> c_int {
    guard(0, || {
        // SAFETY: NUL-terminated by contract.
        is_boot_stage(&unsafe { cstr(line) }.unwrap_or_default()) as c_int
    })
}

fn plan_json(plan: &state::LaunchPlan) -> String {
    use serde_json::json;
    let action = match &plan.action {
        LaunchAction::Nothing => json!({"kind": "nothing"}),
        LaunchAction::BootOnly => json!({"kind": "boot_only"}),
        LaunchAction::ContinueBuild { game, stage, why } => {
            json!({"kind": "continue", "game": game, "stage": stage, "why": why})
        }
        LaunchAction::Run(RunRequest::ChainOnly) => json!({"kind": "run", "request": "chain-only"}),
        LaunchAction::Run(RunRequest::BootOnly) => json!({"kind": "run", "request": "boot-only"}),
        LaunchAction::Run(RunRequest::Extraction {
            keys,
            base,
            update,
            out,
            flag,
            re_resolved,
        }) => json!({
            "kind": "run", "request": "extraction", "keys": keys, "base": base, "update": update, "out": out, "flag": flag, "re_resolved": re_resolved
        }),
    };
    json!({"interrupted": plan.interrupted, "action": action, "notes": plan.notes}).to_string()
}

/// # Safety
/// Strings are NULL or NUL-terminated; `buf` is NULL or valid for `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_decide_launch(
    documents_dir: *const c_char,
    current_build: *const c_char,
    cannot_run_here: c_int,
    framework_path: *const c_char,
    flat_dir: *const c_char,
    buf: *mut c_char,
    cap: c_int,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (d, b, f, fl) = unsafe {
            (
                cstr(documents_dir),
                cstr(current_build),
                cstr(framework_path),
                cstr(flat_dir),
            )
        };
        let Some(d) = d else { return E_BAD_ARG };
        let docs = Docs::new(&d);
        let fw = PathBuf::from(f.unwrap_or_else(|| {
            docs.join("DmGameProbe.framework/DmGameProbe")
                .to_string_lossy()
                .into_owned()
        }));
        let flat = PathBuf::from(
            fl.unwrap_or_else(|| docs.join("extract/flat").to_string_lossy().into_owned()),
        );
        let b = b.unwrap_or_default();
        let plan = state::decide_launch(
            &docs,
            &LaunchFacts {
                current_build: &b,
                // this entry point predates framework ids: the build id alone decides, as it always did (ibuilder_classify_framework_id has the framework id rule)
                current_framework_id: "",
                cannot_run_here: cannot_run_here != 0,
                framework: &fw,
                flat: &flat,
            },
        );
        // SAFETY: forwarded.
        unsafe { fill(buf, cap, &plan_json(&plan)) }
    })
}

/// # Safety
/// Strings are NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_begin_run(
    documents_dir: *const c_char,
    game: *const c_char,
    boot_only: c_int,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (Some(d), g) = (unsafe { cstr(documents_dir) }, unsafe { cstr(game) }) else {
            return E_BAD_ARG;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_micros() as i64)
            .unwrap_or(0);
        let notes = state::begin_run(
            &Docs::new(&d),
            &g.unwrap_or_default(),
            boot_only != 0,
            std::process::id(),
            now,
        );
        if notes.is_empty() {
            0
        } else {
            // a marker that could not be written is an I/O failure, not a bad argument (review r3 §5.7)
            set_error(notes.join("; "));
            E_IO
        }
    })
}

/// # Safety
/// `documents_dir` is NULL or NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_finish_run(documents_dir: *const c_char) {
    guard((), || {
        // SAFETY: NUL-terminated by contract.
        if let Some(d) = unsafe { cstr(documents_dir) } {
            state::clear_run_marker(&Docs::new(&d));
        }
    })
}

// ---- log rotation --------------------------------------------------------------------------------------------------------------------------------------------------------------

/// # Safety
/// `documents_dir` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_logs_mark(
    documents_dir: *const c_char,
    now_epoch_secs: i64,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let Some(d) = (unsafe { cstr(documents_dir) }) else {
            return E_BAD_ARG;
        };
        ibuilder_pipeline::logwindow::mark(&Docs::new(&d), now_epoch_secs);
        0
    })
}

/// # Safety
/// Strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_log_trim(
    documents_dir: *const c_char,
    name: *const c_char,
    now_epoch_secs: i64,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: NUL-terminated by contract.
        let (Some(d), Some(n)) = (unsafe { cstr(documents_dir) }, unsafe { cstr(name) }) else {
            return E_BAD_ARG;
        };
        ibuilder_pipeline::logwindow::trim_to_window(&Docs::new(&d), &n, now_epoch_secs) as c_int
    })
}

// ---- the cover of a Switch 2 package ---------------------------------------------------------------------------------------------------------------------------------------------

/// `ibuilder_nup_icon`: the file is not a readable `.nup` (it cannot be opened, is not one, or its head is damaged or truncated); `ibuilder_last_error()` says why.
pub const E_NUP: c_int = -3;

/// The icon of the Switch 2 package at `nup_path` and the title it names the game by, read from the HEAD of the file only (the superblock, the table of contents, the application
/// package's meta block and the NPDMs: some hundred kilobytes of a 25 GB file; no `prod.keys`, a NUP is not encrypted). The icon is the one the build writes to `icon.jpg`.
///
/// Returns the icon's length in bytes: `> 0` the icon (copied into `buf` when it fits: the answer is `<= cap`; when it is larger nothing is written and the caller asks again with a
/// buffer of that size), `0` the package carries no icon, `IBUILDER_E_NUP` the file is not a readable package, `IBUILDER_E_BAD_ARG` (a NULL path, a negative cap, a NULL buf with a cap) or
/// `IBUILDER_E_PANIC`. `*title_id` (when not NULL) is the program ID the build would name the game by, or 0 when the file does not say; it is set whenever the result is `>= 0`.
///
/// # Safety
/// `nup_path` is NUL-terminated; `buf` is NULL or valid for `cap` bytes; `title_id` is NULL or valid for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_nup_icon(
    nup_path: *const c_char,
    buf: *mut u8,
    cap: c_int,
    title_id: *mut u64,
) -> c_int {
    guard(E_PANIC, || {
        // SAFETY: `title_id` is NULL or writable by contract.
        let put_title = |t: u64| {
            if !title_id.is_null() {
                unsafe { *title_id = t };
            }
        };
        put_title(0);
        // SAFETY: NUL-terminated by contract.
        let Some(path) = (unsafe { cstr(nup_path) }) else {
            return E_BAD_ARG;
        };
        if cap < 0 || (buf.is_null() && cap > 0) {
            return E_BAD_ARG;
        }
        match ibuilder_nup::application_icon(Path::new(&path)) {
            Ok(cover) => {
                put_title(cover.title_id.unwrap_or(0));
                let Some(icon) = cover.icon else { return 0 };
                if !buf.is_null() && icon.len() <= cap as usize {
                    // SAFETY: `icon.len() <= cap` bytes are written into the caller's buffer.
                    unsafe { std::ptr::copy_nonoverlapping(icon.as_ptr(), buf, icon.len()) };
                }
                // an icon is at most ibuilder_nup::META_MAX (32 MiB): it fits a c_int
                icon.len() as c_int
            }
            Err(e) => {
                set_error(e.to_string());
                E_NUP
            }
        }
    })
}

/// The caller's reader of [`ibuilder_nup_icon_read`]: read up to `len` bytes at `offset` into `buf`, return how many (0 at the end), or a negative number on a failure. Called on the calling
/// thread only, synchronously, inside the call.
pub type NupReadFn =
    unsafe extern "C" fn(ctx: *mut c_void, offset: u64, buf: *mut u8, len: c_int) -> i64;

/// A `.nup` read through the caller's callback (an SFTP stream on the NAS: app patch 0084). `ReadAt` wants `Send + Sync`; the callback and its context are used only by the thread that called
/// `ibuilder_nup_icon_read`, inside that call, and the source is dropped before it returns.
struct CallbackSource {
    read: NupReadFn,
    ctx: *mut c_void,
    size: u64,
}

// SAFETY: see the type's comment: the pointer never leaves the calling thread's call.
unsafe impl Send for CallbackSource {}
// SAFETY: as above; `read_at` takes `&self` but every call is on the one calling thread.
unsafe impl Sync for CallbackSource {}

impl ibuilder_nup::ReadAt for CallbackSource {
    fn len(&self) -> u64 {
        self.size
    }

    fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = buf
            .len()
            .min((self.size - offset) as usize)
            .min(c_int::MAX as usize);
        // SAFETY: `buf` is valid for `want` bytes; the callback's contract is to write at most `len` bytes.
        let n = unsafe { (self.read)(self.ctx, offset, buf.as_mut_ptr(), want as c_int) };
        if n < 0 {
            return Err(std::io::Error::other(format!(
                "the reader failed at {offset} ({n})"
            )));
        }
        Ok((n as usize).min(want))
    }
}

/// [`ibuilder_nup_icon`] for a package that is not a local file (a `.nup` on the NAS, read over SFTP): the same answer and the same buffer protocol, the bytes asked of `read` at the offsets the
/// head needs (the superblock, the table of contents, the meta block, the NPDMs). `size` is the package's length.
///
/// # Safety
/// `read` is a valid function for the whole call and `ctx` is what it expects; `buf` is NULL or valid for `cap` bytes; `title_id` is NULL or valid for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ibuilder_nup_icon_read(
    read: Option<NupReadFn>,
    ctx: *mut c_void,
    size: u64,
    buf: *mut u8,
    cap: c_int,
    title_id: *mut u64,
) -> c_int {
    guard(E_PANIC, || {
        let put_title = |t: u64| {
            if !title_id.is_null() {
                // SAFETY: `title_id` is NULL or writable by contract.
                unsafe { *title_id = t };
            }
        };
        put_title(0);
        let Some(read) = read else { return E_BAD_ARG };
        if cap < 0 || (buf.is_null() && cap > 0) {
            return E_BAD_ARG;
        }
        let src = CallbackSource { read, ctx, size };
        match ibuilder_nup::application_icon_from(Box::new(src), Path::new("(remote .nup)")) {
            Ok(cover) => {
                put_title(cover.title_id.unwrap_or(0));
                let Some(icon) = cover.icon else { return 0 };
                if !buf.is_null() && icon.len() <= cap as usize {
                    // SAFETY: `icon.len() <= cap` bytes are written into the caller's buffer.
                    unsafe { std::ptr::copy_nonoverlapping(icon.as_ptr(), buf, icon.len()) };
                }
                icon.len() as c_int
            }
            Err(e) => {
                set_error(e.to_string());
                E_NUP
            }
        }
    })
}

// ---- the early initializer -----------------------------------------------------------------------------------------------------------------------------------------------------

/// A Mach-O initializer (`__DATA,__mod_init_func`): the earliest point the app controls -- before the managed runtime -- takes the guest's address-space block (DECISIONS traps 22-27).
/// iOS only; on other targets the `early-init` feature builds it for tests and `IBUILDER_EARLY_INIT_FORCE=1` makes it act.
#[cfg(any(target_os = "ios", feature = "early-init"))]
mod early {
    extern "C" fn ctor() {
        let force = std::env::var_os("IBUILDER_EARLY_INIT_FORCE").is_some();
        if let Some(home) = std::env::var_os("HOME") {
            let docs = std::path::Path::new(&home).join("Documents");
            let _ =
                std::panic::catch_unwind(|| ibuilder_pipeline::sys::boot::early_init(&docs, force));
        }
    }
    #[used]
    #[unsafe(link_section = "__DATA,__mod_init_func")]
    static CTOR: extern "C" fn() = ctor;
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibuilder_pipeline::testutil::TempDir;

    fn text_of(p: *const c_char) -> String {
        // SAFETY: `with_c_line` hands a NUL-terminated string that lives for the call.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    /// The line a C callback gets: NUL-terminated, an inner NUL made a space (as `c_string`), the thread's buffer reused, and a call made from inside a callback still served.
    #[test]
    fn a_line_for_a_c_callback_is_what_c_string_made() {
        assert_eq!(with_c_line("a\0b", text_of), "a b");
        assert_eq!(
            with_c_line("a\0b", text_of),
            text_of(c_string("a\0b").as_ptr())
        );
        let first = with_c_line("first line", |p| p as usize);
        let second = with_c_line("second", |p| p as usize);
        assert_eq!(
            first, second,
            "the same buffer, no new allocation for a shorter line"
        );
        let (outer, inner) = with_c_line("outer", |p| (text_of(p), with_c_line("inner", text_of)));
        assert_eq!((outer.as_str(), inner.as_str()), ("outer", "inner"));
        assert_eq!(with_c_line("", text_of), "");
    }

    fn opts(docs: &CString, bundle: &CString) -> IbuilderOptions {
        IbuilderOptions {
            struct_size: std::mem::size_of::<IbuilderOptions>() as u32,
            documents_dir: docs.as_ptr(),
            bundle_dir: bundle.as_ptr(),
            log: None,
            log_user: std::ptr::null_mut(),
            progress: None,
            progress_user: std::ptr::null_mut(),
            sign: None,
            sign_user: std::ptr::null_mut(),
            xlate: None,
            spirv_cross: None,
            live_notify: None,
            live_notify_user: std::ptr::null_mut(),
            flags: 0,
            sign_adhoc: None,
            glslang: None,
        }
    }

    unsafe extern "C" fn echo_tool(
        argc: c_int,
        argv: *const *const c_char,
        out: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int {
        let mut parts = Vec::new();
        for i in 0..argc as usize {
            parts.push(
                unsafe { CStr::from_ptr(*argv.add(i)) }
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        assert!(
            unsafe { (*argv.add(argc as usize)).is_null() },
            "argv is NULL-terminated"
        );
        unsafe {
            *out = libc::strdup(CString::new(parts.join(" ")).unwrap().as_ptr());
            *err = std::ptr::null_mut();
        }
        7
    }

    #[test]
    fn a_c_tool_callback_becomes_a_tool_fn_and_its_malloced_strings_are_freed() {
        let f = tool_fn(echo_tool);
        let r = f(&[
            "dmsh_xlate".to_string(),
            "job.json".to_string(),
            "out dir".to_string(),
        ]);
        assert_eq!(
            (r.rc, r.stdout.as_str(), r.stderr.as_str()),
            (7, "dmsh_xlate job.json out dir", "")
        );
    }

    unsafe extern "C" fn sign_cb(
        user: *mut c_void,
        fw: *const c_char,
        exe: *const c_char,
        mode: *mut c_char,
        mc: c_int,
        id: *mut c_char,
        ic: c_int,
        err: *mut c_char,
        ec: c_int,
    ) -> c_int {
        let calls = unsafe { &*(user as *const std::sync::atomic::AtomicUsize) };
        calls.fetch_add(1, Ordering::SeqCst);
        let (fw, exe) = unsafe {
            (
                CStr::from_ptr(fw).to_string_lossy().into_owned(),
                CStr::from_ptr(exe).to_string_lossy().into_owned(),
            )
        };
        unsafe {
            fill(mode, mc, "device(zsign, in-memory .p12)");
            fill(id, ic, "dev.ibuilder.app");
            fill(err, ec, &format!("no certificate for {fw}/{exe}"));
        }
        12
    }

    #[test]
    fn a_c_sign_callback_becomes_a_signer() {
        use std::sync::atomic::AtomicUsize;
        let calls = AtomicUsize::new(0);
        let s = CallbackSigner {
            f: sign_cb,
            adhoc: None,
            user: UserPtr(&calls as *const AtomicUsize as *mut c_void),
        };
        let d = TempDir::new("ffi-sign");
        let ctx = Ctx::builder(d.path()).build();
        let r = s.sign(
            Path::new("/x/DmGameProbe.framework"),
            "DmGameProbe",
            &ctx.view(),
        );
        assert_eq!(
            (r.rc, r.mode.as_str(), r.identifier.as_str()),
            (12, "device(zsign, in-memory .p12)", "dev.ibuilder.app")
        );
        assert!(r.err.contains("/x/DmGameProbe.framework/DmGameProbe"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    unsafe extern "C" fn adhoc_cb(
        user: *mut c_void,
        _fw: *const c_char,
        _exe: *const c_char,
        mode: *mut c_char,
        mc: c_int,
        id: *mut c_char,
        ic: c_int,
        _err: *mut c_char,
        _ec: c_int,
    ) -> c_int {
        let calls = unsafe { &*(user as *const std::sync::atomic::AtomicUsize) };
        calls.fetch_add(100, Ordering::SeqCst);
        unsafe {
            fill(mode, mc, "adhoc(zsign, no certificate: export only)");
            fill(id, ic, "dev.ibuilder.app");
        }
        0
    }

    #[test]
    fn the_ad_hoc_callback_is_the_signers_fallback_and_none_without_it() {
        use std::sync::atomic::AtomicUsize;
        let calls = AtomicUsize::new(0);
        let user = UserPtr(&calls as *const AtomicUsize as *mut c_void);
        let d = TempDir::new("ffi-sign-adhoc");
        let ctx = Ctx::builder(d.path()).build();
        let fw = Path::new("/x/DmGameProbe.framework");
        let without = CallbackSigner {
            f: sign_cb,
            adhoc: None,
            user,
        };
        assert!(without.sign_adhoc(fw, "DmGameProbe", &ctx.view()).is_none());
        let with = CallbackSigner {
            f: sign_cb,
            adhoc: Some(adhoc_cb),
            user,
        };
        let r = with.sign_adhoc(fw, "DmGameProbe", &ctx.view()).unwrap();
        assert_eq!(
            (r.rc, r.mode.as_str()),
            (0, "adhoc(zsign, no certificate: export only)")
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            100,
            "only the ad hoc callback ran"
        );
        assert_eq!(with.sign(fw, "DmGameProbe", &ctx.view()).rc, 12);
    }

    /// a callback that fills every byte of its buffers and no NUL (a text as long as the capacity): `CStr::from_ptr` would run on past the array
    unsafe extern "C" fn full_cb(
        _u: *mut c_void,
        _fw: *const c_char,
        _exe: *const c_char,
        mode: *mut c_char,
        mc: c_int,
        id: *mut c_char,
        ic: c_int,
        err: *mut c_char,
        ec: c_int,
    ) -> c_int {
        unsafe {
            std::ptr::write_bytes(mode, b'M', mc as usize);
            std::ptr::write_bytes(id, b'I', ic as usize);
            std::ptr::write_bytes(err, b'E', ec as usize);
        }
        20
    }

    #[test]
    fn a_sign_callback_that_leaves_no_nul_is_read_within_its_buffers() {
        let s = CallbackSigner {
            f: full_cb,
            adhoc: None,
            user: UserPtr(std::ptr::null_mut()),
        };
        let d = TempDir::new("ffi-sign-full");
        let ctx = Ctx::builder(d.path()).build();
        let r = s.sign(
            Path::new("/x/DmGameProbe.framework"),
            "DmGameProbe",
            &ctx.view(),
        );
        assert_eq!(r.rc, 20);
        assert_eq!(
            (r.mode.len(), r.identifier.len(), r.err.len()),
            (255, 255, 1023),
            "each text ends at its buffer: capacity - 1 characters"
        );
        assert!(
            r.mode.bytes().all(|b| b == b'M')
                && r.identifier.bytes().all(|b| b == b'I')
                && r.err.bytes().all(|b| b == b'E')
        );
    }

    /// the handler's panic is kept for `ibuilder_last_error()`, not swallowed by an inner catch_unwind
    #[test]
    fn a_panic_in_the_live_handler_is_answered_and_recorded() {
        use ibuilder_pipeline::env::Env;
        struct PanicEnv;
        impl Env for PanicEnv {
            fn get(&self, _name: &str) -> Option<String> {
                panic!("boom in the live queue")
            }
            fn set(&self, _name: &str, _value: &str) {}
            fn remove(&self, _name: &str) {}
        }
        sys::boot::set_live(Some(ibuilder_pipeline::live::LiveQueue::new(Arc::new(
            PanicEnv,
        ))));
        let key = CString::new("0123456789abcdef").unwrap();
        let (ctl, code) = ([0u8; 8], [0u8; 8]);
        let mut stage = [0 as c_char; 16];
        // SAFETY: a NUL-terminated key and buffers that outlive the call.
        let rc = unsafe {
            ibuilder_live_handler(
                key.as_ptr(),
                ctl.as_ptr() as *const c_void,
                8,
                code.as_ptr() as *const c_void,
                8,
                stage.as_mut_ptr(),
                16,
            )
        };
        sys::boot::set_live(None);
        assert_eq!(rc, -1);
        let mut buf = [0 as c_char; 256];
        // SAFETY: a stack buffer of 256 bytes.
        unsafe { ibuilder_last_error(buf.as_mut_ptr(), 256) };
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(
            s.contains("panic") && s.contains("boom in the live queue"),
            "{s}"
        );
    }

    #[test]
    fn a_panic_never_crosses_the_boundary() {
        // test builds unwind (the workspace's release profile aborts, but the test harness ignores that): the guard turns the panic into the default and an error
        let r: c_int = guard(E_PANIC, || -> c_int { panic!("boom inside the library") });
        assert_eq!(r, E_PANIC);
        let mut buf = [0 as c_char; 128];
        // SAFETY: a stack buffer of 128 bytes.
        let need = unsafe { ibuilder_last_error(buf.as_mut_ptr(), 128) };
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(need > 0 && s.contains("boom inside the library"), "{s}");
    }

    /// Review r3 §5.7: a marker that cannot be written is an I/O failure, not a bad argument, and the error says which file.
    #[test]
    fn begin_run_reports_a_marker_it_could_not_write_as_an_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let d = TempDir::new("ffi-begin-io");
        let docs = CString::new(d.path().to_string_lossy().as_bytes()).unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        // SAFETY: NUL-terminated strings.
        let rc = unsafe {
            ibuilder_begin_run(docs.as_ptr(), CString::new("G.nsp").unwrap().as_ptr(), 1)
        };
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(rc, E_IO);
        let mut buf = [0 as c_char; 512];
        // SAFETY: a stack buffer of 512 bytes.
        unsafe { ibuilder_last_error(buf.as_mut_ptr(), 512) };
        let s = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        assert!(
            s.contains("could not write") && s.contains("gameapp.boot-only"),
            "{s}"
        );
    }

    #[test]
    fn create_checks_its_arguments() {
        let d = TempDir::new("ffi-args");
        let (docs, bundle) = (
            CString::new(d.path().to_string_lossy().as_bytes()).unwrap(),
            CString::new("/b").unwrap(),
        );
        let mut err = 0;
        // SAFETY: valid arguments / documented NULLs.
        unsafe {
            assert!(ibuilder_create(std::ptr::null(), &mut err).is_null() && err == E_BAD_ARG);
            let mut o = opts(&docs, &bundle);
            o.struct_size = 8;
            assert!(
                ibuilder_create(&o, &mut err).is_null() && err == E_BAD_ARG,
                "a struct from an older header is refused"
            );
            // the header before glslang was appended: taken, and the field past its size is not read (it would be this tool otherwise)
            o = opts(&docs, &bundle);
            o.glslang = Some(echo_tool);
            o.struct_size = OPTIONS_MIN_SIZE as u32;
            let h = ibuilder_create(&o, &mut err);
            assert!(
                !h.is_null(),
                "a struct without the appended glslang is taken"
            );
            assert!((*h).glslang.is_none(), "a field past struct_size is NULL");
            ibuilder_destroy(h);
            o.struct_size = OPTIONS_MIN_SIZE as u32 - 1;
            assert!(ibuilder_create(&o, &mut err).is_null() && err == E_BAD_ARG);
            o.struct_size = std::mem::size_of::<IbuilderOptions>() as u32;
            let h = ibuilder_create(&o, &mut err);
            assert!(
                !h.is_null() && (*h).glslang.is_some(),
                "the full struct passes glslang"
            );
            ibuilder_destroy(h);
            o = opts(&docs, &bundle);
            o.documents_dir = std::ptr::null();
            assert!(ibuilder_create(&o, &mut err).is_null());
            if !cfg!(feature = "mock-stages") {
                o = opts(&docs, &bundle);
                o.flags = FLAG_MOCK_STAGES;
                assert!(
                    ibuilder_create(&o, &mut err).is_null(),
                    "the mock flag is a build-time feature"
                );
            }
            let h = ibuilder_create(&opts(&docs, &bundle), &mut err);
            assert!(!h.is_null());
            ibuilder_destroy(h);
            ibuilder_destroy(std::ptr::null_mut());
        }
    }

    #[test]
    fn a_run_without_a_config_says_so_and_a_second_run_while_busy_is_refused() {
        let d = TempDir::new("ffi-run");
        let (docs, bundle) = (
            CString::new(d.path().to_string_lossy().as_bytes()).unwrap(),
            CString::new(d.path().to_string_lossy().as_bytes()).unwrap(),
        );
        let mut err = 0;
        // SAFETY: valid arguments; the handle is used by this thread only.
        unsafe {
            let h = ibuilder_create(&opts(&docs, &bundle), &mut err);
            assert_eq!(
                ibuilder_run_chain(h, std::ptr::null()),
                2,
                "no extract_out in the config"
            );
            let mut buf = [0 as c_char; 256];
            ibuilder_chain_status(h, buf.as_mut_ptr(), 256);
            assert!(
                CStr::from_ptr(buf.as_ptr())
                    .to_string_lossy()
                    .contains("result=")
            );
            (*h).running.store(true, Ordering::SeqCst);
            assert_eq!(ibuilder_run_chain(h, std::ptr::null()), E_BUSY);
            (*h).running.store(false, Ordering::SeqCst);
            ibuilder_destroy(h);
        }
        let log = std::fs::read_to_string(d.join("pipeline.stages.log")).unwrap();
        assert!(
            log.starts_with("==== chain run, pid ")
                && log.contains("no extract_out in the config: nothing to do"),
            "{log}"
        );
    }

    #[test]
    fn classifiers_and_the_launch_decision_round_trip_through_c_types() {
        let d = TempDir::new("ffi-launch");
        let (docs, build) = (
            CString::new(d.path().to_string_lossy().as_bytes()).unwrap(),
            CString::new("B1").unwrap(),
        );
        // SAFETY: valid NUL-terminated strings and buffers.
        unsafe {
            let attempted = CString::new("attempted pid 5").unwrap();
            assert_eq!(ibuilder_classify_boot_request(1, attempted.as_ptr()), 2);
            assert_eq!(ibuilder_classify_boot_request(0, std::ptr::null()), 0);
            assert_eq!(ibuilder_classify_boot_request(1, std::ptr::null()), 2);
            let stage = CString::new("stage 5/5: booting the guest (x)").unwrap();
            let limit =
                CString::new("[gameapp] boot time limit 30 s reached: sending SIGTERM to self")
                    .unwrap();
            let clean = CString::new("[dm] graceful exit done").unwrap();
            assert_eq!(
                ibuilder_classify_previous_run(stage.as_ptr(), limit.as_ptr(), clean.as_ptr()),
                0
            );
            let mut st: IbuilderStage = std::mem::zeroed();
            let line =
                CString::new("stage 3/5: shaders: translating for this iPhone (500/1000 shaders)")
                    .unwrap();
            assert_eq!(ibuilder_parse_stage(line.as_ptr(), &mut st), 1);
            assert!((st.step, st.of) == (3, 5) && (st.fraction - 0.68).abs() < 1e-9);
            assert_eq!(ibuilder_is_boot_stage(stage.as_ptr()), 1);
            // a request file, an armed boot-only request with a fresh framework: the launch boots it
            std::fs::create_dir_all(d.join("extract/flat")).unwrap();
            std::fs::create_dir_all(d.join("DmGameProbe.framework")).unwrap();
            std::fs::write(d.join("DmGameProbe.framework/DmGameProbe"), "x").unwrap();
            std::fs::write(d.join("gameapp.framework"), "app_build=B1\ngame=G\n").unwrap();
            std::fs::write(
                d.join("gameapp.boot-only"),
                "boot the framework that is already built and signed\n",
            )
            .unwrap();
            let mut buf = vec![0 as c_char; 2048];
            let need = ibuilder_decide_launch(
                docs.as_ptr(),
                build.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
                buf.as_mut_ptr(),
                2048,
            );
            let json: serde_json::Value =
                serde_json::from_str(&CStr::from_ptr(buf.as_ptr()).to_string_lossy()).unwrap();
            assert!(
                need > 0 && json["action"]["kind"] == "boot_only" && json["interrupted"] == false,
                "{json}"
            );
            assert_eq!(
                ibuilder_begin_run(docs.as_ptr(), CString::new("G.nsp").unwrap().as_ptr(), 0),
                0
            );
            assert!(d.join("gameapp.running").exists());
            ibuilder_finish_run(docs.as_ptr());
            assert!(!d.join("gameapp.running").exists());
        }
    }
}
