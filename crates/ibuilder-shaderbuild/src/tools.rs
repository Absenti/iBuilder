//! Implementations of `ibuilder_shader::tools::ShaderTools` (the translator and SPIRV-Cross as the build calls them).
//!
//! * [`FnTools`]: two closures. The app's C callbacks (`ibuilder_tool_fn`, wrapped by `ibuilder-ffi` into the pipeline's `ToolFn`s) and the tests plug in here.
//! * [`ProcessTools`] (feature `host-tools`, Mac only): the engine's `dmsh_xlate` and `spirv-cross` binaries as child processes, argv as the Python's `subprocess.run` passed it.
//! * [`CountingTools`]: any tools, with call counts and the wall time spent inside them (what the build's report and the benchmarks print).
pub use ibuilder_shader::tools::{ShaderTools, ToolRun};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub type ToolFnS = Arc<dyn Fn(&[String]) -> ToolRun + Send + Sync>;

#[derive(Clone)]
pub struct FnTools {
    pub xlate: ToolFnS,
    pub spirv_cross: ToolFnS,
}

impl FnTools {
    pub fn new(xlate: ToolFnS, spirv_cross: ToolFnS) -> FnTools {
        FnTools { xlate, spirv_cross }
    }
}

impl ShaderTools for FnTools {
    fn xlate(&self, argv: &[String]) -> ToolRun {
        (self.xlate)(argv)
    }
    fn spirv_cross(&self, argv: &[String]) -> ToolRun {
        (self.spirv_cross)(argv)
    }
}

/// Calls and nanoseconds inside the tools (sum over threads)
#[derive(Default)]
pub struct ToolStats {
    pub xlate_calls: AtomicU64,
    pub xlate_ns: AtomicU64,
    pub spirv_calls: AtomicU64,
    pub spirv_ns: AtomicU64,
}

pub struct CountingTools<'a> {
    pub inner: &'a dyn ShaderTools,
    pub stats: ToolStats,
}

impl<'a> CountingTools<'a> {
    pub fn new(inner: &'a dyn ShaderTools) -> Self {
        CountingTools {
            inner,
            stats: ToolStats::default(),
        }
    }
}

impl ShaderTools for CountingTools<'_> {
    fn xlate(&self, argv: &[String]) -> ToolRun {
        let t = std::time::Instant::now();
        let r = self.inner.xlate(argv);
        self.stats.xlate_calls.fetch_add(1, Ordering::Relaxed);
        self.stats
            .xlate_ns
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        r
    }
    fn spirv_cross(&self, argv: &[String]) -> ToolRun {
        let t = std::time::Instant::now();
        let r = self.inner.spirv_cross(argv);
        self.stats.spirv_calls.fetch_add(1, Ordering::Relaxed);
        self.stats
            .spirv_ns
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        r
    }
}

#[cfg(feature = "host-tools")]
pub use process::ProcessTools;

#[cfg(feature = "host-tools")]
mod process {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// The engine's native tools as child processes (`dm/gfx/tools/dmsh_xlate_bin/dmsh_xlate`, `spirv_cross_bin/spirv-cross`). Mac only: iOS has no process API, the app passes callbacks.
    #[derive(Clone, Debug)]
    pub struct ProcessTools {
        pub xlate_bin: PathBuf,
        pub spirv_cross_bin: PathBuf,
    }

    impl ProcessTools {
        pub fn new(xlate_bin: impl Into<PathBuf>, spirv_cross_bin: impl Into<PathBuf>) -> Self {
            ProcessTools {
                xlate_bin: xlate_bin.into(),
                spirv_cross_bin: spirv_cross_bin.into(),
            }
        }

        /// `<engine>/dm/gfx/tools/{dmsh_xlate_bin/dmsh_xlate, spirv_cross_bin/spirv-cross}`
        pub fn in_engine(engine: &std::path::Path) -> Self {
            let t = engine.join("dm").join("gfx").join("tools");
            ProcessTools::new(
                t.join("dmsh_xlate_bin").join("dmsh_xlate"),
                t.join("spirv_cross_bin").join("spirv-cross"),
            )
        }

        fn run(bin: &std::path::Path, argv: &[String]) -> ToolRun {
            match Command::new(bin).args(argv).output() {
                Ok(o) => {
                    use std::os::unix::process::ExitStatusExt;
                    let rc = o
                        .status
                        .code()
                        .unwrap_or_else(|| -o.status.signal().unwrap_or(1));
                    ToolRun {
                        rc,
                        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                    }
                }
                Err(e) => ToolRun::fail(127, format!("{}: {e}", bin.display())),
            }
        }
    }

    impl ShaderTools for ProcessTools {
        fn xlate(&self, argv: &[String]) -> ToolRun {
            Self::run(&self.xlate_bin, argv)
        }
        fn spirv_cross(&self, argv: &[String]) -> ToolRun {
            Self::run(&self.spirv_cross_bin, argv)
        }
    }
}
