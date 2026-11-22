//! `ibuilder-shader`: the Rust side of stage 3 (the on-device shader translation).
//!
//! What is ported (see `docs/shaders/SHADER-RUST.md`): the **MSL post-rewrite chain** of `dmsh_build.py finish()` (what `tools/shader_post.py` runs), for pixel, vertex
//! and compute shaders: every step of those stages. [`PostError::Unported`] is returned only for a stage the chain does not cover (gs / hs / ds / ts / ms) and, with the
//! documented trigger, for what `docs/shaders/SHADER-RUST.md` section 1 lists as gated; an `Ok` result is byte-identical to the Python's by construction of the tests, never a guess.
#![forbid(unsafe_code)]

pub mod text;
// generated file, kept exactly as tools/gen_templates.py writes it (rustfmt would reflow it, so a regeneration would not reproduce the committed text)
#[rustfmt::skip]
pub mod templates;
pub mod ampere;
pub mod atomic;
pub mod bit0;
pub mod blend;
pub mod border;
pub mod cs;
pub mod difftest;
pub mod finish;
pub mod fsuint;
pub mod guards;
pub mod hmma;
pub mod interval;
pub mod oracle;
pub mod pervertex;
pub mod pool;
pub mod post;
pub mod romfs;
pub mod side;
pub mod steps;
pub mod tables;
pub mod tf;
pub mod tools;
pub mod utest;
pub mod vs;
pub mod wide;
pub mod zsnap;

pub use post::{Job, PostOut, Stage, post_rewrites};

/// Why the chain did not produce an output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostError {
    /// a step the crate does not port would (or might) fire on this shader: run the Python for it
    Unported(&'static str),
    /// the Python raises ValueError / exits in this step for this shader (its build reports the shader as failed)
    Failed { step: &'static str, msg: String },
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PostError::Unported(s) => write!(f, "unported step `{s}` may fire on this shader"),
            PostError::Failed { step, msg } => write!(f, "{step}: {msg}"),
        }
    }
}
impl std::error::Error for PostError {}

pub(crate) fn failed(step: &'static str, msg: impl Into<String>) -> PostError {
    PostError::Failed {
        step,
        msg: msg.into(),
    }
}
