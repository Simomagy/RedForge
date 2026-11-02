use crate::rpf8::Rpf8Error;
use crate::xml::XmlError;

/// An error reading, writing or converting a game file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Rpf8(#[from] Rpf8Error),
    #[error(transparent)]
    Xml(#[from] XmlError),
    /// The data isn't what the format expects.
    #[error("{0}")]
    Format(String),
    #[error("not supported: {0}")]
    Unsupported(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// `Err(Error::Format(...))` with `format!` arguments.
#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => {
        return Err($crate::Error::Format(format!($($t)*)))
    };
}
