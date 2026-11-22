use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// A damaged / unsupported input (`nso.Error` in the Python, a `ValueError`).
    Format(String),
    /// What the Python reports as `[RED] ...` / `sys.exit(msg)`.
    Red(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Format(m) => f.write_str(m),
            Error::Red(m) => write!(f, "[RED] {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn fmt_err<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Format(m.into()))
}
