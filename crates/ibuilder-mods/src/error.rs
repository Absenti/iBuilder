//! The exceptions of `mods.py`, as values.

use std::fmt;

/// What a function of `mods.py` can raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModsError {
    /// `mods.ModError` (a `ValueError`): a mod file that cannot be used; the message is what the log and the sheet say. The Python catches it in
    /// `exefs_patches` (the file is skipped, said in the log) and in `apply_module` (the module is rewritten as dumped).
    Mod(String),
    /// An `OSError`: the Python message (`[Errno 2] No such file or directory: '...'`). Caught where the Python catches `OSError`, else it propagates.
    Os(String),
    /// An exception that `mods.py` does NOT catch (`struct.error` from `Patch.digest` of a negative offset, `ValueError` from `int()`,
    /// `UnicodeDecodeError` from a `mods.tsv` that is not UTF-8, `LZ4BlockError`, `AttributeError` ...). It propagates to the caller, as in Python:
    /// stage 2 fails; the boot hook (`apply_mods`) logs it and starts the game without mods.
    Unhandled { exc: &'static str, msg: String },
    /// Something the Python does and this port does not (a ZBIC-compressed NSO). Never swallowed into a "left as dumped".
    Unported(String),
}

pub type Result<T> = std::result::Result<T, ModsError>;

impl ModsError {
    pub fn modl(msg: impl Into<String>) -> ModsError {
        ModsError::Mod(msg.into())
    }
    pub fn unhandled(exc: &'static str, msg: impl Into<String>) -> ModsError {
        ModsError::Unhandled {
            exc,
            msg: msg.into(),
        }
    }
    /// `type(error).__name__` of the Python exception.
    pub fn exc_name(&self) -> &str {
        match self {
            ModsError::Mod(_) => "ModError",
            ModsError::Os(_) => "OSError",
            ModsError::Unhandled { exc, .. } => exc,
            ModsError::Unported(_) => "NotImplementedError",
        }
    }
    /// `str(error)`
    pub fn message(&self) -> &str {
        match self {
            ModsError::Mod(m) | ModsError::Os(m) | ModsError::Unported(m) => m,
            ModsError::Unhandled { msg, .. } => msg,
        }
    }
    /// Would the Python's own `except ModError` catch it?
    pub fn is_mod(&self) -> bool {
        matches!(self, ModsError::Mod(_))
    }
    pub fn is_os(&self) -> bool {
        matches!(self, ModsError::Os(_))
    }
}

impl fmt::Display for ModsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ModsError {}
