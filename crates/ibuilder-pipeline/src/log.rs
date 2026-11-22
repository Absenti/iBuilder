//! Log sinks. The Python chain's `log(*a)` printed one line per call into `Documents/pipeline.stages.log` (append, one banner a run:
//! a relaunch must not wipe the previous run's lines, so a run that died leaves its evidence behind, docs/PROFILE-DERIVED.md);
//! the C shim's `logln` wrote `[gameapp] ...` lines into `Documents/gameapp.chain.log`. Both are `Log` here.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// One line at a time; implementations append the newline. Must be callable from any thread.
pub trait Log: Send + Sync {
    fn line(&self, s: &str);
}

impl<F: Fn(&str) + Send + Sync> Log for F {
    fn line(&self, s: &str) {
        self(s)
    }
}

/// Discards everything (the Python tests' `lambda m: None`).
pub struct NullLog;
impl Log for NullLog {
    fn line(&self, _s: &str) {}
}

/// Collects lines (tests, and the C ABI's own buffering).
#[derive(Default)]
pub struct MemLog(Mutex<Vec<String>>);

impl MemLog {
    pub fn new() -> Arc<MemLog> {
        Arc::new(MemLog::default())
    }
    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
    pub fn any(&self, pred: impl Fn(&str) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(|l| pred(l))
    }
    pub fn contains(&self, needle: &str) -> bool {
        self.any(|l| l.contains(needle))
    }
    pub fn last(&self) -> Option<String> {
        self.0.lock().unwrap().last().cloned()
    }
    pub fn text(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }
}
impl Log for MemLog {
    fn line(&self, s: &str) {
        self.0.lock().unwrap().push(s.to_string());
    }
}

/// Appends to a file, every line handed to the kernel before `line` returns (a run that is killed leaves everything written so far: iOS ends an app with SIGKILL, which no user-space buffer
/// survives). Opening banners the run. The line and its newline go out in ONE `write(2)`, from a buffer kept with the file (`writeln!` on an unbuffered `File` made two, one for the text and
/// one for the newline, and the `flush` after it did nothing: review r4 §6.3).
pub struct FileLog {
    file: Mutex<(File, Vec<u8>)>,
    path: PathBuf,
}

impl FileLog {
    /// Open for append and write `==== <banner> ====`.
    pub fn open(path: &Path, banner: &str) -> std::io::Result<FileLog> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "==== {banner} ====")?;
        f.flush()?;
        Ok(FileLog {
            file: Mutex::new((f, Vec::new())),
            path: path.to_path_buf(),
        })
    }
    /// Open for append without a banner (the shim's chain log gets its banner from the caller).
    pub fn append(path: &Path) -> std::io::Result<FileLog> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(FileLog {
            file: Mutex::new((f, Vec::new())),
            path: path.to_path_buf(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[cfg(unix)]
    pub fn raw_fd(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.file.lock().unwrap().0.as_raw_fd()
    }
}

impl Log for FileLog {
    fn line(&self, s: &str) {
        if let Ok(mut g) = self.file.lock() {
            let (f, buf) = &mut *g;
            buf.clear();
            buf.extend_from_slice(s.as_bytes());
            buf.push(b'\n');
            let _ = f.write_all(buf);
            if buf.capacity() > 64 << 10 {
                *buf = Vec::new(); // one huge line does not keep its room for the rest of the run
            }
        }
    }
}

/// Fans one line out to several sinks (the chain log file and the host application's callback).
pub struct TeeLog(pub Vec<Arc<dyn Log>>);
impl Log for TeeLog {
    fn line(&self, s: &str) {
        for l in &self.0 {
            l.line(s);
        }
    }
}

/// A log that can be redirected after the fact (the gates and the live thread hold one `Arc<SwitchLog>` while the chain decides where lines go).
pub struct SwitchLog(Mutex<Arc<dyn Log>>);
impl SwitchLog {
    pub fn new(inner: Arc<dyn Log>) -> Arc<SwitchLog> {
        Arc::new(SwitchLog(Mutex::new(inner)))
    }
    pub fn set(&self, inner: Arc<dyn Log>) {
        *self.0.lock().unwrap() = inner;
    }
}
impl Log for SwitchLog {
    fn line(&self, s: &str) {
        let inner = self.0.lock().unwrap().clone();
        inner.line(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_log_appends_and_banners_each_run() {
        let dir = crate::testutil::TempDir::new("filelog");
        let p = dir.path().join("a.log");
        {
            let l = FileLog::open(&p, "run 1").unwrap();
            l.line("one");
        }
        {
            let l = FileLog::open(&p, "run 2").unwrap();
            l.line("two");
        }
        let text = fs::read_to_string(&p).unwrap();
        assert_eq!(
            text, "==== run 1 ====\none\n==== run 2 ====\ntwo\n",
            "a relaunch must not wipe the previous run's lines"
        );
    }
}
