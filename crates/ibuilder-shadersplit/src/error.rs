//! The two ways a reader can fail.
//!
//! `shader_split.py` fails with `red()` (a `SystemExit`), with a `struct.error` / `ValueError` / `IndexError` from a reader that was handed bytes it does
//! not understand, or with a `KeyError` ...: `scan_file` catches all of them, prints `[RED] <why> (its shaders are left out)` and goes on, so one file no reader
//! can read does not end the split ([`Failure::Red`]). A container the Python reads and this port does not is a different thing: leaving it out silently would
//! give a game fewer shaders than it has, so it ends the split with an explicit error ([`Failure::Unsupported`]), and the doc lists every one.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// `red(msg)` or an exception of the Python reader: this file is left out, the split goes on
    Red(String),
    /// `unsupported container: <name>`: a container the Python can read and this port cannot (yet)
    Unsupported(String),
    /// `unityfs.NoTree`: the serialized file has no type tree and no tree of the same class and hash to borrow (handled by the Unity reader's callers)
    NoTree(String),
}

pub type Res<T> = Result<T, Failure>;

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Red(s) | Failure::NoTree(s) => f.write_str(s),
            Failure::Unsupported(s) => write!(f, "unsupported container: {s}"),
        }
    }
}
impl std::error::Error for Failure {}

/// `red(msg)`: return `Err(Failure::Red(..))`
#[macro_export]
macro_rules! red {
    ($($t:tt)*) => { return Err($crate::error::Failure::Red(format!($($t)*))) };
}

/// `unsupported container: <name> (<where>)`
#[macro_export]
macro_rules! unsupported {
    ($($t:tt)*) => { return Err($crate::error::Failure::Unsupported(format!($($t)*))) };
}

/// What a whole split can end with.
#[derive(Debug)]
pub enum SplitError {
    /// an input (RomFS image, NSO, profile) the split cannot open: the Python raises or exits
    Input(String),
    /// `red()` in `main()` itself (a key whose bytes differ between two copies, a record that cannot be cut, ...)
    Red(String),
    /// a container this port does not read: see the module docs of the crate for the list
    Unsupported(String),
    Io(String),
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SplitError::Input(s) | SplitError::Red(s) | SplitError::Io(s) => f.write_str(s),
            SplitError::Unsupported(s) => write!(f, "unsupported container: {s}"),
        }
    }
}
impl std::error::Error for SplitError {}

impl From<std::io::Error> for SplitError {
    fn from(e: std::io::Error) -> Self {
        SplitError::Io(e.to_string())
    }
}
