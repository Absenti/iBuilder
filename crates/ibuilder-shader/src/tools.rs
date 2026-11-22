//! The two native tools stage 3 calls, as the Python chain calls them: `argv` in, `(rc, stdout, stderr)` out.
//!
//! Both are third-party and stay native: `dmsh_xlate` (C#, Ryujinx.Graphics.Shader; on the phone an AOT callable inside the app, on the Mac the
//! engine's NativeAOT binary as a child process) and SPIRV-Cross (C++; `gameapp_spirv_cross` in the app, `spirv-cross` binary on the Mac).
//! The shapes mirror `shader_device._FakeSubprocess._tool`: the callable gets the argv **without the program name**.
//!
//! * `xlate(argv)`: `argv = [<jobs.json>, <out dir>]`. `<jobs.json>` is a JSON array of job objects (`dmsh_build.job_for`); for each job the tool reads
//!   `<out dir>/<key>.bin` (written by `job_for`) and writes `<out dir>/<key>.spv` and `<out dir>/<key>.res.json`. A job it refuses is reported on
//!   stderr as `[RED] <key>: <message>` lines (`dmsh_build.xlate_errors`), rc non-zero or not.
//! * `spirv_cross(argv)`: the SPIRV-Cross command line without the program name, e.g. `--msl --msl-version 20300 ... <in.spv> --output <out.metal>`;
//!   `["--version"]` is answered with rc 0.
//!
//! Core crates never spawn processes (iOS): the Mac implementation (`ProcessTools`, over `std::process::Command`) lives behind the `host-tools`
//! feature of `ibuilder-shaderbuild`; the phone's implementation wraps the app's C callbacks (`ibuilder-ffi`).

/// What a tool returned (`subprocess.CompletedProcess` without the args).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolRun {
    pub rc: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ToolRun {
    pub fn ok() -> Self {
        ToolRun::default()
    }
    pub fn fail(rc: i32, stderr: impl Into<String>) -> Self {
        ToolRun {
            rc,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// The translator and SPIRV-Cross. Thread-safe: the build calls both from many threads at once.
pub trait ShaderTools: Send + Sync {
    /// `dmsh_xlate <jobs.json> <out dir>`: `argv` = `[jobs.json, out dir]`.
    fn xlate(&self, argv: &[String]) -> ToolRun;
    /// `spirv-cross ...`: `argv` without the program name.
    fn spirv_cross(&self, argv: &[String]) -> ToolRun;
}
