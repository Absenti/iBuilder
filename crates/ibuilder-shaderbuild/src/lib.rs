//! `ibuilder-shaderbuild`: stage 3's build driver in Rust.
//!
//! Ports `dmsh_build.py` (module level: records, keys, cache, outputs, publish; `job_for` and its helpers; batches, `translate`, `fin`, the summary) and the phone driver
//! `gameapp/shader_device.py` (`build` with its epilogue, `live_package`, `live_translate`, `JobMemo` stays in the split). The per-shader `finish()` is package F2's
//! (`ibuilder_shader::finish`), plugged in through [`finish::Finisher`]; the split is package S's.
//!
//! * [`build::build`]: a whole or partial library build over a split's packages and tables -> `b-<chain>.img` (+ `c1_full.tsv`, `cb_extent.tsv`), streamed.
//! * [`live::live_translate`]: shaders the running game met that the library lacks.
//! * [`tools`]: the translator and spirv-cross behind `ibuilder_shader::tools::ShaderTools` (callbacks for the phone, child processes for the Mac behind `host-tools`).
//! * [`jobs`], [`key`], [`cache`], [`library`], [`tablerows`], [`game`], [`records`], [`pyjson`]: the pieces, each documented with the Python it replaces.
//!
//! `docs/shaders/SHADERBUILD-RUST.md` has the design, the verification and the numbers.
#![forbid(unsafe_code)]

pub mod build;
pub mod cache;
pub mod finish;
pub mod game;
pub mod jobs;
pub mod key;
pub mod library;
pub mod live;
pub mod pyjson;
pub mod records;
pub mod rt_build;
pub mod tablerows;
pub mod toolmemo;
pub mod tools;
pub mod util;

#[cfg(feature = "f2-finish")]
pub mod finish_adapter;

pub use build::{BuildEnv, BuildOptions, BuildReport, ShaderResult, build};
pub use finish::{Finished, Finisher};
pub use tools::{CountingTools, FnTools};
pub mod mesh;
