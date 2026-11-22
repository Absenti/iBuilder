use std::fmt;

/// What the Python raises as `pack_nup.Error` (a message, printed as `[RED] ...`), an I/O error of the host, or a cancellation.
#[derive(Debug)]
pub enum Error {
    Msg(String),
    Io(std::io::Error),
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Msg(m) => f.write_str(m),
            Error::Io(e) => write!(f, "{e}"),
            Error::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<String> for Error {
    fn from(m: String) -> Self {
        Error::Msg(m)
    }
}

macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::Error::Msg(format!($($t)*))) };
}
pub(crate) use bail;
