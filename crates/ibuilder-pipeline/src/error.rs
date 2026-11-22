//! The crate's error type. Plain, dependency-free; stage workers (other crates) report through `Failed` with their own text.

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum PipelineError {
    /// A filesystem operation failed; `what` says which.
    Io { what: String, source: io::Error },
    /// A stage worker (an adapter) failed with its own message.
    Failed(String),
    /// A required input is missing (the Python code raised FileNotFoundError / KeyError for these).
    Missing(String),
    /// The caller's cancellation flag was set.
    Cancelled,
}

pub type Result<T> = std::result::Result<T, PipelineError>;

impl PipelineError {
    pub fn io(what: impl Into<String>, source: io::Error) -> Self {
        PipelineError::Io {
            what: what.into(),
            source,
        }
    }
    pub fn failed(msg: impl Into<String>) -> Self {
        PipelineError::Failed(msg.into())
    }
    pub fn missing(msg: impl Into<String>) -> Self {
        PipelineError::Missing(msg.into())
    }
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineError::Io { what, source } => write!(f, "{what}: {source}"),
            PipelineError::Failed(m) | PipelineError::Missing(m) => f.write_str(m),
            PipelineError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PipelineError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for PipelineError {
    fn from(e: io::Error) -> Self {
        PipelineError::Io {
            what: "i/o".into(),
            source: e,
        }
    }
}

/// Attach a description to an `io::Result`.
pub trait IoContext<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| PipelineError::io(what(), e))
    }
}
