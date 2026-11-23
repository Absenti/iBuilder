//! The two native tools the layer libraries need, called as the Python calls them: `argv` without the program name in, `(rc, stdout, stderr)` out.
//!
//! * `spirv_cross`: the SPIRV-Cross command line (`--reflect`, `--msl ...`, `--revision`). In the app: `gameapp_spirv_cross` (`app/c/spvc_cli.c`), which has to know
//!   the flags these ports pass (`--msl-argument-buffer-tier`, `--flip-vert-y`, `--msl-discrete-descriptor-set`, `--msl-dynamic-buffer`: added to spvc_cli.c with this crate).
//! * `glslang`: `glslangValidator` (`-E -S <stage> <file>`, `-V -R --aml --amb -q -S <stage> <file> -o <spv>`, `--version`). Only the GL port calls it. In the app:
//!   `gameapp_glslang` (`app/c/glslang_cli.cpp` over glslang 16.6.0's C++ API, `scripts/build-glslang.sh`), the same subset with the tool's outputs (`tests/gl_oracle.rs`
//!   passes with it built for the Mac). [`FnLayerTools`] without one answers rc 127, which the GL port reports per shader as the Python reports a glslang failure.
//!
//! The results are taken as `subprocess.run(..., text=True)` gives them: [`run`] applies universal newlines to stdout and stderr.

pub use ibuilder_shader::tools::ToolRun;
use std::sync::Arc;

pub trait LayerTools: Send + Sync {
    fn spirv_cross(&self, argv: &[String]) -> ToolRun;
    fn glslang(&self, argv: &[String]) -> ToolRun;
}

/// A tool as a callback (the app's in-process tools, test doubles)
pub type ToolCallback = Arc<dyn Fn(&[String]) -> ToolRun + Send + Sync>;

pub const NO_GLSLANG: &str =
    "glslangValidator is not in this build: an OpenGL game's GLSL cannot be compiled";

/// [`LayerTools`] over callbacks; `glslang: None` answers every glslang call with rc 127 and [`NO_GLSLANG`].
#[derive(Clone)]
pub struct FnLayerTools {
    pub spirv_cross: ToolCallback,
    pub glslang: Option<ToolCallback>,
}

impl LayerTools for FnLayerTools {
    fn spirv_cross(&self, argv: &[String]) -> ToolRun {
        (self.spirv_cross)(argv)
    }
    fn glslang(&self, argv: &[String]) -> ToolRun {
        match &self.glslang {
            Some(f) => f(argv),
            None => ToolRun::fail(127, NO_GLSLANG),
        }
    }
}

/// Which tool [`run`] calls
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    SpirvCross,
    Glslang,
}

/// `subprocess.run([tool, *argv], capture_output=True, text=True)`
pub fn run(tools: &dyn LayerTools, tool: Tool, argv: &[&str]) -> ToolRun {
    let a: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let r = match tool {
        Tool::SpirvCross => tools.spirv_cross(&a),
        Tool::Glslang => tools.glslang(&a),
    };
    ToolRun {
        rc: r.rc,
        stdout: crate::py::text_mode(&r.stdout),
        stderr: crate::py::text_mode(&r.stderr),
    }
}

#[cfg(feature = "host-tools")]
pub use host::ProcessLayerTools;

#[cfg(feature = "host-tools")]
mod host {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// The Mac's tools as child processes: `spirv-cross` (the engine's pinned `dm/gfx/tools/spirv_cross_bin/spirv-cross`, or `$SPIRV_CROSS`) and `glslangValidator`.
    #[derive(Clone, Debug)]
    pub struct ProcessLayerTools {
        pub spirv_cross_bin: PathBuf,
        pub glslang_bin: PathBuf,
    }

    impl ProcessLayerTools {
        pub fn new(spirv_cross_bin: impl Into<PathBuf>, glslang_bin: impl Into<PathBuf>) -> Self {
            ProcessLayerTools {
                spirv_cross_bin: spirv_cross_bin.into(),
                glslang_bin: glslang_bin.into(),
            }
        }

        /// `$SPIRV_CROSS` (as `dmsh_chain.SPIRV_CROSS`) or `spirv-cross`, and `$GLSLANG` or `glslangValidator`, found on `PATH`
        pub fn from_env() -> Self {
            let sc = std::env::var("SPIRV_CROSS").unwrap_or_else(|_| "spirv-cross".into());
            let gl = std::env::var("GLSLANG").unwrap_or_else(|_| "glslangValidator".into());
            ProcessLayerTools::new(sc, gl)
        }

        fn exec(bin: &std::path::Path, argv: &[String]) -> ToolRun {
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

    impl LayerTools for ProcessLayerTools {
        fn spirv_cross(&self, argv: &[String]) -> ToolRun {
            Self::exec(&self.spirv_cross_bin, argv)
        }
        fn glslang(&self, argv: &[String]) -> ToolRun {
            Self::exec(&self.glslang_bin, argv)
        }
    }
}
