//! # ibuilder-pipeline
//!
//! The chain driver of the iPhone app, in Rust: a port of `gameapp/gameapp_pipeline.py` (the CPython program the app embeds today) and of the parts of `gameapp/gameapp_shim.c` and the
//! C# app that are real logic rather than CPython bridging.
//!
//! * [`chain`]: the driver and the [`stage::Stage`] model (profile -> rewrite -> shaders -> assemble -> sign -> boot), result codes, the `ChainReport`.
//! * [`adapters`]: where the stage workers (`ibuilder-rewrite`, `ibuilder-macho`, `ibuilder-sign`, `ibuilder-shader`, `ibuilder-profile`, `ibuilder-pack`) plug in. This crate depends on none of them.
//! * [`settings`], [`shaderpack`], [`profile`]: the settings/env-switch table, launcher.json, shader packs and the shader map, heap sizing and the engine's heap check, address-space decisions.
//! * [`markers`], [`state`]: the run markers and the resume / boot-only state machine (`gameapp.running`, `.args`, `.boot-only`, `.killed`, `.continue`, `.framework`, ...).
//! * [`timings`], [`logwindow`], [`memory`], [`thermal`], [`gate`], [`live`]: build-timings rows, log rotation, memory instrumentation, the thermal and memory gates for parallel stage work,
//!   the live-shader queue.
//! * [`stop`]: the process-wide pause / cancel request that the app's UI raises and every long loop of the chain reads (`RC_PAUSED`, `RC_CANCELLED`).
//! * `sys` (the only `unsafe`): the kernel's memory numbers, the address-space census and reservation, `dlopen` + the host's boot entry, the guest log redirection, the fatal-signal logger.
//!
//! No `fork`, `exec`, `posix_spawn` or `std::process::Command` anywhere: the chain starts no process (the Python build proved the same at run time with an audit hook).
//! iOS portability: everything platform specific is `cfg`-gated (`target_vendor = "apple"` for Mach, `target_os = "ios"` for `os_proc_available_memory` and the early reservation).

#![deny(unsafe_code)]

pub mod adapters;
pub mod assemble;
pub mod boot;
pub mod chain;
pub mod config;
pub mod configtext;
pub mod ctx;
pub mod diag;
pub mod env;
pub mod error;
pub mod gate;
pub mod layout;
pub mod live;
pub mod log;
pub mod logwindow;
pub mod markers;
pub mod memory;
pub mod mods;
pub mod preflight;
pub mod profile;
pub mod rewrite;
pub mod settings;
pub mod shaderpack;
pub mod shaders;
pub mod stage;
pub mod state;
pub mod stop;
pub mod testutil;
pub mod thermal;
pub mod timings;
pub mod tsv;
pub mod util;

#[allow(unsafe_code)]
pub mod sys;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use crate::chain::{ChainReport, run_chain};
pub use crate::ctx::{Ctx, View};
pub use crate::error::{PipelineError, Result};
