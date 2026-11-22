//! Helpers for tests (this crate's and the dependants'): a temp folder that lives under the cargo target dir (never bare /tmp or $TMPDIR),
//! a `write` that makes parents, and fake memory sources. Compiled into the library because the integration tests and the other crates use it;
//! it adds no dependencies and nothing here runs in the shipped app.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A directory removed on drop. Created under `$IBUILDER_TEST_TMP`, else next to the test executable (inside CARGO_TARGET_DIR).
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let base = std::env::var_os("IBUILDER_TEST_TMP")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let exe = std::env::current_exe().expect("current_exe");
                // <target>/<profile>/deps/<exe>  ->  <target>/<profile>/ibuilder-test-tmp
                exe.parent()
                    .and_then(|p| p.parent())
                    .unwrap_or(Path::new("."))
                    .join("ibuilder-test-tmp")
            });
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let path = base.join(format!("{tag}-{}-{n}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.path.join(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Write `text` to `path`, making parent folders.
pub fn write(path: impl AsRef<Path>, text: impl AsRef<[u8]>) {
    let path = path.as_ref();
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, text).unwrap();
}

/// Set a file's modification time (seconds since the epoch).
pub fn set_mtime(path: impl AsRef<Path>, secs: u64) {
    let f = fs::OpenOptions::new()
        .write(true)
        .open(path.as_ref())
        .unwrap();
    f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap();
}
